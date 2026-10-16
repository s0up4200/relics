//! The Library window. It picks the screen: add a Server, sign in, choose a Profile, or Home. It also does the
//! switch to another Profile or Active server, and the sign-out.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, StreamExt as _};

use gpui::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Render,
    StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div, px,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button, focus_scope, input};

use crate::accounts::{AccountChoice, AccountPicker};
use crate::api::{self, Client, Device, Error, Pending, Server, Tokens, Trouble, Url, wire};
use crate::click::Clickable as _;
use crate::home;
use crate::mpv;
use crate::pages::freshness::Change;
use crate::pages::{MenuChoice, Pages};
use crate::player::{Controller, Media, PlaybackError, PlayerClosed, Stopped, ViewClosed};
use crate::player_view::PlayerView;
use crate::profiles::{PickerEvent, ProfilePicker};
use crate::release::ReleaseCheck;
use crate::secrets;
use crate::settings::LocalSettings;
use crate::settings_page::AccountAction;
use crate::sign_in::{SignIn, SignedIn};
use crate::startup;
use crate::state::{Pair, SavedAccount, SavedServer, State};
use crate::video;

/// The Server of a saved Server, named by the host of its URL.
fn server_of(saved: &SavedServer) -> Option<Server> {
    let url = Url::parse(&saved.url).ok()?;
    Some(Server { id: saved.id.clone(), name: url.host_str().unwrap_or_default().to_string(), url })
}

/// The wait before retry number `attempt` (from 0) while the Server is Offline: 2, 4, 8 and 16 seconds, then
/// 30 seconds.
pub fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(if attempt < 4 { 2 << attempt } else { 30 })
}

pub struct LibraryWindow {
    state: State,
    state_dir: PathBuf,
    /// A test session. It reads the tokens from `tokens`, saves no PIN token, and has no video, no release check and
    /// no Live updates socket for the pages that it opens. A token refresh and a sign-out still use the Secret
    /// Service.
    isolated: bool,
    /// The tokens of each saved Account of a test session, keyed by Server id and Account id. A test session reads
    /// them in place of the Secret Service.
    tokens: HashMap<(String, String), Tokens>,
    device: Device,
    screen: Screen,
    url: Entity<InputState>,
    status: Status,
    /// The request in progress. A new request or a dropped window cancels it.
    task: Option<Task<()>>,
    /// Reads the Server settings of the Profile that opens, then opens Home. Leaving the Account cancels it.
    opening: Option<Task<()>>,
    _enter: Subscription,
    /// The subscription to the events of the screen view.
    _screen: Option<Subscription>,
    player: Entity<Controller>,
    /// The last playback error, shown at the bottom of the window.
    player_error: Option<String>,
    _player: Subscription,
    _player_closed: Subscription,
    _stopped: Subscription,
    /// The Player view. It shows in place of the screen while the controller plays in it, so Back returns to
    /// the page where playback started. `None` when the window has no video surface.
    player_view: Option<Entity<PlayerView>>,
    _view_closed: Subscription,
    /// The client of the active Account.
    client: Option<Arc<Client>>,
    /// The Server does not answer. The window shows the Offline banner.
    offline: bool,
    /// Probes the Server while it is Offline.
    probe: Option<Task<()>>,
    /// Waits for the progress sync after the Server answers again, and then reloads the pages. A switch to another
    /// Profile or Active server cancels the reload, so that it cannot reload the pages that replace them.
    recovery: Option<Task<()>>,
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
    /// Gives each Browser sign-in link from the hand-off socket to the sign-in screen.
    _links: Option<Task<()>>,
    /// The release check. A newer release shows the Release notice at the bottom right.
    release: Entity<ReleaseCheck>,
    _release: Subscription,
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

/// An existing signed-in session, with isolated saved state for recovery tests.
pub struct Session {
    pub state: State,
    pub directory: PathBuf,
    pub client: Arc<Client>,
    pub pages: Entity<Pages>,
    pub profiles: Vec<wire::Profile>,
    /// The tokens of each saved Account, keyed by Server id and Account id, in place of the Secret Service.
    pub tokens: HashMap<(String, String), Tokens>,
}

impl LibraryWindow {
    /// `links` are the Browser sign-in links that other processes hand off. `link` is the return link that
    /// started this process when no app waited for it.
    pub fn new(
        links: Option<mpsc::UnboundedReceiver<String>>,
        link: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(None, links, link, window, cx)
    }

