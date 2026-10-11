//! The Library page. The Recommended tab shows the rows of the Library like Home. The Library tab shows the
//! poster grid, with a bar of pills (sort, filters, A to Z, and Series/Episodes on a Shows library) and a chip
//! for each active filter. The page keeps its sort and filters while the app runs.

use std::sync::Arc;

use chrono::Datelike as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::browse::{Browse, Choice};
use crate::grid::{Grid, Move};
use crate::home::{Home, Open, Play, Rows};

/// A Shows library: its grid lists series, or episodes in Episodes mode.
pub fn is_shows(library_type: &str) -> bool {
    matches!(library_type, "series" | "tv" | "tvshows" | "show" | "shows")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Recommended,
    Library,
}

/// A control of the bar, in the order that h and l move through them.
#[derive(Debug, Clone, PartialEq)]
enum Control {
    Tab(Tab),
    Sort,
    Filter,
    Letter,
    Mode,
    Chip(Choice),
    ClearAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Sort,
    Filter,
    Letter,
}

#[derive(Debug, Clone, Copy)]
struct Menu {
    kind: MenuKind,
    /// The focused entry.
    row: usize,
}

pub struct LibraryPage {
    client: Arc<Client>,
    pub library: wire::UserLibrary,
    shows: bool,
    tab: Tab,
    /// The rows of the Recommended tab, made when the tab opens first.
    recommended: Option<Entity<Home>>,
    grid: Entity<Grid>,
    browse: Browse,
    genres: Vec<String>,
    /// The bar has the keyboard focus.
    in_bar: bool,
    /// The focused control of the bar.
    bar: usize,
    menu: Option<Menu>,
    /// The page has the keyboard focus.
    active: bool,
    _genres: Task<()>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for LibraryPage {}
impl EventEmitter<Open> for LibraryPage {}

impl LibraryPage {
    pub fn new(client: Arc<Client>, library: wire::UserLibrary, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shows = is_shows(&library.type_);
        let browse = Browse::default();
        let query = browse.query(&library.id, shows);
        let grid = cx.new(|cx| Grid::new(client.clone(), query, window, cx));
        let events = vec![
            cx.subscribe(&grid, |_, _, Play(item), cx| cx.emit(Play(item.clone()))),
            cx.subscribe(&grid, |_, _, Open(item), cx| cx.emit(Open(item.clone()))),
        ];
        let genres = cx.spawn({
            let (client, id) = (client.clone(), library.id.to_string());
            async move |this, cx| match client.genres(&id).await {
                Ok(genres) => {
                    this.update(cx, |this, cx| {
                        this.genres = genres;
                        cx.notify();
                    })
                    .ok();
                }
                // The filter menu works without genres.
                Err(err) => eprintln!("silo-omarchy: cannot read the genres of library {id}: {err}"),
            }
        });
        Self {
            client,
            library,
            shows,
            tab: Tab::Library,
            recommended: None,
            grid,
            browse,
            genres: Vec::new(),
            in_bar: false,
            bar: 2,
            menu: None,
            active: false,
            _genres: genres,
            _events: events,
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        self.sync(cx);
    }

    /// Gives the focus ring to the part of the page that has the keyboard focus. A control that went away
    /// with a removed chip or a tab change gives the focus to the last control.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.bar = self.bar.min(self.controls().len() - 1);
        let content = self.active && !self.in_bar;
        let tab = self.tab;
        if let Some(home) = &self.recommended {
            home.update(cx, |home, cx| {
                home.active = content && tab == Tab::Recommended;
                cx.notify();
            });
        }
        self.grid.update(cx, |grid, cx| {
            grid.active = content && tab == Tab::Library;
            cx.notify();
        });
        cx.notify();
    }

    fn controls(&self) -> Vec<Control> {
        let mut controls = vec![Control::Tab(Tab::Recommended), Control::Tab(Tab::Library)];
        if self.tab == Tab::Library {
            controls.extend([Control::Sort, Control::Filter, Control::Letter]);
            if self.shows {
                controls.push(Control::Mode);
            }
            let chips = self.browse.chips();
            if !chips.is_empty() {
                controls.extend(chips.into_iter().map(Control::Chip));
                controls.push(Control::ClearAll);
            }
        }
        controls
    }

    fn choices(&self, kind: MenuKind) -> Vec<Choice> {
        match kind {
            MenuKind::Sort => Choice::sort_menu(),
            MenuKind::Filter => Choice::filter_menu(&self.genres, chrono::Local::now().year().into()),
            MenuKind::Letter => Choice::letter_menu(),
        }
    }

    /// Loads the grid again with the sort and filters.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let query = self.browse.query(&self.library.id, self.shows);
        self.grid.update(cx, |grid, cx| grid.set_query(query, cx));
        self.sync(cx);
    }

