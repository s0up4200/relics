//! The Choice list: a list where the user picks one value. The Settings page and the Player view use it.
//!
//! The list holds the focused row and the scroll handle. It turns a key into an `Outcome`, and the screen acts
//! on the outcome. The list sends no events. It calls the screen only through the `on_pick` callback of a click.
//! The screen decides where the list shows, the title and the footer around it, and what a pick does.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, Div, InteractiveElement as _, ParentElement, ScrollHandle, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_omarchy::{ActiveTheme as _, IconName, icon};

use crate::click::Clickable as _;
use crate::keys::Key;

/// One row of a Choice list.
pub struct Choice {
    pub label: String,
    /// The second line, for example the codec and the channels. Empty for no second line.
    pub detail: String,
    pub tags: Vec<&'static str>,
    /// The row is the current value. A check mark shows it.
    pub current: bool,
}

impl Choice {
    /// A choice with only a label.
    pub fn new(label: impl Into<String>, current: bool) -> Self {
        Self { label: label.into(), detail: String::new(), tags: Vec::new(), current }
    }
}

/// What a key did to the list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Moved,
    /// The user picked the row with this index.
    Picked(usize),
    Closed,
    /// The list does not use the key. The screen handles it.
    Unused,
}

/// An open Choice list.
pub struct ChoiceList {
    focus: usize,
    scroll: ScrollHandle,
}

impl ChoiceList {
    /// Opens the list with the focus on the current row, or on the first row when no row is current.
    pub fn open(choices: &[Choice]) -> Self {
        let focus = choices.iter().position(|choice| choice.current).unwrap_or(0);
        let scroll = ScrollHandle::new();
        scroll.scroll_to_item(focus);
        Self { focus, scroll }
    }

    pub fn focus(&self) -> usize {
        self.focus
    }

    /// A key while the list is open. `len` is the number of rows. j/k and the arrows move the focus, which
    /// stops at the first and the last row. Enter picks. Escape, Backspace and h close.
    pub fn key(&mut self, key: Key, len: usize) -> Outcome {
        let focus = match key {
            Key::Up => self.focus.saturating_sub(1),
            Key::Down => (self.focus + 1).min(len.saturating_sub(1)),
            Key::Other("enter") => return Outcome::Picked(self.focus),
            Key::Left | Key::Other("escape" | "backspace") => return Outcome::Closed,
            _ => return Outcome::Unused,
        };
        self.focus = focus;
        self.scroll.scroll_to_item(focus);
        Outcome::Moved
    }

    /// The rows in a scrolling column. A highlight marks the focused row. A click on a row calls `on_pick`
    /// with its index. The screen adds the size and the frame.
    pub fn render(
        &self,
        id: &'static str,
        choices: Vec<Choice>,
        on_pick: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
        cx: &App,
    ) -> Stateful<Div> {
        let t = cx.omarchy();
        let rows = choices.into_iter().enumerate().map(|(i, choice)| {
            let tags = choice.tags.into_iter().map(|tag| {
                div().px(px(6.)).rounded(px(4.)).bg(t.inset).text_size(px(11.)).text_color(t.secondary).child(tag)
            });
            let on_pick = on_pick.clone();
            div()
                .id((id, i))
                .clickable()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(8.))
                .when(i == self.focus, |row| row.bg(t.selection))
                .child(div().w(px(16.)).when(choice.current, |mark| mark.child(icon(IconName::Check).size(px(16.)))))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(6.))
                                .text_color(t.bright)
                                .child(choice.label)
                                .children(tags),
                        )
                        .when(!choice.detail.is_empty(), |text| {
                            text.child(div().text_size(px(12.)).text_color(t.secondary).child(choice.detail))
                        }),
                )
                .on_click(move |_: &ClickEvent, window, cx| on_pick(i, window, cx))
        });
        div().id(id).flex().flex_col().overflow_y_scroll().track_scroll(&self.scroll).children(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::{Choice, ChoiceList, Outcome};
    use crate::keys::Key;

    fn choices(current: Option<usize>) -> Vec<Choice> {
        (0..3).map(|i| Choice::new(format!("{i}"), current == Some(i))).collect()
    }

    #[test]
    fn the_list_opens_on_the_current_row_or_on_the_first_row() {
        assert_eq!(ChoiceList::open(&choices(Some(2))).focus(), 2);
        assert_eq!(ChoiceList::open(&choices(None)).focus(), 0);
    }

    #[test]
    fn j_and_k_move_the_focus_and_stop_at_the_ends() {
        let mut list = ChoiceList::open(&choices(Some(1)));
        assert_eq!(list.key(Key::Down, 3), Outcome::Moved);
        assert_eq!(list.focus(), 2);
        assert_eq!(list.key(Key::Down, 3), Outcome::Moved);
        assert_eq!(list.focus(), 2);
        list.key(Key::Up, 3);
        list.key(Key::Up, 3);
        assert_eq!(list.key(Key::Up, 3), Outcome::Moved);
        assert_eq!(list.focus(), 0);
    }

    #[test]
    fn enter_picks_the_focused_row() {
        let mut list = ChoiceList::open(&choices(Some(1)));
        list.key(Key::Down, 3);
        assert_eq!(list.key(Key::Other("enter"), 3), Outcome::Picked(2));
    }

    #[test]
    fn escape_backspace_and_h_close_and_other_keys_are_unused() {
        let mut list = ChoiceList::open(&choices(None));
        for key in ["escape", "backspace", "h"] {
            assert_eq!(list.key(Key::parse(key), 3), Outcome::Closed);
        }
        assert_eq!(list.key(Key::parse("l"), 3), Outcome::Unused);
        assert_eq!(list.key(Key::parse("space"), 3), Outcome::Unused);
    }
}
