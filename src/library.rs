//! The Library window. It picks the screen: add a Server, sign in, choose a Profile, or Home. It also does the
//! switch to another Profile or Active server, and the sign-out.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, StreamExt as _};

use gpui::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Subscription, Task,
    Window, div, px,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button, focus_scope, input};

use crate::accounts::{AccountChoice, AccountPicker};
use crate::api::{self, Client, Device, Error, Pending, Server, Trouble, Url, wire};
use crate::home;
use crate::pages::{MenuChoice, Pages};
use crate::player::{PlaybackError, PlayerClosed, PlayerWindow};
use crate::profiles::{PickerEvent, ProfilePicker};
use crate::secrets;
use crate::sign_in::{SignIn, SignedIn};
use crate::state::{Pair, SavedAccount, SavedServer, State};

/// The wait before retry number `attempt` (from 0) while the Server is Offline: 2, 4, 8 and 16 seconds, then
/// 30 seconds.
pub fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(if attempt < 4 { 2 << attempt } else { 30 })
}

pub struct LibraryWindow {
    state: State,
    state_dir: PathBuf,
    device: Device,
    screen: Screen,
    url: Entity<InputState>,
    status: Status,
    /// The request in progress. A new request or a dropped window cancels it.
    task: Option<Task<()>>,
    _enter: Subscription,
    /// The subscription to the events of the screen view.
    _screen: Option<Subscription>,
    player: Entity<PlayerWindow>,
    /// The last playback error, shown at the bottom of the window.
    player_error: Option<String>,
    _player: Subscription,
    _player_closed: Subscription,
    /// The client of the active Account.
    client: Option<Arc<Client>>,
    /// The Server does not answer. The window shows the Offline banner.
    offline: bool,
    /// Probes the Server while it is Offline.
    probe: Option<Task<()>>,
    /// The server refused the sign-in during playback. The sign-in screen shows when the player closes.
    sign_in_waits: bool,
    _troubles: Option<Task<()>>,
    /// The subscriptions to the Play and menu events of the pages.
    _pages: Vec<Subscription>,
    /// The pages that showed when the sidebar menu opened the Profile picker or the Server list. Esc there
    /// shows them again.
    menu_back: Option<Entity<Pages>>,
    /// The positions that the server did not get, of each Server and Account that is not active, keyed by
    /// Server id and Account id. They go back into the client when that Account is active again.
    kept: HashMap<(String, String), HashMap<(String, String), Pending>>,
    /// The pages that showed when the server refused the sign-in, and the id of their Profile. The sign-in
    /// opens them again.
    parked: Option<(String, Entity<Pages>)>,
    /// The Profiles of the active Account.
    profiles: Vec<wire::Profile>,
    /// A request that waits for the PIN of its Profile.
    pin_wait: Option<PinWait>,
    _pin_requests: Option<Task<()>>,
}

struct PinWait {
    profile_id: String,
    reply: oneshot::Sender<Option<String>>,
    /// The pages that sent the request. They come back after the PIN entry.
    pages: Entity<Pages>,
}

enum Screen {
    /// The user adds a Server.
    Connect,
    SignIn(Entity<SignIn>),
    Pages(Entity<Pages>),
    Profiles(Entity<ProfilePicker>),
    /// A full-window message, for example while the app restores the sign-in.
    Message(String),
    /// The Server list of the sidebar menu.
    Accounts(Entity<AccountPicker>),
    /// The Server did not answer when the app opened the Account. The value is the text.
    Unreachable(String),
}

enum Status {
    Idle,
    Connecting,
    Failed(String),
}

