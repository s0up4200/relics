//! The panel of the sort, the filters or the A to Z jump, in sections. The Library page opens it from its bar.
//! Search opens it on its own `Browse` and adds its text to the query.
//!
//! The arrows move through the entries, and Up and Down jump to the previous or next section. A text field
//! takes the typed text: a year field takes digits, a Person field takes a name, and the field of a studio,
//! network, country or language narrows its values. Enter in a field applies it.

use std::collections::HashMap;

use gpui::{
    Context, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled, div, px,
};
use gpui_omarchy::{ActiveTheme, Theme};

use crate::api::wire;
use crate::browse::{ADDED_SPANS, Browse, Choice, Facet, RELEASED_SPANS, SortField, Watch};
use crate::click::Clickable as _;
use crate::keys::Key;

/// The most values that the section of a facet shows. Typing in its field narrows them.
const SHOWN_VALUES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    Sort,
    Filter,
    Letter,
}

/// A text field of the filter panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Text {
    YearFrom,
    YearTo,
    Facet(Facet),
}

/// One entry of the panel: a pill to pick, or a text field.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Pick(Choice),
    Field(Text),
}

pub struct Section {
    pub title: &'static str,
    pub entries: Vec<Entry>,
}

/// What a key or a click did in the panel. The caller compares its `Browse` before and after to know if the
/// grid must load again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The panel does not use the key.
    Unused,
    /// The focus or the text of a field changed.
    Moved,
    /// A filter changed. The panel stays open, so that the user can pick more.
    Changed,
    /// The panel closes, after a pick in the sort or A to Z menu, or after Esc.
    Closed,
}

/// A pill of the bar above a grid: the sort, the filters, a chip or Clear all. `on` fills it, `focused` shows
/// the focus ring. The caller adds the click.
pub fn bar_pill(id: impl Into<ElementId>, label: SharedString, on: bool, focused: bool, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .clickable()
        .px(px(18.))
        .py(px(9.))
        .rounded_full()
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .bg(if on { t.foreground } else { gpui::hsla(0., 0., 1., 0.1) })
        .text_color(if on { t.background } else { t.foreground })
        .border_2()
        .border_color(if focused { t.accent } else { gpui::transparent_black() })
        .child(label)
}

/// A chip pill of the bar above a grid: one chosen filter, or Clear all.
#[derive(Debug, Clone, PartialEq)]
pub enum Chip {
    One(Choice),
    ClearAll,
}

impl Chip {
    /// A chip for each chosen filter, then Clear all. Empty when no filter is chosen.
    pub fn all(browse: &Browse) -> Vec<Chip> {
        let mut chips: Vec<_> = browse.chips().into_iter().map(Chip::One).collect();
        if !chips.is_empty() {
            chips.push(Chip::ClearAll);
        }
        chips
    }

    pub fn label(&self) -> String {
        match self {
            Chip::One(choice) => format!("{} ×", choice.chip_label()),
            Chip::ClearAll => "Clear all".into(),
        }
    }

    /// Removes the filter of the chip, or all filters for Clear all.
    pub fn remove(&self, browse: &mut Browse) {
        match self {
            Chip::One(choice) => browse.remove(choice),
            Chip::ClearAll => browse.clear(),
        }
    }
}

/// Gives a key or a click (`act`) to the open panel, and closes the panel on `Outcome::Closed`. Returns the
/// outcome and whether the sort or a filter changed, so that the caller knows if the grid must load again.
/// Returns `None` when no panel is open.
pub fn step(
    menu: &mut Option<Panel>,
    browse: &mut Browse,
    act: impl FnOnce(&mut Panel, &mut Browse) -> Outcome,
) -> Option<(Outcome, bool)> {
    let before = browse.clone();
    let outcome = act(menu.as_mut()?, browse);
    if outcome == Outcome::Closed {
        *menu = None;
    }
    Some((outcome, *browse != before))
}

