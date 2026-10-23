//! The playback controller through its public actions, with a fake media player and a fake Silo server. The
//! tests drive the real coordination: Item replacement, Up Next, Shuffle, recovery and the stop reports. They
//! need no desktop, no Wayland surface, no D-Bus, no libmpv and no real Server.

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Log, Reply, Seen, json, not_found, ok};
use futures::AsyncReadExt as _;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use gpui::{AppContext as _, Entity, TestAppContext};
use http_client::FakeHttpClient;
use relics::api::{Client, wire};
use relics::playback::Input;
use relics::playback::playing::PlayItem;
use relics::playback::tracks::{self, Kind};
use relics::player::{Controller, Handle, Media, PlaybackError, Stopped, ViewClosed};
use relics::settings::SubtitleAppearance;
use serde_json::{Value, json};

/// One operation that the controller asked of the fake media.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    Open,
    Close,
    Load { url: String, start: f64 },
    AddSubtitle(String),
    SelectSubtitle(Option<i64>),
    SelectAudio(i64),
    Seek(f64),
    Pause(bool),
    AddVolume(f64),
    ToggleMute,
    AddSubDelay(f64),
    SetSubDelay(f64),
}

#[derive(Default)]
struct FakeState {
    ops: Vec<Op>,
    /// The event sender of each handle that opened, also of the closed ones.
    handles: Vec<UnboundedSender<Input>>,
    /// The next open fails with this text.
    fail_open: Option<String>,
    fail_load: bool,
    sub_delay: f64,
    /// Each Subtitle appearance that the controller applied, with the number of loads before it.
    appearances: Vec<(usize, SubtitleAppearance)>,
}

/// The fake media. It records each operation, sends the events that the test gives it, and fails when the test
/// asks it to. It knows nothing of the Playback session.
#[derive(Clone, Default)]
struct Fake(Rc<RefCell<FakeState>>);

impl Fake {
    fn ops(&self) -> Vec<Op> {
        self.0.borrow().ops.clone()
    }

    fn count(&self, want: impl Fn(&Op) -> bool) -> usize {
        self.0.borrow().ops.iter().filter(|op| want(op)).count()
    }

    fn loads(&self) -> Vec<String> {
        let ops = self.ops();
        ops.into_iter().filter_map(|op| if let Op::Load { url, .. } = op { Some(url) } else { None }).collect()
    }

    /// Sends `inputs` from the handle that opened last.
    fn send(&self, inputs: impl IntoIterator<Item = Input>) {
        let index = self.0.borrow().handles.len() - 1;
        self.send_from(index, inputs);
    }

    /// Sends `inputs` from the handle that opened as number `index`, from 0.
    fn send_from(&self, index: usize, inputs: impl IntoIterator<Item = Input>) {
        for input in inputs {
            self.0.borrow().handles[index].unbounded_send(input).ok();
        }
    }
}

impl Media for Fake {
    fn open(&self) -> Result<(Box<dyn Handle>, UnboundedReceiver<Input>), String> {
        let mut state = self.0.borrow_mut();
        if let Some(text) = state.fail_open.take() {
            return Err(text);
        }
        let (tx, rx) = mpsc::unbounded();
        state.ops.push(Op::Open);
        state.handles.push(tx);
        state.sub_delay = 0.0;
        Ok((Box::new(FakeHandle(self.clone())), rx))
    }

    fn media_keys(&self) -> bool {
        false
    }
}

struct FakeHandle(Fake);

impl FakeHandle {
    fn record(&self, op: Op) -> anyhow::Result<()> {
        (self.0).0.borrow_mut().ops.push(op);
        Ok(())
    }
}

impl Drop for FakeHandle {
    fn drop(&mut self) {
        (self.0).0.borrow_mut().ops.push(Op::Close);
    }
}

impl Handle for FakeHandle {
    fn load(&self, url: &str, _: &[(String, String)], start: f64, _: Option<i64>) -> anyhow::Result<()> {
        self.record(Op::Load { url: url.into(), start })?;
        if (self.0).0.borrow().fail_load {
            anyhow::bail!("the fake cannot load {url}");
        }
        Ok(())
    }

    fn add_subtitle(&self, url: &str, _: &str, _: &str) -> anyhow::Result<()> {
        self.record(Op::AddSubtitle(url.into()))
    }

    fn select_subtitle(&self, sid: Option<i64>) -> anyhow::Result<()> {
        self.record(Op::SelectSubtitle(sid))
    }

    fn select_audio(&self, aid: i64) -> anyhow::Result<()> {
        self.record(Op::SelectAudio(aid))
    }

    fn seek(&self, position: f64) -> anyhow::Result<()> {
        self.record(Op::Seek(position))
    }

    fn set_pause(&self, paused: bool) -> anyhow::Result<()> {
        self.record(Op::Pause(paused))
    }

    fn add_volume(&self, delta: f64) -> anyhow::Result<()> {
        self.record(Op::AddVolume(delta))
    }

    fn toggle_mute(&self) -> anyhow::Result<()> {
        self.record(Op::ToggleMute)
    }

    fn volume(&self) -> anyhow::Result<(f64, bool)> {
        Ok((100.0, false))
    }

