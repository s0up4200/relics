//! The sign-in screen: the username and password of an Account on one Server, or
//! device sign-in with a code that the user approves on another device.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, div, px,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{ActiveTheme, ButtonVariant, ChoiceItem, button, button_group, focus_scope, input};

use crate::api::{self, Client, Device, DeviceLogin, Error, Server, wire};

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
    /// The providers that take a password. The screen shows a choice only when there is more than one.
    providers: Vec<wire::AuthProvider>,
    /// The chosen entry of the provider choice: 0 is "Automatic", `n` is `providers[n - 1]`.
    provider: usize,
    /// False when the server has no provider that takes a password, for example with OIDC only.
    password_form: bool,
    /// The device sign-in in progress.
    code: Option<DeviceCode>,
    /// The Account signed in, but it has a temporary password.
    password_change: bool,
    /// Why the user must sign in, or why the last try failed.
    message: Option<String>,
    /// Shown when the Secret Service is missing.
    keyring_note: Option<String>,
    signing_in: Option<Task<()>>,
    _enter: [Subscription; 2],
}

struct DeviceCode {
    /// The code in two groups of four, or empty while the app asks for a code.
    code: String,
    verification_uri: String,
    status: String,
}

impl EventEmitter<SignedIn> for SignIn {}

/// The text that tells the user why the app cannot keep the sign-in.
const NO_KEYRING: &str = "The Secret Service keyring is not available, so silo-omarchy keeps your sign-in in memory. You must sign in again at each launch.";

