//! The Profile picker: a full-screen grid of the Profiles of the Account, with the PIN entry inline below the
//! selected Profile. An Account with no Profiles shows a message with Retry and Sign out.

use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement, Render, Styled, Subscription, Task, Window, div, px,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button, input};

use crate::api::{self, Client, PinCheck, wire};

/// The number of Profiles in each row of the picker.
const COLUMNS: usize = 5;

pub enum PickerEvent {
    /// The user chose a Profile. `token` is the PIN token when the user typed the PIN.
    Chosen {
        profile: Box<wire::Profile>,
        token: Option<String>,
    },
    /// The user wants to read the Profiles again.
    Retry,
    SignOut,
}

pub struct ProfilePicker {
    client: Arc<Client>,
    profiles: Vec<wire::Profile>,
    selected: usize,
    /// The PIN entry of the selected Profile, when it is open.
    pin: Option<(Entity<InputState>, Subscription)>,
    /// Why the last PIN check failed.
    message: Option<String>,
    focus_handle: FocusHandle,
    checking: Option<Task<()>>,
}

impl EventEmitter<PickerEvent> for ProfilePicker {}

/// Moves the selection for an arrow or hjkl key, in a grid of `len` Profiles with `COLUMNS` in each row.
pub fn move_choice(index: usize, key: &str, len: usize) -> usize {
    match key {
        "h" | "left" if !index.is_multiple_of(COLUMNS) => index - 1,
        "l" | "right" if index % COLUMNS < COLUMNS - 1 && index + 1 < len => index + 1,
        "k" | "up" if index >= COLUMNS => index - COLUMNS,
        "j" | "down" if index + COLUMNS < len => index + COLUMNS,
        "j" | "down" if index / COLUMNS < (len.max(1) - 1) / COLUMNS => len - 1,
        _ => index,
    }
}

impl ProfilePicker {
    /// Shows the Profiles. With `pin_for`, the PIN entry of that Profile is open.
    pub fn new(
        client: Arc<Client>,
        profiles: Vec<wire::Profile>,
        pin_for: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let selected = pin_for.and_then(|id| profiles.iter().position(|p| *p.id == id)).unwrap_or_default();
        let mut this = Self { client, profiles, selected, pin: None, message: None, focus_handle, checking: None };
        if pin_for.is_some() && this.profiles.get(selected).is_some_and(|p| p.has_pin) {
            this.open_pin(window, cx);
        }
        this
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.pin.is_some() {
            if key == "escape" {
                self.pin = None;
                self.message = None;
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
            return;
        }
        match key {
            "enter" if self.profiles.is_empty() => cx.emit(PickerEvent::Retry),
            "enter" => self.choose(window, cx),
            _ => {
                let selected = move_choice(self.selected, key, self.profiles.len());
                if selected == self.selected {
                    return;
                }
                self.selected = selected;
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.get(self.selected) else {
            return;
        };
        if profile.has_pin {
            self.open_pin(window, cx);
        } else {
            cx.emit(PickerEvent::Chosen { profile: Box::new(profile.clone()), token: None });
        }
    }

    fn open_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pin = cx.new(|cx| InputState::new(window, cx).placeholder("PIN").masked(true));
        pin.update(cx, |pin, cx| pin.focus(window, cx));
        let enter = cx.subscribe_in(&pin, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.check_pin(window, cx);
            }
        });
        self.pin = Some((pin, enter));
        self.message = None;
        cx.notify();
    }

