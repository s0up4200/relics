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

struct PageWindow(Entity<Pages>, Option<Entity<relics::library::LibraryWindow>>);
impl Render for PageWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let root = gpui_omarchy::focus_scope("test-pages").size_full();
        match &self.1 {
            Some(library) => root.child(library.clone()),
            None => root.child(self.0.clone()),
        }
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
    empty: Arc<Mutex<Vec<String>>>,
    pin: Arc<Mutex<bool>>,
    replies: Arc<Mutex<std::collections::HashMap<String, String>>>,
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
        if self.empty.lock().unwrap().iter().any(|prefix| req.path.starts_with(prefix)) {
            return json(200, r#"{"items":[],"page":{"has_more":false}}"#.into());
        }
        if *self.pin.lock().unwrap()
            && req.path.starts_with("catalog/filters")
            && req.header("x-profile-token").is_none()
        {
            return common::problem(403, "profile_verification_required.json");
        }
        if let Some(body) = self
            .replies
            .lock()
            .unwrap()
            .iter()
            .find_map(|(prefix, body)| req.path.starts_with(prefix).then_some(body.clone()))
        {
            return json(200, body);
        }
        let path = req.path.split('?').next().unwrap();
        match (req.method.as_str(), path) {
            (_, "system/info") => ok("get_system_info_ok.json"),
            (_, "profiles") => ok("list_profiles_ok.json"),
            (_, "sync/progress") => ok("sync_progress_ok.json"),
            (_, "profiles/p-owner/verify-pin") => ok("verify_profile_pin_ok.json"),
            (_, "catalog/items/movie:heat-1995/versions") => ok("list_catalog_item_versions_ok.json"),
            (_, "playback/capabilities") => ok("playback_capability_available.json"),
            (_, "playback/start") => ok("playback_start_opaque_ids.json"),
            (_, "user/libraries") => ok("user_libraries.json"),
            (_, "home/layout" | "home/sections" | "library/12/layout") => json(200, r#"{"sections":[]}"#.into()),
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
            (_, "catalog/items/test-series") => {
                let mut item: serde_json::Value = serde_json::from_str(&common::fixture("get_catalog_item_ok.json")).unwrap();
                item["content_id"] = "test-series".into();
                item["type"] = "series".into();
                json(200, item.to_string())
            }
            (_, "catalog/series/test-series/seasons") => ok("list_series_seasons_ok.json"),
            (_, "catalog/series/test-series/seasons/1/episodes") => ok("list_season_episodes_ok.json"),
            ("DELETE", "watched/test-series" | "watched/test-item") => json(204, String::new()),
            (_, "recommendations/similar/test-item") => ok("list_similar_ok.json"),
            (_, "collections/server") => json(200, r#"{"libraries":[]}"#.into()),
            (_, "library/12/collections") => ok("get_library_collections_ok.json"),
            ("GET", "requests/mine") => ok("list_my_requests_ok.json"),
            ("GET", "requests/status") => ok("request_status_ok.json"),
            ("GET", "requests/search") => {
                json(200, r#"{"page":1,"total_pages":1,"total_results":0,"results":[]}"#.into())
            }
            ("POST", "requests/r-2/cancel") => ok("cancel_request_ok.json"),
            ("GET", "settings/contract/capabilities") => ok("get_settings_contract_capabilities_ok.json"),
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
    make_rig(cx, false)
}

fn make_rig(
    cx: &mut TestAppContext,
    library: bool,
) -> (Entity<Pages>, &mut VisualTestContext, Server, Changes, Arc<Client>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        relics::text_input::init(cx);
        common::isolate(cx);
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
    // The Library window makes the client of each Server that it switches to with the HTTP client of the app.
    cx.update(|cx| cx.set_http_client(http.clone()));
    let client = Arc::new(common::client(http, "https://silo.test/"));
    client.set_profile(Some("p-owner".into()));
    let (changes, stream) = mpsc::unbounded();
    let pages_client = client.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let pages = cx.new(|cx| Pages::with_changes(pages_client.clone(), "Test".into(), stream.boxed(), window, cx));
        let library = library.then(|| {
            let mut state = relics::state::State::fresh();
            // The Server list has a second Server, with the same fake behind it.
            for (n, url) in [(1, "https://silo.test/"), (2, "https://silo2.test/")] {
                state.servers.push(relics::state::SavedServer {
                    id: format!("server-{n}"),
                    url: url.into(),
                    accounts: vec![relics::state::SavedAccount {
                        id: format!("account-{n}"),
                        username: "Test".into(),
                        last_profile: Some("p-owner".into()),
                    }],
                });
            }
            state.active = Some(("server-1".into(), "account-1".into()));
            let tokens = relics::api::Tokens { access_token: "acc".into(), refresh_token: "ref".into() };
            let tokens = (1..=2).map(|n| ((format!("server-{n}"), format!("account-{n}")), tokens.clone())).collect();
            // The Profile picker has a second Profile, with no PIN.
            let mut profiles: serde_json::Value =
                serde_json::from_str(&common::fixture("list_profiles_ok.json")).unwrap();
            let mut kid = profiles["items"][0].clone();
            (kid["id"], kid["name"]) = ("p-kid".into(), "Kid".into());
            profiles["items"].as_array_mut().unwrap().push(kid);
            let profiles: relics::api::wire::ProfileCollection = serde_json::from_value(profiles).unwrap();
            let mut profiles = profiles.items;
            profiles[0].has_pin = true;
            let session = relics::library::Session {
                state,
                directory: relics::settings::directory(cx),
                client: pages_client,
                pages: pages.clone(),
                profiles,
                tokens,
            };
            cx.new(|cx| relics::library::LibraryWindow::with_session(session, window, cx))
        });
        PageWindow(pages, library)
    });
    cx.run_until_parked();
    let pages = cx.update(|_, cx| view.read(cx).0.clone());
    (pages, cx, server, changes, client)
}

type Changes = mpsc::UnboundedSender<LiveChange>;

#[gpui::test]
fn an_episode_read_from_before_a_watched_mark_triggers_a_corrective_reload(cx: &mut TestAppContext) {
    let (pages, cx, server, _, client) = rig(cx);
    let episodes = "catalog/series/test-series/seasons/1/episodes";
    server.hold(&[episodes]);
    let detail = cx
        .update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), None, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(detail.clone()), cx));
    cx.run_until_parked();
    assert_eq!(server.count(episodes), 1);
    // Play, Shuffle, Watchlist, Favorite, Watched.
    detail.update(cx, |detail, cx| {
        for _ in 0..4 {
            detail.key(relics::keys::Key::Right, cx);
        }
        detail.key(relics::keys::Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert_eq!(server.requests("DELETE", "watched/test-series").len(), 1);
    assert_eq!(server.count("catalog/items/test-series?"), 1);
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count("catalog/items/test-series?"), 2, "the old episode read queues a reload");
    assert_eq!(server.count(episodes), 2);
}

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
    // Row 6 is the subtitle mode: auto, then always, then off.
    server.hold(&["settings/values/playback."]);
    cx.simulate_keystrokes("j j j j j j l");
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

fn drawn(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.debug_bounds(selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn empty_pages_keep_their_success_after_a_failed_reload(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    *server.empty.lock().unwrap() = vec!["requests/mine".into()];
    for (row, path, retry) in [
        (0, "home/layout", "home-retry"),
        (5, "collections/server", "collections-retry"),
        (6, "requests/mine", "requests-retry"),
    ] {
        server.fail(&[]);
        open_row(cx, row);
        cx.run_until_parked();
        server.fail(&[path]);
        send(cx, &changes, LiveChange::Resync);
        assert!(!drawn(cx, retry), "{path}: an empty successful result still belongs to this page");
    }
    server.fail(&[]);
    search(cx, "empty");
    server.fail(&["catalog/people"]);
    send(cx, &changes, LiveChange::Resync);
    assert!(!drawn(cx, "search-retry"));
    search(cx, "new choice");
    assert!(drawn(cx, "search-retry"), "success for the previous query cannot hide a new query's failure");
    let titles = server.requests("POST", "catalog/query").len();
    click(cx, "search-retry");
    assert!(drawn(cx, "search-retry"), "Retry can fail again");
    assert_eq!(server.requests("POST", "catalog/query").len(), titles, "People Retry does not restart titles");
    server.fail(&[]);
    server.hold(&["catalog/people"]);
    click(cx, "search-retry");
    assert!(drawn(cx, "people-loading"));
    assert!(!drawn(cx, "search-retry"));
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert!(!drawn(cx, "people-loading"));
    assert!(!drawn(cx, "search-retry"));
}

#[gpui::test]
fn episodes_retry_without_restarting_the_item_or_similar_rows(cx: &mut TestAppContext) {
    let (pages, cx, server, _, client) = rig(cx);
    let episodes = "catalog/series/test-series/seasons/1/episodes";
    server.fail(&[episodes]);
    let detail = cx
        .update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), None, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(detail), cx));
    cx.run_until_parked();
    assert!(drawn(cx, "episodes-retry"));
    let item = server.count("catalog/items/test-series?");
    let similar = server.count("recommendations/similar/test-series");
    server.fail(&[]);
    click(cx, "episodes-retry");
    assert!(!drawn(cx, "episodes-retry"));
    assert_eq!(server.count(episodes), 2);
    assert_eq!(server.count("catalog/items/test-series?"), item);
    assert_eq!(server.count("recommendations/similar/test-series"), similar);
}