    /// Start at the real browsing screen without desktop or sign-in startup services.
    pub fn with_session(session: Session, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::settings::use_directory(session.directory.clone(), cx);
        Self::build(Some(session), None, None, window, cx)
    }

    fn build(
        session: Option<Session>,
        links: Option<mpsc::UnboundedReceiver<String>>,
        link: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://silo.example.com"));
        url.update(cx, |url, cx| url.focus(window, cx));
        let enter = cx.subscribe_in(&url, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.connect(window, cx);
            }
        });
        let isolated = session.is_some();
        let (state, state_dir, tokens, browsing) = match session {
            Some(session) => (
                session.state,
                session.directory,
                session.tokens,
                Some((session.client, session.pages, session.profiles)),
            ),
            None => {
                let dir = State::dir();
                let state = State::load(&dir).unwrap_or_else(|err| {
                    eprintln!("relics: cannot read the state in {}: {err}", dir.display());
                    State::fresh()
                });
                (state, dir, HashMap::new(), None)
            }
        };
        let device =
            Device { id: state.device_id.clone(), name: gethostname::gethostname().to_string_lossy().into_owned() };
        let video = if isolated { None } else { video::Surface::new(window) };
        let media = video.clone().map(|video| Box::new(mpv::InView(video)) as Box<dyn Media>);
        let player = cx.new(|cx| Controller::new(media, cx));
        let player_view = video.map(|video| cx.new(|cx| PlayerView::new(player.clone(), video, cx)));
        let view_closed = cx.subscribe_in(&player, window, |this, _, _: &ViewClosed, window, cx| {
            if let Some(view) = &this.player_view {
                view.update(cx, |view, _| view.closed(window));
            }
            if let Screen::Pages(pages) = &this.screen {
                pages.update(cx, |pages, cx| {
                    pages.set_shown(true, cx);
                    pages.focus(window, cx);
                });
            }
            cx.notify();
        });
        // The Server has the new position after the stop report, so the pages reload.
        let stopped = cx.subscribe(&player, |this, _, _: &Stopped, cx| this.refresh_pages(cx));
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
        // A debug build (`cargo run`) sends no request to GitHub. A launch with the startup trace also sends none, so
        // that a series of launches stays under the GitHub limit.
        let http = (!isolated && !cfg!(debug_assertions) && !startup::on()).then(|| cx.http_client());
        let enabled = !isolated && LocalSettings::load(&crate::settings::directory(cx)).check_releases;
        let release = cx.new(|cx| ReleaseCheck::new(http, enabled, cx));
        let release_changed = cx.observe(&release, |_, _, cx| cx.notify());
        // GPUI draws the window once before it opens, but shows that frame only when the compositor asks for the
        // first frame. A next-frame callback runs at that request, before the frame shows.
        if startup::on() {
            window.on_next_frame(|_, _| startup::mark("first-frame"));
        }
        let mut this = Self {
            state,
            state_dir,
            isolated,
            tokens,
            device,
            screen: Screen::Connect,
            url,
            status: Status::Idle,
            task: None,
            opening: None,
            _enter: enter,
            _screen: None,
            player,
            player_error: None,
            _player: player_events,
            _player_closed: player_closed,
            _stopped: stopped,
            player_view,
            _view_closed: view_closed,
            client: None,
            offline: false,
            probe: None,
            recovery: None,
            sign_in_waits: false,
            _troubles: None,
            _pages: Vec::new(),
            menu_back: None,
            kept: HashMap::new(),
            parked: None,
            profiles: Vec::new(),
            pin_wait: None,
            _pin_requests: None,
            _links: links.map(|mut links| {
                cx.spawn(async move |this, cx| {
                    while let Some(link) = links.next().await {
                        let taken = this.update(cx, |this, cx| {
                            if let Screen::SignIn(view) = &this.screen {
                                view.update(cx, |view, cx| view.browser_returned(&link, cx));
                            }
                        });
                        if taken.is_err() {
                            return;
                        }
                    }
                })
            }),
            release,
            _release: release_changed,
        };
        if let Some((client, pages, profiles)) = browsing {
            this.profiles = profiles;
            this.listen_for_pin(&client, window, cx);
            this.listen_for_trouble(&client, window, cx);
            this.client = Some(client.clone());
            this.show_pages(client, pages, window, cx);
        } else {
            match link {
                Some(link) => this.browser_timed_out(&link, window, cx),
                None => this.restore(window, cx),
            }
        }
        this
    }

    /// The browser returned after the window that waited for it closed. The app shows the sign-in screen of the
    /// Server that the link names, or the Connect screen when the app does not know that Server.
    fn browser_timed_out(&mut self, link: &str, window: &mut Window, cx: &mut Context<Self>) {
        let server_id = Url::parse(link)
            .ok()
            .and_then(|url| url.query_pairs().find(|(name, _)| name == "server").map(|(_, id)| id.into_owned()));
        let server = self.state.servers.iter().find(|saved| Some(&saved.id) == server_id.as_ref()).and_then(server_of);
        match server {
            Some(server) => self.show_sign_in(server, Some(api::BROWSER_TIMED_OUT.into()), window, cx),
            None => self.status = Status::Failed(api::BROWSER_TIMED_OUT.into()),
        }
    }

    /// Opens the active Account with the tokens in the Secret Service, or asks the user to sign in.
    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server, account)) = self.state.active() else {
            return;
        };
        let Some(server) = server_of(server) else {
            return;
        };
        let account_id = account.id.clone();
        let saved = self.isolated.then(|| self.tokens.get(&(server.id.clone(), account_id.clone())).cloned());
        self.set_screen(Screen::Message(format!("Signing in to {}…", server.name)), cx);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let tokens = match saved {
                Some(saved) => Ok(saved),
                None => secrets::load(&server.id, &account_id).await,
            };
            startup::mark("keyring");
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
        let has_account = self.state.servers.iter().any(|saved| saved.id == server.id && !saved.accounts.is_empty());
        let view = cx.new(|cx| SignIn::new(server, device, has_account, username, message, window, cx));
        self._screen = Some(cx.subscribe_in(&view, window, |this, _, event: &SignedIn, window, cx| {
            this.signed_in(event, window, cx);
        }));
        self.set_screen(Screen::SignIn(view), cx);
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
        self.set_screen(Screen::Message("Loading Profiles…".into()), cx);
        cx.notify();
        let last = self.state.active().and_then(|(_, account)| account.last_profile.clone());
        let account_id = self.account_id();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let profiles = client.profiles().await;
            startup::mark("profiles");
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
                    this.set_screen(Screen::Unreachable(format!("Can't reach {}", client.server().name)), cx);
                    cx.notify();
                }
                Err(err) => {
                    this.set_screen(Screen::Message(err.to_string()), cx);
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
        if let Some(account) = self.state.active_account_mut() {
            account.last_profile = Some(profile.id.to_string());
        }
        self.save_state();
        self._screen = None;
        self.set_screen(Screen::Message("Loading…".into()), cx);
        cx.notify();
        // Home opens after the Server settings of the Profile come, so that no playback starts with the settings
        // of the Profile before. When the read fails, playback uses the default Server settings.
        self.opening = Some(cx.spawn_in(window, async move |this, cx| {
            if let Err(err) = client.load_server_settings().await {
                eprintln!("relics: cannot read the Server settings: {err}");
            }
            startup::mark("settings");
            cx.background_spawn({
                let client = client.clone();
                async move {
                    if let Err(err) = client.sync_progress().await {
                        eprintln!("relics: progress sync failed: {err}");
                    }
                }
            })
            .detach();
            this.update_in(cx, |this, window, cx| {
                let pages = match this.parked.take() {
                    // The pages missed the changes, and a stop report, while the sign-in showed.
                    Some((id, pages)) if id == *profile.id => {
                        pages.update(cx, |pages, cx| pages.refresh(&[Change::All], cx));
                        pages
                    }
                    // A test session controls the Live updates of the pages that it starts with, and opens no socket.
                    _ if this.isolated => cx.new(|cx| {
                        let changes = futures::stream::pending().boxed();
                        Pages::with_changes(client.clone(), profile.name.clone(), changes, window, cx)
                    }),
                    _ => cx.new(|cx| Pages::new(client.clone(), profile.name.clone(), window, cx)),
                };
                this.show_pages(client, pages, window, cx);
            })
            .ok();
        }));
    }

    fn show_pages(&mut self, client: Arc<Client>, pages: Entity<Pages>, window: &mut Window, cx: &mut Context<Self>) {
        pages.update(cx, |pages, cx| pages.focus(window, cx));
        let play = cx.subscribe_in(&pages, window, {
            let client = client.clone();
            move |this, pages, home::Play(item), window, cx| {
                this.player_error = None;
                this.player.update(cx, |player, cx| player.play(client.clone(), item.clone(), cx));
                if let Some(view) = this.player_view.clone().filter(|_| this.player.read(cx).in_view()) {
                    view.update(cx, |view, cx| view.open(window, cx));
                    pages.update(cx, |pages, cx| pages.set_shown(false, cx));
                }
                cx.notify();
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
        let accounts = cx.subscribe_in(&pages, window, |this, _, action: &AccountAction, window, cx| {
            this.account_action(action, window, cx)
        });
        self.share_pairs(&pages, cx);
        let release = self.release.clone();
        pages.update(cx, |pages, _| pages.release = Some(release));
        self._pages = vec![play, menu, accounts];
        self._screen = None;
        self.set_screen(Screen::Pages(pages), cx);
        cx.notify();
    }

    /// Shows `screen` in place of the screen that shows. Pages that leave the window wait with their reloads until
    /// they show again.
    fn set_screen(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if let Screen::Pages(old) = &self.screen
            && !matches!(&screen, Screen::Pages(new) if new == old)
        {
            old.update(cx, |pages, cx| pages.set_shown(false, cx));
        }
        if let Screen::Pages(new) = &screen {
            new.update(cx, |pages, cx| pages.set_shown(true, cx));
        }
        self.screen = screen;
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
        self.set_screen(Screen::Profiles(view), cx);
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
        if !self.isolated
            && let Some(token) = token.clone()
        {
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
                self.recovery = None;
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
                AccountChoice::Add => this.add_server(window, cx),
                AccountChoice::Back if this.menu_back.is_some() => this.back_to_pages(window, cx),
                // From the sign-in or the Connect screen: open the Active server again.
                AccountChoice::Back => this.restore(window, cx),
            }));
        self.set_screen(Screen::Accounts(view), cx);
        cx.notify();
    }

    /// Leaves the Active server and shows the Connect screen.
    fn add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_account(false, cx).detach();
        self.show_connect(window, cx);
    }

    fn show_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._screen = None;
        self.set_screen(Screen::Connect, cx);
        self.url.update(cx, |url, cx| url.focus(window, cx));
        cx.notify();
    }

    /// Gives the pages the Server and Account pairs for the Accounts section of the Settings page.
    fn share_pairs(&self, pages: &Entity<Pages>, cx: &mut Context<Self>) {
        let pairs = (self.state.pairs(), self.state.active.clone());
        pages.update(cx, |pages, _| pages.pairs = pairs);
    }

    /// An action of the Accounts section of the Settings page.
    fn account_action(&mut self, action: &AccountAction, window: &mut Window, cx: &mut Context<Self>) {
        let is_active = |pair: &Pair| self.state.active == Some((pair.server_id.clone(), pair.account_id.clone()));
        match action {
            AccountAction::AddServer => self.add_server(window, cx),
            AccountAction::SignOut(pair) if is_active(pair) => self.sign_out(window, cx),
            AccountAction::SignOut(pair) => self.sign_out_other(pair, cx),
            AccountAction::Remove(pair) if is_active(pair) => {
                let left = self.leave_account(true, cx);
                self.state.remove(&pair.server_id, &pair.account_id);
                self.save_state();
                self._screen = None;
                self.set_screen(Screen::Message("Signing out…".into()), cx);
                cx.notify();
                let key = (pair.server_id.clone(), pair.account_id.clone());
                self.task = Some(cx.spawn_in(window, async move |this, cx| {
                    left.await;
                    this.update_in(cx, |this, window, cx| {
                        this.kept.remove(&key);
                        match this.state.active {
                            Some(_) => this.restore(window, cx),
                            None => this.show_connect(window, cx),
                        }
                    })
                    .ok();
                }));
            }
            AccountAction::Remove(pair) => {
                self.sign_out_other(pair, cx);
                self.state.remove(&pair.server_id, &pair.account_id);
                self.save_state();
                self.kept.remove(&(pair.server_id.clone(), pair.account_id.clone()));
                if let Screen::Pages(pages) = &self.screen {
                    self.share_pairs(&pages.clone(), cx);
                }
            }
        }
    }

    /// Signs out an Account that is not active: `logout` with its saved tokens, then it deletes the tokens. Its
    /// kept positions stay until the user signs in again or removes it.
    fn sign_out_other(&self, pair: &Pair, cx: &mut Context<Self>) {
        let Some(server) = self.state.servers.iter().find(|s| s.id == pair.server_id).and_then(server_of) else {
            return;
        };
        let (http, device, account_id) = (cx.http_client(), self.device.clone(), pair.account_id.clone());
        cx.spawn(async move |_, _| {
            if let Ok(Some(tokens)) = secrets::load(&server.id, &account_id).await
                && let Err(err) = Client::new(http, server.clone(), device, tokens).sign_out().await
            {
                eprintln!("relics: the server sign-out failed: {err}");
            }
            if let Err(err) = secrets::delete(&server.id, &account_id).await {
                eprintln!("relics: cannot delete the tokens in the Secret Service: {err}");
            }
        })
        .detach();
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

    /// Stops playback and closes the Player view. The task ends when the server has the stop report.
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
        self.opening = None;
        self.probe = None;
        self.recovery = None;
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
                    eprintln!("relics: the server sign-out failed: {err}");
                }
                if let Err(err) = secrets::delete(&key.0, &key.1).await {
                    eprintln!("relics: cannot delete the tokens in the Secret Service: {err}");
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
        self.set_screen(Screen::Message("Signing out…".into()), cx);
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

    /// Reloads all pages: the pages that show now, and the pages behind a picker, the PIN entry or the sign-in
    /// screen when they show again.
    fn refresh_pages(&mut self, cx: &mut Context<Self>) {
        let pages = match &self.screen {
            Screen::Pages(pages) => Some(pages.clone()),
            _ => self
                .menu_back
                .clone()
                .or_else(|| self.pin_wait.as_ref().map(|wait| wait.pages.clone()))
                .or_else(|| self.parked.as_ref().map(|(_, pages)| pages.clone())),
        };
        if let Some(pages) = pages {
            pages.update(cx, |pages, cx| pages.refresh(&[Change::All], cx));
        }
    }

    /// Probes `system/info` with the retry delays until the Server answers. Then it sends the kept progress,
    /// reloads the pages, and opens the Account again when the Server did not answer at launch.
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
            // A switch cancels only the reload. The sync goes on, because the Server can already have its request.
            let (synced, sync_done) = oneshot::channel();
            cx.background_spawn({
                let client = client.clone();
                async move {
                    // A failed sync keeps the positions for the next sync.
                    if let Err(err) = client.sync_progress().await {
                        eprintln!("relics: progress sync failed: {err}");
                    }
                    synced.send(()).ok();
                }
            })
            .detach();
            this.update_in(cx, |this, window, cx| {
                this.offline = false;
                // This task ends here, so it must not drop itself.
                if let Some(probe) = this.probe.take() {
                    probe.detach();
                }
                // The pages reload after the attempt to give the Server the positions of the stop reports that
                // failed offline, also when it failed.
                this.recovery = Some(cx.spawn(async move |this, cx| {
                    sync_done.await.ok();
                    this.update(cx, |this, cx| this.refresh_pages(cx)).ok();
                }));
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
            eprintln!("relics: cannot write the state in {}: {err}", self.state_dir.display());
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
            .child(div().text_size(px(28.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("relics"))
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

impl LibraryWindow {
    /// The Release notice: a pill at the bottom right. A click opens the release page. The × hides it for that
    /// version only.
    fn release_notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let release = self.release.read(cx).release.clone()?;
        if self.state.hidden_release.as_ref() == Some(&release.version) {
            return None;
        }
        let t = cx.omarchy();
        let version = release.version.clone();
        Some(
            div()
                .absolute()
                .bottom(px(24.))
                .right(px(24.))
                .flex()
                .items_center()
                .gap(px(10.))
                .pl(px(16.))
                .pr(px(10.))
                .py(px(8.))
                .rounded_full()
                .bg(t.surface)
                .child(
                    div()
                        .id("release-notice")
                        .clickable()
                        .text_color(t.accent)
                        .on_click(move |_, _, cx| cx.open_url(&release.url))
                        .child(format!("Relics {} is out", release.version)),
                )
                .child(
                    div()
                        .id("release-notice-hide")
                        .clickable()
                        .px(px(4.))
                        .text_color(t.secondary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.hidden_release = Some(version.clone());
                            this.save_state();
                            cx.notify();
                        }))
                        .child("×"),
                ),
        )
    }
}

impl Render for LibraryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(view) = self.player_view.clone().filter(|_| self.player.read(cx).in_view()) {
            // No background and no other screen: the video shows through the Player view.
            return focus_scope("library").size_full().child(view).into_any_element();
        }
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
                    .child(div().text_color(t.secondary).child("Relics retries by itself."))
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
        let page = page.children(self.release_notice(cx));
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
        .into_any_element()
    }
}
