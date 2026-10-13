//! Repairs to the Omarchy palette that gpui-omarchy loads.

use gpui::{App, Hsla, Rgba};
use gpui_omarchy::Theme;

/// Keeps the bright text of the theme readable, now and after each theme change.
pub fn keep_readable(cx: &mut App) {
    repair(cx);
    cx.observe_global::<Theme>(repair).detach();
}

fn repair(cx: &mut App) {
    if let Some(bright) = readable_bright(cx.global::<Theme>()) {
        // The next call sees bright == foreground and writes nothing, so the observer does not loop.
        cx.global_mut::<Theme>().bright = bright;
        gpui_kit::base::Theme::global_mut(cx).tokens.colors.accent_foreground = bright;
    }
}

/// The foreground, when the bright color of the theme is under the WCAG minimum contrast for text on its
/// background, else None.
///
/// A theme without `bright_foreground` gets `selection_foreground` from gpui-omarchy 0.2.0. Many themes set that
/// color for text on a light selection, so it is near black on a dark background (#88).
fn readable_bright(theme: &Theme) -> Option<Hsla> {
    (theme.bright != theme.foreground && contrast(theme.bright, theme.background) < 4.5).then_some(theme.foreground)
}

/// The WCAG contrast ratio of two colors.
fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn luminance(color: Hsla) -> f32 {
    let color: Rgba = color.into();
    let linear = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dark_selection_foreground_gives_way_to_the_foreground() {
        // The Void theme: no bright_foreground, and a selection_foreground the same as the background.
        let theme = Theme::from_colors_toml(
            "Void",
            "background = '#05010C'\nforeground = '#FFFFFF'\naccent = '#BB9AF7'\n\
             selection_foreground = '#05010C'\ncolor1 = '#F07178'\ncolor2 = '#C2B8FF'\ncolor3 = '#DDCCFF'\n",
        )
        .unwrap();
        assert_eq!(readable_bright(&theme), Some(theme.foreground));
    }

    #[test]
    fn a_readable_bright_color_stays() {
        assert_eq!(readable_bright(&Theme::tokyo_night()), None);
    }
}
