//! The Search page and the filmography query of the Person page. Search sends the text 200 ms after the last text change.
//! It shows the matching people in a row above a grid of the matching titles. Enter on a person opens the Person
//! page. A bar below the field sorts and filters the titles with the sort and filter panel of a Library. When the
//! Server takes requests, a Request to add row between the people and the titles shows the titles that are not on
//! the Server, and Enter on one sends a request.

use std::sync::Arc;
use std::time::Duration;

use chrono::Datelike as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, Render, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Task, Window, div, px,
};
use gpui_kit::base::input::{Enter, InputEvent, InputState, MoveDown};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button, input};

use crate::api::{Client, wire};
use crate::artwork::{self, ArtworkCache};
use crate::browse::{Browse, PAGE_SIZE, SortField};
use crate::click::Clickable as _;
use crate::filter_panel::{self, Chip, Facets, MenuKind, Outcome, Panel, bar_pill};
use crate::grid::{Grid, Move};
use crate::home::{Open, Play};
use crate::keys::Key;
use crate::requests;

/// The pause after the last text change before Search sends the text.
pub const DEBOUNCE: Duration = Duration::from_millis(200);
const PHOTO: f32 = 96.;

/// The user pressed Enter on a person in Search or on a cast or crew card of the detail page.
pub struct OpenPerson {
    pub id: String,
    pub name: String,
}

/// The sort and filters of Search when the app starts: the newest added titles first, as in the web client.
pub fn browse() -> Browse {
    Browse { sort: SortField::DateAdded, descending: true, ..Browse::default() }
}

/// The query of the titles that match `text`, with the sort and the filters of `browse`. Search has no A to Z
/// jump and no Episodes mode, so the letter and the mode stay out of the query.
pub fn query(text: &str, browse: &Browse) -> wire::CatalogQuery {
    let (groups, match_) = browse.filters.groups();
    wire::CatalogQuery {
        source: Some("query".into()),
        q: Some(text.into()),
        type_: Some("video".into()),
        sort: Some(browse.sort.wire().into()),
        order: Some(if browse.descending { "desc" } else { "asc" }.into()),
        limit: std::num::NonZeroU64::new(PAGE_SIZE),
        match_,
        groups,
        ..Default::default()
    }
}