    fn open_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Recommended && self.recommended.is_none() {
            let rows = Rows::Library(self.library.id.to_string());
            let home = cx.new(|cx| Home::new(self.client.clone(), rows, window, cx));
            self._events.push(cx.subscribe(&home, |_, _, Play(item), cx| cx.emit(Play(item.clone()))));
            self._events.push(cx.subscribe(&home, |_, _, Open(item), cx| cx.emit(Open(item.clone()))));
            self.recommended = Some(home);
        }
        self.sync(cx);
    }

    fn activate(&mut self, control: Control, window: &mut Window, cx: &mut Context<Self>) {
        let menu = |kind| Some(Menu { kind, row: 0 });
        match control {
            Control::Tab(tab) => self.open_tab(tab, window, cx),
            Control::Sort => self.menu = menu(MenuKind::Sort),
            Control::Filter => self.menu = menu(MenuKind::Filter),
            Control::Letter => self.menu = menu(MenuKind::Letter),
            Control::Mode => {
                self.browse.episodes = !self.browse.episodes;
                self.apply(cx);
            }
            Control::Chip(chip) => {
                self.browse.remove(&chip);
                self.apply(cx);
            }
            Control::ClearAll => {
                self.browse.clear();
                self.apply(cx);
            }
        }
        cx.notify();
    }

    /// Picks the focused entry of the open menu. The filter menu stays open, so that the user can pick more.
    fn choose(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(menu) = &self.menu else {
            return;
        };
        let kind = menu.kind;
        let Some(choice) = self.choices(kind).get(row).cloned() else {
            return;
        };
        self.browse.choose(&choice);
        if kind != MenuKind::Filter {
            self.menu = None;
        }
        self.apply(cx);
    }

    /// Handles a key. Returns false when the page does not use it, for example h or Left at the left edge.
    pub fn key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(Menu { kind, row }) = self.menu {
            let last = self.choices(kind).len().saturating_sub(1);
            match key {
                "j" | "down" | "l" | "right" => self.menu = Some(Menu { kind, row: (row + 1).min(last) }),
                "k" | "up" | "h" | "left" => self.menu = Some(Menu { kind, row: row.saturating_sub(1) }),
                "enter" => self.choose(row, cx),
                "escape" | "backspace" => self.menu = None,
                _ => return false,
            }
            cx.notify();
            return true;
        }
        if self.in_bar {
            let controls = self.controls();
            match key {
                "h" | "left" if self.bar == 0 => return false,
                "h" | "left" => self.bar -= 1,
                "l" | "right" => self.bar = (self.bar + 1).min(controls.len() - 1),
                "j" | "down" => {
                    self.in_bar = false;
                    self.sync(cx);
                }
                "enter" => self.activate(controls[self.bar].clone(), window, cx),
                _ => return false,
            }
            cx.notify();
            return true;
        }
        let into_bar = match self.tab {
            Tab::Recommended => {
                let home = self.recommended.clone();
                if home.is_some_and(|home| home.update(cx, |home, cx| home.key(key, cx))) {
                    return true;
                }
                matches!(key, "k" | "up")
            }
            Tab::Library => match self.grid.update(cx, |grid, cx| grid.key(key, cx)) {
                Move::OutTop => true,
                Move::OutLeft => return false,
                Move::To(_) => return true,
                Move::Stay if key == "enter" && self.grid.read(cx).is_empty() && !self.browse.chips().is_empty() => {
                    self.browse.clear();
                    self.apply(cx);
                    return true;
                }
                Move::Stay => return false,
            },
        };
        if into_bar {
            self.in_bar = true;
            if self.tab == Tab::Recommended {
                self.bar = 0;
            }
            self.sync(cx);
        }
        into_bar
    }

    fn pill(&self, index: usize, label: SharedString, on: bool, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let focused = self.active && self.in_bar && self.bar == index;
        div()
            .id(("library-bar", index))
            .px(px(18.))
            .py(px(9.))
            .rounded_full()
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .bg(if on { t.foreground } else { gpui::hsla(0., 0., 1., 0.1) })
            .text_color(if on { t.background } else { t.foreground })
            .border_2()
            .border_color(if focused { t.accent } else { gpui::transparent_black() })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.bar = index;
                if let Some(control) = this.controls().get(index).cloned() {
                    this.activate(control, window, cx);
                }
            }))
            .child(label)
            .into_any_element()
    }

    fn control(&self, index: usize, control: &Control, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let b = &self.browse;
        let (label, on): (SharedString, bool) = match control {
            &Control::Tab(tab) => {
                let focused = self.active && self.in_bar && self.bar == index;
                let label = if tab == Tab::Recommended { "Recommended" } else { "Library" };
                return div()
                    .id(("library-tab", index))
                    .pb(px(4.))
                    .text_size(px(14.))
                    .text_color(if focused {
                        t.accent
                    } else if self.tab == tab {
                        t.bright
                    } else {
                        t.secondary
                    })
                    .border_b_2()
                    .border_color(if focused {
                        t.accent
                    } else if self.tab == tab {
                        t.foreground
                    } else {
                        gpui::transparent_black()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.open_tab(tab, window, cx)))
                    .child(label)
                    .into_any_element();
            }
            Control::Sort => {
                let arrow = if b.descending { "↓" } else { "↑" };
                (format!("Sort: {} {arrow}", b.sort.label()).into(), false)
            }
            Control::Filter => {
                let count = b.chips().iter().filter(|c| !matches!(c, Choice::Letter(_))).count();
                if count == 0 { ("Filters".into(), false) } else { (format!("Filters · {count}").into(), true) }
            }
            Control::Letter => match b.letter {
                Some(letter) => (format!("A–Z: {letter}").into(), true),
                None => ("A–Z".into(), false),
            },
            Control::Mode => ((if b.episodes { "Episodes" } else { "Series" }).into(), false),
            Control::Chip(Choice::Letter(Some(letter))) => (format!("Starts with {letter} ×").into(), false),
            Control::Chip(chip) => (format!("{} ×", chip.label()).into(), false),
            Control::ClearAll => ("Clear all".into(), false),
        };
        self.pill(index, label, on, cx)
    }

    fn menu(&self, menu: &Menu, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let title = match menu.kind {
            MenuKind::Sort => "Sort",
            MenuKind::Filter => "Filters",
            MenuKind::Letter => "A to Z",
        };
        let entries = self.choices(menu.kind).into_iter().enumerate().map(|(row, choice)| {
            let on = self.browse.is_chosen(&choice);
            let focused = self.active && row == menu.row;
            div()
                .id(("library-menu", row))
                .px(px(14.))
                .py(px(6.))
                .rounded_full()
                .text_size(px(13.))
                .bg(if on { t.foreground } else { gpui::hsla(0., 0., 1., 0.08) })
                .text_color(if on { t.background } else { t.foreground })
                .border_2()
                .border_color(if focused { t.accent } else { gpui::transparent_black() })
                .on_click(cx.listener(move |this, _, _, cx| this.choose(row, cx)))
                .child(choice.label())
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
                    .child(div().text_size(px(12.)).text_color(t.secondary).child("Enter picks · Esc closes")),
            )
            .child(div().flex().flex_wrap().gap(px(8.)).children(entries))
    }
}