    fn check_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((pin, _)), Some(profile)) = (&self.pin, self.profiles.get(self.selected).cloned()) else {
            return;
        };
        let text = pin.read(cx).value().to_string();
        if text.is_empty() || self.checking.is_some() {
            return;
        }
        let client = self.client.clone();
        self.message = None;
        self.checking = Some(cx.spawn_in(window, async move |this, cx| {
            let result = client.verify_pin(&profile.id, &text).await;
            this.update_in(cx, |this, window, cx| {
                this.checking = None;
                let message = match result {
                    Ok(PinCheck::Unlocked(token)) => {
                        return cx.emit(PickerEvent::Chosen { profile: Box::new(profile), token: Some(token) });
                    }
                    Ok(PinCheck::Wrong) => "Wrong PIN".to_string(),
                    Ok(PinCheck::Locked(seconds)) => api::lock_message(seconds),
                    Err(err) => err.to_string(),
                };
                this.message = Some(message);
                if let Some((pin, _)) = &this.pin {
                    pin.update(cx, |pin, cx| pin.set_value("", window, cx));
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn no_profiles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
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
                    .text_color(t.foreground)
                    .child("This account has no profiles. Create one in the Silo web client."),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(
                        button("retry", "Retry", ButtonVariant::Primary, cx)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Retry))),
                    )
                    .child(
                        button("sign-out", "Sign out", ButtonVariant::Secondary, cx)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::SignOut))),
                    ),
            )
    }

    fn card(&self, index: usize, profile: &wire::Profile, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.omarchy().clone();
        let selected = index == self.selected;
        let initial = profile.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let pin = self.pin.as_ref().filter(|_| selected).map(|(pin, _)| {
            let status = match (&self.checking, &self.message) {
                (Some(_), _) => Some(("Checking…".to_string(), t.secondary)),
                (None, Some(message)) => Some((message.clone(), t.danger)),
                (None, None) => None,
            };
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(input("pin", pin, window, cx))
                .children(status.map(|(text, color)| div().text_size(px(12.)).text_color(color).child(text)))
        });
        div()
            .w(px(150.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .size(px(120.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(60.))
                    .bg(t.surface)
                    .border_2()
                    .border_color(if selected { t.accent } else { gpui::transparent_black() })
                    .text_size(px(44.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.bright)
                    .child(initial),
            )
            .child(
                div()
                    .text_size(px(16.))
                    .text_color(if selected { t.bright } else { t.foreground })
                    .child(profile.name.clone()),
            )
            .children(profile.has_pin.then(|| div().text_size(px(11.)).text_color(t.secondary).child("PIN")))
            .children(pin)
            .into_any_element()
    }
}

impl Render for ProfilePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let page = div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(40.))
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font.clone());
        if self.profiles.is_empty() {
            return page.child(self.no_profiles(cx));
        }
        let profiles = self.profiles.clone();
        let mut cards = Vec::new();
        for (index, profile) in profiles.iter().enumerate() {
            cards.push(self.card(index, profile, window, cx));
        }
        page.child(
            div().text_size(px(32.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("Who is watching?"),
        )
        .child(
            div()
                .max_w(px(COLUMNS as f32 * 190.))
                .flex()
                .flex_wrap()
                .justify_center()
                .items_start()
                .gap(px(40.))
                .children(cards),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::move_choice;

    #[test]
    fn hjkl_and_arrows_move_in_the_grid_of_profiles() {
        // Seven Profiles: 0-4 in the first row, 5-6 in the second.
        assert_eq!(move_choice(0, "l", 7), 1);
        assert_eq!(move_choice(1, "left", 7), 0);
        assert_eq!(move_choice(0, "h", 7), 0, "the first Profile stays");
        assert_eq!(move_choice(4, "l", 7), 4, "the end of a row stays");
        assert_eq!(move_choice(6, "right", 7), 6, "the last Profile stays");
        assert_eq!(move_choice(1, "j", 7), 6);
        assert_eq!(move_choice(4, "down", 7), 6, "a short last row takes its last Profile");
        assert_eq!(move_choice(6, "k", 7), 1);
        assert_eq!(move_choice(2, "up", 7), 2);
        assert_eq!(move_choice(0, "j", 3), 0, "one row stays");
        assert_eq!(move_choice(0, "l", 0), 0);
    }
}