impl LibraryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://silo.example.com"));
        url.update(cx, |url, cx| url.focus(window, cx));
        let enter = cx.subscribe_in(&url, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.connect(window, cx);
            }
        });
        let state_dir = State::dir();
        let state = State::load(&state_dir).unwrap_or_else(|err| {
            eprintln!("silo-omarchy: cannot read the state in {}: {err}", state_dir.display());
            State::fresh()
        });
        let device =
            Device { id: state.device_id.clone(), name: gethostname::gethostname().to_string_lossy().into_owned() };
        let player = cx.new(PlayerWindow::new);
        let player_events = cx.subscribe(&player, |this, _, PlaybackError(text), cx| {
            this.player_error = Some(text.clone());
            cx.notify();
        });
        let player_closed = cx.subscribe_in(&player, window, |this, _, _: &PlayerClosed, window, cx| {
            if std::mem::take(&mut this.sign_in_waits) {
                this.signed_out(window, cx);
            }
        });
        // Closing the Library window sends the stop report, closes the player and quits.
        window.on_window_should_close(cx, {
            let player = player.clone();
            move |_, cx| {
                let stopped = player.update(cx, |player, cx| player.quit(cx));
                cx.spawn(async move |cx| {
                    stopped.await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
                false
            }
        });
        let mut this = Self {
            state,
            state_dir,
            device,
            screen: Screen::Connect,
            url,
            status: Status::Idle,
            task: None,
            _enter: enter,
            _screen: None,
            player,
            player_error: None,
            _player: player_events,
            _player_closed: player_closed,
            client: None,
            offline: false,
            probe: None,
            sign_in_waits: false,
            _troubles: None,
            _pages: Vec::new(),
            menu_back: None,
            kept: HashMap::new(),
            parked: None,
            profiles: Vec::new(),
            pin_wait: None,
            _pin_requests: None,
        };
        this.restore(window, cx);
        this
    }

    /// Opens the active Account with the tokens in the Secret Service, or asks the user to sign in.
    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server, account)) = self.state.active() else {
            return;
        };
        let Ok(url) = Url::parse(&server.url) else {
            return;
        };
        let server =
            Server { id: server.id.clone(), name: url.host_str().unwrap_or_default().to_string(), url: url.clone() };
        let account_id = account.id.clone();
        self.screen = Screen::Message(format!("Signing in to {}…", server.name));
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let tokens = secrets::load(&server.id, &account_id).await;
            this.update_in(cx, |this, window, cx| match tokens {
                Ok(Some(tokens)) => {
                    let client = Client::new(cx.http_client(), server, this.device.clone(), tokens);
                    this.open_account(Arc::new(client), window, cx);
                }
                _ => this.show_sign_in(server, None, window, cx),
            })
            .ok();
        }));
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let http = cx.http_client();
        let text = self.url.read(cx).value().to_string();
        self.status = Status::Connecting;
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = api::connect(http, &text).await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(server) => this.show_sign_in(server, None, window, cx),
                    Err(err) => this.status = Status::Failed(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Shows the sign-in screen. The username of the active Account on this Server is filled in.
    fn show_sign_in(&mut self, server: Server, message: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let username =
            self.state.active().filter(|(saved, _)| saved.id == server.id).map(|(_, account)| account.username.clone());
        let device = self.device.clone();
        let view = cx.new(|cx| SignIn::new(server, device, username, message, window, cx));
        self._screen = Some(cx.subscribe_in(&view, window, |this, _, event: &SignedIn, window, cx| {
            this.signed_in(event, window, cx);
        }));
        self.screen = Screen::SignIn(view);
        cx.notify();
    }

    fn signed_in(&mut self, event: &SignedIn, window: &mut Window, cx: &mut Context<Self>) {
        let server = event.client.server();
        // After a refused refresh, the old client of the Account takes the new tokens. It keeps the positions
        // that it did not send, and the pages that use it open again.
        let client = match self.client.clone() {
            Some(old) if old.server().id == server.id && self.account_id() == *event.account.id => {
                old.set_tokens(event.client.tokens());
                old
            }
            _ => {
                self.leave_account(false, cx).detach();
                event.client.clone()
            }
        };
        self.state.remember(
            SavedServer { id: server.id.clone(), url: server.url.to_string(), accounts: Vec::new() },
            SavedAccount {
                id: event.account.id.to_string(),
                username: event.account.username.clone(),
                last_profile: None,
            },
        );
        self.save_state();
        let (server_id, account_id, tokens) = (server.id.clone(), event.account.id.to_string(), client.tokens());
        // When the Secret Service is missing, the tokens stay only in memory. The sign-in screen told the user why.
        cx.background_spawn(async move { secrets::save(&server_id, &account_id, &tokens).await.ok() }).detach();
        self.open_account(client, window, cx);
    }

    /// Keeps the new tokens after each refresh, and opens the Profile of the Account: the last Profile, the
    /// only Profile without a PIN, or else the Profile picker.
    fn open_account(&mut self, client: Arc<Client>, window: &mut Window, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let account_id = self.account_id();
        let server_id = client.server().id.clone();
        // `open_profile` sends the kept positions of the Profile that opens. The others wait for their Profile.
        if let Some(kept) = self.kept.remove(&(server_id.clone(), account_id.clone())) {
            client.adopt_pending(kept);
        }
        client.on_tokens(move |tokens| {
            let (server_id, account_id, tokens) = (server_id.clone(), account_id.clone(), tokens.clone());
            executor.spawn(async move { secrets::save(&server_id, &account_id, &tokens).await.ok() }).detach();
        });
        self.listen_for_pin(&client, window, cx);
        self.listen_for_trouble(&client, window, cx);
        self.client = Some(client.clone());
        self.screen = Screen::Message("Loading Profiles…".into());
        cx.notify();
        let last = self.state.active().and_then(|(_, account)| account.last_profile.clone());
        let account_id = self.account_id();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let profiles = client.profiles().await;
            let launch = profiles.as_ref().ok().and_then(|p| api::launch_profile(p, last.as_deref())).cloned();
            let token = match &launch {
                Some(profile) if profile.has_pin => {
                    secrets::load_pin_token(&client.server().id, &account_id, &profile.id).await.ok().flatten()
                }
                _ => None,
            };
            this.update_in(cx, |this, window, cx| match profiles {
                Ok(profiles) => {
                    this.profiles = profiles;
                    match launch {
                        Some(profile) => this.open_profile(client, profile, token, window, cx),
                        None => this.show_picker(client, None, window, cx),
                    }
                }
                // The trouble listener shows the sign-in screen and probes the Server.
                Err(Error::SignedOut) => {}
                Err(err) if err.is_offline() => {
                    this.screen = Screen::Unreachable(format!("Can't reach {}", client.server().name));
                    cx.notify();
                }
                Err(err) => {
                    this.screen = Screen::Message(err.to_string());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Makes the Profile active and opens Home. `token` is the PIN token of the Profile.
    fn open_profile(
        &mut self,
        client: Arc<Client>,
        profile: wire::Profile,
        token: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pin_wait = None;
        client.set_profile(Some(profile.id.to_string()));
        client.set_profile_token(token);
        // Until the reply comes, playback uses the default Server settings.
        cx.background_spawn({
            let client = client.clone();
            async move {
                if let Err(err) = client.load_server_settings().await {
                    eprintln!("silo-omarchy: cannot read the Server settings: {err}");
                }
                if let Err(err) = client.sync_progress().await {
                    eprintln!("silo-omarchy: progress sync failed: {err}");
                }
            }
        })
        .detach();
        if let Some(account) = self.state.active_account_mut() {
            account.last_profile = Some(profile.id.to_string());
        }
        self.save_state();
        let pages = match self.parked.take() {
            Some((id, pages)) if id == *profile.id => pages,
            _ => cx.new(|cx| Pages::new(client.clone(), profile.name.clone(), window, cx)),
        };
        self.show_pages(client, pages, window, cx);
    }

    fn show_pages(&mut self, client: Arc<Client>, pages: Entity<Pages>, window: &mut Window, cx: &mut Context<Self>) {
        pages.update(cx, |pages, cx| pages.focus(window, cx));
        let play = cx.subscribe(&pages, {
            let client = client.clone();
            move |this, _, home::Play(item), cx| {
                this.player_error = None;
                this.player.update(cx, |player, cx| player.play(client.clone(), item.clone(), cx));
            }
        });
        let menu = cx.subscribe_in(&pages, window, move |this, pages, choice: &MenuChoice, window, cx| match choice {
            MenuChoice::SwitchProfile => {
                this.menu_back = Some(pages.clone());
                this.show_picker(client.clone(), None, window, cx);
            }
            MenuChoice::SwitchServer => {
                this.menu_back = Some(pages.clone());
                this.show_accounts(window, cx);
            }
            MenuChoice::SignOut => this.sign_out(window, cx),
        });
        self._pages = vec![play, menu];
        self._screen = None;
        self.screen = Screen::Pages(pages);
        cx.notify();
    }

    /// Shows the Profile picker. With `pin_for`, the PIN entry of that Profile is open.
    fn show_picker(&mut self, client: Arc<Client>, pin_for: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let profiles = self.profiles.clone();
        let view = cx.new(|cx| ProfilePicker::new(client.clone(), profiles, pin_for, window, cx));
        self._screen =
            Some(cx.subscribe_in(&view, window, move |this, _, event: &PickerEvent, window, cx| match event {
                PickerEvent::Chosen { profile, token } => this.chosen(client.clone(), profile, token, window, cx),
                PickerEvent::Retry => this.open_account(client.clone(), window, cx),
                PickerEvent::SignOut => this.sign_out(window, cx),
                PickerEvent::Back => this.back_to_pages(window, cx),
            }));
        self.screen = Screen::Profiles(view);
        cx.notify();
    }

    /// The user chose a Profile in the picker. When a request waits for the PIN of this Profile, the request
    /// gets the PIN token and Home comes back. Otherwise the app opens the Profile.
    fn chosen(
        &mut self,
        client: Arc<Client>,
        profile: &wire::Profile,
        token: &Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(token) = token.clone() {
            let (server_id, account_id, profile_id) =
                (client.server().id.clone(), self.account_id(), profile.id.to_string());
            // When the Secret Service is missing, the PIN token stays only in memory.
            cx.background_spawn(async move {
                secrets::save_pin_token(&server_id, &account_id, &profile_id, &token).await.ok()
            })
            .detach();
        }
        let current = self.state.active().and_then(|(_, account)| account.last_profile.clone());
        match self.pin_wait.take() {
            Some(wait) if wait.profile_id == *profile.id => {
                wait.reply.send(token.clone()).ok();
                self.show_pages(client, wait.pages, window, cx);
            }
            // The sidebar menu opened the picker, and the user chose the Profile that shows.
            _ if self.menu_back.is_some() && current.as_deref() == Some(&*profile.id) => self.back_to_pages(window, cx),
            _ => {
                self.menu_back = None;
                self.stop_playback(cx).detach();
                self.open_profile(client, profile.clone(), token.clone(), window, cx);
            }
        }
    }

    /// Shows the pages again after Esc in the Profile picker or the Server list of the sidebar menu.
    fn back_to_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(pages), Some(client)) = (self.menu_back.take(), self.client.clone()) {
            self.show_pages(client, pages, window, cx);
        }
    }

    /// Shows the Server list.
    fn show_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (pairs, active) = (self.state.pairs(), self.state.active.clone());
        let view = cx.new(|cx| AccountPicker::new(pairs, active, window, cx));
        self._screen =
            Some(cx.subscribe_in(&view, window, |this, _, choice: &AccountChoice, window, cx| match choice {
                AccountChoice::Open(pair) => this.open_pair(pair, window, cx),
                AccountChoice::Add => {
                    this.leave_account(false, cx).detach();
                    this._screen = None;
                    this.screen = Screen::Connect;
                    this.url.update(cx, |url, cx| url.focus(window, cx));
                    cx.notify();
                }
                AccountChoice::Back if this.menu_back.is_some() => this.back_to_pages(window, cx),
                // From the sign-in or the Connect screen: open the Active server again.
                AccountChoice::Back => this.restore(window, cx),
            }));
        self.screen = Screen::Accounts(view);
        cx.notify();
    }

    /// Makes a Server and Account pair the Active server and opens it.
    fn open_pair(&mut self, pair: &Pair, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = (pair.server_id.clone(), pair.account_id.clone());
        if self.menu_back.is_some() && self.state.active.as_ref() == Some(&chosen) {
            return self.back_to_pages(window, cx);
        }
        self.leave_account(false, cx).detach();
        self.state.active = Some(chosen);
        self.save_state();
        self.restore(window, cx);
    }

    /// Stops playback and closes the Player window. The task ends when the server has the stop report.
    fn stop_playback(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.sign_in_waits = false;
        self.player_error = None;
        self.player.update(cx, |player, cx| player.quit(cx))
    }

    /// Leaves the Active server: it stops playback and drops the pages, the back history and the search and
    /// filter state. The Artwork cache stays. When the stop report is done, the positions that the server did
    /// not get go to `kept`. With `sign_out`, the client first sends the positions of its Profile, calls
    /// `logout` and deletes the tokens of the Account. The task ends after that.
    fn leave_account(&mut self, sign_out: bool, cx: &mut Context<Self>) -> Task<()> {
        let old = self.client.take();
        // A failed stop report or sign-out must not show the Offline banner or the sign-in screen.
        if let Some(old) = &old {
            old.on_trouble(|_| {});
        }
        let stopped = self.stop_playback(cx);
        self.pin_wait = None;
        self.menu_back = None;
        self.parked = None;
        self.probe = None;
        self.offline = false;
        self._troubles = None;
        self._pin_requests = None;
        self._pages.clear();
        self.profiles.clear();
        let Some(old) = old else {
            return stopped;
        };
        let key = (old.server().id.clone(), self.account_id());
        cx.spawn(async move |this, cx| {
            stopped.await;
            if sign_out {
                if let Err(err) = old.sign_out().await {
                    eprintln!("silo-omarchy: the server sign-out failed: {err}");
                }
                if let Err(err) = secrets::delete(&key.0, &key.1).await {
                    eprintln!("silo-omarchy: cannot delete the tokens in the Secret Service: {err}");
                }
            }
            this.update(cx, |this, _| this.kept.entry(key).or_default().extend(old.take_pending())).ok();
        })
    }

    /// Signs the active Account out and shows its sign-in screen. The Server and the Account stay in the
    /// state, so that the user can sign in again quickly.
    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.client.as_ref().map(|client| client.server().clone()) else {
            return;
        };
        let left = self.leave_account(true, cx);
        self._screen = None;
        self.screen = Screen::Message("Signing out…".into());
        cx.notify();
        // The sign-in screen waits for the delete, so that the delete cannot remove the tokens of a new sign-in.
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            left.await;
            this.update_in(cx, |this, window, cx| this.show_sign_in(server, None, window, cx)).ok();
        }));
    }

    /// Opens the PIN entry each time that the server answers 403 `profile_verification_required`. The client
    /// sends the request again with the PIN token that the user unlocks.
    fn listen_for_pin(&mut self, client: &Arc<Client>, window: &mut Window, cx: &mut Context<Self>) {
        let (requests, mut asked) = mpsc::unbounded::<(String, oneshot::Sender<Option<String>>)>();
        client.on_pin_required(move |profile_id| {
            let (reply, answer) = oneshot::channel();
            requests.unbounded_send((profile_id, reply)).ok();
            async move { answer.await.ok().flatten() }.boxed()
        });
        let client = client.clone();
        self._pin_requests = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some((profile_id, reply)) = asked.next().await {
                this.update_in(cx, |this, window, cx| {
                    // Only the pages send profile requests. On another screen the request fails.
                    let Screen::Pages(pages) = &this.screen else {
                        return;
                    };
                    this.pin_wait = Some(PinWait { profile_id: profile_id.clone(), reply, pages: pages.clone() });
                    this.show_picker(client.clone(), Some(&profile_id), window, cx);
                })
                .ok();
            }
        }));
    }

    /// Shows the Offline banner, or the sign-in screen, when a request of the client finds the Server
    /// Offline or the sign-in refused.
    fn listen_for_trouble(&mut self, client: &Arc<Client>, window: &mut Window, cx: &mut Context<Self>) {
        let (troubles, mut told) = mpsc::unbounded();
        client.on_trouble(move |trouble| {
            troubles.unbounded_send(trouble).ok();
        });
        self._troubles = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(trouble) = told.next().await {
                let handled = this.update_in(cx, |this, window, cx| match trouble {
                    Trouble::Offline => this.probe_until_online(window, cx),
                    Trouble::SignedOut if this.player.read(cx).is_open() => this.sign_in_waits = true,
                    Trouble::SignedOut => this.signed_out(window, cx),
                });
                if handled.is_err() {
                    return;
                }
            }
        }));
    }

    /// Shows the sign-in screen with the username filled in, unless it shows already.
    fn signed_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = &self.client else {
            return;
        };
        if let Screen::Pages(pages) = &self.screen
            && let Some((_, account)) = self.state.active()
            && let Some(profile) = &account.last_profile
        {
            self.parked = Some((profile.clone(), pages.clone()));
        }
        if !matches!(self.screen, Screen::SignIn(_)) {
            self.show_sign_in(client.server().clone(), Some(Error::SignedOut.to_string()), window, cx);
        }
    }

    /// Probes `system/info` with the retry delays until the Server answers. Then it sends the kept progress,
    /// and opens the Account again when the Server did not answer at launch.
    fn probe_until_online(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if self.probe.is_some() {
            return;
        }
        self.offline = true;
        cx.notify();
        self.probe = Some(cx.spawn_in(window, async move |this, cx| {
            let mut attempt = 0;
            loop {
                cx.background_executor().timer(retry_delay(attempt)).await;
                if client.probe().await.is_ok() {
                    break;
                }
                attempt = attempt.saturating_add(1);
            }
            cx.background_spawn({
                let client = client.clone();
                async move {
                    client
                        .sync_progress()
                        .await
                        .inspect_err(|err| eprintln!("silo-omarchy: progress sync failed: {err}"))
                        .ok()
                }
            })
            .detach();
            this.update_in(cx, |this, window, cx| {
                this.offline = false;
                // This task ends here, so it must not drop itself.
                if let Some(probe) = this.probe.take() {
                    probe.detach();
                }
                if matches!(this.screen, Screen::Unreachable(_)) {
                    this.open_account(client, window, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Retry on the "Can't reach" screen: opens the Account again now. A failure starts the retries again.
    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.probe = None;
        self.offline = false;
        self.open_account(client, window, cx);
    }

    fn account_id(&self) -> String {
        self.state.active.clone().map(|(_, account)| account).unwrap_or_default()
    }

    fn save_state(&self) {
        if let Err(err) = self.state.save(&self.state_dir) {
            eprintln!("silo-omarchy: cannot write the state in {}: {err}", self.state_dir.display());
        }
    }

    fn render_connect(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let (message, color) = match &self.status {
            Status::Idle => (String::new(), t.secondary),
            Status::Connecting => ("Connecting…".into(), t.secondary),
            Status::Failed(err) => (err.clone(), t.danger),
        };
        div()
            .w(px(420.))
            .p(px(32.))
            .gap(px(14.))
            .flex()
            .flex_col()
            .rounded(px(16.))
            .bg(t.surface)
            .child(div().text_size(px(28.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("silo"))
            .child(div().text_size(px(13.)).text_color(t.secondary).child("Server address"))
            .child(input("server-url", &self.url, window, cx))
            .child(
                button("connect", "Connect", ButtonVariant::Primary, cx)
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
            )
            .child(div().min_h(px(18.)).text_size(px(13.)).text_color(color).child(message))
    }
}

impl LibraryWindow {
    /// The button below the Connect and sign-in screens that opens the Server list, when there are saved
    /// Servers.
    fn switch_server_button(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        (!self.state.servers.is_empty()).then(|| {
            div().absolute().bottom(px(24.)).child(
                button("switch-server", "Switch server", ButtonVariant::Secondary, cx)
                    .on_click(cx.listener(|this, _, window, cx| this.show_accounts(window, cx))),
            )
        })
    }
}

impl Render for LibraryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let page = focus_scope("library")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font.clone());
        let page = match &self.screen {
            Screen::Connect => page.child(self.render_connect(window, cx)).children(self.switch_server_button(cx)),
            Screen::SignIn(view) => page.child(view.clone()).children(self.switch_server_button(cx)),
            Screen::Accounts(view) => page.child(view.clone()),
            Screen::Pages(view) => page.child(view.clone()),
            Screen::Profiles(view) => page.child(view.clone()),
            Screen::Message(text) => page.child(div().max_w(px(560.)).text_color(t.secondary).child(text.clone())),
            Screen::Unreachable(text) => page.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.))
                    .child(div().text_size(px(20.)).text_color(t.bright).child(text.clone()))
                    .child(div().text_color(t.secondary).child("silo-omarchy retries by itself."))
                    .child(
                        button("retry", "Retry", ButtonVariant::Primary, cx)
                            .on_click(cx.listener(|this, _, window, cx| this.retry(window, cx))),
                    ),
            ),
        };
        let banner = self.offline && !matches!(self.screen, Screen::Unreachable(_));
        let page = page.children(banner.then(|| {
            div()
                .absolute()
                .top(px(16.))
                .px(px(16.))
                .py(px(8.))
                .rounded_full()
                .bg(t.surface)
                .text_color(t.warning)
                .child("Offline, retrying")
        }));
        page.children(self.player_error.clone().map(|text| {
            div()
                .absolute()
                .bottom(px(24.))
                .left(px(24.))
                .px(px(16.))
                .py(px(10.))
                .rounded(px(12.))
                .bg(t.surface)
                .text_color(t.danger)
                .child(text)
        }))
    }
}
