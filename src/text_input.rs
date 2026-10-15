//! Shortcuts shared by every text input in the Library window.
use gpui::{App, KeyBinding};
use gpui_kit::base::input::*;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("super-a", SelectAll, Some("Input")),
        KeyBinding::new("super-c", Copy, Some("Input")),
        KeyBinding::new("super-x", Cut, Some("Input")),
        KeyBinding::new("super-v", Paste, Some("Input")),
        KeyBinding::new("super-z", Undo, Some("Input")),
        KeyBinding::new("super-shift-z", Redo, Some("Input")),
        KeyBinding::new("super-delete", DeleteToEndOfLine, Some("Input")),
        KeyBinding::new("super-backspace", DeleteToBeginningOfLine, Some("Input")),
    ]);
}