/// The filter values that the server lists for a Library or for the catalog.
#[derive(Debug, Clone, Default)]
pub struct Facets {
    pub genres: Vec<String>,
    pub languages: Vec<String>,
    pub studios: Vec<String>,
    pub networks: Vec<String>,
    pub countries: Vec<String>,
}

impl From<wire::CatalogFilters> for Facets {
    fn from(filters: wire::CatalogFilters) -> Self {
        Facets {
            genres: filters.genres,
            languages: filters.original_languages,
            studios: filters.studios,
            networks: filters.networks,
            countries: filters.countries,
        }
    }
}

impl Facets {
    /// The values of a facet. A Person facet has none: the user types the name.
    fn values(&self, facet: Facet) -> &[String] {
        match facet {
            Facet::Language => &self.languages,
            Facet::Studio => &self.studios,
            Facet::Network => &self.networks,
            Facet::Country => &self.countries,
            Facet::Actor | Facet::Director | Facet::Writer | Facet::Producer => &[],
        }
    }
}

/// The values that contain `text`, ignoring case.
fn matches<'a>(values: &'a [String], text: &str) -> impl Iterator<Item = &'a String> {
    let needle = text.trim().to_lowercase();
    values.iter().filter(move |value| value.to_lowercase().contains(&needle)).take(SHOWN_VALUES)
}

pub struct Panel {
    pub kind: MenuKind,
    pub facets: Facets,
    this_year: i64,
    /// The focused entry, counted over all sections.
    row: usize,
    texts: HashMap<Text, String>,
    scroll: ScrollHandle,
}

impl Panel {
    pub fn new(kind: MenuKind, facets: Facets, this_year: i64) -> Self {
        Panel { kind, facets, this_year, row: 0, texts: HashMap::new(), scroll: ScrollHandle::new() }
    }

    /// The typed text of a field.
    pub fn text(&self, text: Text) -> &str {
        self.texts.get(&text).map_or("", String::as_str)
    }

    pub fn sections(&self) -> Vec<Section> {
        let section =
            |title, choices: Vec<Choice>| Section { title, entries: choices.into_iter().map(Entry::Pick).collect() };
        match self.kind {
            MenuKind::Sort => vec![
                section("Sort by", SortField::ALL.map(Choice::Sort).into()),
                section("Order", vec![Choice::Descending(false), Choice::Descending(true)]),
            ],
            MenuKind::Letter => vec![section("A to Z", Choice::letter_menu())],
            MenuKind::Filter => self.filter_sections(section),
        }
    }

