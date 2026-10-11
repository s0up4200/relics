//! The pages of the active Profile: the sidebar, the page that shows, and the back history. Esc or Backspace
//! goes back to the page and the card that had the focus. 1 to 3 open Home, Movies and Shows, and / opens
//! Search. The sidebar shrinks to an icon rail when the window is narrow and on the detail page.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement, Render, StatefulInteractiveElement as _, Styled, Subscription, Task,
    Window, div, px,
};
use gpui_omarchy::{ActiveTheme, keycap};

use crate::api::{Client, wire};
use crate::detail::{self, Detail};
use crate::home::{Home, Open, Play, Rows};
use crate::library_page::{LibraryPage, is_shows};
use crate::search::{OpenPerson, PersonPage, Search};

/// Below this window width, the sidebar is an icon rail.
const NARROW: f32 = 1100.;

/// The width of the sidebar in a window that is `viewport` pixels wide, on the detail page or another page.
pub fn sidebar_width(viewport: f32, detail: bool) -> f32 {
    if detail || viewport < NARROW { 64. } else { 240. }
}

/// The sidebar row that a shortcut key opens: 1 is Home (row 0), 2 the first Movies library and 3 the first
/// Shows library. Row 1 is the first entry of `libraries`.
pub fn shortcut(key: &str, libraries: &[wire::UserLibrary]) -> Option<usize> {
    let row_of = |shows: bool| libraries.iter().position(|l| is_shows(&l.type_) == shows).map(|i| i + 1);
    match key {
        "1" => Some(0),
        "2" => row_of(false),
        "3" => row_of(true),
        _ => None,
    }
}

#[derive(Clone)]
pub enum Page {
    Home(Entity<Home>),
    Library(Entity<LibraryPage>),
    Detail(Entity<Detail>),
    Search(Entity<Search>),
    Person(Entity<PersonPage>),
}

impl Page {
    fn set_active(&self, active: bool, cx: &mut gpui::App) {
        match self {
            Page::Home(home) => home.update(cx, |home, cx| {
                home.active = active;
                cx.notify();
            }),
            Page::Library(page) => page.update(cx, |page, cx| page.set_active(active, cx)),
            Page::Detail(page) => page.update(cx, |page, cx| {
                page.active = active;
                cx.notify();
            }),
            Page::Search(page) => page.update(cx, |page, cx| page.set_active(active, cx)),
            Page::Person(page) => page.update(cx, |page, cx| page.set_active(active, cx)),
        }
    }

    fn into_any_element(self) -> AnyElement {
        match self {
            Page::Home(home) => home.into_any_element(),
            Page::Library(page) => page.into_any_element(),
            Page::Detail(page) => page.into_any_element(),
            Page::Search(page) => page.into_any_element(),
            Page::Person(page) => page.into_any_element(),
        }
    }
}

pub struct Pages {
    client: Arc<Client>,
    profile_name: String,
    /// The video Libraries, in the sidebar order.
    libraries: Vec<wire::UserLibrary>,
    home: Entity<Home>,
    page: Page,
    /// The pages that Esc goes back to, the last one on top.
    back: Vec<Page>,
    /// The page of each Library that the user opened, so that its sort and filters stay while the app runs.
    library_pages: HashMap<String, Entity<LibraryPage>>,
    /// The Search page, so that the text and the results stay while the app runs.
    search: Option<Entity<Search>>,
    /// The focused sidebar row, while the sidebar has the keyboard focus.
    sidebar: Option<usize>,
    focus_handle: FocusHandle,
    _libraries: Task<()>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for Pages {}

impl Pages {
    pub fn new(client: Arc<Client>, profile_name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let home = cx.new(|cx| Home::new(client.clone(), Rows::Home, window, cx));
        let mut pages = Self {
            client,
            profile_name,
            libraries: Vec::new(),
            home: home.clone(),
            page: Page::Home(home.clone()),
            back: Vec::new(),
            library_pages: HashMap::new(),
            search: None,
            sidebar: None,
            focus_handle: cx.focus_handle(),
            _libraries: Task::ready(()),
            _events: Vec::new(),
        };
        pages.listen(&home, window, cx);
        pages.page.set_active(true, cx);
        pages.load_libraries(cx);
        pages.focus(window, cx);
        pages
    }