    fn sub_delay(&self) -> anyhow::Result<f64> {
        Ok((self.0).0.borrow().sub_delay)
    }

    fn add_sub_delay(&self, delta: f64) -> anyhow::Result<()> {
        (self.0).0.borrow_mut().sub_delay += delta;
        self.record(Op::AddSubDelay(delta))
    }

    fn set_sub_delay(&self, seconds: f64) -> anyhow::Result<()> {
        (self.0).0.borrow_mut().sub_delay = seconds;
        self.record(Op::SetSubDelay(seconds))
    }

    fn set_subtitle_appearance(&self, appearance: &SubtitleAppearance) -> anyhow::Result<()> {
        let loads = self.0.loads().len();
        (self.0).0.borrow_mut().appearances.push((loads, appearance.clone()));
        Ok(())
    }
}

/// The fake Silo server. A request whose path starts with a prefix in `fail` gets no answer (a network failure).
/// A request with a prefix in `hold` gets its answer only at `release`.
#[derive(Clone, Default)]
struct Server {
    seen: Log,
    fail: Arc<Mutex<Vec<String>>>,
    hold: Arc<Mutex<Vec<String>>>,
    held: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
    /// The number of start requests, so that each Playback session gets its own session id.
    starts: Arc<Mutex<usize>>,
    /// The number of shuffle advances.
    advances: Arc<Mutex<usize>>,
    /// The body of each Item detail. `None` gives the server fixture, which has no `effective_subtitle_*` fields.
    detail: Arc<Mutex<Option<Value>>>,
    /// The stored Subtitle appearance that the effective read gives.
    appearance: Arc<Mutex<Value>>,
    /// The other items of the effective read.
    settings: Arc<Mutex<Vec<Value>>>,
}

impl Server {
    fn requests(&self, method: &str, prefix: &str) -> Vec<Seen> {
        self.seen.lock().unwrap().iter().filter(|r| r.method == method && r.path.starts_with(prefix)).cloned().collect()
    }

    fn count(&self, method: &str, prefix: &str) -> usize {
        self.requests(method, prefix).len()
    }

    fn fail(&self, prefixes: &[&str]) {
        *self.fail.lock().unwrap() = prefixes.iter().map(|p| p.to_string()).collect();
    }

    /// Each Item detail is the fixture with the `effective_subtitle_*` fields, with `fields` over it.
    fn detail(&self, fields: Value) {
        let mut body: Value =
            serde_json::from_str(&common::fixture("../own/get_catalog_item_effective_subtitles.json")).unwrap();
        body.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        *self.detail.lock().unwrap() = Some(body);
    }

    fn hold(&self, prefixes: &[&str]) {
        *self.hold.lock().unwrap() = prefixes.iter().map(|p| p.to_string()).collect();
    }

    /// Lets the request that the server held first go on. The other held requests still wait.
    fn release_first(&self) {
        let mut held = self.held.lock().unwrap();
        if !held.is_empty() {
            held.remove(0).send(()).ok();
        }
    }

    /// Stops holding, and lets the held requests go on.
    fn release(&self) {
        self.hold(&[]);
        for held in self.held.lock().unwrap().drain(..) {
            held.send(()).ok();
        }
    }

    /// The content ids of the Items that started, in order. Each start reads the version list twice: for the start
    /// request, and for the Quality list.
    fn started(&self) -> Vec<String> {
        let versions = self.requests("GET", "catalog/items/");
        let mut ids: Vec<String> =
            versions.iter().filter_map(|r| r.path.strip_suffix("/versions")).map(|p| p[14..].to_string()).collect();
        ids.dedup();
        ids
    }

    fn reply(&self, req: &Seen) -> Reply {
        if self.fail.lock().unwrap().iter().any(|p| req.path.starts_with(p.as_str())) {
            return None;
        }
        let path = req.path.split('?').next().unwrap();
        match (req.method.as_str(), path) {
            ("GET", p) if p.ends_with("/versions") => json(200, two_versions()),
            ("GET", p) if p.starts_with("catalog/items/") => match self.detail.lock().unwrap().clone() {
                Some(body) => json(200, body.to_string()),
                None => ok("get_catalog_item_ok.json"),
            },
            ("GET", "playback/capabilities") => ok("playback_capability_available.json"),
            ("GET", "settings/contract/capabilities") => ok("get_settings_contract_capabilities_ok.json"),
            ("GET", "settings/values/effective") => {
                let item = json!({
                    "key": "playback.subtitle_appearance", "source": "profile_device", "definition_revision": 1,
                    "value": self.appearance.lock().unwrap().clone(),
                });
                let mut items = self.settings.lock().unwrap().clone();
                items.push(item);
                json(200, json!({ "revision": 17, "items": items }).to_string())
            }
            ("POST", "playback/start") => {
                let mut starts = self.starts.lock().unwrap();
                *starts += 1;
                json(200, common::fixture("playback_start_opaque_ids.json").replace(SESSION, &session(*starts)))
            }
            ("GET", "markers/files/120") => json(200, markers()),
            ("GET", "catalog/series/series:severance/seasons/1/episodes") => ok("list_catalog_item_episodes_ok.json"),
            ("POST", "auth/refresh") => json(
                200,
                json!({ "access_token": common::jwt(1_900_000_000), "refresh_token": "ref2", "expires_in": 28800 })
                    .to_string(),
            ),
            ("POST", p) if p.ends_with("/replan") => ok("../own/playback_replan_failure_recovery.json"),
            ("POST", p) if p.ends_with("/progress") => ok("playback_progress_applied.json"),
            ("POST", "shuffles/s1/advance") => {
                let mut advances = self.advances.lock().unwrap();
                *advances += 1;
                json(200, shuffle(*advances).to_string())
            }
            ("DELETE", _) => ok("playback_stop_completed.json"),
            _ => not_found(),
        }
    }
}

