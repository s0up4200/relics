//! When the Server data of the pages reloads. The Live updates, the refresh after playback stops, Offline recovery
//! and the return of parked pages all come here as a `Change`. Each part of a page that loads Server data has a
//! `Fresh`, and names the changes that reach it with `Deps`. A part that shows reloads when a change reaches it. A
//! hidden part (an unselected tab, the back history, or pages behind a full-screen picker) waits until it shows
//! again, and then reloads one time. Keyboard focus does not determine visibility.
//!
//! A change that comes while a part loads waits for the end of that load, and then the part reloads one time. A
//! write of the user (a setting, a cancelled request, a favorite) holds the reloads of its part until it ends, and
//! a read that started before the write ended does not apply. A failed load waits for the next change or Retry.

use std::collections::HashSet;
use std::time::Duration;

use futures::StreamExt as _;
use futures::stream::BoxStream;
use gpui::{App, AppContext as _, Context, Entity, EntityId, Task};

use super::{Page, Pages};
use crate::api::live::LiveChange;

/// A change of the Server data. Several changes in one batch reload each part one time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Each part: the socket missed changes, playback stopped, the Server came back, or parked pages came back.
    All,
    /// The state of the Profile for an Item, or the series of an episode.
    Item { item: Option<String>, series: Option<String> },
    /// An Item changed in a Library. `None` is no Library, or no Item.
    Catalog { library: Option<String>, item: Option<String> },
    /// A Library was added, removed or changed. `None` is no Library.
    Libraries { library: Option<String> },
    /// A Server setting changed. `None` is no key. A change of the Metadata language or the Metadata exceptions
    /// reaches each part.
    Settings { key: Option<String> },
}

/// The changes that reach a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deps {
    /// Each change: Home and the poster grids. A grid cannot tell if a changed Item is in its query.
    Every,
    /// The Recommended tab of the Library with this id: each change, except a catalog or Library change of another
    /// Library.
    Library(String),
    /// The Detail page of the Item `id`. `series` is the series of an episode page.
    Item { id: String, series: Option<String> },
    /// Data of the catalog: the collections, the Person page, the Search results and the filter values. With a
    /// Library, only the changes of that Library and the changes that name no Library.
    Catalog(Option<String>),
    /// The Library list of the sidebar.
    Libraries,
    /// Only a full refresh: My requests and the request permission.
    Recovery,
    /// The Server settings of the Settings page: a full refresh and a change of a Server setting.
    Settings,
}

impl Change {
    pub fn reaches(&self, deps: &Deps) -> bool {
        let same = |library: &Option<String>, only: &Option<String>| match (library, only) {
            (Some(library), Some(only)) => library == only,
            _ => true,
        };
        match (self, deps) {
            (Change::All, _) => true,
            // The Server applies the Metadata language to its catalog answers, so it reloads each part, as a
            // Profile switch does.
            (Change::Settings { key }, deps) => {
                *deps == Deps::Settings || key.as_deref().is_some_and(crate::settings::changes_the_catalog)
            }
            (_, Deps::Every) => true,
            (_, Deps::Recovery | Deps::Settings) => false,
            (Change::Item { .. }, Deps::Library(_)) => true,
            (Change::Catalog { library, .. } | Change::Libraries { library }, Deps::Library(id)) => {
                same(library, &Some(id.clone()))
            }
            (Change::Item { item, series }, Deps::Item { id, series: page_series }) => {
                item.as_deref() == Some(id)
                    || series.as_deref().is_some_and(|s| s == id || Some(s) == page_series.as_deref())
            }
            // The event of an episode does not name its series, so each Item change reaches each Detail page.
            (Change::Catalog { item, .. }, Deps::Item { .. }) => item.is_some(),
            (Change::Libraries { .. }, Deps::Item { .. }) => false,
            (Change::Item { .. }, Deps::Catalog(_) | Deps::Libraries) => false,
            (Change::Catalog { library, .. } | Change::Libraries { library }, Deps::Catalog(only)) => {
                same(library, only)
            }
            (Change::Catalog { .. }, Deps::Libraries) => false,
            (Change::Libraries { .. }, Deps::Libraries) => true,
        }
    }
}

/// A read of a part, from `Fresh::begin`, `Fresh::more` or `Fresh::ticket`.
#[derive(Clone, Copy)]
pub struct Ticket {
    load: u64,
    write: u64,
}