    /// Gives the keyboard focus to the pages, for example when they come back after the PIN entry.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    fn load_libraries(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self._libraries = cx.spawn(async move |this, cx| match client.libraries().await {
            Ok(libraries) => {
                this.update(cx, |this, cx| {
                    this.libraries = libraries;
                    cx.notify();
                })
                .ok();
            }
            // The sidebar shows Home only. It reads the list again when it gets the focus.
            Err(err) => eprintln!("silo-omarchy: cannot read the libraries: {err}"),
        });
    }

    fn listen<T: EventEmitter<Play> + EventEmitter<Open>>(
        &mut self,
        view: &Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._events.push(cx.subscribe(view, |_, _, Play(item), cx| cx.emit(Play(item.clone()))));
        self._events
            .push(cx.subscribe_in(view, window, |this, _, Open(item), window, cx| this.open_item(item, window, cx)));
    }

    /// Opens the detail page of an Item. For an episode or a season, the page of its series with the season.
    fn open_item(&mut self, item: &wire::CatalogItem, window: &mut Window, cx: &mut Context<Self>) {
        let (id, season) = detail::target(item);
        let client = self.client.clone();
        let page = cx.new(|cx| Detail::new(client, id, season, cx));
        cx.subscribe(&page, |_, _, Play(item), cx| cx.emit(Play(item.clone()))).detach();
        cx.subscribe_in(&page, window, |this, _, person, window, cx| this.open_person(person, window, cx)).detach();
        self.open(Page::Detail(page), cx);
    }

    /// Opens the Person grid.
    fn open_person(&mut self, person: &OpenPerson, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let page = cx.new(|cx| PersonPage::new(client, person, window, cx));
        // The page can live in the back history after a sidebar move, so its subscriptions go with it.
        cx.subscribe(&page, |_, _, Play(item), cx| cx.emit(Play(item.clone()))).detach();
        cx.subscribe_in(&page, window, |this, _, Open(item), window, cx| this.open_item(item, window, cx)).detach();
        self.open(Page::Person(page), cx);
    }

    /// The Search page, made the first time, with the keyboard focus in the field.
    fn search_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Search> {
        let search =
            match &self.search {
                Some(search) => search.clone(),
                None => {
                    let client = self.client.clone();
                    let search = cx.new(|cx| Search::new(client, cx));
                    self.listen(&search, window, cx);
                    self._events.push(cx.subscribe_in(&search, window, |this, _, person, window, cx| {
                        this.open_person(person, window, cx)
                    }));
                    self.search = Some(search.clone());
                    search
                }
            };
        search.update(cx, |search, cx| search.focus_field(cx));
        search
    }

    /// Shows the Search page with the keyboard focus in the field.
    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = self.search_page(window, cx);
        match &self.page {
            Page::Search(_) => self.page.set_active(true, cx),
            _ => self.open(Page::Search(search), cx),
        }
    }

    /// Shows `page` and puts the page that showed on the back history.
    pub fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        self.back.push(self.page.clone());
        self.show(page, cx);
    }

    fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page.set_active(false, cx);
        page.set_active(self.sidebar.is_none(), cx);
        self.page = page;
        cx.notify();
    }

    /// Goes back to the last page of the history. Returns false when there is none.
    fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(page) = self.back.pop() else {
            return false;
        };
        self.show(page, cx);
        true
    }

    /// The sidebar row of Search, below the Libraries.
    fn search_row(&self) -> usize {
        self.libraries.len() + 1
    }

    /// Opens a sidebar row (0 is Home) and clears the back history. The keyboard focus goes to the page.
    fn open_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let page = match row.checked_sub(1).and_then(|i| self.libraries.get(i)).cloned() {
            None if row == self.search_row() => Page::Search(self.search_page(window, cx)),
            None => Page::Home(self.home.clone()),
            Some(library) => {
                let id = library.id.to_string();
                let page = match self.library_pages.get(&id) {
                    Some(page) => page.clone(),
                    None => {
                        let client = self.client.clone();
                        let page = cx.new(|cx| LibraryPage::new(client, library, window, cx));
                        self.listen(&page, window, cx);
                        self.library_pages.insert(id, page.clone());
                        page
                    }
                };
                Page::Library(page)
            }
        };
        self.back.clear();
        self.sidebar = None;
        self.show(page, cx);
    }

    /// The sidebar row of the page that shows. A detail page or a Person grid has the row of the page that
    /// opened it.
    fn current_row(&self, cx: &gpui::App) -> usize {
        let mut pages = std::iter::once(&self.page).chain(self.back.iter().rev());
        match pages.find(|p| !matches!(p, Page::Detail(_) | Page::Person(_))) {
            Some(Page::Search(_)) => self.search_row(),
            Some(Page::Library(page)) => {
                let id = &page.read(cx).library.id;
                self.libraries.iter().position(|l| l.id == *id).map_or(0, |i| i + 1)
            }
            _ => 0,
        }
    }

    /// Gives a key to the page. `typed` is the text that the key types, for the search field.
    fn page_key(&mut self, key: &str, typed: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.page.clone() {
            Page::Home(home) => home.update(cx, |home, cx| home.key(key, cx)),
            Page::Library(page) => page.update(cx, |page, cx| page.key(key, window, cx)),
            Page::Detail(page) => page.update(cx, |page, cx| page.key(key, cx)),
            Page::Search(page) => page.update(cx, |page, cx| page.key(key, typed, window, cx)),
            Page::Person(page) => page.update(cx, |page, cx| page.key(key, cx)),
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let m = keystroke.modifiers;
        let typed = keystroke.key_char.as_deref().filter(|_| !m.control && !m.alt && !m.platform);
        let used = (self.sidebar.is_none() && self.page_key(key, typed, window, cx))
            || match (self.sidebar, key) {
                (_, "/") => {
                    self.sidebar = None;
                    self.open_search(window, cx);
                    true
                }
                (_, key) if let Some(row) = shortcut(key, &self.libraries) => {
                    self.open_row(row, window, cx);
                    true
                }
                (None, "escape" | "backspace") => self.go_back(cx),
                (None, "h" | "left") => {
                    if self.libraries.is_empty() {
                        self.load_libraries(cx);
                    }
                    self.sidebar = Some(self.current_row(cx));
                    self.page.set_active(false, cx);
                    true
                }
                (Some(row), "j" | "down") => {
                    self.sidebar = Some((row + 1).min(self.search_row()));
                    true
                }
                (Some(row), "k" | "up") => {
                    self.sidebar = Some(row.saturating_sub(1));
                    true
                }
                (Some(row), "enter") => {
                    self.open_row(row, window, cx);
                    true
                }
                (Some(_), "l" | "right" | "escape") => {
                    self.sidebar = None;
                    self.page.set_active(true, cx);
                    true
                }
                _ => false,
            };
        if used {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn sidebar(&self, rail: bool, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let current = self.current_row(cx);
        let names = std::iter::once("Home".to_string())
            .chain(self.libraries.iter().map(|l| l.name.clone()))
            .chain(["Search".to_string()]);
        let rows = names.enumerate().map(|(row, name)| {
            let key = ["1", "2", "3"].into_iter().find(|key| shortcut(key, &self.libraries) == Some(row));
            let key = key.or((row == self.search_row()).then_some("/"));
            let label = if rail { name.chars().next().map(String::from).unwrap_or_default() } else { name };
            div()
                .id(("sidebar", row))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .gap(px(12.))
                .when(rail, |row| row.justify_center())
                .when(row == current, |row| row.bg(t.selected_fill()).text_color(t.bright))
                .border_2()
                .border_color(if self.sidebar == Some(row) { t.accent } else { gpui::transparent_black() })
                .on_click(cx.listener(move |this, _, window, cx| this.open_row(row, window, cx)))
                .child(div().flex_1().truncate().child(label))
                .children(key.filter(|_| !rail).map(|key| keycap(key, cx)))
        });
        div()
            .w(px(if rail { 64. } else { 240. }))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .p(px(if rail { 8. } else { 14. }))
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
                    .child(if rail { "s" } else { "silo" }),
            )
            .children(rows)
            .child(div().flex_1())
            .children((!rail).then(|| {
                div()
                    .px(px(10.))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(14.)).text_color(t.bright).child(self.profile_name.clone()))
                    .child(div().text_size(px(11.)).text_color(t.secondary).child(self.client.server().name.clone()))
            }))
    }
}