/// The session id of the fixture start reply.
const SESSION: &str = "11111111-1111-4111-8111-111111111111";

/// The session id of start request `n`, from 1.
fn session(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// The stream URL of the session of start request `n`.
fn stream(n: usize) -> String {
    format!("https://silo.test/api/v2/stream/{}", session(n))
}

/// The versions fixture with a second file, 121.
fn two_versions() -> String {
    let mut body: Value = serde_json::from_str(&common::fixture("list_catalog_item_versions_ok.json")).unwrap();
    let mut second = body["items"][0].clone();
    second["file_id"] = json!("121");
    body["items"].as_array_mut().unwrap().push(second);
    body.to_string()
}

/// The Markers of file 120: an intro from 100 to 140 seconds.
fn markers() -> String {
    let mut body: Value = serde_json::from_str(&common::fixture("get_file_markers_ok.json")).unwrap();
    body["marker_segments"] = json!([{"kind": "intro", "start_seconds": 100.0, "end_seconds": 140.0}]);
    body.to_string()
}

/// The movies of the catalog fixture.
fn movies() -> Vec<Value> {
    let page: Value = serde_json::from_str(&common::fixture("query_catalog_items_ok.json")).unwrap();
    page["items"].as_array().unwrap().clone()
}

/// The shuffle `s1` of the movies after `advances` advances: each advance moves one movie on.
fn shuffle(advances: usize) -> Value {
    let movies = movies();
    json!({
        "id": "s1", "created_at": "2026-01-01T00:00:00.000Z", "updated_at": "2026-01-01T00:00:00.000Z",
        "scope": {"kind": "library", "id": "1", "title": "Movies"},
        "current": movies[advances % 3], "next": movies[(advances + 1) % 3],
    })
}

fn movie() -> PlayItem {
    PlayItem::from(&serde_json::from_value::<wire::CatalogItem>(movies()[0].clone()).unwrap())
}

/// Episode `n` (1 or 2) of season 1 of the fixture series.
fn episode(n: usize) -> PlayItem {
    let page: Value = serde_json::from_str(&common::fixture("list_catalog_item_episodes_ok.json")).unwrap();
    let episode: wire::Episode = serde_json::from_value(page["items"][n - 1].clone()).unwrap();
    PlayItem::episode("series:severance".into(), episode)
}

/// The tracks that the fake media reports: English and German audio, English subtitles. English plays.
fn file_tracks() -> Input {
    Input::Tracks(tracks::parse(
        r#"[{"id":1,"type":"audio","lang":"eng","selected":true},{"id":2,"type":"audio","lang":"ger"},
            {"id":1,"type":"sub","lang":"eng"}]"#,
    ))
}

struct Rig {
    player: Entity<Controller>,
    fake: Fake,
    server: Server,
    client: Arc<Client>,
    /// The number of `Stopped` events.
    stopped: Rc<Cell<usize>>,
    errors: Rc<RefCell<Vec<String>>>,
    view_closed: Rc<Cell<usize>>,
}