fn keep_progress(client: &Client, server: &Server, cx: &mut VisualTestContext) {
    futures::executor::block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    server.fail(&["playback/11111111-1111-4111-8111-111111111111"]);
    futures::executor::block_on(client.stop_playback(
        "11111111-1111-4111-8111-111111111111",
        Some(&relics::playback::Sample { sequence: 1, position: 75.5, paused: false }),
    ))
    .unwrap_err();
    cx.run_until_parked();
    server.fail(&[]);
}

fn recover(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

#[gpui::test]
fn library_recovery_waits_for_progress_sync_before_refreshing(cx: &mut TestAppContext) {
    let (_, cx, server, _, client) = make_rig(cx, true);
    keep_progress(&client, &server, cx);
    server.hold(&["sync/progress"]);
    let home = server.count("home/layout");
    recover(cx);
    assert_eq!(server.count("system/info"), 1);
    assert_eq!(server.requests("POST", "sync/progress").len(), 1);
    assert_eq!(server.count("home/layout"), home, "page refresh waits for the held sync");
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count("home/layout"), home + 1);
    let sync = &server.requests("POST", "sync/progress")[0];
    let body: serde_json::Value = serde_json::from_str(&sync.body).unwrap();
    assert_eq!(body["items"][0]["position_ms"], 75500);
    futures::executor::block_on(client.sync_progress()).unwrap();
    assert_eq!(server.requests("POST", "sync/progress").len(), 1, "successful sync consumed the saved progress");
}

#[gpui::test]
fn library_refreshes_after_failed_sync_and_sends_unsent_progress_later(cx: &mut TestAppContext) {
    let (_, cx, server, _, client) = make_rig(cx, true);
    keep_progress(&client, &server, cx);
    server.fail(&["sync/progress"]);
    server.hold(&["sync/progress"]);
    let home = server.count("home/layout");
    recover(cx);
    assert_eq!(server.count("home/layout"), home);
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count("home/layout"), home + 1, "failure still ends the recovery attempt");
    server.fail(&[]);
    recover(cx);
    let requests = server.requests("POST", "sync/progress");
    assert_eq!(requests.len(), 2);
    let body: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(body["items"][0]["position_ms"], 75500, "failed sync retains progress for recovery");
}

#[gpui::test]
fn library_defers_reload_behind_profile_server_and_pin_screens(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = make_rig(cx, true);
    for menu_keys in ["m enter", "m down enter"] {
        cx.simulate_keystrokes(menu_keys);
        let home = server.count("home/layout");
        for _ in 0..3 {
            send(cx, &changes, LiveChange::Resync);
        }
        assert_eq!(server.count("home/layout"), home);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(server.count("home/layout"), home + 1, "one reload on return from the picker");
    }
    *server.pin.lock().unwrap() = true;
    cx.simulate_keystrokes("/");
    cx.run_until_parked();
    let filters = server.count("catalog/filters");
    for _ in 0..3 {
        send(cx, &changes, LiveChange::Resync);
    }
    assert_eq!(server.count("catalog/filters"), filters, "PIN entry hides the pages");
    assert!(filters > 0, "Search sent the request that asks for a PIN");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_input("1234");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(server.requests("POST", "profiles/p-owner/verify-pin").len(), 1);
    // The held request retries with the token, then one queued refresh runs.
    assert_eq!(server.count("catalog/filters"), filters + 2);
}