const WAITING: &str = "Waiting for approval…";

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
        let (http, list_server, list_device) = (cx.http_client(), server.clone(), device.clone());
        cx.spawn(async move |this, cx| {
            // Without the list, the screen keeps the password form and shows no choice.
            let Ok(providers) = api::password_providers(&http, &list_server, &list_device).await else { return };
            this.update(cx, |this, cx| {
                this.password_form = !providers.is_empty();
                this.providers = providers;
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            server,
            device,
            username,
            password,
            providers: Vec::new(),
            provider: 0,
            password_form: true,
            code: None,
            password_change: false,
            message,
            keyring_note: None,
            signing_in: None,
            _enter: enter,
        }
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        let username = self.username.read(cx).value().trim().to_string();
        let password = self.password.read(cx).value().to_string();
        if username.is_empty() || password.is_empty() {
            self.message = Some("Type your username and password.".into());
            cx.notify();
            return;
        }
        let provider = self.provider.checked_sub(1).and_then(|i| self.providers.get(i)).map(|p| p.id.clone());
        let (http, server, device) = (cx.http_client(), self.server.clone(), self.device.clone());
        self.message = None;
        self.signing_in = Some(cx.spawn(async move |this, cx| {
            let result = Client::login(http, server, device, &username, &password, provider.as_deref()).await;
            this.update(cx, |this, cx| {
                this.signing_in = None;
                match result {
                    Ok((client, account)) => this.signed_in(client, account, cx),
                    Err(err) => this.message = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Gets a device sign-in code and polls until the user approves it on another device.
    fn sign_in_with_code(&mut self, cx: &mut Context<Self>) {
        let (http, server, device) = (cx.http_client(), self.server.clone(), self.device.clone());
        self.message = None;
        self.code =
            Some(DeviceCode {
                code: String::new(), verification_uri: String::new(), status: "Getting a code…".into()
            });
        self.signing_in = Some(cx.spawn(async move |this, cx| {
            let start = match api::start_device_login(&http, &server, &device).await {
                Ok(start) => start,
                Err(err) => {
                    this.update(cx, |this, cx| this.stop_code(Some(err.to_string()), cx)).ok();
                    return;
                }
            };
            let code = DeviceCode {
                code: api::user_code(&start.user_code),
                verification_uri: start.verification_uri.clone(),
                status: WAITING.into(),
            };
            this.update(cx, |this, cx| {
                this.code = Some(code);
                cx.notify();
            })
            .ok();
            let mut wait = Duration::from_secs(start.interval.max(1) as u64);
            loop {
                cx.background_executor().timer(wait).await;
                let result =
                    api::poll_device_login(http.clone(), server.clone(), device.clone(), &start.device_code).await;
                let next = this.update(cx, |this, cx| this.polled(result, wait, cx));
                match next {
                    Ok(Some(next)) => wait = next,
                    _ => return,
                }
            }
        }));
        cx.notify();
    }

    /// Shows the result of one poll. Returns the time until the next poll, or `None` to stop.
    fn polled(
        &mut self,
        result: Result<DeviceLogin, Error>,
        wait: Duration,
        cx: &mut Context<Self>,
    ) -> Option<Duration> {
        let status = |this: &mut Self, text: String| {
            if let Some(code) = &mut this.code {
                code.status = text;
            }
        };
        let next = match result {
            Ok(DeviceLogin::Waiting { opened, poll_after }) => {
                status(self, if opened { "Continue on your other device." } else { WAITING }.into());
                Some(poll_after)
            }
            // The poll got no reply. The code is still valid, so poll again.
            Err(err @ Error::Network(_)) => {
                status(self, err.to_string());
                Some(wait)
            }
            Ok(DeviceLogin::SignedIn(client, account)) => {
                self.stop_code(None, cx);
                self.signed_in(*client, account, cx);
                None
            }
            Ok(DeviceLogin::Ended(text)) => {
                self.stop_code(Some(text.into()), cx);
                None
            }
            Err(err) => {
                self.stop_code(Some(err.to_string()), cx);
                None
            }
        };
        cx.notify();
        next
    }

    /// Ends device sign-in and shows the password form again.
    // ponytail: no cancelDeviceLogin call; an abandoned code expires on the server after 15 minutes.
    fn stop_code(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        self.code = None;
        self.signing_in = None;
        self.message = message;
        cx.notify();
    }

    /// Opens the Account, unless it has a temporary password. Such an Account can do
    /// nothing in the app until the user sets a new password in the web client.
    fn signed_in(&mut self, client: Client, account: wire::Account, cx: &mut Context<Self>) {
        if account.password_change_required {
            self.password_change = true;
            return;
        }
        cx.emit(SignedIn { client: Arc::new(client), account });
    }

    /// Drops the session of an Account with a temporary password and shows the form again.
    // ponytail: the app forgets the tokens but does not call logout; #35 adds the server logout.
    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.password_change = false;
        self.message = None;
        self.password.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn form(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        if self.providers.len() > 1 {
            let items = std::iter::once(ChoiceItem::new("", "Automatic"))
                .chain(self.providers.iter().map(|p| ChoiceItem::new(p.id.clone(), p.display_name.clone())))
                .collect();
            let view = cx.entity().downgrade();
            let on_change = move |index, _: &mut Window, cx: &mut gpui::App| {
                view.update(cx, |this, cx| {
                    this.provider = index;
                    cx.notify();
                })
                .ok();
            };
            rows.push(button_group("provider", items, Some(self.provider), on_change, window, cx).into_any_element());
        }
        if self.password_form {
            rows.push(input("username", &self.username, window, cx).into_any_element());
            rows.push(input("password", &self.password, window, cx).into_any_element());
            rows.push(
                button("sign-in", "Sign in", ButtonVariant::Primary, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx)))
                    .into_any_element(),
            );
        }
        let variant = if self.password_form { ButtonVariant::Outline } else { ButtonVariant::Primary };
        rows.push(
            button("sign-in-with-code", "Sign in with a code", variant, cx)
                .on_click(cx.listener(|this, _, _, cx| this.sign_in_with_code(cx)))
                .into_any_element(),
        );
        rows
    }
}

impl Render for SignIn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let note = |text: &Option<String>, color| div().text_size(px(13.)).text_color(color).children(text.clone());
        let body: Vec<gpui::AnyElement> = if self.password_change {
            vec![
                div().child("Set a new password in the Silo web client, then sign in again.").into_any_element(),
                button("sign-out", "Sign out", ButtonVariant::Primary, cx)
                    .on_click(cx.listener(|this, _, window, cx| this.sign_out(window, cx)))
                    .into_any_element(),
            ]
        } else if let Some(code) = &self.code {
            let mut rows = vec![
                div()
                    .text_size(px(40.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.bright)
                    .child(code.code.clone())
                    .into_any_element(),
            ];
            if !code.verification_uri.is_empty() {
                rows.push(
                    div()
                        .text_size(px(13.))
                        .child(format!("Open {} on another device and type this code.", code.verification_uri))
                        .into_any_element(),
                );
            }
            rows.push(note(&Some(code.status.clone()), t.secondary).into_any_element());
            rows.push(
                button("cancel-code", "Cancel", ButtonVariant::Secondary, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.stop_code(None, cx)))
                    .into_any_element(),
            );
            rows
        } else {
            let mut rows = self.form(window, cx);
            rows.push(
                match self.signing_in {
                    Some(_) => note(&Some("Signing in…".into()), t.secondary),
                    None => note(&self.message, t.danger),
                }
                .min_h(px(18.))
                .into_any_element(),
            );
            rows
        };
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
                    .children(body)
                    .child(note(&self.keyring_note, t.warning)),
            )
    }
}