fn rig(cx: &mut TestAppContext) -> Rig {
    let server = Server::default();
    let fake_server = server.clone();
    let http = FakeHttpClient::create(move |req| {
        let server = fake_server.clone();
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
    let fake = Fake::default();
    let media = fake.clone();
    let player = cx.new(|cx| Controller::new(Some(Box::new(media)), cx));
    let (stopped, errors, view_closed) =
        (Rc::new(Cell::new(0)), Rc::new(RefCell::new(Vec::new())), Rc::new(Cell::new(0)));
    cx.update(|cx| {
        let count = stopped.clone();
        cx.subscribe(&player, move |_, _: &Stopped, _| count.set(count.get() + 1)).detach();
        let texts = errors.clone();
        cx.subscribe(&player, move |_, PlaybackError(text), _| texts.borrow_mut().push(text.clone())).detach();
        let count = view_closed.clone();
        cx.subscribe(&player, move |_, _: &ViewClosed, _| count.set(count.get() + 1)).detach();
    });
    Rig { player, fake, server, client, stopped, errors, view_closed }
}

impl Rig {
    fn play(&self, item: PlayItem, cx: &mut TestAppContext) {
        let client = self.client.clone();
        self.player.update(cx, |player, cx| player.play(client, item, cx));
        cx.run_until_parked();
    }

    fn step(&self, input: Input, cx: &mut TestAppContext) {
        self.player.update(cx, |player, cx| player.step(input, cx));
        cx.run_until_parked();
    }

    /// Sends `inputs` from the open handle of the fake media.
    fn events(&self, inputs: impl IntoIterator<Item = Input>, cx: &mut TestAppContext) {
        self.fake.send(inputs);
        cx.run_until_parked();
    }

    /// The file of the session loads, with this duration, and plays at `position`.
    fn loaded(&self, duration: f64, position: f64, cx: &mut TestAppContext) {
        self.events([Input::FileLoaded, Input::Duration(duration), Input::Position(position)], cx);
    }

    fn seconds(&self, seconds: u64, cx: &mut TestAppContext) {
        for _ in 0..seconds {
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
        }
    }

    fn read<T>(&self, cx: &mut TestAppContext, read: impl FnOnce(&Controller) -> T) -> T {
        self.player.read_with(cx, |player, _| read(player))
    }

    fn stops(&self) -> Vec<String> {
        self.server.requests("DELETE", "playback/").into_iter().map(|r| r.path).collect()
    }
}

#[gpui::test]
fn a_new_item_shows_only_its_own_state_and_plays_on_the_same_handle(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(episode(1), cx);
    assert_eq!(rig.fake.ops(), [Op::Open, Op::Load { url: stream(1), start: 12.5 }]);
    rig.loaded(3420.0, 105.0, cx);
    let shown =
        rig.read(cx, |p| (p.position(), p.prompt().to_string(), p.markers().len(), p.next_title().map(String::from)));
    assert_eq!(shown, (Some(105.0), "Skip intro".into(), 1, Some("S1:E2 · Good News About Hell".into())));
    assert!(rig.read(cx, Controller::can_go_next));

    let client = rig.client.clone();
    rig.player.update(cx, |player, cx| player.play(client, movie(), cx));
    let shown = rig.read(cx, |p| (p.position(), p.prompt().to_string(), p.markers().len(), p.next_title().is_some()));
    assert_eq!(shown, (None, String::new(), 0, false), "nothing of the episode shows");
    assert!(!rig.read(cx, Controller::can_go_next));
    cx.run_until_parked();
    assert_eq!(rig.stops(), vec![format!("playback/{}", session(1))], "the episode ended with its stop report");
    assert_eq!(rig.server.started(), ["episode:severance-s01e01", "movie:heat-1995"]);
    assert_eq!(rig.fake.count(|op| *op == Op::Open), 1, "the movie plays on the open handle");
    assert_eq!(rig.fake.loads(), [stream(1), stream(2)]);
    assert_eq!(rig.fake.count(|op| *op == Op::Close), 0);
    assert!(rig.fake.ops().contains(&Op::SetSubDelay(0.0)), "the Subtitle delay of the series does not carry");
    assert_eq!(rig.view_closed.get(), 0, "the Player view stays");
}

#[gpui::test]
fn next_and_previous_play_the_episodes_that_the_lookup_found(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 200.0, cx);
    rig.step(Input::Previous, cx);
    assert_eq!(rig.server.started(), ["episode:severance-s01e01"], "the first episode has no previous episode");
    rig.step(Input::Next, cx);
    rig.loaded(3420.0, 200.0, cx);
    assert_eq!(rig.read(cx, |p| p.next_title().map(String::from)), None, "the last episode has no next episode");
    rig.step(Input::Previous, cx);
    assert_eq!(
        rig.server.started(),
        ["episode:severance-s01e01", "episode:severance-s01e02", "episode:severance-s01e01"]
    );
    assert_eq!(rig.stops().len(), 2);
}

#[gpui::test]
fn up_next_counts_down_and_plays_the_next_episode_with_the_track_choice_and_subtitle_delay(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 200.0, cx);
    rig.events([file_tracks()], cx);
    rig.step(Input::PickTrack { kind: Kind::Audio, id: Some(2) }, cx);
    rig.player.update(cx, |player, cx| player.add_sub_delay(0.5, cx));
    rig.events([Input::Position(3411.0)], cx);
    assert_eq!(rig.read(cx, Controller::up_next), Some(10));
    rig.seconds(3, cx);
    assert_eq!(rig.read(cx, Controller::up_next), Some(7));
    rig.seconds(6, cx);
    assert_eq!(rig.server.started(), ["episode:severance-s01e01"], "the countdown still runs");
    rig.seconds(1, cx);
    assert_eq!(rig.server.started(), ["episode:severance-s01e01", "episode:severance-s01e02"]);
    assert_eq!(rig.read(cx, Controller::up_next), None);
    assert_eq!(rig.fake.count(|op| matches!(op, Op::SetSubDelay(_))), 0);
    assert_eq!(rig.read(cx, Controller::sub_delay), Some(0.5), "the Subtitle delay carries in the series");
    rig.loaded(3420.0, 0.0, cx);
    rig.events([file_tracks()], cx);
    assert_eq!(rig.fake.ops().last(), Some(&Op::SelectAudio(2)), "the Track picker choice carries");
}

