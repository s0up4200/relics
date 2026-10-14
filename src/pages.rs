//! The pages of the active Profile: the sidebar, the page that shows, and the back history. Esc or Backspace
//! goes back to the page and the card that had the focus. 1 to 3 open Home, Movies and Shows, and / opens
//! Search. The sidebar also opens the Watchlist, the History, the collections of the Server and My requests. The sidebar shrinks to an icon rail when the window is narrow and on the detail page. The sidebar
//! footer, or m, opens the menu with Switch profile, Switch server and Sign out. The , key or the Settings row
//! at the bottom of the sidebar opens the Settings page. Esc goes back from it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, StatefulInteractiveElement as _,
    Styled, Subscription, Task, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, IconName, icon, keycap};

use crate::api::live::LiveChange;
use crate::api::{Client, wire};
use crate::browse;
use crate::click::Clickable as _;
use crate::collections::{Collections, OpenCollection, Scope};
use crate::detail::{self, Detail};
use crate::grid::{GridPage, Source};
use crate::home::{Home, Open, OpenGrid, Play, Rows};
use crate::keys::Key;
use crate::library_page::{LibraryPage, is_shows};
use crate::person::PersonPage;
use crate::requests::Requests;
use crate::search::{OpenPerson, Search};
use crate::settings_page::{AccountAction, SettingsPage};
use crate::state::Pair;

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

/// An entry of the sidebar menu. The Library window does the switch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuChoice {
    SwitchProfile,
    SwitchServer,
    SignOut,
}

const MENU: [(MenuChoice, &str); 3] = [
    (MenuChoice::SwitchProfile, "Switch profile"),
    (MenuChoice::SwitchServer, "Switch server"),
    (MenuChoice::SignOut, "Sign out"),
];

/// What a refresh reloads. Home, the Recommended tabs and the Detail page do not know where a refresh came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refresh {
    /// The Library list, Home, each Recommended tab that the user opened, and each Detail page.
    All,
    /// Home, each opened Recommended tab, and the Detail page of the Item, of the series, or of an episode of the
    /// series.
    Item { item: Option<String>, series: Option<String> },
    /// Home, the Recommended tab of the Library, and each Detail page. A `library` of `None` is each opened
    /// Recommended tab. An `item` of `None` is no Detail page.
    Library { library: Option<String>, item: Option<String> },
    /// The Library list, Home, and the Recommended tab of the Library. A `library` of `None` is each opened
    /// Recommended tab.
    Libraries { library: Option<String> },
}

impl Refresh {
    /// The refresh reloads the Recommended tab of the Library `id`.
    pub fn reaches_library(&self, id: &str) -> bool {
        match self {
            Refresh::Library { library: Some(library), .. } | Refresh::Libraries { library: Some(library) } => {
                library == id
            }
            _ => true,
        }
    }

    /// The refresh reloads the Detail page of the Item `page`. `page_series` is the series of an episode page.
    pub fn reaches_detail(&self, page: &str, page_series: Option<&str>) -> bool {
        match self {
            Refresh::All => true,
            // The event of an episode does not name its series, so each Item change reaches each Detail page.
            Refresh::Library { item, .. } => item.is_some(),
            Refresh::Libraries { .. } => false,
            Refresh::Item { item, series } => {
                item.as_deref() == Some(page) || series.as_deref().is_some_and(|s| s == page || Some(s) == page_series)
            }
        }
    }
}

/// Lets the first refresh through at once and opens a window of `period`. The window holds the refreshes that
/// come in it, and lets them through together at its end. A window that held refreshes opens the next window.
pub struct Throttle {
    pub period: Duration,
    open: bool,
    held: Vec<Refresh>,
    /// The timer of the open window.
    window: Task<()>,
}

impl Throttle {
    pub fn new(period: Duration) -> Self {
        Self { period, open: false, held: Vec::new(), window: Task::ready(()) }
    }

    /// Returns true when `refresh` runs now and a window opens. Otherwise the open window holds it.
    pub fn push(&mut self, refresh: Refresh) -> bool {
        if !self.open {
            self.open = true;
            return true;
        }
        if !self.held.contains(&refresh) {
            self.held.push(refresh);
        }
        false
    }

