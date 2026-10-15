//! Page freshness: when the pages load Server data again. The tests drive the real pages with keys, a stream of
//! Live updates that the test controls, and a fake Server that answers in memory, so no test reaches a real host.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Log, Reply, Seen, json, not_found, ok};
use futures::AsyncReadExt as _;
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext};
use gpui::{VisualTestContext, Window};
use http_client::FakeHttpClient;
use relics::api::Client;
use relics::api::live::LiveChange;
use relics::pages::Pages;
use relics::settings::SubtitleMode;

struct PageWindow(Entity<Pages>);
impl Render for PageWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_omarchy::focus_scope("test-pages").size_full().child(self.0.clone())
    }
}

/// The fake Server. A path that starts with a prefix in `fail` gets no answer (a network failure). A request with
/// a prefix in `hold` gets its answer at once, but the client gets it only at `release`.
#[derive(Clone, Default)]
struct Server {
    seen: Log,
    fail: Arc<Mutex<Vec<String>>>,
    hold: Arc<Mutex<Vec<String>>>,
    held: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
    /// The subtitle mode of the Profile: the last value that a write sent.
    mode: Arc<Mutex<String>>,
}

impl Server {
    fn requests(&self, method: &str, prefix: &str) -> Vec<Seen> {
        self.seen.lock().unwrap().iter().filter(|r| r.method == method && r.path.starts_with(prefix)).cloned().collect()
    }

    /// The number of GET requests with a path that starts with `prefix`.
    fn count(&self, prefix: &str) -> usize {
        self.requests("GET", prefix).len()
    }

    fn fail(&self, prefixes: &[&str]) {
        *self.fail.lock().unwrap() = prefixes.iter().map(|p| p.to_string()).collect();
    }

    fn hold(&self, prefixes: &[&str]) {
        *self.hold.lock().unwrap() = prefixes.iter().map(|p| p.to_string()).collect();
    }

    /// Lets the held requests go on.
    fn release(&self) {
        for held in self.held.lock().unwrap().drain(..) {
            held.send(()).ok();
        }
    }