#[gpui::test]
fn play_now_plays_the_next_episode_at_once_and_cancel_closes_the_view_at_the_end(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 3411.0, cx);
    rig.step(Input::Action, cx);
    assert_eq!(rig.server.started(), ["episode:severance-s01e01", "episode:severance-s01e02"]);

    let rig = self::rig(cx);
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 3411.0, cx);
    rig.step(Input::CancelUpNext, cx);
    assert_eq!(rig.read(cx, Controller::up_next), None);
    rig.seconds(15, cx);
    assert_eq!(rig.server.started(), ["episode:severance-s01e01"], "the next episode does not play by itself");
    rig.events([Input::Position(3419.0), Input::EndOfFile], cx);
    assert_eq!(rig.fake.ops().last(), Some(&Op::Close));
    assert_eq!(rig.view_closed.get(), 1);
    assert_eq!(rig.stops().len(), 1);
}

#[gpui::test]
fn a_shuffle_advances_through_the_server_picks_until_still_watching_asks(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(PlayItem::shuffled(&serde_json::from_value(shuffle(0)).unwrap()), cx);
    assert!(rig.read(cx, Controller::in_shuffle));
    let titles: Vec<String> = movies().iter().map(|m| m["title"].as_str().unwrap().to_string()).collect();
    for played in 1..=3 {
        assert_eq!(rig.read(cx, |p| p.next_title().map(String::from)), Some(titles[played % 3].clone()));
        rig.loaded(1000.0, 995.0, cx);
        rig.seconds(10, cx);
        assert_eq!(rig.server.count("POST", "shuffles/s1/advance"), played);
    }
    let ids: Vec<String> = movies().iter().map(|m| m["content_id"].as_str().unwrap().to_string()).collect();
    assert_eq!(rig.server.started(), [ids[0].clone(), ids[1].clone(), ids[2].clone(), ids[0].clone()]);
    let advance = &rig.server.requests("POST", "shuffles/s1/advance")[2];
    assert!(advance.body.contains(&ids[2]), "the shuffle moves past the pick that ended");

    rig.loaded(1000.0, 995.0, cx);
    rig.seconds(10, cx);
    assert!(rig.read(cx, Controller::still_watching), "the fourth pick asks first");
    assert_eq!(rig.fake.ops().last(), Some(&Op::Pause(true)));
    assert_eq!(rig.server.count("POST", "shuffles/s1/advance"), 3);
    rig.step(Input::Action, cx);
    assert!(!rig.read(cx, Controller::still_watching));
    assert_eq!(rig.server.count("POST", "shuffles/s1/advance"), 4, "Continue plays the next pick");
    assert!(rig.fake.ops().contains(&Op::Pause(false)));
}

#[gpui::test]
fn a_version_switch_plays_the_other_file_at_the_position_with_the_track_choice_and_delay(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    rig.events([file_tracks()], cx);
    rig.step(Input::PickTrack { kind: Kind::Audio, id: Some(2) }, cx);
    rig.player.update(cx, |player, cx| player.add_sub_delay(-0.25, cx));
    rig.step(Input::SwitchVersion("121".into()), cx);
    let start: Value = serde_json::from_str(&rig.server.requests("POST", "playback/start")[1].body).unwrap();
    assert_eq!((start["file_id"].as_str(), start["start_position"].as_f64()), (Some("121"), Some(300.0)));
    assert_eq!(rig.fake.count(|op| *op == Op::Open), 1);
    assert_eq!(rig.read(cx, Controller::sub_delay), Some(-0.25));
    rig.loaded(7200.0, 300.0, cx);
    rig.events([file_tracks()], cx);
    assert_eq!(rig.fake.ops().last(), Some(&Op::SelectAudio(2)));
}

#[gpui::test]
fn media_that_cannot_open_or_load_ends_or_loses_the_session(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.fake.0.borrow_mut().fail_open = Some("no GPU".into());
    rig.play(movie(), cx);
    assert_eq!(*rig.errors.borrow(), ["The player cannot start: no GPU"]);
    assert_eq!(rig.stops().len(), 1);
    assert_eq!(rig.view_closed.get(), 1);
    assert!(!rig.read(cx, Controller::is_open));
    assert_eq!(rig.stopped.get(), 1);

    let rig = self::rig(cx);
    rig.fake.0.borrow_mut().fail_load = true;
    rig.play(movie(), cx);
    assert_eq!(rig.fake.loads().len(), 3, "the load, the reload and the load of the new plan");
    assert_eq!(rig.read(cx, |p| p.prompt().to_string()), "Lost connection to the server");
    assert!(rig.errors.borrow().is_empty());
}

#[gpui::test]
fn a_failed_load_refreshes_and_reloads_then_replans_and_back_cleans_up(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(movie(), cx);
    rig.events([Input::LoadFailed], cx);
    assert_eq!(rig.server.count("POST", "auth/refresh"), 1);
    assert_eq!(rig.fake.loads(), [stream(1), stream(1)], "a reload with the new token");
    rig.events([Input::LoadFailed], cx);
    assert_eq!(rig.server.count("POST", "playback/"), 2, "a start and a replan");
    assert!(rig.fake.loads()[2].ends_with("/master.m3u8"), "the new plan loads");
    rig.events([Input::LoadFailed], cx);
    assert_eq!(rig.read(cx, |p| p.prompt().to_string()), "Lost connection to the server");
    assert!(rig.errors.borrow().is_empty(), "the failure shows in the Player view");
    rig.player.update(cx, |player, cx| player.back(cx));
    cx.run_until_parked();
    assert_eq!(rig.fake.ops().last(), Some(&Op::Close));
    assert_eq!((rig.stops().len(), rig.view_closed.get(), rig.stopped.get()), (1, 1, 1));
}