/// Keeps progress that the Server did not get, holds the sync of the recovery, and lets the Server answer again.
/// With `fail`, the held sync fails.
fn hold_recovery_sync(client: &Client, server: &Server, fail: bool, cx: &mut VisualTestContext) {
    keep_progress(client, server, cx);
    if fail {
        server.fail(&["sync/progress"]);
    }
    server.hold(&["sync/progress"]);
    recover(cx);
    assert_eq!(server.requests("POST", "sync/progress").len(), 1, "the recovery sync waits");
}

/// Completes the held sync of the old recovery and checks that it went to the old Profile on the old Server, and
/// that it reloads none of the pages that replaced the old pages.
fn complete_old_sync(server: &Server, cx: &mut VisualTestContext) {
    let home = server.count("home/layout");
    server.hold(&[]);
    server.fail(&[]);
    server.release();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(server.count("home/layout"), home, "the old recovery reloads no replacement page");
    let syncs = server.requests("POST", "sync/progress");
    assert_eq!(syncs.len(), 1, "no other Profile or Server sends the kept progress");
    assert_owner_progress(&syncs[0]);
}

/// The sync sends the kept progress of `p-owner` to the first Server.
fn assert_owner_progress(sync: &Seen) {
    assert!(sync.url.starts_with("https://silo.test/"), "{}", sync.url);
    assert_eq!(sync.header("x-profile-id"), Some("p-owner"));
    let body: serde_json::Value = serde_json::from_str(&sync.body).unwrap();
    assert_eq!(body["items"][0]["position_ms"], 75500);
}

#[gpui::test]
fn old_recovery_cannot_reload_the_pages_of_the_next_profile(cx: &mut TestAppContext) {
    let (_, cx, server, _, client) = make_rig(cx, true);
    hold_recovery_sync(&client, &server, false, cx);
    let home = server.count("home/layout");
    cx.simulate_keystrokes("m enter right enter");
    cx.run_until_parked();
    assert_eq!(server.count("home/layout"), home + 1, "the pages of the next Profile load");
    complete_old_sync(&server, cx);
    futures::executor::block_on(client.sync_progress()).unwrap();
    assert_eq!(server.requests("POST", "sync/progress").len(), 1, "the next Profile has no kept progress");
}

#[gpui::test]
fn old_recovery_cannot_reload_the_pages_of_the_next_server(cx: &mut TestAppContext) {
    let (_, cx, server, _, client) = make_rig(cx, true);
    hold_recovery_sync(&client, &server, true, cx);
    let home = server.requests("GET", "home/layout").len();
    cx.simulate_keystrokes("m down enter down enter");
    cx.run_until_parked();
    let next = server.requests("GET", "home/layout");
    assert_eq!(next.len(), home + 1, "the pages of the next Server load");
    assert!(next[home].url.starts_with("https://silo2.test/"), "{}", next[home].url);
    complete_old_sync(&server, cx);
    cx.simulate_keystrokes("m down enter up enter");
    cx.run_until_parked();
    let syncs = server.requests("POST", "sync/progress");
    assert_eq!(syncs.len(), 2, "the progress of the failed sync goes out when its Account is active again");
    assert_owner_progress(&syncs[1]);
}

#[gpui::test]
fn grid_retries_the_missing_page_and_keeps_the_first_page(cx: &mut TestAppContext) {
    let (pages, cx, server, _, _) = rig(cx);
    open_row(cx, 3);
    server.fail(&["catalog/query"]);
    cx.simulate_keystrokes("right");
    assert!(drawn(cx, "grid-retry"));
    let before = server.requests("POST", "catalog/query");
    assert!(before.last().unwrap().body.contains(r#""cursor":"c2""#));
    server.fail(&[]);
    click(cx, "grid-retry");
    let after = server.requests("POST", "catalog/query");
    assert_eq!(after.len(), before.len() + 1);
    assert!(after.last().unwrap().body.contains(r#""cursor":"c2""#));
    // Focus was on b when the next page failed. Enter must still open b.
    cx.update(|window, cx| pages.update(cx, |pages, cx| pages.focus(window, cx)));
    cx.simulate_keystrokes("enter");
    assert_eq!(server.count("catalog/items/b?"), 1);
}

#[gpui::test]
fn first_load_retry_retries_only_the_failed_page(cx: &mut TestAppContext) {
    let (pages, cx, server, _, _) = rig(cx);
    for (row, path, retry) in [
        (5, "collections/server", "collections-retry"),
        (6, "requests/mine", "requests-retry"),
        (7, "settings/values/effective", "settings-retry"),
    ] {
        server.fail(&[path]);
        cx.update(|window, cx| pages.update(cx, |pages, cx| pages.focus(window, cx)));
        open_row(cx, row);
        assert!(drawn(cx, retry), "{path}: first load failed");
        server.hold(&[path]);
        click(cx, retry);
        assert!(!drawn(cx, retry), "Retry clears the failed operation's error while it loads");
        server.hold(&[]);
        server.release();
        cx.run_until_parked();
        assert!(drawn(cx, retry), "Retry can fail again");
        server.fail(&[]);
        click(cx, retry);
        assert!(!drawn(cx, retry));
    }
}

#[gpui::test]
fn episodes_show_loading_during_retry_and_main_content_stays_available(cx: &mut TestAppContext) {
    let (pages, cx, server, _, client) = rig(cx);
    let episodes = "catalog/series/test-series/seasons/1/episodes";
    server.fail(&[episodes]);
    let detail = cx
        .update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), None, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(detail), cx));
    cx.run_until_parked();
    server.fail(&[]);
    server.hold(&[episodes]);
    click(cx, "episodes-retry");
    assert!(drawn(cx, "episodes-loading"));
    assert!(!drawn(cx, "detail-retry"));
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert!(!drawn(cx, "episodes-loading"));
}

#[gpui::test]
fn an_old_search_failure_cannot_replace_the_new_query(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    search(cx, "old");
    server.fail(&["catalog/people"]);
    server.hold(&["catalog/people"]);
    send(cx, &changes, LiveChange::Resync);
    server.fail(&[]);
    server.hold(&[]);
    search(cx, "person");
    server.release();
    cx.run_until_parked();
    assert!(!drawn(cx, "search-retry"));
    cx.simulate_keystrokes("enter enter");
    assert_eq!(server.count("catalog/people/7"), 1, "Enter still opens the new query's Person");
}

#[gpui::test]
fn a_season_change_rejects_the_previous_episode_response(cx: &mut TestAppContext) {
    use relics::keys::Key;
    let (pages, cx, server, _, client) = rig(cx);
    let first = "catalog/series/test-series/seasons/1/episodes";
    let second = "catalog/series/test-series/seasons/2/episodes";
    let mut seasons: serde_json::Value = serde_json::from_str(&common::fixture("list_series_seasons_ok.json")).unwrap();
    let mut season = seasons["items"][0].clone();
    season["season_number"] = 2.into();
    season["content_id"] = "season:second".into();
    seasons["items"].as_array_mut().unwrap().push(season);
    server.replies.lock().unwrap().insert("catalog/series/test-series/seasons?".into(), seasons.to_string());
    let mut episodes: serde_json::Value =
        serde_json::from_str(&common::fixture("list_season_episodes_ok.json")).unwrap();
    episodes["items"][0]["content_id"] = "episode:second".into();
    episodes["items"][0]["season_number"] = 2.into();
    server.replies.lock().unwrap().insert(second.into(), episodes.to_string());
    server.hold(&[first]);
    let detail = cx
        .update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), None, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(detail.clone()), cx));
    cx.run_until_parked();
    assert_eq!(server.count(first), 1);
    detail.update(cx, |detail, cx| {
        for key in [Key::Down, Key::Right, Key::Other("enter"), Key::Down, Key::Down] {
            detail.key(key, cx);
        }
    });
    cx.run_until_parked();
    assert_eq!(server.count(second), 1);
    assert!(drawn(cx, "episode-episode:second"));
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert!(drawn(cx, "episode-episode:second"));
    assert!(!drawn(cx, "episode-episode:severance-s01e01"));
}