    fn reply(&self, req: &Seen) -> Reply {
        if self.fail.lock().unwrap().iter().any(|p| req.path.starts_with(p.as_str())) {
            return None;
        }
        let path = req.path.split('?').next().unwrap();
        match (req.method.as_str(), path) {
            (_, "user/libraries") => ok("user_libraries.json"),
            (_, "home/layout" | "library/12/layout") => json(200, r#"{"sections":[]}"#.into()),
            (_, "catalog/filters") => json(
                200,
                r#"{"authors":[],"narrators":[],"series":[],"content_ratings":[],"genres":[],"studios":[],"networks":[],"countries":[],"original_languages":[]}"#.into(),
            ),
            (_, "catalog/people") if req.path.contains("q=person") => {
                json(200, r#"{"items":[{"id":"7","name":"Test Person"}]}"#.into())
            }
            (_, "catalog/people") => json(200, r#"{"items":[]}"#.into()),
            (_, "catalog/people/7") => json(200, r#"{"id":"7","name":"Test Person"}"#.into()),
            (_, "catalog/query") if req.body.contains(r#""q":"item""#) => json(
                200,
                r#"{"items":[{"content_id":"test-item","type":"movie","title":"Test Item","genres":[],"keywords":[],"status":"matched"}],"total":1,"total_exact":true,"window_cursor":"test","page":{"has_more":false}}"#.into(),
            ),
            // The Watchlist has two pages.
            (_, "catalog/query") if req.body.contains(r#""source":"watchlist""#) => {
                let more = !req.body.contains(r#""cursor":"c2""#);
                let ids: &[&str] = if more { &["a", "b", "c"] } else { &["d", "e"] };
                let items: Vec<_> = ids.iter().map(|id| serde_json::json!({"content_id": id, "type": "movie", "title": id, "genres": [], "keywords": [], "status": "matched"})).collect();
                let page = if more { serde_json::json!({"has_more": true, "next_cursor": "c2"}) } else { serde_json::json!({"has_more": false}) };
                json(200, serde_json::json!({"items": items, "total": 5, "total_exact": true, "window_cursor": "w", "page": page}).to_string())
            }
            (_, "catalog/query") => ok("query_catalog_items_ok.json"),
            (_, "catalog/items/test-item") => ok("get_catalog_item_ok.json"),
            (_, "catalog/items/test-item/versions") => ok("list_catalog_item_versions_ok.json"),
            (_, "recommendations/similar/test-item") => ok("list_similar_ok.json"),
            (_, "collections/server") => json(200, r#"{"libraries":[]}"#.into()),
            (_, "library/12/collections") => ok("get_library_collections_ok.json"),
            ("GET", "requests/mine") => ok("list_my_requests_ok.json"),
            ("GET", "requests/status") => ok("request_status_ok.json"),
            ("GET", "requests/search") => {
                json(200, r#"{"page":1,"total_pages":1,"total_results":0,"results":[]}"#.into())
            }
            ("POST", "requests/r-2/cancel") => ok("cancel_request_ok.json"),
            ("GET", "settings/values/effective") => {
                let mode = self.mode.lock().unwrap().clone();
                let item = serde_json::json!({"key": "playback.subtitle_mode", "value": mode, "source": "profile", "definition_revision": 1});
                json(200, serde_json::json!({"revision": 1, "items": [item]}).to_string())
            }
            ("PUT", "settings/values/playback.subtitle_mode") => {
                let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
                *self.mode.lock().unwrap() = body["value"].as_str().unwrap().to_string();
                ok("update_setting_value_ok.json")
            }
            _ => not_found(),
        }
    }
}

/// The pages of the Profile `p-owner` on the fake Server, with a stream of Live updates that the test sends.
fn rig(cx: &mut TestAppContext) -> (Entity<Pages>, &mut VisualTestContext, Server, Changes, Arc<Client>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
    });
    let server = Server { mode: Arc::new(Mutex::new("auto".into())), ..Server::default() };
    let fake = server.clone();
    let http = FakeHttpClient::create(move |req| {
        let server = fake.clone();
        async move {
            let method = req.method().to_string();
            let url = req.uri().to_string();
            let path = url.split_once("api/v2/").map_or(url.as_str(), |(_, path)| path).to_string();
            let headers = req.headers().iter().map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_string())).collect();
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            let request = Seen { method, url, path, headers, body };
            server.seen.lock().unwrap().push(request.clone());
            let reply = server.reply(&request);
            if server.hold.lock().unwrap().iter().any(|p| request.path.starts_with(p.as_str())) {
                let (go, wait) = oneshot::channel();
                server.held.lock().unwrap().push(go);
                wait.await.ok();
            }
            reply.ok_or_else(|| anyhow::anyhow!("no route to host"))
        }
    });
    let client = Arc::new(common::client(http, "https://silo.test/"));
    client.set_profile(Some("p-owner".into()));
    let (changes, stream) = mpsc::unbounded();
    let pages_client = client.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        PageWindow(cx.new(|cx| Pages::with_changes(pages_client, "Test".into(), stream.boxed(), window, cx)))
    });
    cx.run_until_parked();
    let pages = cx.update(|_, cx| view.read(cx).0.clone());
    (pages, cx, server, changes, client)
}

type Changes = mpsc::UnboundedSender<LiveChange>;

/// Sends `change` as a Live update and lets the pages act on it.
fn send(cx: &mut VisualTestContext, changes: &Changes, change: LiveChange) {
    changes.unbounded_send(change).unwrap();
    cx.run_until_parked();
}

/// Opens sidebar row `row` from Home: 0 is Home, 1 the Movies library, 2 Search, 3 Watchlist, 4 History, 5
/// Collections, 6 My requests and 7 Settings.
fn open_row(cx: &mut VisualTestContext, row: usize) {
    cx.simulate_keystrokes("1 left");
    for _ in 0..row {
        cx.simulate_keystrokes("down");
    }
    cx.simulate_keystrokes("enter");
}

/// Types `text` in Search and waits for the search.
fn search(cx: &mut VisualTestContext, text: &str) {
    cx.simulate_keystrokes("/ super-a");
    cx.simulate_input(text);
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
}

fn catalog(library: Option<&str>) -> LiveChange {
    LiveChange::Catalog { library: library.map(Into::into), item: None }
}

#[gpui::test]
fn recovery_reloads_collections_my_requests_and_settings_after_a_failed_first_load(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    for (row, path) in [(5, "collections/server"), (6, "requests/mine"), (7, "settings/values/effective")] {
        server.fail(&[path]);
        open_row(cx, row);
        assert_eq!(server.count(path), 1, "{path}: the first load fails");
        server.fail(&[]);
        cx.run_until_parked();
        assert_eq!(server.count(path), 1, "{path}: a failure alone does not retry");
        send(cx, &changes, LiveChange::Resync);
        assert_eq!(server.count(path), 2, "{path}: the recovery reloads it");
        send(cx, &changes, LiveChange::UserState { item: Some("movie:m".into()), series: None, progress: false });
        send(cx, &changes, catalog(None));
        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        let expected = if path == "collections/server" { 3 } else { 2 };
        assert_eq!(server.count(path), expected, "{path}: only a catalog change reaches the collections");
    }
}

#[gpui::test]
fn recovery_reloads_every_part_of_search(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    server.fail(&["catalog/", "requests/"]);
    search(cx, "heat");
    assert_eq!(server.count("requests/search"), 0, "no request permission, so no Request to add");
    let before = [server.count("catalog/people"), server.count("catalog/filters"), server.count("requests/status")];
    server.fail(&[]);
    send(cx, &changes, LiveChange::Resync);
    let after = [server.count("catalog/people"), server.count("catalog/filters"), server.count("requests/status")];
    assert_eq!(after, before.map(|n| n + 1), "the people, the filter values and the request permission");
    assert_eq!(server.count("requests/search"), 1, "the permission came back, so Request to add loads");
    assert!(server.requests("POST", "catalog/query").last().unwrap().body.contains(r#""q":"heat""#));
}

#[gpui::test]
fn recovery_reloads_the_person_and_the_filmography(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    search(cx, "person");
    server.fail(&["catalog/people/7", "catalog/query"]);
    cx.simulate_keystrokes("enter enter");
    let queries = server.requests("POST", "catalog/query").len();
    assert_eq!(server.count("catalog/people/7"), 1);
    server.fail(&[]);
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count("catalog/people/7"), 2);
    let filmography =
        server.requests("POST", "catalog/query")[queries..].iter().filter(|r| r.body.contains("person")).count();
    assert_eq!(filmography, 1, "the filmography grid reloads one time");
}

#[gpui::test]
fn hidden_pages_wait_until_they_show_and_then_reload_one_time(cx: &mut TestAppContext) {
    let (pages, cx, server, changes, _) = rig(cx);
    search(cx, "item");
    cx.simulate_keystrokes("enter enter");
    assert_eq!(server.count("catalog/items/test-item?"), 1, "the Detail page opened over Search");
    let people = server.count("catalog/people");
    send(cx, &changes, LiveChange::Resync);
    send(cx, &changes, catalog(None));
    assert_eq!(server.count("catalog/items/test-item?"), 2, "the Detail page shows, so it reloads");
    assert_eq!(server.count("catalog/people"), people, "Search is in the back history");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(server.count("catalog/people"), people + 1, "two changes, one reload when Search shows");

    // A full-screen picker hides the pages. The sidebar focus does not.
    pages.update(cx, |pages, cx| pages.set_shown(false, cx));
    let (home, sidebar) = (server.count("home/layout"), server.count("user/libraries"));
    send(cx, &changes, LiveChange::Resync);
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count("catalog/people"), people + 1, "the pages wait behind the picker");
    assert_eq!(server.count("user/libraries"), sidebar, "the sidebar waits too");
    pages.update(cx, |pages, cx| pages.set_shown(true, cx));
    cx.run_until_parked();
    assert_eq!(server.count("catalog/people"), people + 2, "one reload after the picker");
    assert_eq!(server.count("user/libraries"), sidebar + 1);
    assert_eq!(server.count("home/layout"), home, "Home does not show");
    cx.simulate_keystrokes("1 left");
    let home = server.count("home/layout");
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count("home/layout"), home + 1, "Home shows while the sidebar has the focus");
}