#[gpui::test]
fn an_old_refresh_reply_updates_the_tokens_but_not_the_replacement_session(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    rig.server.hold(&["auth/refresh"]);
    rig.events([Input::LoadFailed], cx);
    assert_eq!(rig.server.count("POST", "auth/refresh"), 1);
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 50.0, cx);
    rig.server.release();
    cx.run_until_parked();
    assert_eq!(rig.client.tokens().refresh_token, "ref2", "the refresh completed");
    assert_eq!(rig.fake.loads(), [stream(1), stream(2)], "no reload");
    assert_eq!(rig.read(cx, Controller::position), Some(50.0));
}

#[gpui::test]
fn an_old_replan_reply_does_not_reach_the_replacement_session(cx: &mut TestAppContext) {
    let rig = rig(cx);
    let replans = [format!("playback/{}/replan", session(1)), format!("playback/{}/replan", session(2))];
    rig.server.hold(&[&replans[0], &replans[1]]);
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    rig.events([Input::LoadFailed], cx);
    rig.events([Input::LoadFailed], cx);
    rig.play(episode(1), cx);
    // The episode also waits for a replan, so a stray reply would load its plan.
    rig.loaded(3420.0, 50.0, cx);
    rig.events([Input::LoadFailed], cx);
    rig.events([Input::LoadFailed], cx);
    assert_eq!(rig.server.count("POST", "playback/0"), 2, "two replans wait");
    rig.server.release_first();
    cx.run_until_parked();
    assert_eq!(rig.fake.loads(), [stream(1), stream(1), stream(2), stream(2)], "no load of the plan of the movie");
    rig.server.release();
    cx.run_until_parked();
    assert!(rig.fake.loads()[4].ends_with("/master.m3u8"), "the replan of the episode loads its plan");
}

#[gpui::test]
fn events_of_a_closed_handle_do_not_reach_the_next_item(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(movie(), cx);
    rig.player.update(cx, |player, cx| player.back(cx));
    cx.run_until_parked();
    rig.server.hold(&["playback/start"]);
    rig.play(episode(1), cx);
    rig.fake.send_from(0, [Input::FileLoaded, Input::Duration(7200.0), Input::Position(300.0)]);
    cx.run_until_parked();
    assert_eq!(rig.read(cx, Controller::position), None);
    rig.server.release();
    cx.run_until_parked();
    assert_eq!(rig.fake.count(|op| *op == Op::Open), 2, "the episode plays on a new handle");
}

// Issue #115: the handle plays the old file on until the next Item loads its own file, and the events of the
// handle name no file. So the session of the next Item gets the events of the old file until then.

#[gpui::test]
#[ignore = "fault found in #115; the fix needs its own spec"]
fn the_end_of_the_old_file_does_not_end_the_next_item_before_it_loads(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 3412.0, cx);
    rig.server.hold(&["playback/start"]);
    rig.step(Input::Action, cx);
    // Play now on the Up Next card. The episode plays on to its end before the start reply of the next episode.
    rig.events([Input::Position(3419.9), Input::EndOfFile], cx);
    rig.server.release();
    cx.run_until_parked();
    assert_eq!(rig.server.started(), ["episode:severance-s01e01", "episode:severance-s01e02"]);
    assert_eq!(rig.fake.loads(), [stream(1), stream(2)], "the next episode loads");
}

#[gpui::test]
#[ignore = "fault found in #115; the fix needs its own spec"]
fn the_file_of_the_old_item_does_not_set_the_position_of_the_next_item(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(movie(), cx);
    rig.server.hold(&["playback/start"]);
    rig.play(episode(1), cx);
    // The movie file loads after the episode replaced the movie, before the start reply of the episode.
    rig.loaded(7200.0, 300.0, cx);
    assert_eq!(rig.read(cx, Controller::position), None);
    rig.server.release();
    cx.run_until_parked();
    rig.events([Input::LoadFailed], cx);
    assert_eq!(
        rig.fake.ops().last(),
        Some(&Op::Load { url: stream(2), start: 12.5 }),
        "the recovery reloads the episode at its own start, not at the position of the movie"
    );
}

#[gpui::test]
fn the_pages_reload_only_after_the_stop_report_ends_also_when_it_fails(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    rig.server.hold(&["playback/0"]);
    rig.player.update(cx, |player, cx| player.back(cx));
    cx.run_until_parked();
    assert_eq!((rig.stops().len(), rig.stopped.get()), (1, 0), "the stop report waits");
    rig.server.release();
    cx.run_until_parked();
    assert_eq!(rig.stopped.get(), 1);

    rig.play(movie(), cx);
    rig.server.fail(&["playback/0"]);
    rig.player.update(cx, |player, cx| player.back(cx));
    cx.run_until_parked();
    assert_eq!((rig.stops().len(), rig.stopped.get()), (2, 2), "a failed report also lets the pages reload");
}

