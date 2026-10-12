//! The Library page. The Recommended tab shows the rows of the Library like Home, and the Collections tab its
//! collections. The Library tab shows the poster grid, with a bar of pills (sort, filters, A to Z,
//! Series/Episodes on a Shows library, and Shuffle) and a chip for each active filter. The page keeps its sort
//! and filters while the app runs.

use std::sync::Arc;

use chrono::Datelike as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task,
    Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::browse::{Browse, Choice};
use crate::click::Clickable as _;
use crate::collections::{Collections, OpenCollection, Scope};
use crate::filter_panel::{self, Chip, Facets, MenuKind, Outcome, Panel, bar_pill};
use crate::grid::{Grid, Move};
use crate::home::{Home, Open, OpenGrid, Play, Rows};
use crate::keys::Key;
use crate::player::start_shuffle;

/// A Shows library: its grid lists series, or episodes in Episodes mode.
pub fn is_shows(library_type: &str) -> bool {
    matches!(library_type, "series" | "tv" | "tvshows" | "show" | "shows")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Recommended,
    Library,
    Collections,
}

/// A control of the bar, in the order that h and l move through them.
#[derive(Debug, Clone, PartialEq)]
enum Control {
    Tab(Tab),
    Sort,
    Filter,
    Letter,
    Mode,
    Shuffle,
    Chip(Chip),
}

