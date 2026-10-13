//! The sign-in screen of an Account on one Server. It shows each method that the Server offers: Network
//! sign-in, Browser sign-in with each OIDC provider, the username and password, and device sign-in with a code
//! that the user approves on another device.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, div, px, svg,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{
    ActiveTheme, Button, ButtonVariant, ChoiceItem, IconName, button, button_group, focus_scope, icon, input,
};

use crate::api::{self, BrowserReturn, BrowserSignIn, Client, Device, DeviceLogin, Error, Server, Url, wire};

/// The user signed in.
pub struct SignedIn {
    pub client: Arc<Client>,
    pub account: wire::Account,
}

pub struct SignIn {
    server: Server,
    device: Device,
    /// The app already has an Account on the Server, so Browser sign-in asks the provider for its account
    /// chooser.
    has_account: bool,
    username: Entity<InputState>,
    password: Entity<InputState>,
    /// The sign-in methods of the Server. Until the list arrives, the screen shows the password form only.
    methods: api::SignInMethods,
    /// The chosen entry of the provider choice: 0 is "Automatic", `n` is `methods.password[n - 1]`.
    provider: usize,
    /// False when the server has no provider that takes a password, for example with OIDC only.
    password_form: bool,
    /// The device sign-in in progress.
    code: Option<DeviceCode>,
    /// The Browser sign-in that waits for the browser to return.
    browser: Option<Browser>,
    /// The sign-in of an Account with a temporary password. Sign out ends it on the server.
    password_change: Option<Client>,
    /// Why the user must sign in, or why the last try failed.
    message: Option<String>,
    /// Shown when the Secret Service is missing.
    keyring_note: Option<String>,
    signing_in: Option<Task<()>>,
    _enter: [Subscription; 2],
}

struct Browser {
    sign_in: BrowserSignIn,
    /// The page that "Open again" opens.
    url: Url,
    /// Ends the wait after `api::BROWSER_SIGN_IN_TIMEOUT`.
    _timeout: Task<()>,
}

struct DeviceCode {
    /// The code in two groups of four, or empty while the app asks for a code.
    code: String,
    verification_uri: String,
    status: String,
}

impl EventEmitter<SignedIn> for SignIn {}

/// The text that tells the user why the app cannot keep the sign-in.
const NO_KEYRING: &str = "The Secret Service keyring is not available, so Relics keeps your sign-in in memory. You must sign in again at each launch.";

const WAITING: &str = "Waiting for approval…";