/// When one part of a page reloads.
#[derive(Debug, Default)]
pub struct Fresh {
    /// A change reached the part after its last load began.
    due: bool,
    shown: bool,
    loading: bool,
    /// The writes of the user that run.
    writes: usize,
    /// Counts the loads, so that the result of a superseded load does not apply.
    load: u64,
    /// Counts the starts and ends of writes, so that a read from before a write does not apply.
    write: u64,
}

impl Fresh {
    /// A load starts that reads the data again: the first load, Retry, a new query, or a reload. It supersedes the
    /// load before it.
    pub fn begin(&mut self) -> Ticket {
        self.due = false;
        self.more()
    }

    /// A load starts that adds to the data, for example the next page.
    pub fn more(&mut self) -> Ticket {
        self.loading = true;
        self.load += 1;
        self.ticket()
    }

    /// A child read, such as the episodes of a Detail page, belongs to the load and writes of its part.
    pub fn ticket(&self) -> Ticket {
        Ticket { load: self.load, write: self.write }
    }

    /// The load of `ticket` ended. True when its result applies: no newer load began, and no write ran meanwhile. A
    /// result that a write made old is dropped, and the part reloads after the writes.
    pub fn end(&mut self, ticket: Ticket) -> bool {
        if ticket.load != self.load {
            return false;
        }
        self.loading = false;
        self.accepts(ticket)
    }

    /// A read still belongs to this load and no write ran meanwhile. An old child read queues a reload without
    /// ending the load of its part.
    pub fn accepts(&mut self, ticket: Ticket) -> bool {
        if ticket.load != self.load {
            return false;
        }
        if ticket.write != self.write {
            self.due = true;
            return false;
        }
        true
    }

    /// The part dropped its load and its data, for example an empty Search: nothing to reload.
    pub fn clear(&mut self) {
        self.due = false;
        self.loading = false;
        self.load += 1;
    }

    /// A write of the user starts.
    pub fn write(&mut self) {
        self.writes += 1;
        self.write += 1;
    }

    /// A write ended or was dropped. True when the part must reload now.
    pub fn written(&mut self) -> bool {
        self.writes -= 1;
        self.write += 1;
        self.ready()
    }

    /// A load runs.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// The part must reload now: a change reached it, it shows, and no load or write runs.
    pub fn ready(&self) -> bool {
        self.due && self.shown && !self.loading && self.writes == 0
    }

    fn touch(&mut self, shown: bool, reached: bool) -> bool {
        self.shown = shown;
        self.due |= reached;
        self.ready()
    }
}

/// A view with parts that load Server data.
pub trait Reload: Sized + 'static {
    /// The changes that reach each part, by part number.
    fn deps(&self) -> Vec<Deps>;
    fn fresh(&mut self, part: usize) -> &mut Fresh;
    /// Loads `part` again, with `Fresh::begin`.
    fn reload(&mut self, part: usize, cx: &mut Context<Self>);
}

/// A part of a page showed or hid, for example after a tab change. The pages apply the visibility again.
pub struct Moved;

/// Lets the first change through at once and opens a window of `period`. The window holds the changes that come in
/// it, and lets them through together at its end. A window that held changes opens the next window. When `leading`
/// is false, the window also holds the first change.
struct Throttle {
    period: Duration,
    leading: bool,
    open: bool,
    held: Vec<Change>,
    /// The timer of the open window.
    window: Task<()>,
}

impl Throttle {
    fn new(period: Duration) -> Self {
        Self { period, leading: true, open: false, held: Vec::new(), window: Task::ready(()) }
    }

    /// A window that also holds its first change.
    fn trailing(period: Duration) -> Self {
        Self { leading: false, ..Self::new(period) }
    }

    /// Returns the changes to run now when a window opens, or `None` when the open window holds `change`.
    fn push(&mut self, change: Change) -> Option<Vec<Change>> {
        let opens = !self.open;
        self.open = true;
        if opens && self.leading {
            return Some(vec![change]);
        }
        if !self.held.contains(&change) {
            self.held.push(change);
        }
        opens.then(Vec::new)
    }

    /// The end of the window: the changes to run now. The next window stays open when there are any.
    fn tick(&mut self) -> Vec<Change> {
        let held = std::mem::take(&mut self.held);
        self.open = !held.is_empty();
        held
    }
}

/// The freshness state of the pages.
pub struct Coordinator {
    /// The window of the progress events of the Live updates: 30 seconds.
    progress: Throttle,
    /// The window of the catalog and Library events: 2 seconds.
    catalog: Throttle,
    /// The window of the Server settings events: 1 second, so that a burst, for example a save of several keys on
    /// another client, costs one read.
    settings: Throttle,
    /// The Library list of the sidebar.
    libraries: Fresh,
    /// The pages show in the window, not behind a full-screen picker or the PIN entry.
    shown: bool,
    /// The Live updates of the Profile. The socket lives as long as the pages.
    _live: Task<()>,
}