pub struct LibraryPage {
    client: Arc<Client>,
    pub library: wire::UserLibrary,
    shows: bool,
    tab: Tab,
    /// The rows of the Recommended tab, made when the tab opens first.
    recommended: Option<Entity<Home>>,
    /// The collection rows of the Collections tab, made when the tab opens first.
    collections: Option<Entity<Collections>>,
    grid: Entity<Grid>,
    browse: Browse,
    /// The filter values of the Library, for the filter panel.
    facets: Facets,
    /// The bar has the keyboard focus.
    in_bar: bool,
    /// The focused control of the bar.
    bar: usize,
    /// The open sort, filter or A to Z panel.
    menu: Option<Panel>,
    /// The page has the keyboard focus.
    active: bool,
    /// Why the last Shuffle failed.
    shuffle_error: Option<String>,
    _shuffle: Task<()>,
    _facets: Task<()>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for LibraryPage {}
impl EventEmitter<Open> for LibraryPage {}
impl EventEmitter<OpenGrid> for LibraryPage {}
impl EventEmitter<OpenCollection> for LibraryPage {}

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
        let facets = cx.spawn({
            let (client, id) = (client.clone(), library.id.to_string());
            async move |this, cx| match client.filters(Some(&id)).await {
                Ok(filters) => {
                    this.update(cx, |this, cx| {
                        this.facets = filters.into();
                        if let Some(menu) = &mut this.menu {
                            menu.facets = this.facets.clone();
                        }
                        cx.notify();
                    })
                    .ok();
                }
                // The filter panel works without the values: it has no genre, language, studio, network or
                // country section then.
                Err(err) => {
                    eprintln!("silo-omarchy: cannot read the filter values of library {id}: {err}")
                }
            }
        });
        Self {
            client,
            library,
            shows,
            tab: Tab::Library,
            recommended: None,
            collections: None,
            grid,
            browse,
            facets: Facets::default(),
            in_bar: false,
            bar: 3,
            menu: None,
            active: false,
            shuffle_error: None,
            _shuffle: Task::ready(()),
            _facets: facets,
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
        if let Some(collections) = &self.collections {
            collections.update(cx, |collections, cx| {
                collections.active = content && tab == Tab::Collections;
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
        let mut controls =
            vec![Control::Tab(Tab::Recommended), Control::Tab(Tab::Library), Control::Tab(Tab::Collections)];
        if self.tab == Tab::Library {
            controls.extend([Control::Sort, Control::Filter, Control::Letter]);
            if self.shows {
                controls.push(Control::Mode);
            }
            controls.push(Control::Shuffle);
            controls.extend(Chip::all(&self.browse).into_iter().map(Control::Chip));
        }
        controls
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
            self._events.push(cx.subscribe(&home, |_, _, open: &OpenGrid, cx| cx.emit(open.clone())));
            self.recommended = Some(home);
        }
        if tab == Tab::Collections && self.collections.is_none() {
            let scope = Scope::Library(self.library.id.to_string());
            let collections = cx.new(|cx| Collections::new(self.client.clone(), scope, cx));
            self._events.push(cx.subscribe(&collections, |_, _, open: &OpenCollection, cx| cx.emit(open.clone())));
            self.collections = Some(collections);
        }
        self.sync(cx);
    }

    fn activate(&mut self, control: Control, window: &mut Window, cx: &mut Context<Self>) {
        let menu = |kind| Some(Panel::new(kind, self.facets.clone(), chrono::Local::now().year().into()));
        match control {
            Control::Tab(tab) => self.open_tab(tab, window, cx),
            Control::Sort => self.menu = menu(MenuKind::Sort),
            Control::Filter => self.menu = menu(MenuKind::Filter),
            Control::Letter => self.menu = menu(MenuKind::Letter),
            Control::Mode => {
                self.browse.episodes = !self.browse.episodes;
                self.apply(cx);
            }
            Control::Shuffle => {
                self.shuffle_error = None;
                let (client, id) = (self.client.clone(), self.library.id.to_string());
                self._shuffle = start_shuffle(client, "library", id, cx, |this, err| this.shuffle_error = Some(err));
            }
            Control::Chip(chip) => {
                chip.remove(&mut self.browse);
                self.apply(cx);
            }
        }
        cx.notify();
    }

    /// Gives a key or a click to the open panel, and loads the grid again when the sort or a filter changed.
    fn step_menu(
        &mut self,
        act: impl FnOnce(&mut Panel, &mut Browse) -> Outcome,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        let (outcome, changed) = filter_panel::step(&mut self.menu, &mut self.browse, act)?;
        if changed {
            self.apply(cx);
        }
        cx.notify();
        Some(outcome)
    }

    fn click_menu(&mut self, row: usize, cx: &mut Context<Self>) {
        self.step_menu(|menu, browse| menu.click(row, browse), cx);
    }

    /// Handles a key. `typed` is the text that the key types, for a text field of the filter panel. Returns
    /// false when the page does not use the key, for example h or Left at the left edge.
    pub fn key(&mut self, key: Key, typed: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(outcome) = self.step_menu(|menu, browse| menu.key(key, typed, browse), cx) {
            return outcome != Outcome::Unused;
        }
        if self.in_bar {
            let controls = self.controls();
            match key {
                Key::Left if self.bar == 0 => return false,
                Key::Left => self.bar -= 1,
                Key::Right => self.bar = (self.bar + 1).min(controls.len() - 1),
                Key::Down => {
                    self.in_bar = false;
                    self.sync(cx);
                }
                Key::Other("enter") => self.activate(controls[self.bar].clone(), window, cx),
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
                key == Key::Up
            }
            Tab::Collections => {
                let collections = self.collections.clone();
                if collections.is_some_and(|c| c.update(cx, |c, cx| c.key(key, cx))) {
                    return true;
                }
                key == Key::Up
            }
            Tab::Library => match self.grid.update(cx, |grid, cx| grid.key(key, cx)) {
                Move::OutTop => true,
                Move::OutLeft => return false,
                Move::To(_) => return true,
                Move::Stay
                    if key == Key::Other("enter")
                        && self.grid.read(cx).is_empty()
                        && !self.browse.chips().is_empty() =>
                {
                    self.browse.clear();
                    self.apply(cx);
                    return true;
                }
                Move::Stay => return false,
            },
        };
        if into_bar {
            self.in_bar = true;
            match self.tab {
                Tab::Recommended => self.bar = 0,
                Tab::Collections => self.bar = 2,
                Tab::Library => {}
            }
            self.sync(cx);
        }
        into_bar
    }

    fn pill(&self, index: usize, label: SharedString, on: bool, cx: &Context<Self>) -> AnyElement {
        let focused = self.active && self.in_bar && self.bar == index;
        bar_pill(("library-bar", index), label, on, focused, cx.omarchy())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.in_bar = true;
                this.bar = index;
                if let Some(control) = this.controls().get(index).cloned() {
                    this.activate(control, window, cx);
                }
            }))
            .into_any_element()
    }

    fn control(&self, index: usize, control: &Control, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let b = &self.browse;
        let (label, on): (SharedString, bool) = match control {
            &Control::Tab(tab) => {
                let focused = self.active && self.in_bar && self.bar == index;
                let label = match tab {
                    Tab::Recommended => "Recommended",
                    Tab::Library => "Library",
                    Tab::Collections => "Collections",
                };
                return div()
                    .id(("library-tab", index))
                    .clickable()
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.in_bar = true;
                        this.bar = index;
                        this.open_tab(tab, window, cx);
                    }))
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
            Control::Chip(chip) => (chip.label().into(), false),
            Control::Shuffle => ("Shuffle".into(), false),
        };
        self.pill(index, label, on, cx)
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
                Control::Chip(_) => chips.push(element),
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
                    .children(
                        self.shuffle_error.clone().map(|err| div().text_size(px(13.)).text_color(t.danger).child(err)),
                    )
                    .child(div().flex_1())
                    .children(count.map(|count| div().text_size(px(13.)).text_color(t.secondary).child(count))),
            );
            if !chips.is_empty() {
                header = header.child(div().flex().flex_wrap().gap(px(8.)).children(chips));
            }
            if let Some(menu) = &self.menu {
                header = header.child(menu.render(&self.browse, self.active, cx, Self::click_menu));
            }
        }
        let content = match (self.tab, &self.recommended) {
            (Tab::Recommended, Some(home)) => home.clone().into_any_element(),
            (Tab::Recommended, None) => div().into_any_element(),
            (Tab::Collections, _) => match &self.collections {
                Some(collections) => collections.clone().into_any_element(),
                None => div().into_any_element(),
            },
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
        div().size_full().flex().flex_col().child(header).child(
            div()
                .flex_1()
                .min_h_0()
                // A click in the content moves the focus from the bar to the content.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.in_bar = false;
                        this.sync(cx);
                    }),
                )
                .child(content),
        )
    }
}