impl Render for Pages {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let viewport = window.viewport_size().width.to_f64() as f32;
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font.clone())
            .text_size(px(14.))
            .child(self.sidebar(sidebar_width(viewport, matches!(self.page, Page::Detail(_))) < 240., cx))
            .child(div().flex_1().min_w_0().h_full().child(self.page.clone().into_any_element()))
    }
}

#[cfg(test)]
mod tests {
    use super::{shortcut, sidebar_width};
    use crate::api::wire;

    fn library(id: &str, kind: &str) -> wire::UserLibrary {
        serde_json::from_value(serde_json::json!({"id": id, "name": id, "type": kind, "sort_order": 0})).unwrap()
    }

    #[test]
    fn one_to_three_open_home_the_first_movies_library_and_the_first_shows_library() {
        let libraries = [library("shows", "series"), library("films", "movies"), library("kids", "movies")];
        assert_eq!(shortcut("1", &libraries), Some(0));
        assert_eq!(shortcut("2", &libraries), Some(2));
        assert_eq!(shortcut("3", &libraries), Some(1));
        assert_eq!(shortcut("4", &libraries), None);
        assert_eq!(shortcut("3", &[]), None, "no Shows library");
    }

    #[test]
    fn a_narrow_window_and_the_detail_page_shrink_the_sidebar_to_the_rail() {
        assert_eq!(sidebar_width(1400., false), 240.);
        assert_eq!(sidebar_width(960., false), 64.);
        assert_eq!(sidebar_width(1400., true), 64.);
    }
}
