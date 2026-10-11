//! The Library window. It picks the screen: add a Server, sign in, choose a Profile, or Home.

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

use crate::api::{self, Client, Device, Error, Server, Trouble, Url, wire};
use crate::home::{self, Home};
use crate::player::{PlaybackError, PlayerClosed, PlayerWindow};
use crate::profiles::{PickerEvent, ProfilePicker};
use crate::secrets;
use crate::sign_in::{SignIn, SignedIn};
use crate::state::{SavedAccount, SavedServer, State};

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
    /// The subscription to the Play events of Home.
    _play: Option<Subscription>,
    /// The Profiles of the active Account.
    profiles: Vec<wire::Profile>,
    /// A request that waits for the PIN of its Profile.
    pin_wait: Option<PinWait>,
    _pin_requests: Option<Task<()>>,
}

struct PinWait {
    profile_id: String,
    reply: oneshot::Sender<Option<String>>,
    /// The Home that sent the request. It comes back after the PIN entry.
    home: Entity<Home>,
}

enum Screen {
    /// The user adds a Server.
    Connect,
    SignIn(Entity<SignIn>),
    Home(Entity<Home>),
    Profiles(Entity<ProfilePicker>),
    /// A full-window message, for example while the app restores the sign-in.
    Message(String),
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
            _play: None,
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
        // After a refused refresh, the new client sends the positions that the old client kept.
        if let Some(old) = self.client.take()
            && old.server().id == server.id
            && self.account_id() == *event.account.id
        {
            event.client.adopt_pending(old.take_pending());
        }
        self.state.remember(
            SavedServer { id: server.id.clone(), url: server.url.to_string(), accounts: Vec::new() },
            SavedAccount {
                id: event.account.id.to_string(),
                username: event.account.username.clone(),
                last_profile: None,
            },
        );
        self.save_state();
        let (server_id, account_id, tokens) = (server.id.clone(), event.account.id.to_string(), event.client.tokens());
        // When the Secret Service is missing, the tokens stay only in memory. The sign-in screen told the user why.
        cx.background_spawn(async move { secrets::save(&server_id, &account_id, &tokens).await.ok() }).detach();
        self.open_account(event.client.clone(), window, cx);
    }

    /// Keeps the new tokens after each refresh, and opens the Profile of the Account: the last Profile, the
    /// only Profile without a PIN, or else the Profile picker.
    fn open_account(&mut self, client: Arc<Client>, window: &mut Window, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let account_id = self.account_id();
        let server_id = client.server().id.clone();
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
        let home = cx.new(|cx| Home::new(client.clone(), profile.name.clone(), window, cx));
        self.show_home(client, home, cx);
    }

    fn show_home(&mut self, client: Arc<Client>, home: Entity<Home>, cx: &mut Context<Self>) {
        self._play = Some(cx.subscribe(&home, {
            let client = client.clone();
            move |this, _, home::Play(item), cx| {
                this.player_error = None;
                this.player.update(cx, |player, cx| player.play(client.clone(), item.into(), cx));
            }
        }));
        self._screen = None;
        self.screen = Screen::Home(home);
        cx.notify();
    }

    /// Shows the Profile picker. With `pin_for`, the PIN entry of that Profile is open.
    fn show_picker(&mut self, client: Arc<Client>, pin_for: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let profiles = self.profiles.clone();
        let view = cx.new(|cx| ProfilePicker::new(client.clone(), profiles, pin_for, window, cx));
        self._screen = Some(cx.subscribe_in(&view, window, move |this, _, event: &PickerEvent, window, cx| {
            match event {
                PickerEvent::Chosen { profile, token } => this.chosen(client.clone(), profile, token, window, cx),
                PickerEvent::Retry => this.open_account(client.clone(), window, cx),
                // ponytail: no server sign-out and no keyring delete yet; #35 adds them.
                PickerEvent::SignOut => this.show_sign_in(client.server().clone(), None, window, cx),
            }
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
        match self.pin_wait.take() {
            Some(wait) if wait.profile_id == *profile.id => {
                wait.reply.send(token.clone()).ok();
                self.show_home(client, wait.home, cx);
            }
            _ => self.open_profile(client, profile.clone(), token.clone(), window, cx),
        }
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
                    // Only Home sends profile requests. On another screen the request fails.
                    let Screen::Home(home) = &this.screen else {
                        return;
                    };
                    this.pin_wait = Some(PinWait { profile_id: profile_id.clone(), reply, home: home.clone() });
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
            Screen::Connect => page.child(self.render_connect(window, cx)),
            Screen::SignIn(view) => page.child(view.clone()),
            Screen::Home(view) => page.child(view.clone()),
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