#[gpui::test]
fn quit_waits_for_the_last_stop_report(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.server.hold(&["playback/0"]);
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 50.0, cx);
    let quit = rig.player.update(cx, |player, cx| player.quit(cx));
    let done = Rc::new(Cell::new(false));
    let set = done.clone();
    cx.foreground_executor()
        .spawn(async move {
            quit.await;
            set.set(true);
        })
        .detach();
    cx.run_until_parked();
    assert_eq!(rig.stops().len(), 2);
    assert!(!done.get(), "quit waits for the stop report");
    rig.server.release();
    cx.run_until_parked();
    assert!(done.get());
    assert_eq!(rig.stopped.get(), 2, "the older report finished too");
    assert_eq!(rig.fake.ops().last(), Some(&Op::Close));
}

#[gpui::test]
fn the_player_view_controls_reach_the_media(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    rig.step(Input::PlayPause, cx);
    rig.step(Input::SeekBy(10.0), cx);
    rig.step(Input::PickTrack { kind: Kind::Sub, id: None }, cx);
    rig.player.update(cx, |player, cx| {
        player.add_volume(5.0, cx);
        player.toggle_mute(cx);
    });
    let ops = rig.fake.ops();
    let tail = &ops[ops.len() - 5..];
    assert_eq!(tail, [Op::Pause(true), Op::Seek(310.0), Op::SelectSubtitle(None), Op::AddVolume(5.0), Op::ToggleMute]);
    assert_eq!(rig.read(cx, Controller::volume), Some((100.0, false)));
    rig.events([Input::Seeked(310.0)], cx);
    assert_eq!(rig.read(cx, Controller::position), Some(310.0), "the seek bar follows the seek");
}

/// The detail read of the movie, with no version list.
const MOVIE_DETAIL: &str = "catalog/items/movie:heat-1995?";

/// The subtitle operations of the fake media.
fn subtitles(rig: &Rig) -> Vec<Op> {
    rig.fake.ops().into_iter().filter(|op| matches!(op, Op::AddSubtitle(_) | Op::SelectSubtitle(_))).collect()
}

/// The subtitle choice of the Profile defaults for the movie: the forced English track, as the audio is English.
const FALLBACK: Op = Op::SelectSubtitle(Some(1));

#[gpui::test]
fn the_subtitle_choice_uses_the_effective_values_of_the_item_that_mpris_also_gets(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.server.detail(json!({}));
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    assert_eq!(subtitles(&rig), [Op::SelectSubtitle(Some(2))], "the Japanese track of the file, always on");
    assert_eq!(rig.server.count("GET", MOVIE_DETAIL), 1, "one detail read for the start and MPRIS");
    assert_eq!(rig.read(cx, |p| p.title().map(String::from)), Some("Heat".into()));
}

#[gpui::test]
fn without_values_of_the_item_the_subtitle_choice_uses_the_profile_values_and_an_empty_language_shows_none(
    cx: &mut TestAppContext,
) {
    let auto = |language: Value| {
        Some(json!({
            "effective_subtitle_language": language, "effective_subtitle_mode": "auto",
            "effective_show_forced_subtitles": true,
        }))
    };
    let cases = [
        ("no effective_subtitle_* fields", None, false, vec![FALLBACK]),
        ("a null language", auto(Value::Null), false, vec![FALLBACK]),
        ("an empty language", auto(json!("")), false, vec![]),
        ("a failed detail read", Some(json!({})), true, vec![FALLBACK]),
    ];
    for (case, fields, fail, want) in cases {
        let rig = rig(cx);
        if let Some(fields) = fields {
            rig.server.detail(fields);
        }
        if fail {
            rig.server.fail(&[MOVIE_DETAIL]);
        }
        rig.play(movie(), cx);
        rig.loaded(7200.0, 300.0, cx);
        assert_eq!(subtitles(&rig), want, "{case}");
    }
}

#[gpui::test]
fn the_start_does_not_wait_for_the_detail_and_the_session_waits_for_it_up_to_2_seconds(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.server.detail(json!({}));
    rig.server.hold(&[MOVIE_DETAIL]);
    rig.play(movie(), cx);
    assert_eq!(rig.server.count("POST", "playback/start"), 1, "the start request goes out at once");
    assert!(rig.fake.loads().is_empty(), "the session waits for the detail");
    rig.server.release();
    cx.run_until_parked();
    rig.loaded(7200.0, 300.0, cx);
    assert_eq!(subtitles(&rig), [Op::SelectSubtitle(Some(2))]);

    let rig = self::rig(cx);
    rig.server.detail(json!({}));
    rig.server.hold(&[MOVIE_DETAIL]);
    rig.play(movie(), cx);
    rig.seconds(1, cx);
    assert!(rig.fake.loads().is_empty());
    rig.seconds(1, cx);
    assert_eq!(rig.fake.loads(), [stream(1)], "after 2 seconds playback starts without the detail");
    rig.loaded(7200.0, 300.0, cx);
    assert_eq!(subtitles(&rig), [FALLBACK]);
    rig.server.release();
    cx.run_until_parked();
    assert_eq!(rig.read(cx, |p| p.title().map(String::from)), Some("Heat".into()), "MPRIS gets the late detail");
    assert_eq!(rig.server.count("GET", MOVIE_DETAIL), 1);
}