/// The query of the filmography on the Person page: the titles of the Person, the newest first, as in the web
/// client. `type_` is "movie" or "series" for the Movies and Series tabs, and `None` for All.
pub fn person_query(person_id: &str, type_: Option<&str>) -> wire::CatalogQuery {
    wire::CatalogQuery {
        source: Some("person".into()),
        person_id: person_id.try_into().ok(),
        type_: type_.map(Into::into),
        sort: Some("year".into()),
        order: Some("desc".into()),
        limit: std::num::NonZeroU64::new(PAGE_SIZE),
        ..Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Field,
    /// A control of the sort and filter bar.
    Bar(usize),
    Person(usize),
    /// A card of the Request to add row.
    Request(usize),
    Titles,
}

/// A control of the sort and filter bar, in the order that h and l move through them.
#[derive(Debug, Clone, PartialEq)]
enum Control {
    Sort,
    Filter,
    Chip(Chip),
}

pub struct Search {
    client: Arc<Client>,
    field: Entity<InputState>,
    focus_handle: FocusHandle,
    focus_pending: bool,
    focus: Focus,
    /// The page has the keyboard focus, so the focused control shows the focus ring.
    pub active: bool,
    /// The text of the results that show. Empty: no results show.
    shown: String,
    people: Vec<wire::Person>,
    people_thumbs: Vec<Option<Arc<RenderImage>>>,
    people_error: Option<String>,
    people_loading: bool,
    /// The Server takes requests from this user, so Search shows Request to add.
    requests_on: bool,
    /// The titles of the search that are not on the Server.
    to_add: Vec<wire::RequestMediaResult>,
    /// "Sending…", or the error of the last request.
    request_note: Option<String>,
    /// The grid of the titles, made at the first search.
    pub titles: Option<Entity<Grid>>,
    /// The sort and filters of the titles. They stay while the app runs.
    browse: Browse,
    /// The filter values of the whole catalog, for the filter panel.
    facets: Facets,
    /// The open sort or filter panel.
    menu: Option<Panel>,
    artwork: Entity<ArtworkCache>,
    _facets: Task<()>,
    _debounce: Task<()>,
    _people: Task<()>,
    _requests: Task<()>,
    _to_add: Task<()>,
    _send: Task<()>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for Search {}
impl EventEmitter<Open> for Search {}
impl EventEmitter<OpenPerson> for Search {}

impl Search {
    pub fn new(client: Arc<Client>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.drop_thumbs(cx)).detach();
        let facets = cx.spawn({
            let client = client.clone();
            async move |this, cx| match client.filters(None).await {
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
                Err(err) => eprintln!("relics: cannot read the filter values of the catalog: {err}"),
            }
        });
        let requests = cx.spawn({
            let client = client.clone();
            async move |this, cx| match client.request_status().await {
                Ok(status) => {
                    this.update(cx, |this, _| this.requests_on = status.allowed && status.requests_enabled).ok();
                }
                // Search works without Request to add.
                Err(err) => eprintln!("relics: cannot read the request status: {err}"),
            }
        });
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Titles and people"));
        let changes = cx.subscribe_in(&field, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                if this.focus != Focus::Field {
                    this.set_focus(Focus::Field, cx);
                }
                if this.field.read(cx).value().is_empty() && this.shown.is_empty() {
                    this._debounce = Task::ready(());
                    return;
                }
                this._debounce = cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(DEBOUNCE).await;
                    this.update_in(cx, |this, window, cx| {
                        let text = this.field.read(cx).value().trim().to_string();
                        this.search(text, window, cx);
                    })
                    .ok();
                });
            }
        });
        Self {
            client,
            field,
            focus_handle: cx.focus_handle(),
            focus_pending: true,
            focus: Focus::Field,
            active: false,
            shown: String::new(),
            people: Vec::new(),
            people_thumbs: Vec::new(),
            people_error: None,
            people_loading: false,
            requests_on: false,
            to_add: Vec::new(),
            request_note: None,
            titles: None,
            browse: browse(),
            facets: Facets::default(),
            menu: None,
            artwork: artwork::global(cx),
            _facets: facets,
            _debounce: Task::ready(()),
            _people: Task::ready(()),
            _requests: requests,
            _to_add: Task::ready(()),
            _send: Task::ready(()),
            _events: vec![changes],
        }
    }

    fn drop_thumbs(&mut self, cx: &mut gpui::App) {
        for thumb in self.people_thumbs.drain(..).flatten() {
            cx.drop_image(thumb, None);
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        self.set_focus(self.focus, cx);
    }

    /// Puts the keyboard focus in the search field, for example when the user presses /.
    pub fn focus_field(&mut self, cx: &mut Context<Self>) {
        self.set_focus(Focus::Field, cx);
    }

    /// Moves the focus. A focus outside the bar closes the panel. A bar control that went away with a removed
    /// chip gives the focus to the last control.
    fn set_focus(&mut self, focus: Focus, cx: &mut Context<Self>) {
        self.focus_pending = true;
        self.focus = match focus {
            Focus::Bar(i) => Focus::Bar(i.min(self.controls().len() - 1)),
            focus => {
                self.menu = None;
                focus
            }
        };
        let titles = self.active && focus == Focus::Titles;
        if let Some(grid) = &self.titles {
            grid.update(cx, |grid, cx| {
                grid.active = titles;
                cx.notify();
            });
        }
        cx.notify();
    }

    fn controls(&self) -> Vec<Control> {
        let mut controls = vec![Control::Sort, Control::Filter];
        controls.extend(Chip::all(&self.browse).into_iter().map(Control::Chip));
        controls
    }

    /// Loads the titles again with the sort and filters.
    fn apply(&mut self, cx: &mut Context<Self>) {
        if let Some(grid) = self.titles.as_ref().filter(|_| !self.shown.is_empty()) {
            let query = query(&self.shown, &self.browse);
            grid.update(cx, |grid, cx| grid.set_query(query, cx));
        }
        cx.notify();
    }

    /// Enter on bar control `index`: open the sort or filter panel, or remove a chip or all of them.
    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(control) = self.controls().get(index).cloned() else {
            return;
        };
        let menu = |kind| Some(Panel::new(kind, self.facets.clone(), chrono::Local::now().year().into()));
        match control {
            Control::Sort => self.menu = menu(MenuKind::Sort),
            Control::Filter => self.menu = menu(MenuKind::Filter),
            Control::Chip(chip) => {
                chip.remove(&mut self.browse);
                self.apply(cx);
            }
        }
        self.set_focus(Focus::Bar(index), cx);
    }

    /// Gives a key or a click to the open panel, and loads the titles again when the sort or a filter changed.
    fn step_menu(
        &mut self,
        act: impl FnOnce(&mut Panel, &mut Browse) -> Outcome,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        let (outcome, changed) = filter_panel::step(&mut self.menu, &mut self.browse, act)?;
        if changed {
            self.apply(cx);
        }
        self.set_focus(self.focus, cx);
        Some(outcome)
    }

    fn edit_menu(&mut self, text: filter_panel::Text, value: String, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.edit(text, value);
        }
        cx.notify();
    }

    fn menu_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        self.key(key, window, cx);
    }

    pub fn editing(&self, window: &Window, cx: &gpui::App) -> bool {
        self.field.read(cx).focus_handle(cx).is_focused(window)
            || self.menu.as_ref().is_some_and(|menu| menu.editing(window, cx))
    }

    fn click_menu(&mut self, row: usize, cx: &mut Context<Self>) {
        self.step_menu(|menu, browse| menu.click(row, browse), cx);
    }

    fn has_titles(&self, cx: &gpui::App) -> bool {
        !self.shown.is_empty() && self.titles.as_ref().is_some_and(|grid| !grid.read(cx).is_empty())
    }

    /// Sends `text` for the titles and the people. A new search drops the requests of the old one.
    fn search(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_thumbs(cx);
        self.people.clear();
        self.people_error = None;
        self._people = Task::ready(());
        self.to_add.clear();
        self.request_note = None;
        self._to_add = Task::ready(());
        self._send = Task::ready(());
        self.shown = text.clone();
        // The old results go away. The bar and its panel stay.
        if !matches!(self.focus, Focus::Bar(_)) {
            self.set_focus(Focus::Field, cx);
        }
        if text.is_empty() {
            self.people_loading = false;
            return;
        }
        let query = query(&text, &self.browse);
        match &self.titles {
            Some(grid) => grid.update(cx, |grid, cx| grid.set_query(query, cx)),
            None => {
                let client = self.client.clone();
                let grid = cx.new(|cx| Grid::new(client, query, window, cx));
                self._events.push(cx.subscribe(&grid, |_, _, Play(item), cx| cx.emit(Play(item.clone()))));
                self._events.push(cx.subscribe(&grid, |_, _, Open(item), cx| cx.emit(Open(item.clone()))));
                self.titles = Some(grid);
            }
        }
        if self.requests_on {
            let (client, text) = (self.client.clone(), text.clone());
            self._to_add = cx.spawn(async move |this, cx| match client.search_requests(&text, 1).await {
                Ok(page) => {
                    this.update(cx, |this, cx| {
                        this.to_add = requests::to_add(page);
                        cx.notify();
                    })
                    .ok();
                }
                // As in the web client, a failed request search hides the row.
                Err(err) => eprintln!("relics: cannot search the titles to request: {err}"),
            });
        }
        self.people_loading = true;
        let client = self.client.clone();
        self._people = cx.spawn(async move |this, cx| {
            let result = client.people(&text).await;
            this.update(cx, |this, cx| {
                this.people_loading = false;
                match result {
                    Ok(people) => {
                        this.people_thumbs =
                            people.iter().map(|p| p.photo_thumbhash.as_deref().and_then(artwork::thumbhash)).collect();
                        this.people = people;
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                    }
                    // The Library window shows Offline or the sign-in screen. The row shows Retry.
                    Err(err) => this.people_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn open_person(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(person) = self.people.get(index) {
            let open = OpenPerson { id: person.id.to_string(), name: person.name.clone() };
            self.set_focus(Focus::Person(index), cx);
            cx.emit(open);
        }
    }

    /// Enter on a Request to add card: send a request for the title, when the user can request it.
    fn send_request(&mut self, index: usize, cx: &mut Context<Self>) {
        self.set_focus(Focus::Request(index), cx);
        let Some(result) = self.to_add.get(index).filter(|r| r.request.requestable) else {
            return;
        };
        let Some(create) = requests::create(result) else {
            return;
        };
        let (client, tmdb_id) = (self.client.clone(), result.tmdb_id);
        self.request_note = Some("Sending…".into());
        self._send = cx.spawn(async move |this, cx| {
            let result = client.create_request(&create).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(request) => {
                        this.request_note = None;
                        if let Some(r) = this.to_add.iter_mut().find(|r| r.tmdb_id == tmdb_id) {
                            r.request.requestable = false;
                            r.request.state = Some(request.state);
                        }
                    }
                    Err(err) => this.request_note = Some(format!("Not sent: {err}")),
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// Where Down goes from the people row or the bar: the Request to add row, then the titles.
    fn below_people(&self, cx: &gpui::App) -> Option<Focus> {
        if !self.to_add.is_empty() {
            Some(Focus::Request(0))
        } else if self.has_titles(cx) {
            Some(Focus::Titles)
        } else {
            None
        }
    }

    /// Handles a page action. Returns false when the page does not use the key,
    /// for example Esc in an empty field.
    pub fn key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.field.read(cx).focus_handle(cx).is_focused(window) {
            self.focus = Focus::Field;
            self.menu = None;
        }
        if let Some(menu) = &mut self.menu {
            menu.sync_focus(window, cx);
        }
        if let Some(outcome) = self.step_menu(|menu, browse| menu.key(key, browse), cx) {
            return outcome != Outcome::Unused;
        }
        let focus = match self.focus {
            Focus::Field => match key {
                Key::Other("escape") if !self.field.read(cx).value().is_empty() => {
                    self.field.update(cx, |field, cx| field.set_value("", window, cx));
                    self._debounce = Task::ready(());
                    self.search(String::new(), window, cx);
                    Focus::Field
                }
                Key::Down => Focus::Bar(0),
                Key::Other("enter") if !self.people.is_empty() => Focus::Person(0),
                Key::Other("enter") => self.below_people(cx).unwrap_or(Focus::Field),
                _ => return false,
            },
            _ if key == Key::Other("/") => Focus::Field,
            Focus::Bar(i) => match key {
                Key::Other("enter") => {
                    self.activate(i, cx);
                    return true;
                }
                Key::Left if i == 0 => return false,
                Key::Left => Focus::Bar(i - 1),
                Key::Right => Focus::Bar(i + 1),
                Key::Up => Focus::Field,
                Key::Down if !self.people.is_empty() => Focus::Person(0),
                Key::Down => self.below_people(cx).unwrap_or(Focus::Bar(i)),
                _ => return false,
            },
            Focus::Person(i) => match key {
                Key::Other("enter") => {
                    self.open_person(i, cx);
                    return true;
                }
                Key::Left if i > 0 => Focus::Person(i - 1),
                Key::Right if i + 1 < self.people.len() => Focus::Person(i + 1),
                Key::Right => Focus::Person(i),
                Key::Down => match self.below_people(cx) {
                    Some(focus) => focus,
                    None => return false,
                },
                Key::Up => Focus::Bar(0),
                _ => return false,
            },
            Focus::Request(i) => match key {
                Key::Other("enter") => {
                    self.send_request(i, cx);
                    return true;
                }
                Key::Left if i > 0 => Focus::Request(i - 1),
                Key::Right => Focus::Request((i + 1).min(self.to_add.len() - 1)),
                Key::Down if self.has_titles(cx) => Focus::Titles,
                Key::Up if !self.people.is_empty() => Focus::Person(0),
                Key::Up => Focus::Bar(0),
                _ => return false,
            },
            Focus::Titles => {
                let Some(grid) = self.titles.clone() else {
                    return false;
                };
                match grid.update(cx, |grid, cx| grid.key(key, cx)) {
                    Move::To(_) => Focus::Titles,
                    Move::OutTop if !self.to_add.is_empty() => Focus::Request(0),
                    Move::OutTop if !self.people.is_empty() => Focus::Person(0),
                    Move::OutTop => Focus::Bar(0),
                    Move::OutLeft | Move::Stay => return false,
                }
            }
        };
        self.set_focus(focus, cx);
        true
    }

    /// The sort and filter pills, and the chips of the active filters with Clear all.
    fn bar(&self, cx: &Context<Self>) -> (Vec<AnyElement>, Vec<AnyElement>) {
        let b = &self.browse;
        let (mut pills, mut chips) = (Vec::new(), Vec::new());
        for (index, control) in self.controls().into_iter().enumerate() {
            let (label, on): (SharedString, bool) = match &control {
                Control::Sort => {
                    let arrow = if b.descending { "↓" } else { "↑" };
                    (format!("Sort: {} {arrow}", b.sort.label()).into(), false)
                }
                Control::Filter => match b.chips().len() {
                    0 => ("Filters".into(), false),
                    count => (format!("Filters · {count}").into(), true),
                },
                Control::Chip(chip) => (chip.label().into(), false),
            };
            let focused = self.active && self.focus == Focus::Bar(index);
            let pill = bar_pill(("search-bar", index), label, on, focused, cx.omarchy())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_focus(Focus::Bar(index), cx);
                    this.activate(index, cx);
                }))
                .into_any_element();
            match control {
                Control::Sort | Control::Filter => pills.push(pill),
                Control::Chip(_) => chips.push(pill),
            }
        }
        (pills, chips)
    }

    fn people_row(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let cards = self.people.iter().enumerate().map(|(i, person)| {
            let focused = self.active && self.focus == Focus::Person(i);
            let url = person.photo_url.as_deref().and_then(|url| self.client.server().url.join(url).ok());
            div()
                .id(("search-person", i))
                .clickable()
                .w(px(PHOTO + 16.))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .on_click(cx.listener(move |this, _, _, cx| this.open_person(i, cx)))
                .child(
                    div()
                        .size(px(PHOTO))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(t.surface)
                        .border_2()
                        .border_color(if focused { t.accent } else { gpui::transparent_black() })
                        .child(artwork::image(
                            url.map(String::from),
                            self.people_thumbs.get(i).cloned().flatten(),
                            PHOTO,
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(13.))
                        .text_center()
                        .text_color(if focused { t.bright } else { t.foreground })
                        .truncate()
                        .child(person.name.clone()),
                )
        });
        div().id("search-people").flex().gap(px(12.)).overflow_x_scroll().restrict_scroll_to_axis().children(cards)
    }

    /// The Request to add row: the titles of the search that are not on the Server.
    fn request_row(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let cards = self.to_add.iter().enumerate().map(|(i, result)| {
            let focused = self.active && self.focus == Focus::Request(i);
            let requestable = result.request.requestable;
            div()
                .id(("search-request", i))
                .clickable()
                .w(px(200.))
                .flex_none()
                .px(px(14.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .rounded(px(12.))
                .bg(t.surface)
                .border_2()
                .border_color(if focused { t.accent } else { gpui::transparent_black() })
                .on_click(cx.listener(move |this, _, _, cx| this.send_request(i, cx)))
                .child(
                    div()
                        .truncate()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if focused { t.bright } else { t.foreground })
                        .child(result.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.secondary)
                        .child(requests::kind_line(&result.media_type, result.year)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if requestable { t.accent } else { t.secondary })
                        .child(requests::result_status(result)),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.bright)
                    .child("Request to add")
                    .children(self.request_note.clone().map(|note| {
                        div().text_size(px(14.)).font_weight(FontWeight::NORMAL).text_color(t.secondary).child(note)
                    })),
            )
            .child(
                div()
                    .id("search-requests")
                    .flex()
                    .gap(px(12.))
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .children(cards),
            )
    }
}

impl Render for Search {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        if self.active && self.focus_pending {
            self.focus_pending = false;
            if self.focus == Focus::Field {
                self.field.update(cx, |field, cx| field.focus(window, cx));
            } else {
                window.focus(&self.focus_handle, cx);
            }
        }
        let field = div()
            .id("search-field-container")
            .debug_selector(|| "search-field".into())
            .on_click(cx.listener(|this, _, _, cx| this.set_focus(Focus::Field, cx)))
            .max_w(px(640.))
            .on_action(cx.listener(|this, _: &Enter, window, cx| {
                this.key(Key::Other("enter"), window, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveDown, window, cx| {
                this.key(Key::Down, window, cx);
            }))
            .child(input("search-field", &self.field, window, cx));
        let mut header =
            div().px(px(40.)).pt(px(28.)).pb(px(18.)).flex().flex_col().gap(px(18.)).child(
                div().text_size(px(34.)).font_weight(FontWeight::EXTRA_BOLD).text_color(t.bright).child("Search"),
            );
        header = header.child(field);
        let (pills, chips) = self.bar(cx);
        header = header.child(div().flex().items_center().gap(px(8.)).children(pills));
        if !chips.is_empty() {
            header = header.child(div().flex().flex_wrap().gap(px(8.)).children(chips));
        }
        if let Some(menu) = &mut self.menu {
            header = header.child(menu.render(
                &self.browse,
                self.active,
                &self.focus_handle,
                window,
                cx,
                filter_panel::Events { click: Self::click_menu, edit: Self::edit_menu, key: Self::menu_key },
            ));
        }
        if let Some(error) = &self.people_error {
            header =
                header.child(div().flex().items_center().gap(px(14.)).text_color(t.danger).child(error.clone()).child(
                    button("search-retry", "Retry", ButtonVariant::Secondary, cx).on_click(cx.listener(
                        |this, _, window, cx| {
                            let text = this.shown.clone();
                            this.search(text, window, cx);
                        },
                    )),
                ));
        }
        if !self.people.is_empty() {
            header = header.child(self.people_row(cx));
        }
        if !self.to_add.is_empty() {
            header = header.child(self.request_row(cx));
        }
        let titles = self.titles.as_ref().filter(|_| !self.shown.is_empty());
        let nothing = titles.is_some_and(|grid| grid.read(cx).is_empty())
            && self.people.is_empty()
            && self.to_add.is_empty()
            && !self.people_loading
            && self.people_error.is_none();
        let content = match titles {
            _ if nothing => div()
                .px(px(40.))
                .text_size(px(20.))
                .text_color(t.bright)
                .child(format!("No results for {}", self.shown))
                .into_any_element(),
            Some(grid) => div()
                .size_full()
                .px(px(40.))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.set_focus(Focus::Titles, cx)))
                .child(grid.clone())
                .into_any_element(),
            None => div().into_any_element(),
        };
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().child(content))
    }
}