#[gpui::test]
fn similar_rows_fail_quietly_and_keep_their_last_success(cx: &mut TestAppContext) {
    let (pages, cx, server, changes, client) = rig(cx);
    let detail =
        cx.update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-item".into(), None, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(detail.clone()), cx));
    cx.run_until_parked();
    detail.update(cx, |detail, cx| {
        for _ in 0..20 {
            detail.key(relics::keys::Key::Down, cx);
        }
    });
    assert!(drawn(cx, "similar-card"));
    server.fail(&["recommendations/similar"]);
    send(cx, &changes, LiveChange::Resync);
    assert!(drawn(cx, "similar-card"));
    assert!(!drawn(cx, "detail-retry"));
}

#[gpui::test]
fn a_similar_response_from_before_a_write_cannot_fill_the_row(cx: &mut TestAppContext) {
    let (pages, cx, server, _, client) = rig(cx);
    let similar = "recommendations/similar/test-item";
    server.hold(&[similar]);
    let detail =
        cx.update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-item".into(), None, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(detail.clone()), cx));
    cx.run_until_parked();
    // A working hero action proves that the main Item does not wait for More Like This.
    detail.update(cx, |detail, cx| {
        for _ in 0..3 {
            detail.key(relics::keys::Key::Right, cx);
        }
        detail.key(relics::keys::Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert_eq!(server.requests("DELETE", "watched/test-item").len(), 1);
    server.replies.lock().unwrap().insert(similar.into(), r#"{"items":[]}"#.into());
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count("catalog/items/test-item?"), 2, "the obsolete child queues a fresh parent load");
    detail.update(cx, |detail, cx| {
        for _ in 0..20 {
            detail.key(relics::keys::Key::Down, cx);
        }
    });
    assert!(!drawn(cx, "similar-card"));
}

#[gpui::test]
fn a_successful_home_reload_retries_failed_artwork(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (pages, cx, server, changes, client) = rig(cx);
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let http = FakeHttpClient::create(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        async { anyhow::bail!("artwork is offline") }
    });
    cx.update(|_, cx| {
        let dir = relics::settings::directory(cx).join("retry-artwork");
        relics::artwork::install(http, dir, cx);
    });
    let mut layout: serde_json::Value = serde_json::from_str(&common::fixture("get_home_layout_ok.json")).unwrap();
    layout["sections"].as_array_mut().unwrap().truncate(1);
    let mut sections: serde_json::Value = serde_json::from_str(&common::fixture("list_home_sections_ok.json")).unwrap();
    sections["sections"].as_array_mut().unwrap().truncate(1);
    for item in sections["sections"][0]["items"].as_array_mut().unwrap() {
        for key in ["poster_url", "backdrop_url", "still_url"] {
            item[key] = "https://artwork.test/image.webp".into();
        }
    }
    server.replies.lock().unwrap().insert("home/layout".into(), layout.to_string());
    server.replies.lock().unwrap().insert("home/sections".into(), sections.to_string());
    let home =
        cx.update(|window, cx| cx.new(|cx| relics::home::Home::new(client, relics::home::Rows::Home, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Home(home), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    let first = requests.load(Ordering::SeqCst);
    assert!(first > 0, "the visible artwork failed through its separate fake client");
    server.fail(&["home/layout"]);
    send(cx, &changes, LiveChange::Resync);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert_eq!(requests.load(Ordering::SeqCst), first, "a failed page load does not retry artwork");
    server.fail(&[]);
    send(cx, &changes, LiveChange::Resync);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert!(requests.load(Ordering::SeqCst) > first, "the existing successful-load trigger retries artwork");
}

#[gpui::test]
fn optional_search_failures_keep_request_results_and_leave_the_page_usable(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    let row = serde_json::json!({"page":1,"total_pages":1,"total_results":1,"results":[{
        "availability":"missing","in_watchlist":false,"media_type":"movie","title":"Missing movie","tmdb_id":42,
        "request":{"requestable":true,"following":false,"requested_by_viewer":false}
    }]});
    server.replies.lock().unwrap().insert("requests/search".into(), row.to_string());
    search(cx, "missing");
    assert!(drawn(cx, "request-row"));
    server.fail(&["requests/search", "requests/status", "catalog/filters"]);
    send(cx, &changes, LiveChange::Resync);
    assert!(drawn(cx, "request-row"), "optional reload failures keep the request results");
    assert!(!drawn(cx, "search-retry"));
    // A different query has no previous optional row to retain.
    search(cx, "another");
    assert!(!drawn(cx, "request-row"));
    assert!(!drawn(cx, "search-retry"), "a failed optional first load does not become a page error");
}

#[gpui::test]
fn a_filter_change_rejects_the_held_grid_response(cx: &mut TestAppContext) {
    let (pages, cx, server, _, client) = rig(cx);
    let query = relics::browse::Browse::default().query("12", false);
    server.hold(&["catalog/query"]);
    let page =
        cx.update(|window, cx| cx.new(|cx| relics::grid::GridPage::new(client, "Filtered".into(), query, window, cx)));
    let grid = page.read_with(cx, |page, _| page.grid.clone());
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Grid(page), cx));
    cx.run_until_parked();
    server.hold(&[]);
    let new = serde_json::json!({"items":[{"content_id":"new-filter","type":"movie","title":"New filter","genres":[],"keywords":[],"status":"matched"}],"total":1,"total_exact":true,"window_cursor":"new","page":{"has_more":false}});
    server.replies.lock().unwrap().insert("catalog/query".into(), new.to_string());
    let mut browse = relics::browse::Browse::default();
    browse.filters.genres = vec!["Crime".into()];
    grid.update(cx, |grid, cx| grid.set_query(browse.query("12", false), cx));
    cx.run_until_parked();
    server.release();
    cx.run_until_parked();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let _opened = cx.update(|_, cx| {
        cx.subscribe(&grid, move |_, event: &relics::home::Open, _| {
            seen.lock().unwrap().push(event.0.content_id.clone());
        })
    });
    grid.update(cx, |grid, cx| grid.key(relics::keys::Key::Other("enter"), cx));
    cx.run_until_parked();
    assert_eq!(*opened.lock().unwrap(), ["new-filter"]);
}

#[gpui::test]
fn a_previous_items_held_reply_does_not_hide_the_current_items_failure(cx: &mut TestAppContext) {
    let (pages, cx, server, _, client) = rig(cx);
    server.hold(&["catalog/items/test-item"]);
    let previous = cx.update(|window, cx| {
        cx.new(|cx| relics::detail::Detail::new(client.clone(), "test-item".into(), None, window, cx))
    });
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(previous), cx));
    cx.run_until_parked();
    server.fail(&["catalog/items/test-series"]);
    let current = cx
        .update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), None, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Detail(current), cx));
    cx.run_until_parked();
    assert!(drawn(cx, "detail-retry"));
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
    assert!(drawn(cx, "detail-retry"));
}

