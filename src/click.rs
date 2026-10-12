//! The mouse rule of the Library window: each focus target is clickable, shows a hover state, and a click does
//! what Enter does on it. The click handler of a view sets its own focus first, then runs the Enter action.

use gpui::{StatefulInteractiveElement, Styled};

/// A clickable focus target: a pointer cursor, and a dimmer look on hover that works on any background.
pub trait Clickable: StatefulInteractiveElement + Styled + Sized {
    fn clickable(self) -> Self {
        self.cursor_pointer().hover(|style| style.opacity(0.75))
    }
}

impl<T: StatefulInteractiveElement + Styled> Clickable for T {}
