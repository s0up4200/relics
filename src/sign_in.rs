//! The sign-in screen: the username and password of an Account on one Server.

use std::sync::Arc;

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, div, px,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button, focus_scope, input};

use crate::api::{Client, Device, Server, wire};

/// The user signed in.
pub struct SignedIn {
    pub client: Arc<Client>,
    pub account: wire::Account,
}

pub struct SignIn {
    server: Server,
    device: Device,
    username: Entity<InputState>,
    password: Entity<InputState>,
    /// Why the user must sign in, or why the last try failed.
    message: Option<String>,
    /// Shown when the Secret Service is missing.
    keyring_note: Option<String>,
    signing_in: Option<Task<()>>,
    _enter: [Subscription; 2],
}

impl EventEmitter<SignedIn> for SignIn {}

/// The text that tells the user why the app cannot keep the sign-in.
const NO_KEYRING: &str = "The Secret Service keyring is not available, so silo-omarchy keeps your sign-in in memory. You must sign in again at each launch.";

impl SignIn {
    pub fn new(
        server: Server,
        device: Device,
        username: Option<String>,
        message: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prefilled = username.is_some();
        let username = cx
            .new(|cx| InputState::new(window, cx).placeholder("Username").default_value(username.unwrap_or_default()));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("Password").masked(true));
        let first = if prefilled { &password } else { &username };
        first.update(cx, |input, cx| input.focus(window, cx));
        let on_enter =
            |this: &mut Self, _: &Entity<InputState>, event: &InputEvent, _: &mut Window, cx: &mut Context<Self>| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.sign_in(cx);
                }
            };
        let enter = [cx.subscribe_in(&username, window, on_enter), cx.subscribe_in(&password, window, on_enter)];
        cx.spawn(async |this, cx| {
            let missing = crate::secrets::check().await.is_err();
            this.update(cx, |this, cx| {
                this.keyring_note = missing.then(|| NO_KEYRING.to_string());
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self { server, device, username, password, message, keyring_note: None, signing_in: None, _enter: enter }
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        let username = self.username.read(cx).value().trim().to_string();
        let password = self.password.read(cx).value().to_string();
        if username.is_empty() || password.is_empty() {
            self.message = Some("Type your username and password.".into());
            cx.notify();
            return;
        }
        let (http, server, device) = (cx.http_client(), self.server.clone(), self.device.clone());
        self.message = None;
        self.signing_in = Some(cx.spawn(async move |this, cx| {
            let result = Client::login(http, server, device, &username, &password).await;
            this.update(cx, |this, cx| {
                this.signing_in = None;
                match result {
                    Ok((client, account)) => cx.emit(SignedIn { client: Arc::new(client), account }),
                    Err(err) => this.message = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for SignIn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let note = |text: &Option<String>, color| div().text_size(px(13.)).text_color(color).children(text.clone());
        focus_scope("sign-in")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font.clone())
            .child(
                div()
                    .w(px(420.))
                    .p(px(32.))
                    .gap(px(14.))
                    .flex()
                    .flex_col()
                    .rounded(px(16.))
                    .bg(t.surface)
                    .child(
                        div().text_size(px(28.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("silo"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.secondary)
                            .child(format!("Sign in to {}", self.server.name)),
                    )
                    .child(input("username", &self.username, window, cx))
                    .child(input("password", &self.password, window, cx))
                    .child(
                        button("sign-in", "Sign in", ButtonVariant::Primary, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                    )
                    .child(
                        match self.signing_in {
                            Some(_) => note(&Some("Signing in…".into()), t.secondary),
                            None => note(&self.message, t.danger),
                        }
                        .min_h(px(18.)),
                    )
                    .child(note(&self.keyring_note, t.warning)),
            )
    }
}