#[gpui::test]
fn a_write_that_rejects_pagination_does_not_leave_loading_on_the_background_reload(cx: &mut TestAppContext) {
    let (_, cx, server, _, _) = rig(cx);
    let mut first: serde_json::Value = serde_json::from_str(&common::fixture("list_my_requests_ok.json")).unwrap();
    first["page"] = serde_json::json!({"has_more":true,"next_cursor":"c2"});
    server.replies.lock().unwrap().insert("requests/mine".into(), first.to_string());
    open_row(cx, 6);
    server.hold(&["requests/mine"]);
    cx.simulate_keystrokes("down up enter");
    assert_eq!(server.requests("POST", "requests/r-2/cancel").len(), 1);
    server.replies.lock().unwrap().insert("requests/mine".into(), common::fixture("list_my_requests_ok.json"));
    server.release();
    cx.run_until_parked();
    assert_eq!(server.count("requests/mine"), 3, "the rejected next page queues a reload after the write");
    assert!(!drawn(cx, "requests-status"), "the completed next page cannot leave loading text on a background reload");
    server.hold(&[]);
    server.release();
    cx.run_until_parked();
}

fn set_reply(server: &Server, path: &str, body: serde_json::Value) {
    server.replies.lock().unwrap().insert(path.into(), body.to_string());
}

fn catalog_reply(ids: &[&str]) -> serde_json::Value {
    let items: Vec<_> = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "content_id": id, "type": "movie", "title": id, "genres": [], "keywords": [], "status": "matched"
            })
        })
        .collect();
    serde_json::json!({"items": items, "total": ids.len(), "total_exact": true, "window_cursor": "w", "page": {"has_more": false}})
}

#[gpui::test]
fn grid_replacement_keeps_the_item_then_clamps_and_clears_the_target(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    set_reply(&server, "catalog/query", catalog_reply(&["a", "b", "c"]));
    let grid = cx.update(|window, cx| {
        cx.new(|cx| relics::grid::Grid::new(client, relics::api::wire::CatalogQuery::default(), window, cx))
    });
    cx.run_until_parked();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&grid, move |_, event: &relics::home::Open, _| {
            seen.lock().unwrap().push(event.0.content_id.clone());
        })
    });
    grid.update(cx, |grid, cx| {
        grid.key(Key::Down, cx);
    });
    for ids in [&["b", "a", "c"][..], &["x"], &[]] {
        set_reply(&server, "catalog/query", catalog_reply(ids));
        grid.update(cx, |grid, cx| grid.reload(0, cx));
        cx.run_until_parked();
        grid.update(cx, |grid, cx| {
            grid.key(Key::Other("enter"), cx);
        });
    }
    assert_eq!(*opened.lock().unwrap(), ["b", "x"]);
}

fn home_reply(server: &Server, rows: &[(&str, &[&str], bool)]) {
    let sections: Vec<_> = rows
        .iter()
        .map(|(id, ids, all)| {
            serde_json::json!({
                "id": id, "section_type": "recently_added", "title": id, "featured": false,
                "item_limit": 20, "is_custom": false, "customized": false,
                "total_count": ids.len() + usize::from(*all), "items": catalog_reply(ids)["items"]
            })
        })
        .collect();
    set_reply(server, "home/layout", serde_json::json!({"sections": sections}));
    set_reply(server, "home/sections", serde_json::json!({"sections": sections}));
}

#[gpui::test]
fn home_replacement_follows_sections_items_and_view_all(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    home_reply(&server, &[("one", &["a", "b"], true), ("two", &["c"], true)]);
    let home =
        cx.update(|window, cx| cx.new(|cx| relics::home::Home::new(client, relics::home::Rows::Home, window, cx)));
    cx.run_until_parked();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let _item = cx.update(|_, cx| {
        cx.subscribe(&home, move |_, event: &relics::home::Open, _| {
            seen.lock().unwrap().push(event.0.content_id.clone());
        })
    });
    let queries = Arc::new(Mutex::new(Vec::new()));
    let seen = queries.clone();
    let _all = cx.update(|_, cx| {
        cx.subscribe(&home, move |_, event: &relics::home::OpenGrid, _| {
            seen.lock().unwrap().push(event.title.clone());
        })
    });
    home.update(cx, |home, cx| {
        home.key(Key::Right, cx);
    });
    home_reply(&server, &[("two", &["c"], true), ("one", &["b", "a"], true)]);
    home.update(cx, |home, cx| home.reload(0, cx));
    cx.run_until_parked();
    home.update(cx, |home, cx| {
        home.key(Key::Other("enter"), cx);
        home.key(Key::Right, cx);
        home.key(Key::Right, cx);
    });
    home_reply(&server, &[("one", &["a", "b", "d"], true), ("two", &["c"], true)]);
    home.update(cx, |home, cx| home.reload(0, cx));
    cx.run_until_parked();
    home.update(cx, |home, cx| {
        home.key(Key::Other("enter"), cx);
    });
    assert_eq!(*opened.lock().unwrap(), ["b"]);
    assert_eq!(*queries.lock().unwrap(), ["one"]);
    home_reply(&server, &[]);
    home.update(cx, |home, cx| home.reload(0, cx));
    cx.run_until_parked();
    home.update(cx, |home, cx| {
        home.key(Key::Other("enter"), cx);
    });
    assert_eq!(opened.lock().unwrap().len(), 1);
    assert_eq!(queries.lock().unwrap().len(), 1);
}

