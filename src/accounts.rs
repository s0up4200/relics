//! The Server list of the sidebar menu: each Server and Account pair, with "Add server" below. The user
//! chooses the pair that becomes the Active server.

use gpui::{
    Context, EventEmitter, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement,
    Render, StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_omarchy::ActiveTheme;

use crate::state::Pair;

pub enum AccountChoice {
    Open(Pair),
    Add,
    /// Esc: the user keeps the Active server.
    Back,
}

pub struct AccountPicker {
    pairs: Vec<Pair>,
    /// The pair of the Active server, which the list marks.
    active: Option<(String, String)>,
    /// The selected row. The row after the last pair is "Add server".
    selected: usize,
    focus_handle: FocusHandle,
}

impl EventEmitter<AccountChoice> for AccountPicker {}

impl AccountPicker {
    pub fn new(
        pairs: Vec<Pair>,
        active: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let selected = pairs.iter().position(|p| active == Some((p.server_id.clone(), p.account_id.clone())));
        Self { pairs, active, selected: selected.unwrap_or_default(), focus_handle }
    }

    fn choose(&mut self, row: usize, cx: &mut Context<Self>) {
        cx.emit(match self.pairs.get(row) {
            Some(pair) => AccountChoice::Open(pair.clone()),
            None => AccountChoice::Add,
        });
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "j" | "down" => self.selected = (self.selected + 1).min(self.pairs.len()),
            "k" | "up" => self.selected = self.selected.saturating_sub(1),
            "enter" => self.choose(self.selected, cx),
            "escape" | "backspace" => cx.emit(AccountChoice::Back),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for AccountPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let labels = self.pairs.iter().map(|p| {
            let active = self.active == Some((p.server_id.clone(), p.account_id.clone()));
            (p.server_name.clone(), Some(p.username.clone()), active)
        });
        let rows =
            labels.chain([("Add server".to_string(), None, false)]).enumerate().map(|(row, (name, user, active))| {
                div()
                    .id(("account", row))
                    .px(px(16.))
                    .py(px(12.))
                    .rounded(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .bg(t.surface)
                    .border_2()
                    .border_color(if row == self.selected { t.accent } else { gpui::transparent_black() })
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(row, cx)))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .text_color(t.bright)
                            .child(name)
                            .children(user.map(|user| div().text_size(px(12.)).text_color(t.secondary).child(user))),
                    )
                    .children(active.then(|| div().text_size(px(12.)).text_color(t.accent).child("Active")))
            });
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key))
            .w(px(420.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .pb(px(10.))
                    .text_size(px(28.))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(t.bright)
                    .child("Switch server"),
            )
            .children(rows)
    }
}