impl Default for Coordinator {
    fn default() -> Self {
        Self {
            progress: Throttle::new(Duration::from_secs(30)),
            catalog: Throttle::new(Duration::from_secs(2)),
            settings: Throttle::trailing(Duration::from_secs(1)),
            libraries: Fresh { shown: true, ..Fresh::default() },
            shown: true,
            _live: Task::ready(()),
        }
    }
}

impl Coordinator {
    /// The Library list of the sidebar.
    pub fn libraries(&mut self) -> &mut Fresh {
        &mut self.libraries
    }
}

/// One pass over the retained parts. Each part counts one time, with the visibility of its first place.
struct Walk<'a> {
    changes: &'a [Change],
    seen: HashSet<EntityId>,
}

impl Walk<'_> {
    fn visit<T: Reload>(&mut self, view: &Entity<T>, shown: bool, cx: &mut App) {
        if !self.seen.insert(view.entity_id()) {
            return;
        }
        view.update(cx, |view, cx| {
            for (part, deps) in view.deps().iter().enumerate() {
                let reached = self.changes.iter().any(|change| change.reaches(deps));
                if view.fresh(part).touch(shown, reached) {
                    view.reload(part, cx);
                }
            }
        });
    }

    /// Each page kind names its parts. A page with no Server data of its own names only its views.
    fn page(&mut self, page: &Page, shown: bool, cx: &mut App) {
        match page {
            Page::Home(home) => self.visit(home, shown, cx),
            Page::Library(library) => {
                self.visit(library, shown, cx);
                let (recommended, collections, grid) = library.read(cx).tabs();
                if let Some((home, tab)) = recommended {
                    self.visit(&home, shown && tab, cx);
                }
                if let Some((collections, tab)) = collections {
                    self.visit(&collections, shown && tab, cx);
                }
                self.visit(&grid.0, shown && grid.1, cx);
            }
            Page::Detail(detail) => self.visit(detail, shown, cx),
            Page::Search(search) => {
                self.visit(search, shown, cx);
                if let Some(titles) = search.read(cx).titles.clone() {
                    self.visit(&titles, shown, cx);
                }
            }
            Page::Person(person) => {
                self.visit(person, shown, cx);
                self.visit(&person.read(cx).grid.clone(), shown, cx);
            }
            Page::Grid(page) | Page::Watchlist(page) | Page::History(page) => {
                self.visit(&page.read(cx).grid.clone(), shown, cx)
            }
            Page::Collections(collections) => self.visit(collections, shown, cx),
            Page::Requests(requests) => self.visit(requests, shown, cx),
            Page::Settings(settings) => self.visit(settings, shown, cx),
        }
    }
}

impl Pages {
    /// Listens to `changes`, the Live updates of the Profile.
    pub(super) fn listen_live(&mut self, mut changes: BoxStream<'static, LiveChange>, cx: &mut Context<Self>) {
        self.freshness._live = cx.spawn(async move |this, cx| {
            while let Some(change) = changes.next().await {
                if this.update(cx, |this, cx| this.live(change, cx)).is_err() {
                    return;
                }
            }
        });
    }

    fn live(&mut self, change: LiveChange, cx: &mut Context<Self>) {
        match change {
            // The socket can have missed a change of a Server setting too.
            LiveChange::Resync => self.refresh(&[Change::All, Change::Settings { key: None }], cx),
            LiveChange::UserState { item, series, progress: false } => {
                self.refresh(&[Change::Item { item, series }], cx)
            }
            LiveChange::UserState { item, series, progress: true } => {
                self.throttled(|this| &mut this.freshness.progress, Change::Item { item, series }, cx)
            }
            LiveChange::Catalog { library, item } => {
                self.throttled(|this| &mut this.freshness.catalog, Change::Catalog { library, item }, cx)
            }
            LiveChange::Libraries { library } => {
                self.throttled(|this| &mut this.freshness.catalog, Change::Libraries { library }, cx)
            }
            LiveChange::Settings { key } => {
                self.throttled(|this| &mut this.freshness.settings, Change::Settings { key }, cx)
            }
        }
    }