#[gpui::test]
fn request_replacement_keeps_the_request_and_empty_results_cannot_cancel(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let mut body: serde_json::Value = serde_json::from_str(&common::fixture("list_my_requests_ok.json")).unwrap();
    body["items"][1]["status"] = "pending".into();
    set_reply(&server, "requests/mine", body.clone());
    let requests = cx.update(|_, cx| cx.new(|cx| relics::requests::Requests::new(client, cx)));
    cx.run_until_parked();
    body["items"].as_array_mut().unwrap().reverse();
    set_reply(&server, "requests/mine", body);
    requests.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    requests.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert_eq!(server.requests("POST", "requests/r-2/cancel").len(), 1);
    set_reply(&server, "requests/mine", serde_json::json!({"items":[], "page":{"has_more":false}}));
    requests.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    requests.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    assert_eq!(server.requests("POST", "requests/").len(), 1);
}

#[gpui::test]
fn collection_replacement_follows_group_and_card_and_uses_the_first_remaining_group(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let mut body: serde_json::Value =
        serde_json::from_str(&common::fixture("get_library_collections_ok.json")).unwrap();
    let mut other = body["groups"][0]["collections"][0].clone();
    other["id"] = "u2".into();
    body["groups"][0]["collections"].as_array_mut().unwrap().push(other);
    set_reply(&server, "library/12/collections", body.clone());
    let collections = cx.update(|_, cx| {
        cx.new(|cx| relics::collections::Collections::new(client, relics::collections::Scope::Library("12".into()), cx))
    });
    cx.run_until_parked();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&collections, move |_, event: &relics::collections::OpenCollection, _| {
            seen.lock().unwrap().push(event.id.clone());
        })
    });
    collections.update(cx, |page, cx| {
        page.key(Key::Right, cx);
    });
    body["groups"][0]["sort_order"] = 10000.into();
    body["groups"][0]["collections"].as_array_mut().unwrap().reverse();
    set_reply(&server, "library/12/collections", body.clone());
    collections.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    collections.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    body["groups"] = serde_json::json!([]);
    set_reply(&server, "library/12/collections", body.clone());
    collections.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    collections.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    body["ungrouped"] = serde_json::Value::Null;
    set_reply(&server, "library/12/collections", body);
    collections.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    collections.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    assert_eq!(*opened.lock().unwrap(), ["u2", "c1"]);
}

#[gpui::test]
fn detail_replacement_keeps_chosen_version_and_keyboard_focus_independent(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let mut body: serde_json::Value =
        serde_json::from_str(&common::fixture("list_catalog_item_versions_ok.json")).unwrap();
    let first = body["items"][0].clone();
    let mut second = first.clone();
    second["file_id"] = "121".into();
    body["items"] = serde_json::json!([first, second]);
    set_reply(&server, "catalog/items/test-item/versions", body.clone());
    let detail =
        cx.update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-item".into(), None, window, cx)));
    cx.run_until_parked();
    let played = Arc::new(Mutex::new(Vec::new()));
    let seen = played.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&detail, move |_, event: &relics::home::Play, _| {
            seen.lock().unwrap().push(event.0.file_id.clone());
        })
    });
    detail.update(cx, |page, cx| {
        for key in [Key::Down, Key::Right, Key::Other("enter"), Key::Left] {
            page.key(key, cx);
        }
    });
    body["items"].as_array_mut().unwrap().reverse();
    set_reply(&server, "catalog/items/test-item/versions", body);
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("p"), cx);
        page.key(Key::Other("enter"), cx);
        page.key(Key::Other("p"), cx);
    });
    assert_eq!(*played.lock().unwrap(), [Some("121".into()), Some("120".into())]);
}

#[gpui::test]
fn detail_replacement_keeps_action_identity_when_play_becomes_resume(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let mut item: serde_json::Value = serde_json::from_str(&common::fixture("get_catalog_item_ok.json")).unwrap();
    item["position_seconds"] = serde_json::Value::Null;
    set_reply(&server, "catalog/items/test-item?", item.clone());
    let detail =
        cx.update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-item".into(), None, window, cx)));
    cx.run_until_parked();
    let played = Arc::new(Mutex::new(Vec::new()));
    let seen = played.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&detail, move |_, event: &relics::home::Play, _| {
            seen.lock().unwrap().push(event.0.start_over);
        })
    });
    item["position_seconds"] = 20.into();
    set_reply(&server, "catalog/items/test-item?", item.clone());
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
        for _ in 0..3 {
            page.key(Key::Right, cx);
        }
    });
    item["position_seconds"] = serde_json::Value::Null;
    set_reply(&server, "catalog/items/test-item?", item);
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert_eq!(*played.lock().unwrap(), [false]);
    assert_eq!(
        server.requests("PUT", "favorites/test-item").len() + server.requests("DELETE", "favorites/test-item").len(),
        1
    );
}

#[gpui::test]
fn detail_replacement_keeps_chosen_season_focus_and_menu_together(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let mut seasons: serde_json::Value = serde_json::from_str(&common::fixture("list_series_seasons_ok.json")).unwrap();
    let mut second = seasons["items"][0].clone();
    second["season_number"] = 2.into();
    second["content_id"] = "season:second".into();
    seasons["items"].as_array_mut().unwrap().push(second);
    set_reply(&server, "catalog/series/test-series/seasons?", seasons.clone());
    let episodes: serde_json::Value = serde_json::from_str(&common::fixture("list_season_episodes_ok.json")).unwrap();
    set_reply(&server, "catalog/series/test-series/seasons/2/episodes", episodes);
    let detail = cx.update(|window, cx| {
        cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), Some(2), window, cx))
    });
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        for key in [Key::Down, Key::Left, Key::Other("o")] {
            page.key(key, cx);
        }
    });
    seasons["items"].as_array_mut().unwrap().reverse();
    set_reply(&server, "catalog/series/test-series/seasons?", seasons.clone());
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    assert_eq!(server.count("catalog/series/test-series/seasons/1/episodes"), 0);
    assert_eq!(server.count("catalog/series/test-series/seasons/2/episodes"), 2);
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert_eq!(server.requests("POST", "watched/series:severance-S01").len(), 1);
    detail.update(cx, |page, cx| {
        page.key(Key::Other("o"), cx);
    });
    seasons["items"].as_array_mut().unwrap().truncate(1);
    set_reply(&server, "catalog/series/test-series/seasons?", seasons);
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert_eq!(server.requests("POST", "watched/").len(), 1, "the removed season closes its menu");
}