impl SignIn {
    pub fn new(
        server: Server,
        device: Device,
        has_account: bool,
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
            let Ok(methods) = api::sign_in_methods(&http, &list_server, &list_device).await else { return };
            this.update(cx, |this, cx| {
                this.password_form = !methods.password.is_empty();
                this.methods = methods;
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            server,
            device,
            has_account,
            username,
            password,
            methods: api::SignInMethods::default(),
            provider: 0,
            password_form: true,
            code: None,
            browser: None,
            password_change: None,
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
        let provider = self.provider.checked_sub(1).and_then(|i| self.methods.password.get(i)).map(|p| p.id.clone());
        let (http, server, device) = (cx.http_client(), self.server.clone(), self.device.clone());
        self.message = None;
        self.signing_in = Some(cx.spawn(async move |this, cx| {
            let result = Client::login(http, server, device, &username, &password, provider.as_deref()).await;
            this.update(cx, |this, cx| this.finish(result.map_err(|err| err.to_string()), cx)).ok();
        }));
        cx.notify();
    }

    /// Network sign-in: one click, no password.
    fn sign_in_with_network(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.methods.network.clone() else { return };
        let (http, server, device) = (cx.http_client(), self.server.clone(), self.device.clone());
        self.message = None;
        self.signing_in = Some(cx.spawn(async move |this, cx| {
            let result = Client::network_sign_in(http, server, device, &provider).await;
            this.update(cx, |this, cx| this.finish(result.map_err(|err| err.to_string()), cx)).ok();
        }));
        cx.notify();
    }

    /// Browser sign-in with the OIDC provider `methods.oauth[index]`. When the Server cannot return the result
    /// to an app, it runs Sign in with a code and opens the approval page in the browser.
    fn sign_in_with_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(provider) = self.methods.oauth.get(index).cloned() else { return };
        let (http, server, device) = (cx.http_client(), self.server.clone(), self.device.clone());
        let has_account = self.has_account;
        self.message = None;
        self.signing_in = Some(cx.spawn(async move |this, cx| {
            let started = BrowserSignIn::start(&http, &server, &device, &provider, has_account).await;
            this.update(cx, |this, cx| {
                this.signing_in = None;
                match started {
                    Ok(Some((sign_in, url))) => this.wait_for_browser(sign_in, url, cx),
                    Ok(None) => this.sign_in_with_code(true, cx),
                    Err(err) => this.message = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Opens the sign-in page in the browser and waits for the return link. A new start replaces the wait.
    fn wait_for_browser(&mut self, sign_in: BrowserSignIn, url: Url, cx: &mut Context<Self>) {
        cx.open_url(url.as_str());
        let timeout = cx.spawn(async |this, cx| {
            cx.background_executor().timer(api::BROWSER_SIGN_IN_TIMEOUT).await;
            this.update(cx, |this, cx| this.stop_browser(Some(api::BROWSER_TIMED_OUT.into()), cx)).ok();
        });
        self.browser = Some(Browser { sign_in, url, _timeout: timeout });
    }

    /// Takes a return link of the browser. With no Browser sign-in waiting, for example after Cancel, the link
    /// does nothing.
    pub fn browser_returned(&mut self, link: &str, cx: &mut Context<Self>) {
        let Some(browser) = &self.browser else { return };
        let code = match browser.sign_in.check(link) {
            BrowserReturn::NotOurs => return,
            BrowserReturn::Failed(text) => return self.stop_browser(Some(text), cx),
            BrowserReturn::Code(code) => code,
        };
        // The wait ends here, so that a second copy of the link cannot redeem the code again.
        let Some(Browser { sign_in, .. }) = self.browser.take() else { return };
        let (http, device) = (cx.http_client(), self.device.clone());
        self.signing_in = Some(cx.spawn(async move |this, cx| {
            let result = sign_in.redeem(http, device, &code).await;
            this.update(cx, |this, cx| this.finish(result, cx)).ok();
        }));
        cx.notify();
    }

    /// Ends the Browser sign-in and shows the methods again.
    fn stop_browser(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        self.browser = None;
        self.message = message;
        cx.notify();
    }

    /// Gets a device sign-in code and polls until the user approves it on another device. With `open`, the app
    /// opens the approval page of the code in the browser.
    fn sign_in_with_code(&mut self, open: bool, cx: &mut Context<Self>) {
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
                if open {
                    cx.open_url(&start.verification_uri_complete);
                }
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

    /// Shows the result of a sign-in request: the Account, or the text of the error.
    fn finish(&mut self, result: Result<(Client, wire::Account), String>, cx: &mut Context<Self>) {
        self.signing_in = None;
        match result {
            Ok((client, account)) => self.signed_in(client, account, cx),
            Err(text) => self.message = Some(text),
        }
        cx.notify();
    }

    /// Opens the Account, unless it has a temporary password. Such an Account can do
    /// nothing in the app until the user sets a new password in the web client.
    fn signed_in(&mut self, client: Client, account: wire::Account, cx: &mut Context<Self>) {
        if account.password_change_required {
            self.password_change = Some(client);
            return;
        }
        cx.emit(SignedIn { client: Arc::new(client), account });
    }

    /// Ends the session of an Account with a temporary password on the server and shows the form again.
    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(client) = self.password_change.take() {
            cx.background_spawn(async move {
                if let Err(err) = client.sign_out().await {
                    eprintln!("relics: the server sign-out failed: {err}");
                }
            })
            .detach();
        }
        self.message = None;
        self.password.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// The methods, top to bottom: Network sign-in, each OIDC provider, the password form, and Sign in with a
    /// code. The first button is the primary one.
    fn form(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = cx.omarchy().clone();
        let mut rows = Vec::new();
        let variant = |rows: &Vec<gpui::AnyElement>| {
            if rows.is_empty() { ButtonVariant::Primary } else { ButtonVariant::Outline }
        };
        if let Some(provider) = &self.methods.network {
            let variant = variant(&rows);
            let color = if variant == ButtonVariant::Primary { t.accent } else { t.foreground };
            rows.push(
                method_button("network", continue_as(provider), tailscale_logo(color), variant, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.sign_in_with_network(cx)))
                    .into_any_element(),
            );
        }
        for (index, provider) in self.methods.oauth.iter().enumerate() {
            let label = format!("Sign in with {}", provider.display_name);
            rows.push(
                method_button(("oauth", index), label, icon(IconName::KeyRound), variant(&rows), cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.sign_in_with_provider(index, cx)))
                    .into_any_element(),
            );
        }
        if self.password_form {
            let variant = variant(&rows);
            if self.methods.password.len() > 1 {
                let items = std::iter::once(ChoiceItem::new("", "Automatic"))
                    .chain(self.methods.password.iter().map(|p| ChoiceItem::new(p.id.clone(), p.display_name.clone())))
                    .collect();
                let view = cx.entity().downgrade();
                let on_change = move |index, _: &mut Window, cx: &mut gpui::App| {
                    view.update(cx, |this, cx| {
                        this.provider = index;
                        cx.notify();
                    })
                    .ok();
                };
                rows.push(
                    button_group("provider", items, Some(self.provider), on_change, window, cx).into_any_element(),
                );
            }
            rows.push(input("username", &self.username, window, cx).into_any_element());
            rows.push(input("password", &self.password, window, cx).into_any_element());
            rows.push(
                button("sign-in", "Sign in", variant, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx)))
                    .into_any_element(),
            );
        }
        rows.push(
            method_button(
                "sign-in-with-code",
                "Sign in with a code",
                icon(IconName::MonitorSmartphone),
                variant(&rows),
                cx,
            )
            .on_click(cx.listener(|this, _, _, cx| this.sign_in_with_code(false, cx)))
            .into_any_element(),
        );
        rows
    }

    /// The waiting screen of a Browser sign-in.
    fn browser_wait(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = cx.omarchy().clone();
        vec![
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(t.bright)
                .child("Continue in your browser")
                .into_any_element(),
            div()
                .text_size(px(13.))
                .child("Sign in at the provider. The app continues when the browser returns.")
                .into_any_element(),
            button("open-again", "Open again", ButtonVariant::Primary, cx)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(browser) = &this.browser {
                        cx.open_url(browser.url.as_str());
                    }
                }))
                .into_any_element(),
            button("cancel-browser", "Cancel", ButtonVariant::Secondary, cx)
                .on_click(cx.listener(|this, _, _, cx| this.stop_browser(None, cx)))
                .into_any_element(),
        ]
    }
}

/// A button with an icon before its label.
fn method_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    icon: impl IntoElement,
    variant: ButtonVariant,
    cx: &gpui::App,
) -> Button {
    let label = label.into();
    button(id, "", variant, cx).accessibility_label(label.clone()).child(icon).child(label)
}

/// The label of the Network sign-in button: "Continue as <name> (<username>)", with the parts that the Server
/// gives.
fn continue_as(provider: &wire::AuthProvider) -> String {
    let (name, username) = provider
        .network_identity
        .as_ref()
        .map(|who| (who.display_name.as_str(), who.username.as_str()))
        .unwrap_or_default();
    match (name.is_empty(), username.is_empty()) {
        (false, false) => format!("Continue as {name} ({username})"),
        (false, true) => format!("Continue as {name}"),
        (true, false) => format!("Continue as {username}"),
        (true, true) => format!("Continue with {}", provider.display_name),
    }
}

/// The Tailscale logo. GPUI draws it as a mask in `color`, and the faint dots keep their opacity.
fn tailscale_logo(color: Hsla) -> impl IntoElement {
    svg().path("icons/tailscale.svg").size(px(16.)).flex_shrink_0().text_color(color)
}

impl Render for SignIn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let note = |text: &Option<String>, color| div().text_size(px(13.)).text_color(color).children(text.clone());
        let body: Vec<gpui::AnyElement> = if self.password_change.is_some() {
            vec![
                div().child("Set a new password in the Silo web client, then sign in again.").into_any_element(),
                button("sign-out", "Sign out", ButtonVariant::Primary, cx)
                    .on_click(cx.listener(|this, _, window, cx| this.sign_out(window, cx)))
                    .into_any_element(),
            ]
        } else if self.browser.is_some() {
            self.browser_wait(cx)
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
                        div()
                            .text_size(px(28.))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(t.bright)
                            .child("relics"),
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