    /// Runs `change` through the window that `window` picks.
    fn throttled(&mut self, window: fn(&mut Self) -> &mut Throttle, change: Change, cx: &mut Context<Self>) {
        let Some(now) = window(self).push(change) else {
            return;
        };
        if !now.is_empty() {
            self.refresh(&now, cx);
        }
        let period = window(self).period;
        window(self).window = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(period).await;
                let open = this.update(cx, |this, cx| {
                    let held = window(this).tick();
                    this.refresh(&held, cx);
                    !held.is_empty()
                });
                if !matches!(open, Ok(true)) {
                    return;
                }
            }
        });
    }

    /// Reloads each retained part that `changes` reach one time. A part that shows reloads now, and keeps its scroll
    /// position and focus. A hidden part reloads when it shows again.
    pub fn refresh(&mut self, changes: &[Change], cx: &mut Context<Self>) {
        let shown = self.freshness.shown;
        let reached = changes.iter().any(|change| change.reaches(&Deps::Libraries));
        if self.freshness.libraries.touch(shown, reached) {
            self.load_libraries(cx);
        }
        let mut walk = Walk { changes, seen: HashSet::new() };
        // The page that shows comes first, so that it counts as shown when it is also in the back history.
        walk.page(&self.page, shown, cx);
        let kept = self.library_pages.values().cloned().map(Page::Library).chain(self.search.clone().map(Page::Search));
        let hidden: Vec<_> =
            self.back.iter().cloned().chain(kept).chain(std::iter::once(Page::Home(self.home.clone()))).collect();
        for page in &hidden {
            walk.page(page, false, cx);
        }
        // Playback reads the Server settings at the start of each Item, so the client reads them also when no
        // Settings page shows. A Settings page that shows reads them itself.
        // ponytail: a hidden Settings page reads them again when it shows, so that costs a second read.
        let settings = changes.iter().any(|change| matches!(change, Change::Settings { .. }));
        if settings && !(shown && matches!(self.page, Page::Settings(_))) {
            let client = self.client.clone();
            cx.background_spawn(async move {
                if let Err(err) = client.load_server_settings().await {
                    eprintln!("relics: cannot read the Server settings again: {err}");
                }
            })
            .detach();
        }
    }

    /// Applies the visibility of each part after a move: a part that became due while hidden reloads.
    pub(super) fn sync(&mut self, cx: &mut Context<Self>) {
        self.refresh(&[], cx);
    }

    /// The pages show in the window, or wait behind a full-screen picker or the PIN entry.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.freshness.shown = shown;
        self.sync(cx);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Change, Deps, Fresh, Throttle};

    #[test]
    fn an_item_change_reaches_its_detail_page_and_the_detail_page_of_its_series() {
        let detail = |id: &str, series: Option<&str>| Deps::Item { id: id.into(), series: series.map(Into::into) };
        let episode = Change::Item { item: Some("episode:e1".into()), series: Some("series:s".into()) };
        assert!(episode.reaches(&detail("episode:e1", Some("series:s"))));
        assert!(episode.reaches(&detail("series:s", None)), "the page of the series");
        assert!(episode.reaches(&detail("episode:e2", Some("series:s"))), "another episode of the series");
        assert!(!episode.reaches(&detail("movie:m", None)));
        let movie = Change::Item { item: Some("movie:m".into()), series: None };
        assert!(movie.reaches(&detail("movie:m", None)));
        assert!(!movie.reaches(&detail("episode:e1", None)), "no series matches no series");
        assert!(Change::All.reaches(&detail("movie:m", None)));
        let catalog = |library: Option<&str>, item: Option<&str>| Change::Catalog {
            library: library.map(Into::into),
            item: item.map(Into::into),
        };
        assert!(!catalog(None, None).reaches(&detail("movie:m", None)));
        assert!(catalog(Some("7"), Some("movie:m")).reaches(&detail("movie:m", None)));
        assert!(
            catalog(Some("7"), Some("episode:e1")).reaches(&detail("series:s", None)),
            "the event does not name the series of an episode, so each Item change reaches each Detail page"
        );
        assert!(!Change::Libraries { library: Some("7".into()) }.reaches(&detail("movie:m", None)));
    }

    #[test]
    fn a_library_change_reaches_only_the_parts_of_its_library() {
        let catalog = |id: Option<&str>| Change::Catalog { library: id.map(Into::into), item: None };
        let libraries = |id: Option<&str>| Change::Libraries { library: id.map(Into::into) };
        for change in [catalog, libraries] {
            assert!(change(Some("7")).reaches(&Deps::Library("7".into())));
            assert!(!change(Some("7")).reaches(&Deps::Library("8".into())));
            assert!(change(None).reaches(&Deps::Library("8".into())), "no Library reaches each tab");
            assert!(change(Some("7")).reaches(&Deps::Catalog(Some("7".into()))));
            assert!(!change(Some("7")).reaches(&Deps::Catalog(Some("8".into()))));
            assert!(change(Some("7")).reaches(&Deps::Catalog(None)));
        }
        assert!(Change::Item { item: None, series: None }.reaches(&Deps::Library("8".into())));
        assert!(!Change::Item { item: None, series: None }.reaches(&Deps::Catalog(None)));
        assert!(libraries(None).reaches(&Deps::Libraries));
        assert!(!catalog(None).reaches(&Deps::Libraries));
    }

    #[test]
    fn only_a_full_refresh_reaches_the_recovery_parts() {
        assert!(Change::All.reaches(&Deps::Recovery));
        assert!(!Change::Item { item: Some("movie:m".into()), series: None }.reaches(&Deps::Recovery));
        assert!(!Change::Catalog { library: None, item: None }.reaches(&Deps::Recovery));
        assert!(!Change::Libraries { library: None }.reaches(&Deps::Recovery));
        let setting = Change::Settings { key: None };
        assert!(setting.reaches(&Deps::Settings));
        assert!(Change::All.reaches(&Deps::Settings));
        assert!(!setting.reaches(&Deps::Every), "a setting does not reload Home and the grids");
        assert!(!setting.reaches(&Deps::Recovery));
        assert!(!Change::Item { item: None, series: None }.reaches(&Deps::Settings));
    }

    #[test]
    fn a_burst_runs_one_change_now_and_one_batch_at_the_end_of_each_window() {
        let mut throttle = Throttle::new(Duration::from_secs(30));
        let a = Change::Catalog { library: Some("1".into()), item: None };
        let b = Change::Catalog { library: Some("2".into()), item: None };
        assert_eq!(throttle.push(a.clone()), Some(vec![a.clone()]), "the first change runs now");
        assert_eq!(throttle.push(a.clone()), None);
        assert_eq!(throttle.push(b.clone()), None);
        assert_eq!(throttle.push(a.clone()), None);
        assert_eq!(throttle.tick(), [a.clone(), b], "the window ends with one of each change");
        assert_eq!(throttle.push(a.clone()), None, "the trailing batch opened the next window");
        assert_eq!(throttle.tick(), std::slice::from_ref(&a));
        assert_eq!(throttle.tick(), [], "a quiet window closes");
        assert_eq!(throttle.push(a.clone()), Some(vec![a]), "the next change runs now");
    }

    #[test]
    fn a_trailing_window_holds_its_first_change_too() {
        let mut throttle = Throttle::trailing(Duration::from_secs(1));
        let a = Change::Settings { key: Some("a".into()) };
        let b = Change::Settings { key: None };
        assert_eq!(throttle.push(a.clone()), Some(vec![]), "a window opens and nothing runs now");
        assert_eq!(throttle.push(b.clone()), None);
        assert_eq!(throttle.push(a.clone()), None);
        assert_eq!(throttle.tick(), [a.clone(), b], "the window ends with one of each change");
        assert_eq!(throttle.tick(), [], "a quiet window closes");
        assert_eq!(throttle.push(a), Some(vec![]), "the next change opens a new window");
    }

    #[test]
    fn a_read_from_before_a_write_does_not_apply_and_the_part_reloads_after_the_write() {
        let mut fresh = Fresh::default();
        assert!(!fresh.touch(true, false));
        let read = fresh.begin();
        fresh.write();
        assert!(!fresh.end(read), "the read started before the write ended");
        assert!(!fresh.ready(), "the write still runs");
        assert!(fresh.written(), "one reload after the write");
        let read = fresh.begin();
        assert!(fresh.end(read));
        assert!(!fresh.ready(), "a failed or done load alone does not reload");
    }

    #[test]
    fn a_child_read_rejects_writes_without_ending_the_main_load() {
        let mut fresh = Fresh::default();
        fresh.touch(true, false);
        let main = fresh.begin();
        let child = fresh.ticket();
        fresh.write();
        assert!(!fresh.accepts(child));
        assert!(!fresh.written(), "the main load still runs");
        assert!(fresh.loading());
        assert!(!fresh.end(main));
        assert!(fresh.ready());
        let main = fresh.begin();
        assert!(!fresh.accepts(child), "a new load supersedes its old child reads");
        assert!(fresh.end(main));
        assert!(fresh.accepts(fresh.ticket()));
        assert!(!fresh.ready());
    }
}