#[gpui::test]
fn episode_replacement_keeps_menu_identity_and_empty_rows_return_to_play(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let path = "catalog/series/test-series/seasons/1/episodes";
    let mut episodes: serde_json::Value =
        serde_json::from_str(&common::fixture("list_season_episodes_ok.json")).unwrap();
    let detail = cx
        .update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), None, window, cx)));
    cx.run_until_parked();
    let played = Arc::new(Mutex::new(0));
    let seen = played.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&detail, move |_, _: &relics::home::Play, _| {
            *seen.lock().unwrap() += 1;
        })
    });
    detail.update(cx, |page, cx| {
        for key in [Key::Down, Key::Down, Key::Down, Key::Other("o")] {
            page.key(key, cx);
        }
    });
    episodes["items"].as_array_mut().unwrap().reverse();
    set_reply(&server, path, episodes.clone());
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert_eq!(
        server.requests("POST", "watched/episode:severance-s01e01").len()
            + server.requests("DELETE", "watched/episode:severance-s01e01").len(),
        1
    );
    detail.update(cx, |page, cx| {
        page.key(Key::Other("o"), cx);
    });
    episodes["items"] = serde_json::json!([]);
    set_reply(&server, path, episodes);
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    assert_eq!(*played.lock().unwrap(), 1, "an empty episode row returns to the hero Play action");
}

#[gpui::test]
fn sidebar_replacement_keeps_libraries_and_fixed_entries_and_returns_removed_libraries_home(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    let mut libraries: serde_json::Value = serde_json::from_str(&common::fixture("user_libraries.json")).unwrap();
    let mut other = libraries["items"][0].clone();
    other["id"] = "13".into();
    other["name"] = "Other".into();
    other["sort_order"] = 1.into();
    cx.simulate_keystrokes("left down");
    libraries["items"].as_array_mut().unwrap().insert(0, other);
    set_reply(&server, "user/libraries", libraries);
    send(cx, &changes, LiveChange::Resync);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(server.count("catalog/filters?library_id=12"), 1, "the focused Library follows its id");
    assert_eq!(server.count("catalog/filters?library_id=13"), 0);
    set_reply(&server, "user/libraries", serde_json::json!({"items":[]}));
    cx.executor().advance_clock(Duration::from_secs(6));
    send(cx, &changes, LiveChange::Resync);
    cx.simulate_keystrokes("left down enter");
    cx.run_until_parked();
    assert!(drawn(cx, "search-field"), "the removed open Library returns to Home, whose next sidebar entry is Search");
    cx.simulate_keystrokes("1 left down");
    set_reply(&server, "user/libraries", serde_json::from_str(&common::fixture("user_libraries.json")).unwrap());
    cx.executor().advance_clock(Duration::from_secs(6));
    send(cx, &changes, LiveChange::Resync);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(drawn(cx, "search-field"), "Search retains its meaning when a Library appears above it");
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> gpui::Bounds<gpui::Pixels> {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}"))
}

fn wheel(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>, x: f32, y: f32) {
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(x), gpui::px(y))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
}

#[gpui::test]
fn home_replacement_preserves_sideways_scroll_and_the_top_section_offset(cx: &mut TestAppContext) {
    use relics::pages::freshness::Reload;
    let (pages, cx, server, _, client) = rig(cx);
    let ids: Vec<_> = (0..14).map(|n| format!("item-{n}")).collect();
    let ids: Vec<_> = ids.iter().map(String::as_str).collect();
    home_reply(&server, &[("one", &ids, false), ("two", &ids, false), ("three", &ids, false)]);
    let home =
        cx.update(|window, cx| cx.new(|cx| relics::home::Home::new(client, relics::home::Rows::Home, window, cx)));
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Home(home.clone()), cx));
    cx.run_until_parked();
    let before = bounds(cx, "home-card-one-2");
    wheel(cx, before.center(), -200., 0.);
    let sideways = bounds(cx, "home-card-one-2");
    assert!(sideways.origin.x < before.origin.x, "the first row scrolled sideways");
    wheel(cx, sideways.center(), 0., -400.);
    let before = bounds(cx, "home-card-two-2");
    wheel(cx, before.center(), -150., 0.);
    let anchor = bounds(cx, "home-card-two-2");
    home_reply(&server, &[("new", &ids, false), ("one", &ids, false), ("two", &ids, false), ("three", &ids, false)]);
    home.update(cx, |home, cx| home.reload(0, cx));
    cx.run_until_parked();
    assert_eq!(
        bounds(cx, "home-card-two-2").origin,
        anchor.origin,
        "the top section keeps its vertical offset and sideways scroll"
    );
    wheel(cx, anchor.center(), 0., 400.);
    assert_eq!(
        bounds(cx, "home-card-one-2").origin.x,
        sideways.origin.x,
        "the other row retains its own sideways scroll"
    );
}

#[gpui::test]
fn collection_replacement_preserves_each_groups_sideways_scroll(cx: &mut TestAppContext) {
    use relics::pages::freshness::Reload;
    let (pages, cx, server, _, client) = rig(cx);
    let mut body: serde_json::Value =
        serde_json::from_str(&common::fixture("get_library_collections_ok.json")).unwrap();
    let card = body["groups"][0]["collections"][0].clone();
    let cards: Vec<_> = (0..14)
        .map(|n| {
            let mut card = card.clone();
            card["id"] = format!("c{n}").into();
            card
        })
        .collect();
    body["groups"][0]["collections"] = serde_json::json!(cards);
    body["ungrouped"]["collections"] = serde_json::json!(cards);
    set_reply(&server, "library/12/collections", body.clone());
    let collections = cx.update(|_, cx| {
        cx.new(|cx| relics::collections::Collections::new(client, relics::collections::Scope::Library("12".into()), cx))
    });
    pages.update(cx, |pages, cx| pages.open(relics::pages::Page::Collections(collections.clone()), cx));
    cx.run_until_parked();
    let before = bounds(cx, "collection-card-1-Mine-c2");
    wheel(cx, before.center(), -200., 0.);
    let first = bounds(cx, "collection-card-1-Mine-c2");
    assert!(first.origin.x < before.origin.x);
    let before = bounds(cx, "collection-card-1-Collections-c2");
    wheel(cx, before.center(), -100., 0.);
    let second = bounds(cx, "collection-card-1-Collections-c2");
    assert!(second.origin.x < before.origin.x);
    body["groups"][0]["sort_order"] = 10000.into();
    set_reply(&server, "library/12/collections", body);
    collections.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    assert_eq!(bounds(cx, "collection-card-1-Mine-c2").origin.x, first.origin.x);
    assert_eq!(bounds(cx, "collection-card-1-Collections-c2").origin.x, second.origin.x);
}