#[gpui::test]
fn a_library_page_reloads_only_its_open_tabs(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    open_row(cx, 1);
    let grid = server.requests("POST", "catalog/query").len();
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count("library/12/layout"), 0, "the Recommended tab was never opened");
    assert_eq!(server.count("library/12/collections"), 0, "the Collections tab was never opened");
    assert_eq!(server.requests("POST", "catalog/query").len(), grid + 1, "the grid shows");
    assert_eq!(server.count("catalog/filters?library_id=12"), 2, "the filter values recover");
}

#[gpui::test]
fn changes_during_a_load_give_one_follow_up_load(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    open_row(cx, 6);
    server.hold(&["requests/mine"]);
    send(cx, &changes, LiveChange::Resync);
    send(cx, &changes, LiveChange::Resync);
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count("requests/mine"), 2, "the reload runs, and the changes wait for it");
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count("requests/mine"), 3, "one follow-up load");
}

#[gpui::test]
fn a_burst_of_catalog_changes_reloads_now_and_one_time_at_the_end_of_the_window(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    open_row(cx, 5);
    send(cx, &changes, catalog(Some("12")));
    send(cx, &changes, catalog(None));
    send(cx, &changes, LiveChange::Libraries { library: Some("12".into()) });
    assert_eq!(server.count("collections/server"), 2, "the first change goes through at once");
    cx.executor().advance_clock(Duration::from_millis(1999));
    cx.run_until_parked();
    assert_eq!(server.count("collections/server"), 2);
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(server.count("collections/server"), 3, "one reload for the whole batch");
    assert_eq!(server.count("user/libraries"), 2, "the Library change reloads the sidebar in the batch");
}