    /// The end of the window: the refreshes to run now. The next window stays open when there are any.
    pub fn tick(&mut self) -> Vec<Refresh> {
        let held = std::mem::take(&mut self.held);
        self.open = !held.is_empty();
        held
    }
}

#[derive(Clone)]
pub enum Page {
    Home(Entity<Home>),
    Library(Entity<LibraryPage>),
    Detail(Entity<Detail>),
    Search(Entity<Search>),
    /// The full list of a row.
    Grid(Entity<GridPage>),
    Person(Entity<PersonPage>),
    Watchlist(Entity<GridPage>),
    History(Entity<GridPage>),
    /// The collections of the Server.
    Collections(Entity<Collections>),
    /// My requests.
    Requests(Entity<Requests>),
    Settings(Entity<SettingsPage>),
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
            Page::Grid(page) | Page::Watchlist(page) | Page::History(page) => {
                page.update(cx, |page, cx| page.set_active(active, cx))
            }
            Page::Collections(page) => page.update(cx, |page, cx| {
                page.active = active;
                cx.notify();
            }),
            Page::Requests(page) => page.update(cx, |page, cx| {
                page.active = active;
                cx.notify();
            }),
            Page::Settings(page) => page.update(cx, |page, cx| {
                page.active = active;
                cx.notify();
            }),
        }
    }

    /// The page shows again. A page that became stale while it did not show reloads.
    fn shown(&self, cx: &mut gpui::App) {
        match self {
            Page::Home(home) => home.update(cx, |home, cx| home.shown(cx)),
            Page::Library(page) => page.update(cx, |page, cx| page.shown(cx)),
            Page::Detail(page) => page.update(cx, |page, cx| page.shown(cx)),
            _ => {}
        }
    }

    fn into_any_element(self) -> AnyElement {
        match self {
            Page::Home(home) => home.into_any_element(),
            Page::Library(page) => page.into_any_element(),
            Page::Detail(page) => page.into_any_element(),
            Page::Search(page) => page.into_any_element(),
            Page::Person(page) => page.into_any_element(),
            Page::Grid(page) | Page::Watchlist(page) | Page::History(page) => page.into_any_element(),
            Page::Collections(page) => page.into_any_element(),
            Page::Requests(page) => page.into_any_element(),
            Page::Settings(page) => page.into_any_element(),
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
    /// The selected entry of the sidebar menu, while the menu is open.
    menu: Option<usize>,
    /// Each Server and Account pair, and the Active server, for the Accounts section of the Settings page.
    pub pairs: (Vec<Pair>, Option<(String, String)>),
    focus_handle: FocusHandle,
    /// The window of the progress events of the Live updates: 30 seconds.
    progress: Throttle,
    /// The window of the catalog events of the Live updates: 2 seconds.
    catalog: Throttle,
    /// The Live updates of the Profile. The socket lives as long as the pages.
    _live: Task<()>,
    _libraries: Task<()>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for Pages {}
impl EventEmitter<MenuChoice> for Pages {}
impl EventEmitter<AccountAction> for Pages {}

impl Pages {
    pub fn new(client: Arc<Client>, profile_name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let home = cx.new(|cx| Home::new(client.clone(), Rows::Home, window, cx));
        cx.subscribe_in(&home, window, |this, _, open: &OpenGrid, window, cx| this.open_grid(open, window, cx))
            .detach();
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
            menu: None,
            pairs: (Vec::new(), None),
            focus_handle: cx.focus_handle(),
            progress: Throttle::new(Duration::from_secs(30)),
            catalog: Throttle::new(Duration::from_secs(2)),
            _live: Task::ready(()),
            _libraries: Task::ready(()),
            _events: Vec::new(),
        };
        pages.listen(&home, window, cx);
        pages.page.set_active(true, cx);
        pages.load_libraries(cx);
        pages.listen_live(cx);
        pages.focus(window, cx);
        pages
    }

    fn listen_live(&mut self, cx: &mut Context<Self>) {
        let mut changes = self.client.live_updates();
        self._live = cx.spawn(async move |this, cx| {
            while let Some(change) = changes.next().await {
                if this.update(cx, |this, cx| this.live(change, cx)).is_err() {
                    return;
                }
            }
        });
    }

    fn live(&mut self, change: LiveChange, cx: &mut Context<Self>) {
        match change {
            LiveChange::Resync => self.refresh(&Refresh::All, cx),
            LiveChange::UserState { item, series, progress: false } => {
                self.refresh(&Refresh::Item { item, series }, cx)
            }
            LiveChange::UserState { item, series, progress: true } => {
                self.throttled(|this| &mut this.progress, Refresh::Item { item, series }, cx)
            }
            LiveChange::Catalog { library, item } => {
                self.throttled(|this| &mut this.catalog, Refresh::Library { library, item }, cx)
            }
            LiveChange::Libraries { library } => {
                self.throttled(|this| &mut this.catalog, Refresh::Libraries { library }, cx)
            }
        }
    }

    /// Runs `refresh` through the window that `window` picks.
    fn throttled(&mut self, window: fn(&mut Self) -> &mut Throttle, refresh: Refresh, cx: &mut Context<Self>) {
        if !window(self).push(refresh.clone()) {
            return;
        }
        self.refresh(&refresh, cx);
        let period = window(self).period;
        window(self).window = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(period).await;
                let open = this.update(cx, |this, cx| {
                    let held = window(this).tick();
                    for refresh in &held {
                        this.refresh(refresh, cx);
                    }
                    !held.is_empty()
                });
                if !matches!(open, Ok(true)) {
                    return;
                }
            }
        });
    }

    /// Reloads the pages that `refresh` reaches. A page that shows reloads now, and keeps its scroll position and
    /// focus. A page that does not show becomes stale, and reloads one time when it shows again.
    pub fn refresh(&mut self, refresh: &Refresh, cx: &mut Context<Self>) {
        if matches!(refresh, Refresh::All | Refresh::Libraries { .. }) {
            self.load_libraries(cx);
        }
        let home = matches!(&self.page, Page::Home(home) if *home == self.home);
        self.home.update(cx, |page, cx| page.refresh(home, cx));
        for page in self.library_pages.values() {
            let shows = matches!(&self.page, Page::Library(p) if p == page);
            page.update(cx, |page, cx| {
                if refresh.reaches_library(&page.library.id.to_string()) {
                    page.refresh(shows, cx);
                }
            });
        }
        for page in std::iter::once(&self.page).chain(&self.back) {
            if let Page::Detail(detail) = page {
                let shows = matches!(&self.page, Page::Detail(p) if p == detail);
                detail.update(cx, |detail, cx| {
                    let (id, series) = detail.shows();
                    if refresh.reaches_detail(id, series) {
                        detail.refresh(shows, cx);
                    }
                });
            }
        }
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
                    // The list can be shorter now.
                    this.sidebar = this.sidebar.map(|row| row.min(this.settings_row()));
                    cx.notify();
                })
                .ok();
            }
            // The sidebar shows Home only. It reads the list again when it gets the focus.
            Err(err) => eprintln!("relics: cannot read the libraries: {err}"),
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

    /// Opens the detail page of an Item. For a season, the page of its series with the season.
    fn open_item(&mut self, item: &wire::CatalogItem, window: &mut Window, cx: &mut Context<Self>) {
        let (id, season) = detail::target(item);
        let client = self.client.clone();
        let page = cx.new(|cx| Detail::new(client, id, season, window, cx));
        cx.subscribe(&page, |_, _, Play(item), cx| cx.emit(Play(item.clone()))).detach();
        cx.subscribe_in(&page, window, |this, _, person, window, cx| this.open_person(person, window, cx)).detach();
        cx.subscribe_in(&page, window, |this, _, Open(item), window, cx| this.open_item(item, window, cx)).detach();
        self.open(Page::Detail(page), cx);
    }

    /// Opens the Person page.
    fn open_person(&mut self, person: &OpenPerson, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let page = cx.new(|cx| PersonPage::new(client, person, window, cx));
        cx.subscribe(&page, |_, _, Play(item), cx| cx.emit(Play(item.clone()))).detach();
        cx.subscribe_in(&page, window, |this, _, Open(item), window, cx| this.open_item(item, window, cx)).detach();
        self.open(Page::Person(page), cx);
    }

    /// Opens a page with the grid of a query, for example "View all" of a row.
    fn open_grid(&mut self, open: &OpenGrid, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.grid_page(open.title.clone(), open.query.clone(), window, cx);
        self.open(Page::Grid(page), cx);
    }

    fn grid_page(
        &mut self,
        title: String,
        source: impl Into<Source>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<GridPage> {
        let client = self.client.clone();
        let page = cx.new(|cx| GridPage::new(client, title, source, window, cx));
        // The page can live in the back history after a sidebar move, so its subscriptions go with it.
        cx.subscribe(&page, |_, _, Play(item), cx| cx.emit(Play(item.clone()))).detach();
        cx.subscribe_in(&page, window, |this, _, Open(item), window, cx| this.open_item(item, window, cx)).detach();
        page
    }

    /// Opens a page with the grid of the Items of a collection.
    fn open_collection(&mut self, open: &OpenCollection, window: &mut Window, cx: &mut Context<Self>) {
        let source = Source::Collection { library_id: open.library_id.clone(), id: open.id.clone() };
        let page = self.grid_page(open.title.clone(), source, window, cx);
        self.open(Page::Grid(page), cx);
    }

    /// A new page with the collections of the Server.
    fn collections_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<Collections> {
        let client = self.client.clone();
        let page = cx.new(|cx| Collections::new(client, Scope::Server, cx));
        cx.subscribe_in(&page, window, |this, _, open: &OpenCollection, window, cx| {
            this.open_collection(open, window, cx)
        })
        .detach();
        page
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

    /// Opens a new Settings page, which reads the Server settings again.
    fn open_settings(&mut self, cx: &mut Context<Self>) {
        let (client, (pairs, active)) = (self.client.clone(), self.pairs.clone());
        let page = cx.new(|cx| SettingsPage::new(client, pairs, active, cx));
        cx.subscribe(&page, |_, _, action: &AccountAction, cx| cx.emit(action.clone())).detach();
        self.sidebar = None;
        self.open(Page::Settings(page), cx);
    }

    /// Shows `page` and puts the page that showed on the back history.
    pub fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        self.back.push(self.page.clone());
        self.show(page, cx);
    }

    fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page.set_active(false, cx);
        page.set_active(self.sidebar.is_none(), cx);
        page.shown(cx);
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

    /// The sidebar row of the Watchlist, below Search.
    fn watchlist_row(&self) -> usize {
        self.search_row() + 1
    }

    /// The sidebar row of the History, below the Watchlist.
    fn history_row(&self) -> usize {
        self.search_row() + 2
    }

    /// The sidebar row of the Collections page, below the History.
    fn collections_row(&self) -> usize {
        self.search_row() + 3
    }

    /// The sidebar row of My requests, below the Collections page.
    fn requests_row(&self) -> usize {
        self.search_row() + 4
    }

    /// The sidebar row of Settings, at the bottom.
    fn settings_row(&self) -> usize {
        self.search_row() + 5
    }

    /// Opens a sidebar row (0 is Home) and clears the back history. The keyboard focus goes to the page.
    /// Settings keeps the history, so that Esc goes back from it.
    fn open_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if row == self.settings_row() {
            return self.open_settings(cx);
        }
        let page = match row.checked_sub(1).and_then(|i| self.libraries.get(i)).cloned() {
            None if row == self.search_row() => Page::Search(self.search_page(window, cx)),
            None if row == self.watchlist_row() => {
                Page::Watchlist(self.grid_page("Watchlist".into(), browse::source_query("watchlist"), window, cx))
            }
            None if row == self.history_row() => {
                Page::History(self.grid_page("History".into(), browse::source_query("history"), window, cx))
            }
            None if row == self.collections_row() => Page::Collections(self.collections_page(window, cx)),
            None if row == self.requests_row() => {
                let client = self.client.clone();
                Page::Requests(cx.new(|cx| Requests::new(client, cx)))
            }
            None => Page::Home(self.home.clone()),
            Some(library) => {
                let id = library.id.to_string();
                let page = match self.library_pages.get(&id) {
                    Some(page) => page.clone(),
                    None => {
                        let client = self.client.clone();
                        let page = cx.new(|cx| LibraryPage::new(client, library, window, cx));
                        self.listen(&page, window, cx);
                        self._events.push(cx.subscribe_in(&page, window, |this, _, open: &OpenGrid, window, cx| {
                            this.open_grid(open, window, cx)
                        }));
                        self._events.push(cx.subscribe_in(
                            &page,
                            window,
                            |this, _, open: &OpenCollection, window, cx| this.open_collection(open, window, cx),
                        ));
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

    /// The sidebar row of the page that shows. A detail page, a Person page or the full list of a row has the
    /// row of the page that opened it.
    fn current_row(&self, cx: &gpui::App) -> usize {
        let mut pages = std::iter::once(&self.page).chain(self.back.iter().rev());
        match pages.find(|p| !matches!(p, Page::Detail(_) | Page::Grid(_) | Page::Person(_))) {
            Some(Page::Search(_)) => self.search_row(),
            Some(Page::Watchlist(_)) => self.watchlist_row(),
            Some(Page::History(_)) => self.history_row(),
            Some(Page::Collections(_)) => self.collections_row(),
            Some(Page::Requests(_)) => self.requests_row(),
            Some(Page::Settings(_)) => self.settings_row(),
            Some(Page::Library(page)) => {
                let id = &page.read(cx).library.id;
                self.libraries.iter().position(|l| l.id == *id).map_or(0, |i| i + 1)
            }
            _ => 0,
        }
    }

    fn page_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if self.sidebar.take().is_some() {
            self.page.set_active(true, cx);
        }
        cx.notify();
    }

    /// Gives a key to the page. `typed` is the text that the key types, for the search field.
    fn page_key(&mut self, key: Key, typed: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.page.clone() {
            Page::Home(home) => home.update(cx, |home, cx| home.key(key, cx)),
            Page::Library(page) => page.update(cx, |page, cx| page.key(key, typed, window, cx)),
            Page::Detail(page) => page.update(cx, |page, cx| page.key(key, cx)),
            Page::Person(page) => page.update(cx, |page, cx| page.key(key, cx)),
            Page::Search(page) => page.update(cx, |page, cx| page.key(key, typed, window, cx)),
            Page::Grid(page) | Page::Watchlist(page) | Page::History(page) => {
                page.update(cx, |page, cx| page.key(key, cx))
            }
            Page::Collections(page) => page.update(cx, |page, cx| page.key(key, cx)),
            Page::Requests(page) => page.update(cx, |page, cx| page.key(key, cx)),
            Page::Settings(page) => page.update(cx, |page, cx| page.key(key, cx)),
        }
    }

    /// Opens or closes the sidebar menu.
    fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = if self.menu.is_some() { None } else { Some(0) };
        cx.notify();
    }

    fn choose(&mut self, choice: MenuChoice, cx: &mut Context<Self>) {
        self.menu = None;
        cx.emit(choice);
        cx.notify();
    }

    /// A key while the menu is open. The menu takes each key.
    fn menu_key(&mut self, selected: usize, key: Key, cx: &mut Context<Self>) {
        match key {
            Key::Down => self.menu = Some((selected + 1).min(MENU.len() - 1)),
            Key::Up => self.menu = Some(selected.saturating_sub(1)),
            Key::Other("enter") => self.choose(MENU[selected].0, cx),
            Key::Other("escape" | "backspace" | "m") => self.menu = None,
            _ => {}
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = Key::parse(&keystroke.key);
        if let Some(selected) = self.menu {
            return self.menu_key(selected, key, cx);
        }
        let m = keystroke.modifiers;
        let typed = keystroke.key_char.as_deref().filter(|_| !m.control && !m.alt && !m.platform);
        let used = (self.sidebar.is_none() && self.page_key(key, typed, window, cx))
            || match (self.sidebar, key) {
                (_, Key::Other("/")) => {
                    self.sidebar = None;
                    self.open_search(window, cx);
                    true
                }
                (_, Key::Other(",")) => {
                    self.open_settings(cx);
                    true
                }
                (_, Key::Other("m")) => {
                    self.toggle_menu(cx);
                    true
                }
                (_, Key::Other(key)) if let Some(row) = shortcut(key, &self.libraries) => {
                    self.open_row(row, window, cx);
                    true
                }
                (None, Key::Other("escape" | "backspace")) => self.go_back(cx),
                (None, Key::Left) => {
                    if self.libraries.is_empty() {
                        self.load_libraries(cx);
                    }
                    self.sidebar = Some(self.current_row(cx));
                    self.page.set_active(false, cx);
                    true
                }
                (Some(row), Key::Down) => {
                    self.sidebar = Some((row + 1).min(self.settings_row()));
                    true
                }
                (Some(row), Key::Up) => {
                    self.sidebar = Some(row.saturating_sub(1));
                    true
                }
                (Some(row), Key::Other("enter")) => {
                    self.open_row(row, window, cx);
                    true
                }
                (Some(_), Key::Right | Key::Other("escape")) => {
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
        let library = |l: &wire::UserLibrary| {
            let icon = match l.type_.as_str() {
                "movies" | "movie" => IconName::Film,
                kind if is_shows(kind) => IconName::Tv,
                _ => IconName::Layers,
            };
            (icon, l.name.clone())
        };
        let names =
            std::iter::once((IconName::House, "Home".to_string())).chain(self.libraries.iter().map(library)).chain(
                [
                    (IconName::Search, "Search"),
                    (IconName::Bookmark, "Watchlist"),
                    (IconName::RotateCcwClock, "History"),
                    (IconName::LibraryBig, "Collections"),
                    (IconName::Inbox, "My requests"),
                    (IconName::Settings, "Settings"),
                ]
                .map(|(icon, name)| (icon, name.to_string())),
            );
        let mut rows: Vec<_> = names
            .enumerate()
            .map(|(row, (icon_name, name))| {
                let key = ["1", "2", "3"].into_iter().find(|key| shortcut(key, &self.libraries) == Some(row));
                let key = key.or((row == self.search_row()).then_some("/"));
                let key = key.or((row == self.settings_row()).then_some(","));
                div()
                    .id(("sidebar", row))
                    .clickable()
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
                    .child(icon(icon_name))
                    .when(!rail, |row| row.child(div().flex_1().truncate().child(name)))
                    .children(key.filter(|_| !rail).map(|key| keycap(key, cx)))
            })
            .collect();
        let settings = rows.pop();
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
                    .child(if rail { "r" } else { "relics" }),
            )
            .children(rows)
            .child(div().flex_1())
            .children(settings)
            .children(self.menu.map(|selected| self.menu_list(selected, cx)))
            .child(self.footer(rail, cx))
    }

    /// The Profile and the Server of the pages. A click opens the menu.
    fn footer(&self, rail: bool, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let initial = self.profile_name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        div()
            .id("account-menu")
            .clickable()
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .gap(px(12.))
            .when(rail, |row| row.justify_center())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(cx)))
            .when(rail, |row| row.text_color(t.bright).child(initial))
            .when(!rail, |row| {
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div().truncate().text_size(px(14.)).text_color(t.bright).child(self.profile_name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(t.secondary)
                                .child(self.client.server().name.clone()),
                        ),
                )
                .child(keycap("m", cx))
            })
    }

    fn menu_list(&self, selected: usize, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let entries = MENU.iter().enumerate().map(|(row, &(choice, label))| {
            div()
                .id(("menu", row))
                .clickable()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(8.))
                .truncate()
                .border_2()
                .border_color(if row == selected { t.accent } else { gpui::transparent_black() })
                .on_click(cx.listener(move |this, _, _, cx| this.choose(choice, cx)))
                .child(label)
        });
        div().p(px(4.)).flex().flex_col().gap(px(2.)).rounded(px(12.)).bg(t.surface).children(entries)
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
            // The open menu widens the rail to the full sidebar, so that its entries have room.
            .child(self.sidebar(
                sidebar_width(viewport, matches!(self.page, Page::Detail(_))) < 240. && self.menu.is_none(),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    // A click on the page moves the focus from the sidebar and the menu to the page. A right-click
                    // does too, because it opens the menu of an episode card.
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::page_click))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::page_click))
                    .child(self.page.clone().into_any_element()),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Refresh, Throttle, shortcut, sidebar_width};
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

    #[test]
    fn an_item_change_reaches_its_detail_page_and_the_detail_page_of_its_series() {
        let episode = Refresh::Item { item: Some("episode:e1".into()), series: Some("series:s".into()) };
        assert!(episode.reaches_detail("episode:e1", Some("series:s")));
        assert!(episode.reaches_detail("series:s", None), "the page of the series");
        assert!(episode.reaches_detail("episode:e2", Some("series:s")), "another episode of the series");
        assert!(!episode.reaches_detail("movie:m", None));
        let movie = Refresh::Item { item: Some("movie:m".into()), series: None };
        assert!(movie.reaches_detail("movie:m", None));
        assert!(!movie.reaches_detail("episode:e1", None), "no series matches no series");
        assert!(Refresh::All.reaches_detail("movie:m", None));
        let library = |library: Option<&str>, item: Option<&str>| Refresh::Library {
            library: library.map(Into::into),
            item: item.map(Into::into),
        };
        assert!(!library(None, None).reaches_detail("movie:m", None));
        assert!(library(Some("7"), Some("movie:m")).reaches_detail("movie:m", None), "a catalog change of the Item");
        assert!(
            library(Some("7"), Some("episode:e1")).reaches_detail("series:s", None),
            "the event does not name the series of an episode, so each Item change reaches each Detail page"
        );
        assert!(!Refresh::Libraries { library: Some("7".into()) }.reaches_detail("movie:m", None));
    }

    #[test]
    fn a_library_change_reaches_only_the_recommended_tab_of_its_library() {
        let library = |id: Option<&str>| Refresh::Library { library: id.map(Into::into), item: None };
        assert!(library(Some("7")).reaches_library("7"));
        assert!(!library(Some("7")).reaches_library("8"));
        assert!(library(None).reaches_library("8"), "no Library reaches each tab");
        let libraries = |id: Option<&str>| Refresh::Libraries { library: id.map(Into::into) };
        assert!(libraries(Some("7")).reaches_library("7"));
        assert!(!libraries(Some("7")).reaches_library("8"));
        assert!(libraries(None).reaches_library("8"));
        assert!(Refresh::All.reaches_library("8"));
        assert!(Refresh::Item { item: None, series: None }.reaches_library("8"));
    }

    #[test]
    fn a_burst_runs_one_refresh_now_and_one_at_the_end_of_each_window() {
        let mut throttle = Throttle::new(Duration::from_secs(30));
        let a = Refresh::Library { library: Some("1".into()), item: None };
        let b = Refresh::Library { library: Some("2".into()), item: None };
        assert!(throttle.push(a.clone()), "the first refresh runs now");
        assert!(!throttle.push(a.clone()));
        assert!(!throttle.push(b.clone()));
        assert!(!throttle.push(a.clone()));
        assert_eq!(throttle.tick(), [a.clone(), b], "the window ends with one refresh of each target");
        assert!(!throttle.push(a.clone()), "the trailing refresh opened the next window");
        assert_eq!(throttle.tick(), std::slice::from_ref(&a));
        assert_eq!(throttle.tick(), [], "a quiet window closes");
        assert!(throttle.push(a), "the next refresh runs now");
    }
}