#[gpui::test]
fn home_replacement_keeps_missing_targets_positions_then_uses_the_first_card(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    home_reply(&server, &[("one", &["a", "b", "c"], false), ("two", &["x", "y", "z"], false)]);
    let home =
        cx.update(|window, cx| cx.new(|cx| relics::home::Home::new(client, relics::home::Rows::Home, window, cx)));
    cx.run_until_parked();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&home, move |_, event: &relics::home::Open, _| {
            seen.lock().unwrap().push(event.0.content_id.clone());
        })
    });
    home.update(cx, |page, cx| {
        page.key(Key::Down, cx);
        page.key(Key::Right, cx);
    });
    for rows in [
        vec![("one", &["a", "b", "c"][..], false), ("two", &["x", "z"][..], false)],
        vec![("one", &["a", "b", "c"][..], false), ("three", &["p", "q"][..], false)],
        vec![("one", &["a"][..], false), ("three", &["p"][..], false)],
    ] {
        home_reply(&server, &rows);
        home.update(cx, |page, cx| page.reload(0, cx));
        cx.run_until_parked();
        home.update(cx, |page, cx| {
            page.key(Key::Other("enter"), cx);
        });
    }
    assert_eq!(*opened.lock().unwrap(), ["z", "q", "a"]);
}

#[gpui::test]
fn collection_replacement_clamps_a_removed_card_within_its_group(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let mut body: serde_json::Value =
        serde_json::from_str(&common::fixture("get_library_collections_ok.json")).unwrap();
    let mut second = body["groups"][0]["collections"][0].clone();
    second["id"] = "u2".into();
    body["groups"][0]["collections"].as_array_mut().unwrap().push(second);
    set_reply(&server, "library/12/collections", body.clone());
    let page = cx.update(|_, cx| {
        cx.new(|cx| relics::collections::Collections::new(client, relics::collections::Scope::Library("12".into()), cx))
    });
    cx.run_until_parked();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&page, move |_, event: &relics::collections::OpenCollection, _| {
            seen.lock().unwrap().push(event.id.clone());
        })
    });
    page.update(cx, |page, cx| {
        page.key(Key::Right, cx);
    });
    body["groups"][0]["collections"].as_array_mut().unwrap().truncate(1);
    set_reply(&server, "library/12/collections", body);
    page.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    page.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    assert_eq!(*opened.lock().unwrap(), ["u1"]);
}

#[gpui::test]
fn episode_replacement_keeps_the_play_target_and_clamps_a_shorter_row(cx: &mut TestAppContext) {
    use relics::keys::Key;
    use relics::pages::freshness::Reload;
    let (_, cx, server, _, client) = rig(cx);
    let path = "catalog/series/test-series/seasons/1/episodes";
    let mut episodes: serde_json::Value =
        serde_json::from_str(&common::fixture("list_season_episodes_ok.json")).unwrap();
    let mut third = episodes["items"][0].clone();
    third["content_id"] = "episode:third".into();
    episodes["items"].as_array_mut().unwrap().push(third);
    set_reply(&server, path, episodes.clone());
    let player = cx.update(|_, cx| cx.new(|cx| relics::player::Controller::new(None, cx)));
    let playback_client = client.clone();
    let detail = cx
        .update(|window, cx| cx.new(|cx| relics::detail::Detail::new(client, "test-series".into(), None, window, cx)));
    cx.run_until_parked();
    // Episode version requests get a local 404, so the controller cannot reach mpv.
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&detail, move |_, event: &relics::home::Play, cx| {
            player.update(cx, |player, cx| player.play(playback_client.clone(), event.0.clone(), cx));
        })
    });
    detail.update(cx, |page, cx| {
        for key in [Key::Down, Key::Down, Key::Down, Key::Down] {
            page.key(key, cx);
        }
    });
    episodes["items"].as_array_mut().unwrap().swap(0, 1);
    set_reply(&server, path, episodes.clone());
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert!(server.count("catalog/items/episode:severance-s01e02/versions") > 0);
    detail.update(cx, |page, cx| {
        page.key(Key::Down, cx);
        page.key(Key::Down, cx);
    });
    episodes["items"] = serde_json::json!([episodes["items"][1]]);
    set_reply(&server, path, episodes);
    detail.update(cx, |page, cx| page.reload(0, cx));
    cx.run_until_parked();
    detail.update(cx, |page, cx| {
        page.key(Key::Other("enter"), cx);
    });
    cx.run_until_parked();
    assert!(server.count("catalog/items/episode:severance-s01e01/versions") > 0);
    assert!(server.requests("POST", "playback/start").is_empty(), "the local 404 stops playback before mpv");
}

#[gpui::test]
fn an_open_choice_list_closes_when_its_row_goes_away(cx: &mut TestAppContext) {
    let (_, cx, server, changes, _) = rig(cx);
    open_row(cx, 7);
    // Row 0 is Skip intros. Enter opens its Choice list, with the focus on its second choice, "Ask to skip".
    cx.simulate_keystrokes("enter");
    // The server goes back to revision 6, which does not know Skip intros, and the page reloads.
    let mut capabilities: serde_json::Value =
        serde_json::from_str(&common::fixture("get_settings_contract_capabilities_ok.json")).unwrap();
    capabilities["manifest_revision"] = 6.into();
    set_reply(&server, "settings/contract/capabilities", capabilities);
    send(cx, &changes, LiveChange::Resync);
    // Auto-skip recaps (off) now has the focus. With the list closed, Enter toggles it on. With the old list
    // open, Enter picks its second choice and writes off.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let writes = server.requests("PUT", "settings/values");
    let bodies: Vec<_> = writes.iter().map(|req| (req.path.as_str(), req.body.as_str())).collect();
    assert_eq!(bodies, [("settings/values/playback.auto_skip_recap?scope=profile", r#"{"value":true}"#)]);
}