#[gpui::test]
fn a_replan_keeps_the_effective_values_of_the_item(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.server.detail(json!({}));
    rig.play(movie(), cx);
    rig.events([Input::LoadFailed], cx);
    rig.events([Input::LoadFailed], cx);
    assert!(rig.fake.loads()[2].ends_with("/master.m3u8"), "the new plan loads");
    rig.loaded(7200.0, 300.0, cx);
    let added =
        subtitles(&rig).into_iter().any(|op| matches!(op, Op::AddSubtitle(url) if url.contains("/subtitles/2.")));
    assert!(added, "the Japanese track of the new plan: {:?}", subtitles(&rig));
}

#[gpui::test]
fn an_episode_with_no_known_season_reads_its_detail_once_and_still_finds_the_next_episode(cx: &mut TestAppContext) {
    let rig = rig(cx);
    rig.server.detail(json!({
        "content_id": "episode:severance-s01e01", "type": "episode", "series_id": "series:severance",
        "season_number": 1,
    }));
    let mut card = movies()[0].clone();
    let series = json!({
        "content_id": "series:severance", "type": "series", "play_content_id": "episode:severance-s01e01",
        "series_id": null, "season_number": null,
    });
    card.as_object_mut().unwrap().extend(series.as_object().unwrap().clone());
    rig.play(PlayItem::from(&serde_json::from_value::<wire::CatalogItem>(card).unwrap()), cx);
    assert_eq!(rig.read(cx, |p| p.next_title().map(String::from)), Some("S1:E2 · Good News About Hell".into()));
    assert_eq!(rig.server.count("GET", "catalog/items/episode:severance-s01e01?"), 1);
}

#[gpui::test]
fn the_subtitle_appearance_applies_when_the_player_opens_and_a_change_applies_from_the_next_item(
    cx: &mut TestAppContext,
) {
    let rig = rig(cx);
    let set = |appearance: Value| {
        *rig.server.appearance.lock().unwrap() = appearance;
        futures::executor::block_on(rig.client.load_server_settings()).unwrap();
    };
    set(json!({ "fontSize": "small" }));
    rig.play(episode(1), cx);
    set(json!({ "position": "top" }));
    rig.loaded(3420.0, 105.0, cx);
    let client = rig.client.clone();
    rig.player.update(cx, |player, cx| player.play(client, episode(2), cx));
    cx.run_until_parked();
    let small = SubtitleAppearance { font_size: "small".into(), ..SubtitleAppearance::default() };
    let top = SubtitleAppearance { position: "top".into(), ..SubtitleAppearance::default() };
    assert_eq!(rig.fake.0.borrow().appearances, [(0, small), (1, top)]);
    assert_eq!(rig.fake.count(|op| *op == Op::Open), 1);
}

/// The `quality_preference` and the `bandwidth_cap_kbps` of each request whose path ends with `suffix`.
fn qualities(rig: &Rig, suffix: &str) -> Vec<(Value, Value)> {
    let requests = rig.server.requests("POST", "playback/");
    let bodies = requests.iter().filter(|r| r.path.ends_with(suffix)).map(|r| serde_json::from_str(&r.body).unwrap());
    bodies.map(|body: Value| (body["quality_preference"].clone(), body["bandwidth_cap_kbps"].clone())).collect()
}

#[gpui::test]
fn a_session_asks_for_the_preferred_quality_and_the_cap_and_a_ladder_pick_lasts_until_the_next_item(
    cx: &mut TestAppContext,
) {
    let rig = rig(cx);
    *rig.server.settings.lock().unwrap() = vec![
        json!({ "key": "playback.preferred_quality", "value": "720p", "source": "profile", "definition_revision": 1 }),
        json!({ "key": "playback.max_bitrate_kbps", "value": 4000, "source": "profile", "definition_revision": 1 }),
    ];
    futures::executor::block_on(rig.client.load_server_settings()).unwrap();
    let preferred = (json!("720p"), json!(4000));
    let pick = (json!("1080p-medium"), json!(4000));
    rig.play(movie(), cx);
    rig.loaded(7200.0, 300.0, cx);
    rig.step(Input::PickQuality("original".into()), cx);
    assert_eq!(qualities(&rig, "/replan"), [], "the source plays already");
    rig.step(Input::PickQuality("1080p-medium".into()), cx);
    rig.loaded(7200.0, 30.0, cx);
    rig.events([Input::LoadFailed], cx);
    rig.events([Input::LoadFailed], cx);
    assert_eq!(qualities(&rig, "/replan"), [pick.clone(), pick.clone()], "the pick, then a recovery");
    rig.play(episode(1), cx);
    rig.loaded(3420.0, 50.0, cx);
    rig.events([Input::LoadFailed], cx);
    rig.events([Input::LoadFailed], cx);
    assert_eq!(qualities(&rig, "playback/start"), [preferred.clone(), preferred.clone()]);
    assert_eq!(qualities(&rig, "/replan")[2..], [preferred], "the next Item starts with the preferred quality");
}
