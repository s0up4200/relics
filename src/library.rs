//! The Library window. It picks the screen: add a Server, sign in, or Home.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Subscription, Task,
    Window, div, px,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button, focus_scope, input};

use crate::api::{self, Client, Device, Error, Server, Url};
use crate::home::{self, Home};
use crate::secrets;
use crate::sign_in::{SignIn, SignedIn};
use crate::state::{SavedAccount, SavedServer, State};

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
}

enum Screen {
    /// The user adds a Server.
    Connect,
    SignIn(Entity<SignIn>),
    Home(Entity<Home>),
    /// A full-window message, for example while the app restores the sign-in.
    Message(String),
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

    /// Keeps the new tokens after each refresh, and opens the Profile of the Account.
    fn open_account(&mut self, client: Arc<Client>, window: &mut Window, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let account_id = self.state.active.clone().map(|(_, account)| account).unwrap_or_default();
        let server_id = client.server().id.clone();
        client.on_tokens(move |tokens| {
            let (server_id, account_id, tokens) = (server_id.clone(), account_id.clone(), tokens.clone());
            executor.spawn(async move { secrets::save(&server_id, &account_id, &tokens).await.ok() }).detach();
        });
        self.screen = Screen::Message("Loading Profiles…".into());
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let profiles = client.profiles().await;
            this.update_in(cx, |this, window, cx| {
                this.screen = match profiles {
                    Ok(profiles) => match api::auto_profile(&profiles) {
                        Some(profile) => {
                            client.set_profile(Some(profile.id.to_string()));
                            if let Some(account) = this.state.active_account_mut() {
                                account.last_profile = Some(profile.id.to_string());
                            }
                            this.save_state();
                            let home = cx.new(|cx| Home::new(client.clone(), profile.name.clone(), window, cx));
                            let server = client.server().clone();
                            this._screen = Some(cx.subscribe_in(
                                &home,
                                window,
                                move |this, _, _: &home::SignedOut, window, cx| {
                                    this.show_sign_in(server.clone(), Some(Error::SignedOut.to_string()), window, cx);
                                },
                            ));
                            Screen::Home(home)
                        }
                        // ponytail: the Profile picker and the no-Profiles screen come in #23.
                        None if profiles.is_empty() => {
                            Screen::Message("This Account has no Profiles. Create one in the Silo web client.".into())
                        }
                        None => Screen::Message(
                            "This Account has more than one Profile or a PIN. The Profile picker is not ready yet."
                                .into(),
                        ),
                    },
                    Err(Error::SignedOut) => {
                        return this.show_sign_in(
                            client.server().clone(),
                            Some(Error::SignedOut.to_string()),
                            window,
                            cx,
                        );
                    }
                    Err(err) => Screen::Message(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
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
        match &self.screen {
            Screen::Connect => page.child(self.render_connect(window, cx)),
            Screen::SignIn(view) => page.child(view.clone()),
            Screen::Home(view) => page.child(view.clone()),
            Screen::Message(text) => page.child(div().max_w(px(560.)).text_color(t.secondary).child(text.clone())),
        }
    }
}