    fn filter_sections(&self, section: impl Fn(&'static str, Vec<Choice>) -> Section) -> Vec<Section> {
        let decades = (1900..=self.this_year / 10 * 10).rev().step_by(10).map(Choice::Decade);
        let mut year = section("Year", decades.collect());
        year.entries.splice(0..0, [Entry::Field(Text::YearFrom), Entry::Field(Text::YearTo)]);
        let mut sections = vec![
            section("Watch status", Watch::ALL.map(Choice::Watch).into()),
            section("Video quality", vec![Choice::FourK, Choice::Hdr, Choice::DolbyVision]),
            section("Minimum IMDb rating", (5..=9).map(Choice::MinRating).collect()),
            year,
            section("Added in the last", ADDED_SPANS.map(Choice::AddedIn).into()),
            section("Released in the last", RELEASED_SPANS.map(Choice::ReleasedIn).into()),
        ];
        if !self.facets.genres.is_empty() {
            sections.push(section("Genres", self.facets.genres.iter().cloned().map(Choice::Genre).collect()));
        }
        for facet in [Facet::Language, Facet::Studio, Facet::Network, Facet::Country] {
            let values = self.facets.values(facet);
            if values.is_empty() {
                continue;
            }
            let picks = matches(values, self.text(Text::Facet(facet)))
                .map(|value| Entry::Pick(Choice::Facet(facet, value.clone())));
            let entries = std::iter::once(Entry::Field(Text::Facet(facet))).chain(picks).collect();
            sections.push(Section { title: facet.label(), entries });
        }
        for facet in [Facet::Actor, Facet::Director, Facet::Writer, Facet::Producer] {
            sections.push(Section { title: facet.label(), entries: vec![Entry::Field(Text::Facet(facet))] });
        }
        sections
    }

    /// The index of the first entry of each section.
    fn starts(sections: &[Section]) -> Vec<usize> {
        sections.iter().scan(0, |start, s| Some(std::mem::replace(start, *start + s.entries.len()))).collect()
    }

    pub fn focused(&self) -> Entry {
        let entries: Vec<_> = self.sections().into_iter().flat_map(|s| s.entries).collect();
        entries[self.row.min(entries.len() - 1)].clone()
    }

    /// Handles a key. `typed` is the text that the key types, for a text field.
    pub fn key(&mut self, key: Key, typed: Option<&str>, browse: &mut Browse) -> Outcome {
        let sections = self.sections();
        let starts = Self::starts(&sections);
        let last = sections.iter().map(|s| s.entries.len()).sum::<usize>() - 1;
        self.row = self.row.min(last);
        let section = starts.iter().rposition(|start| *start <= self.row).unwrap_or(0);
        let focused = self.focused();
        if let Entry::Field(text) = focused
            && let Some(outcome) = self.field_key(text, key, typed, browse)
        {
            return outcome;
        }
        self.row = match key {
            Key::Right => (self.row + 1).min(last),
            Key::Left => self.row.saturating_sub(1),
            Key::Down => starts.get(section + 1).copied().unwrap_or((self.row + 1).min(last)),
            Key::Up if section > 0 => starts[section - 1],
            Key::Up => self.row.saturating_sub(1),
            Key::Other("enter") => {
                return match focused {
                    Entry::Pick(choice) => self.pick(&choice, browse),
                    Entry::Field(_) => Outcome::Unused,
                };
            }
            Key::Other("escape" | "backspace") => return Outcome::Closed,
            _ => return Outcome::Unused,
        };
        let section = starts.iter().rposition(|start| *start <= self.row).unwrap_or(0);
        self.scroll.scroll_to_item(section);
        Outcome::Moved
    }

    /// A click on an entry: it gets the focus, and a pill is picked as with Enter.
    pub fn click(&mut self, row: usize, browse: &mut Browse) -> Outcome {
        self.row = row;
        match self.focused() {
            Entry::Pick(choice) => self.pick(&choice, browse),
            Entry::Field(_) => Outcome::Moved,
        }
    }

    fn pick(&mut self, choice: &Choice, browse: &mut Browse) -> Outcome {
        browse.choose(choice);
        if self.kind == MenuKind::Filter { Outcome::Changed } else { Outcome::Closed }
    }

    /// A key in a text field, or `None` when the key moves the focus.
    fn field_key(&mut self, text: Text, key: Key, typed: Option<&str>, browse: &mut Browse) -> Option<Outcome> {
        let buffer = self.texts.entry(text).or_default();
        match key {
            Key::Other("escape") if !buffer.is_empty() => buffer.clear(),
            Key::Other("escape") => return None,
            Key::Other("backspace") => _ = buffer.pop(),
            Key::Other("enter") => return Some(self.submit(text, browse)),
            _ => {
                let digits = matches!(text, Text::YearFrom | Text::YearTo);
                let typed = typed
                    .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
                    .filter(|t| !digits || t.chars().all(|c| c.is_ascii_digit()))?;
                buffer.push_str(typed);
            }
        }
        Some(Outcome::Moved)
    }

    /// Enter in a text field. An empty field removes its filter. A facet with values takes the first value that
    /// matches the text. A Person facet takes the text as the name.
    fn submit(&mut self, text: Text, browse: &mut Browse) -> Outcome {
        let typed = self.text(text).trim().to_string();
        let f = &mut browse.filters;
        match text {
            Text::YearFrom | Text::YearTo => {
                // The field takes only digits, so only an empty field gives no year.
                let year = typed.parse().ok();
                *if text == Text::YearFrom { &mut f.year_from } else { &mut f.year_to } = year;
            }
            Text::Facet(facet) => {
                let values = self.facets.values(facet);
                let value = if typed.is_empty() || values.is_empty() {
                    Some(typed)
                } else {
                    match matches(values, &typed).next() {
                        Some(value) => Some(value.clone()),
                        None => return Outcome::Moved,
                    }
                };
                f.facets.retain(|(f, _)| *f != facet);
                f.facets.extend(value.filter(|v| !v.is_empty()).map(|v| (facet, v)));
            }
        }
        self.texts.remove(&text);
        Outcome::Changed
    }

    /// The panel. `active` shows the focus ring. A click on entry `row` calls `on_click(view, row, cx)`.
    pub fn render<V: 'static>(
        &self,
        browse: &Browse,
        active: bool,
        cx: &Context<V>,
        on_click: fn(&mut V, usize, &mut Context<V>),
    ) -> impl IntoElement {
        let t = cx.omarchy();
        let title = match self.kind {
            MenuKind::Sort => "Sort",
            MenuKind::Filter => "Filters",
            MenuKind::Letter => "A to Z",
        };
        let hint = match self.kind {
            MenuKind::Filter => "Enter picks · ↑↓ sections · type in a field · Esc closes",
            _ => "Enter picks · Esc closes",
        };
        let mut row = 0;
        let sections = self.sections().into_iter().map(|section| {
            let pills = section.entries.into_iter().map(|entry| {
                let index = row;
                row += 1;
                let focused = active && index == self.row;
                let (label, on, field): (SharedString, bool, bool) = match &entry {
                    Entry::Pick(choice) => (choice.label().into(), browse.is_chosen(choice), false),
                    Entry::Field(text) => (self.field_label(*text, browse, focused).into(), false, true),
                };
                div()
                    .id(("filter-panel", index))
                    .clickable()
                    .px(px(14.))
                    .py(px(6.))
                    .rounded_full()
                    .text_size(px(13.))
                    .bg(if on {
                        t.foreground
                    } else if field {
                        gpui::hsla(0., 0., 0., 0.25)
                    } else {
                        gpui::hsla(0., 0., 1., 0.08)
                    })
                    .text_color(if on { t.background } else { t.foreground })
                    .border_2()
                    .border_color(if focused { t.accent } else { gpui::transparent_black() })
                    .on_click(cx.listener(move |view, _, _, cx| on_click(view, index, cx)))
                    .child(label)
            });
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_size(px(12.)).text_color(t.secondary).child(section.title))
                .child(div().flex().flex_wrap().gap(px(8.)).children(pills.collect::<Vec<_>>()))
        });
        div()
            .p(px(16.))
            .rounded(px(12.))
            .bg(t.surface)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .items_baseline()
                    .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child(title))
                    .child(div().text_size(px(12.)).text_color(t.secondary).child(hint)),
            )
            .child(
                div()
                    .id("filter-panel-sections")
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .children(sections.collect::<Vec<_>>()),
            )
    }

    /// The label of a text field: the typed text with a caret, else the value of its filter, else a hint.
    fn field_label(&self, text: Text, browse: &Browse, focused: bool) -> String {
        let typed = self.text(text);
        if focused || !typed.is_empty() {
            let name = match text {
                Text::YearFrom => "From",
                Text::YearTo => "To",
                Text::Facet(facet) => facet.label(),
            };
            return format!("{name}: {typed}▏");
        }
        let f = &browse.filters;
        match text {
            Text::YearFrom => f.year_from.map_or("From year…".into(), |year| format!("From {year}")),
            Text::YearTo => f.year_to.map_or("To year…".into(), |year| format!("To {year}")),
            Text::Facet(facet) if self.facets.values(facet).is_empty() => {
                f.facet(facet).map_or("Type a name…".into(), |name| name.to_string())
            }
            Text::Facet(_) => "Type to narrow…".into(),
        }
    }
}
