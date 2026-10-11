//! The Library window views. For now the window shows the screen where the user
//! adds a Server.

use gpui::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Subscription, Task,
    Window, div, px,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button, focus_scope, input};

use crate::api::{self, Server};

pub struct LibraryWindow {
    url: Entity<InputState>,
    status: Status,
    /// The request in progress. A new request or a dropped window cancels it.
    connecting: Option<Task<()>>,
    _enter: Subscription,
}

enum Status {
    Idle,
    Connecting,
    Failed(String),
    Connected(Server),
}

impl LibraryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://silo.example.com"));
        url.update(cx, |url, cx| url.focus(window, cx));
        let enter = cx.subscribe_in(&url, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.connect(cx);
            }
        });
        Self { url, status: Status::Idle, connecting: None, _enter: enter }
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let http = cx.http_client();
        let text = self.url.read(cx).value().to_string();
        self.status = Status::Connecting;
        self.connecting = Some(cx.spawn(async move |this, cx| {
            let result = api::connect(http, &text).await;
            this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(server) => Status::Connected(server),
                    Err(err) => Status::Failed(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for LibraryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let (message, color) = match &self.status {
            Status::Idle => (String::new(), t.secondary),
            Status::Connecting => ("Connecting…".into(), t.secondary),
            Status::Failed(err) => (err.clone(), t.danger),
            Status::Connected(server) => (format!("Connected to {}", server.name), t.success),
        };
        focus_scope("library")
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
                    .child(div().text_size(px(28.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("silo"))
                    .child(div().text_size(px(13.)).text_color(t.secondary).child("Server address"))
                    .child(input("server-url", &self.url, window, cx))
                    .child(
                        button("connect", "Connect", ButtonVariant::Primary, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                    )
                    .child(div().min_h(px(18.)).text_size(px(13.)).text_color(color).child(message)),
            )
    }
}