impl Render for LibraryPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let (mut tabs, mut pills, mut chips) = (Vec::new(), Vec::new(), Vec::new());
        for (index, control) in self.controls().iter().enumerate() {
            let element = self.control(index, control, cx);
            match control {
                Control::Tab(_) => tabs.push(element),
                Control::Chip(_) | Control::ClearAll => chips.push(element),
                _ => pills.push(element),
            }
        }
        let grid = self.grid.read(cx);
        let count = grid.total().map(|total| format!("{total} items"));
        let empty = grid.is_empty();
        let mut header = div()
            .px(px(40.))
            .pt(px(28.))
            .pb(px(18.))
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(
                div()
                    .text_size(px(34.))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(t.bright)
                    .child(self.library.name.clone()),
            )
            .child(div().flex().gap(px(22.)).children(tabs));
        if self.tab == Tab::Library {
            header = header.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(pills)
                    .child(div().flex_1())
                    .children(count.map(|count| div().text_size(px(13.)).text_color(t.secondary).child(count))),
            );
            if !chips.is_empty() {
                header = header.child(div().flex().flex_wrap().gap(px(8.)).children(chips));
            }
            if let Some(menu) = &self.menu {
                header = header.child(self.menu(menu, cx));
            }
        }
        let content = match (self.tab, &self.recommended) {
            (Tab::Recommended, Some(home)) => home.clone().into_any_element(),
            (Tab::Recommended, None) => div().into_any_element(),
            (Tab::Library, _) if empty && self.browse.chips().is_empty() => {
                div().px(px(40.)).text_color(t.secondary).child("This Library is empty.").into_any_element()
            }
            (Tab::Library, _) if empty => div()
                .px(px(40.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(14.))
                .child(div().text_size(px(20.)).text_color(t.bright).child("No matches"))
                .child(button("library-clear", "Clear filters", ButtonVariant::Primary, cx).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.browse.clear();
                        this.apply(cx);
                    },
                )))
                .into_any_element(),
            (Tab::Library, _) => div().size_full().px(px(40.)).child(self.grid.clone()).into_any_element(),
        };
        div().size_full().flex().flex_col().child(header).child(div().flex_1().min_h_0().child(content))
    }
}
