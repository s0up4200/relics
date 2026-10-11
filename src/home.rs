//! Home: the sidebar, and the Home sections of the active Profile as rows that scroll
//! sideways. A Featured section shows as a hero above the other rows.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, EventEmitter, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement, Render, ScrollHandle, StatefulInteractiveElement as _, Styled, Task, Window, div, px,
};
use gpui_omarchy::ActiveTheme;

use crate::api::{Client, Error, HomeSection, wire};

/// A request failed because the server refused the refresh token.
pub struct SignedOut;

/// The user pressed p on a card.
pub struct Play(pub wire::CatalogItem);

pub struct Home {
    client: Arc<Client>,
    profile_name: String,
    /// The sections in display order: the Featured sections first, then the others in layout order.
    sections: Vec<HomeSection>,
    status: Option<String>,
    focus: Focus,
    focus_handle: FocusHandle,
    page_scroll: ScrollHandle,
    row_scrolls: Vec<ScrollHandle>,
    _loading: Task<()>,
}

impl EventEmitter<SignedOut> for Home {}
impl EventEmitter<Play> for Home {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Card { row: usize, col: usize },
}

/// Moves the focus for an arrow or hjkl key. `rows` holds the number of cards in each row.
pub fn move_focus(focus: Focus, key: &str, rows: &[usize]) -> Focus {
    let first_card_in = |mut range: Box<dyn Iterator<Item = usize>>, col: usize| {
        range.find(|&row| rows[row] > 0).map(|row| Focus::Card { row, col: col.min(rows[row] - 1) })
    };
    let moved = match (focus, key) {
        (Focus::Sidebar, "l" | "right") => first_card_in(Box::new(0..rows.len()), 0),
        (Focus::Card { col: 0, .. }, "h" | "left") => Some(Focus::Sidebar),
        (Focus::Card { row, col }, "h" | "left") => Some(Focus::Card { row, col: col - 1 }),
        (Focus::Card { row, col }, "l" | "right") if col + 1 < rows[row] => Some(Focus::Card { row, col: col + 1 }),
        (Focus::Card { row, col }, "j" | "down") => first_card_in(Box::new(row + 1..rows.len()), col),
        (Focus::Card { row, col }, "k" | "up") => first_card_in(Box::new((0..row).rev()), col),
        _ => None,
    };
    moved.unwrap_or(focus)
}

impl Home {
    pub fn new(client: Arc<Client>, profile_name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let loading = cx.spawn({
            let client = client.clone();
            async move |this, cx| {
                let result = client.home().await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(mut sections) => {
                            sections.sort_by_key(|s| !s.featured);
                            this.row_scrolls = sections.iter().map(|_| ScrollHandle::new()).collect();
                            this.sections = sections;
                            this.status = None;
                            this.focus = move_focus(Focus::Sidebar, "l", &this.row_lengths());
                        }
                        Err(Error::SignedOut) => cx.emit(SignedOut),
                        Err(err) => this.status = Some(err.to_string()),
                    }
                    cx.notify();
                })
                .ok();
            }
        });
        Self {
            client,
            profile_name,
            sections: Vec::new(),
            status: Some("Loading…".into()),
            focus: Focus::Sidebar,
            focus_handle,
            page_scroll: ScrollHandle::new(),
            row_scrolls: Vec::new(),
            _loading: loading,
        }
    }

    fn row_lengths(&self) -> Vec<usize> {
        self.sections.iter().map(|s| s.items.len()).collect()
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let (Focus::Card { row, col }, "p") = (self.focus, event.keystroke.key.as_str()) {
            cx.emit(Play(self.sections[row].items[col].clone()));
            cx.stop_propagation();
            return;
        }
        let focus = move_focus(self.focus, event.keystroke.key.as_str(), &self.row_lengths());
        if focus == self.focus {
            return;
        }
        if let Focus::Card { row, col } = focus {
            self.page_scroll.scroll_to_item(row);
            self.row_scrolls[row].scroll_to_item(col);
        }
        self.focus = focus;
        cx.stop_propagation();
        cx.notify();
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let selected = self.focus == Focus::Sidebar;
        div()
            .w(px(240.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .p(px(14.))
            .gap(px(4.))
            .bg(t.inset)
            .border_r_1()
            .border_color(t.divider())
            .child(
                div()
                    .px(px(10.))
                    .pb(px(14.))
                    .text_size(px(22.))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(t.bright)
                    .child("silo"),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(t.selected_fill())
                    .text_color(t.bright)
                    .border_2()
                    .border_color(if selected { t.accent } else { gpui::transparent_black() })
                    .child("Home"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .px(px(10.))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(14.)).text_color(t.bright).child(self.profile_name.clone()))
                    .child(div().text_size(px(11.)).text_color(t.secondary).child(self.client.server().name.clone())),
            )
    }

    fn row(&self, index: usize, section: &HomeSection, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let (width, height, title_size) = if section.featured { (560., 300., 32.) } else { (150., 225., 14.) };
        let cards = section.items.iter().enumerate().map(|(col, item)| {
            let focused = self.focus == Focus::Card { row: index, col };
            div()
                .w(px(width))
                .h(px(height))
                .flex_none()
                .flex()
                .items_end()
                .p(px(14.))
                .rounded(px(12.))
                .bg(t.surface)
                .border_2()
                .border_color(if focused { t.accent } else { gpui::transparent_black() })
                .child(
                    div()
                        .text_size(px(title_size))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if focused { t.bright } else { t.foreground })
                        .child(item.title.clone()),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .px(px(40.))
                    .flex()
                    .items_baseline()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.bright)
                            .child(section.title.clone()),
                    )
                    .children(
                        section.view_all.then(|| div().text_size(px(11.)).text_color(t.secondary).child("VIEW ALL")),
                    ),
            )
            .child(
                div()
                    .id(("home-row", index))
                    .px(px(40.))
                    .flex()
                    .gap(px(14.))
                    .overflow_x_scroll()
                    .track_scroll(&self.row_scrolls[index])
                    .children(cards),
            )
            .into_any_element()
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let rows: Vec<_> = self.sections.iter().enumerate().map(|(i, s)| self.row(i, s, cx)).collect();
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font.clone())
            .text_size(px(14.))
            .child(self.sidebar(cx))
            .child(
                div()
                    .id("home-page")
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(36.))
                    .pt(px(32.))
                    .pb(px(80.))
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .children(rows)
                    .children(
                        self.status.clone().map(|status| div().px(px(40.)).text_color(t.secondary).child(status)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Focus, move_focus};

    #[test]
    fn hjkl_and_arrows_move_between_the_sidebar_and_the_cards() {
        let rows = [3, 0, 2];
        let card = |row, col| Focus::Card { row, col };
        assert_eq!(move_focus(Focus::Sidebar, "l", &rows), card(0, 0));
        assert_eq!(move_focus(card(0, 0), "h", &rows), Focus::Sidebar);
        assert_eq!(move_focus(card(0, 1), "right", &rows), card(0, 2));
        assert_eq!(move_focus(card(0, 2), "l", &rows), card(0, 2), "the last card stays");
        assert_eq!(move_focus(card(0, 2), "j", &rows), card(2, 1), "an empty row is skipped");
        assert_eq!(move_focus(card(2, 1), "up", &rows), card(0, 1));
        assert_eq!(move_focus(card(0, 1), "k", &rows), card(0, 1));
        assert_eq!(move_focus(Focus::Sidebar, "j", &rows), Focus::Sidebar);
        assert_eq!(move_focus(Focus::Sidebar, "l", &[]), Focus::Sidebar);
    }
}