#[gpui::test]
fn progress_changes_wait_30_seconds_and_do_not_reach_my_requests(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    let progress = |item: &str| LiveChange::UserState { item: Some(item.into()), series: None, progress: true };
    let home = server.count("home/layout");
    send(cx, &changes, progress("movie:a"));
    send(cx, &changes, progress("movie:b"));
    send(cx, &changes, progress("movie:c"));
    assert_eq!(server.count("home/layout"), home + 1);
    cx.executor().advance_clock(Duration::from_secs(29));
    cx.run_until_parked();
    assert_eq!(server.count("home/layout"), home + 1);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(server.count("home/layout"), home + 2, "one reload for the two held changes");
    open_row(cx, 6);
    send(cx, &changes, progress("movie:d"));
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(server.count("requests/mine"), 1);
}

#[gpui::test]
fn a_reload_waits_for_a_setting_write_and_a_read_from_before_the_write_does_not_apply(cx: &mut TestAppContext) {
    let (_, cx, server, changes, client) = rig(cx);
    let effective = "settings/values/effective";
    open_row(cx, 7);
    assert_eq!(server.count(effective), 1);
    // Row 2 is the subtitle mode: auto, then always, then off.
    server.hold(&["settings/values/playback."]);
    cx.simulate_keystrokes("j j l");
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count(effective), 1, "the reload waits for the write");
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count(effective), 2, "one reload after the write");
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Always);

    server.hold(&[effective]);
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count(effective), 3, "the read has the value from before the next write");
    cx.simulate_keystrokes("l");
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Off);
    server.release();
    cx.run_until_parked();
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Off, "the old read does not apply");
    assert_eq!(server.count(effective), 4, "the page reads again after the write");
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Off);
}

#[gpui::test]
fn an_item_change_reloads_the_detail_page_of_that_item_only(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    search(cx, "item");
    cx.simulate_keystrokes("enter enter");
    let item = |id: &str| LiveChange::UserState { item: Some(id.into()), series: None, progress: false };
    let home = server.count("home/layout");
    send(cx, &changes, item("movie:other"));
    assert_eq!(server.count("catalog/items/test-item?"), 1, "another Item");
    send(cx, &changes, item("test-item"));
    assert_eq!(server.count("catalog/items/test-item?"), 2);
    assert_eq!(server.count("home/layout"), home, "Home is in the back history");
    cx.simulate_keystrokes("1");
    cx.run_until_parked();
    assert_eq!(server.count("home/layout"), home + 1, "Home reloads one time when it shows");
}

#[gpui::test]
fn a_refresh_during_the_load_of_the_next_page_waits_for_it(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    let watchlist = || {
        server
            .requests("POST", "catalog/query")
            .into_iter()
            .filter(|r| r.body.contains("watchlist"))
            .collect::<Vec<_>>()
    };
    open_row(cx, 3);
    assert_eq!(watchlist().len(), 1);
    server.hold(&["catalog/query"]);
    cx.simulate_keystrokes("right");
    assert!(watchlist()[1].body.contains(r#""cursor":"c2""#), "the next page loads");
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(watchlist().len(), 2, "the reload waits for the next page");
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    let reload = watchlist();
    assert_eq!(reload.len(), 4, "one reload after the next page, over both pages");
    assert!(!reload[2].body.contains(r#""cursor":"c2""#), "the reload starts at the first page");
    assert!(reload[3].body.contains(r#""cursor":"c2""#), "and covers the Items that the grid loaded");
}

#[gpui::test]
fn a_follow_up_load_waits_while_its_page_is_hidden(cx: &mut TestAppContext) {
    let (pages, cx, server, changes, _) = rig(cx);
    open_row(cx, 6);
    server.hold(&["requests/mine"]);
    send(cx, &changes, LiveChange::Resync);
    send(cx, &changes, LiveChange::Resync);
    assert_eq!(server.count("requests/mine"), 2, "a reload runs, and a follow-up is due");
    pages.update(cx, |pages, cx| pages.set_shown(false, cx));
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count("requests/mine"), 2, "hiding does not cancel the load, and the follow-up waits");
    pages.update(cx, |pages, cx| pages.set_shown(true, cx));
    cx.run_until_parked();
    assert_eq!(server.count("requests/mine"), 3, "the follow-up runs when the page shows");
}
