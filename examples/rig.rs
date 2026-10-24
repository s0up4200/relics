//! The live-run rig (docs/rig.md): the real Library window, signed in to the fake Silo server of the tests.
//!
//! The app sends its API requests to the in-memory fake, so no API request leaves the process. A listener on
//! 127.0.0.1 serves the events socket, takes the same API calls from `curl`, and sends a socket frame on
//! `POST /rig/event`. It also serves the test video of `tests/media` to mpv, and `POST /rig/playback/fail` makes
//! the next request for the video fail. All app state and the mpv log go to the directory in the first argument.
//!
//! Run: `cargo run --example rig -- <scratch dir>`
// The rig plays the Server of the events socket, as tests/live.rs does (clippy.toml).
#![allow(clippy::disallowed_types)]
// tungstenite fixes the error type of the handshake callback.
#![allow(clippy::result_large_err)]

#[path = "../tests/common/mod.rs"]
mod common;

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_tungstenite::tungstenite::handshake::server::{Request, Response};
use async_tungstenite::tungstenite::{self, Message};
use common::{Reply, Seen, json, not_found, ok};
use futures::AsyncReadExt as _;
use gpui::{
    AppContext as _, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size,
};
use relics::library::{LibraryWindow, Session};
use relics::pages::Pages;
use relics::state::{SavedAccount, SavedServer, State};
use serde_json::{Value, json};

const PROFILE: &str = "p-owner";
/// The length of `tests/media/sample.mp4` and of its HLS copy, in seconds.
const VIDEO_SECONDS: f64 = 6.0;

/// The state of the fake Server that the agent changes.
#[derive(Clone, Default)]
struct Fake {
    /// The stored Server settings of the Profile, by key.
    settings: Arc<Mutex<BTreeMap<String, Value>>>,
    /// One sender for each open events socket.
    sockets: Arc<Mutex<Vec<Sender<String>>>>,
    /// The fixture of each successful operation in `tests/fixtures/silo/index.json`, by method and path.
    fixtures: Arc<HashMap<(String, String), String>>,
    /// The number of plans that the fake gave. It numbers the Playback sessions and the plans.
    plans: Arc<AtomicU32>,
    /// The number of requests for the test video that fail before the video plays again.
    failing: Arc<AtomicU32>,
}

impl Fake {
    fn new() -> Self {
        let index: Value = serde_json::from_str(&common::fixture("index.json")).unwrap();
        let fixtures: HashMap<_, _> = index["fixtures"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["expected_status"].as_u64().is_some_and(|status| status < 300))
            // The last insert wins, so after the reverse the first fixture of an operation in the index wins.
            .rev()
            .map(|f| {
                let path = f["request"]["path"].as_str().unwrap();
                let path = path.trim_start_matches("/api/v2/").split('?').next().unwrap();
                let key = (f["request"]["method"].as_str().unwrap().to_string(), path.to_string());
                (key, f["body_file"].as_str().unwrap().to_string())
            })
            .collect();
        Self { fixtures: Arc::new(fixtures), ..Self::default() }
    }

    /// The reply of the fake Server, with one line on stderr for each request.
    fn reply(&self, req: &Seen) -> Reply {
        let reply = self.route(req);
        let status = reply.as_ref().map_or(0, |reply| reply.status().as_u16());
        eprintln!("rig: {} {} -> {status}", req.method, req.path);
        reply
    }

    fn route(&self, req: &Seen) -> Reply {
        let path = req.path.split('?').next().unwrap();
        let setting = path.strip_prefix("settings/values/").filter(|key| *key != "effective");
        match (req.method.as_str(), path, setting) {
            ("POST", "events/ws-ticket", _) => {
                let ticket = json!({"ticket": "rig", "expires_in": 30, "max_connection_seconds": 86400, "protocol": "silo.events.v2"});
                json(201, ticket.to_string())
            }
            ("GET", "settings/values/effective", _) => {
                let items: Vec<_> = self.settings.lock().unwrap().iter().map(|(key, value)| {
                    json!({"key": key, "value": value, "source": "profile", "definition_revision": 1, "scope": "profile", "profile_id": PROFILE})
                }).collect();
                json(200, json!({"revision": 1, "items": items}).to_string())
            }
            ("PUT", _, Some(key)) => {
                let Ok(body) = serde_json::from_str::<Value>(&req.body) else {
                    return common::problem(422, "validation_failed_body.json");
                };
                self.settings.lock().unwrap().insert(key.to_string(), body["value"].clone());
                json(200, json!({"key": key, "value": body["value"], "scope": "profile", "revision": 1}).to_string())
            }
            ("DELETE", _, Some(key)) => match self.settings.lock().unwrap().remove(key) {
                Some(_) => json(204, String::new()),
                None => not_found(),
            },
            ("POST", "playback/start", _) => json(201, self.plan(&req.body, None).to_string()),
            ("POST", path, _)
                if let Some(session) = path.strip_prefix("playback/").and_then(|p| p.strip_suffix("/replan")) =>
            {
                json(200, self.plan(&req.body, Some(session)).to_string())
            }
            ("POST", path, _) if path.starts_with("playback/") && path.ends_with("/progress") => {
                ok("playback_progress_applied.json")
            }
            ("DELETE", path, _) if path.starts_with("playback/") => ok("playback_stop_completed.json"),
            (method, path, _) => match self.fixtures.get(&(method.to_string(), path.to_string())) {
                Some(file) => ok(file),
                None => not_found(),
            },
        }
    }

    /// The playable answer to the start or replan request `body` for the test video, in the Playback session
    /// `session`. A start has no session, so the fake gives it a new one. The quality `original`, `auto` or none
    /// gives the Original file. Each other quality gives a transcode plan, at the step of the Quality ladder with
    /// that label, or else at the 1080p Medium step. A start position past the end of the video starts at 0. The
    /// plan has one subtitle track, which only a burn-in can show. A replan that names it gives a plan that draws
    /// it into the video. The video does not change, so the rig shows no subtitles.
    fn plan(&self, body: &str, session: Option<&str>) -> Value {
        let body: Value = serde_json::from_str(body).unwrap_or_default();
        if session.is_some() {
            eprintln!("rig: replan {} with subtitle {}", body["operation"], body["selected_tracks"]["subtitle"]);
        }
        let quality = body["quality_preference"].as_str().unwrap_or_default();
        let original = matches!(quality, "" | "auto" | "original");
        let (fixture, url) = match original {
            true => ("playback_start_opaque_ids.json", "rig/media/sample.mp4"),
            false => ("../own/playback_start_transcode.json", "rig/media/hls/index.m3u8"),
        };
        let steps = json!([
            {"label": "original", "display_name": "Original", "height": 180, "bitrate_kbps": 60, "preserves_source": true},
            {"label": "1080p-medium", "display_name": "1080p Medium", "height": 1080, "bitrate_kbps": 6000, "preserves_source": false},
            {"label": "720p-low", "display_name": "720p Low", "height": 720, "bitrate_kbps": 1500, "preserves_source": false},
        ]);
        let position = body["start_position"].as_f64().or(body["position_seconds"].as_f64());
        let position = position.filter(|position| *position < VIDEO_SECONDS).unwrap_or(0.0);
        let n = self.plans.fetch_add(1, Ordering::SeqCst) + 1;
        let session = session.map_or_else(|| format!("rig-{n}"), str::to_string);
        let mut decision: Value = serde_json::from_str(&common::fixture(fixture)).unwrap();
        decision["session_id"] = session.clone().into();
        let plan = &mut decision["playback_plan"];
        plan["session_id"] = session.into();
        plan["plan_id"] = format!("plan:rig-{n}").into();
        plan["plan_attempt_key"] = format!("v3:rig-{n}").into();
        plan["stream"]["url"] = url.into();
        plan["timeline"] = json!({
            "source_start_seconds": position, "stream_origin_seconds": 0, "player_start_seconds": position,
            "timeline_offset_seconds": 0, "can_seek_anywhere": true, "seek_restoration": "player_position",
        });
        plan["subtitle"]["inventory"] = json!([{
            "track_id": "rig:subtitle:0", "combined_index": 0, "source": "embedded", "codec": "hdmv_pgs_subtitle",
            "language": "eng", "label": "English (PGS)", "forced": false, "default": false, "hearing_impaired": false,
            "delivery": "burn_in_only",
        }]);
        let subtitle = &body["selected_tracks"]["subtitle"];
        if body["operation"].is_string() && subtitle["index"] == 0 {
            plan["subtitle"]["mode"] = "burn_in".into();
            plan["selected_tracks"]["subtitle"] = subtitle.clone();
        }
        plan["source"]["duration_seconds"] = VIDEO_SECONDS.into();
        if !original {
            let step = steps.as_array().unwrap().iter().skip(1).find(|step| step["label"] == quality);
            let step = step.unwrap_or(&steps[1]);
            plan["effective_recipe"]["height"] = step["height"].clone();
            plan["effective_recipe"]["bitrate_kbps"] = step["bitrate_kbps"].clone();
        }
        plan["available_qualities"] = steps;
        decision
    }

    /// The reply to `GET /rig/media/<name>`: the file `tests/media/<name>` from the first byte of the `Range`
    /// header, or a 503 while `failing` is above 0.
    fn media(&self, name: &str, range: Option<&str>) -> (u16, &'static str, String, Vec<u8>) {
        if self.failing.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok() {
            return (503, "text/plain", String::new(), Vec::new());
        }
        let file = format!("{}/tests/media/{name}", env!("CARGO_MANIFEST_DIR"));
        let Some(bytes) = (!name.contains("..")).then(|| std::fs::read(file).ok()).flatten() else {
            return (404, "text/plain", String::new(), Vec::new());
        };
        let kind = match name.rsplit('.').next() {
            Some("m3u8") => "application/vnd.apple.mpegurl",
            Some("ts") => "video/mp2t",
            _ => "video/mp4",
        };
        // A range is `bytes=start-` or `bytes=start-end`. The rig sends each byte from the start to the end.
        let first = range.and_then(|r| r.strip_prefix("bytes=")?.split('-').next()?.parse().ok());
        match first {
            Some(first) if first < bytes.len() => {
                let extra = format!("Content-Range: bytes {first}-{}/{}\r\n", bytes.len() - 1, bytes.len());
                (206, kind, extra, bytes[first..].to_vec())
            }
            _ => (200, kind, String::new(), bytes),
        }
    }

    /// Sends `frame` to each open events socket, and gives the number of sockets that took it.
    fn send(&self, frame: &str) -> usize {
        let mut sockets = self.sockets.lock().unwrap();
        sockets.retain(|socket| socket.send(frame.to_string()).is_ok());
        sockets.len()
    }
}

/// Serves the listener: the events socket, `POST /rig/event`, and each other request through the fake.
fn serve(listener: TcpListener, fake: Fake) {
    for stream in listener.incoming().flatten() {
        let fake = fake.clone();
        std::thread::spawn(move || {
            if let Err(err) = connection(stream, &fake) {
                eprintln!("rig: connection failed: {err}");
            }
        });
    }
}

fn connection(stream: TcpStream, fake: &Fake) -> anyhow::Result<()> {
    // The client sends the request line in one write, so one peek sees it.
    let mut start = [0; 32];
    let seen = stream.peek(&mut start)?;
    if start[..seen].starts_with(b"GET /api/v2/events/ws ") {
        return socket(stream, fake);
    }
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let Some((name, value)) = line.trim_end().split_once(':') else { break };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    let header = |name: &str| headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
    let length = header("Content-Length").and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8(body)?;
    let (mut content_type, mut more_headers) = ("application/json", String::new());
    let (status, body) = if let Some(name) = target.strip_prefix("/rig/media/") {
        let reply = fake.media(name, header("Range").as_deref());
        eprintln!("rig: {method} {target} -> {}", reply.0);
        (content_type, more_headers) = (reply.1, reply.2);
        (reply.0, reply.3)
    } else if (method.as_str(), target.as_str()) == ("POST", "/rig/playback/fail") {
        let failing = fake.failing.fetch_add(1, Ordering::SeqCst) + 1;
        eprintln!("rig: the next {failing} request(s) for the video fail");
        (200, format!("{{\"failing\":{failing}}}").into_bytes())
    } else if (method.as_str(), target.as_str()) == ("POST", "/rig/event") {
        let sockets = fake.send(&body);
        eprintln!("rig: event to {sockets} socket(s): {body}");
        (200, format!("{{\"sockets\":{sockets}}}").into_bytes())
    } else {
        let path = target.split_once("/api/v2/").map_or(target.as_str(), |(_, path)| path).to_string();
        let seen = Seen { method, url: target.clone(), path, headers, body };
        match fake.reply(&seen) {
            Some(reply) => {
                let status = reply.status().as_u16();
                let mut body = String::new();
                futures::executor::block_on(reply.into_body().read_to_string(&mut body))?;
                (status, body.into_bytes())
            }
            None => (502, Vec::new()),
        }
    };
    let head = format!(
        "HTTP/1.1 {status} rig\r\nContent-Type: {content_type}\r\n{more_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    (&stream).write_all(&[head.into_bytes(), body].concat())?;
    Ok(())
}

/// Accepts the events socket, then sends each frame of `POST /rig/event` and a ping each 20 seconds, as the Server
/// does. The app sends only its subscribe frame, so the rig reads no frame.
fn socket(stream: TcpStream, fake: &Fake) -> anyhow::Result<()> {
    let mut socket = tungstenite::accept_hdr(stream, |_: &Request, mut reply: Response| {
        reply.headers_mut().insert("Sec-WebSocket-Protocol", "silo.events.v2".parse().unwrap());
        Ok(reply)
    })?;
    let (tx, rx) = mpsc::channel();
    fake.sockets.lock().unwrap().push(tx);
    eprintln!("rig: events socket open");
    socket.send(Message::text(r#"{"type":"hello"}"#))?;
    loop {
        let message = match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(frame) => Message::text(frame),
            Err(RecvTimeoutError::Timeout) => Message::Ping(Default::default()),
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        if let Err(err) = socket.send(message) {
            eprintln!("rig: events socket closed: {err}");
            return Ok(());
        }
    }
}

fn main() {
    let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: cargo run --example rig -- <scratch dir>");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&dir).expect("make the scratch directory");
    // A sign-out in the rig window must not delete items of the real keyring.
    relics::secrets::turn_off();
    let listener = TcpListener::bind("127.0.0.1:0").expect("listen on 127.0.0.1");
    let root = format!("http://{}/", listener.local_addr().unwrap());
    eprintln!("rig: fake Silo at {root}");
    let fake = Fake::new();
    std::thread::spawn({
        let fake = fake.clone();
        move || serve(listener, fake)
    });
    let (http, _) = common::http(move |req| fake.reply(req));

    gpui_kit::application().with_assets(relics::assets::Assets).run(move |cx| {
        gpui_omarchy::init(cx);
        relics::text_input::init(cx);
        relics::theme::keep_readable(cx);
        cx.set_http_client(http.clone());
        relics::artwork::install(http.clone(), dir.join("artwork"), cx);
        let client = Arc::new(common::client(http, &root));
        client.set_profile(Some(PROFILE.into()));
        let profiles: relics::api::wire::ProfileCollection =
            serde_json::from_str(&common::fixture("list_profiles_ok.json")).unwrap();
        let mut state = State::fresh();
        let account =
            SavedAccount { id: "account-1".into(), username: "rig".into(), last_profile: Some(PROFILE.into()) };
        let server = client.server().id.clone();
        state.servers.push(SavedServer { id: server.clone(), url: root.clone(), accounts: vec![account] });
        state.active = Some((server.clone(), "account-1".into()));
        let tokens = HashMap::from([((server, "account-1".into()), client.tokens())]);
        cx.open_window(
            WindowOptions {
                app_id: Some("relics-rig".into()),
                titlebar: Some(TitlebarOptions { title: Some("Relics rig".into()), ..Default::default() }),
                window_background: WindowBackgroundAppearance::Transparent,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1400.), px(900.)), cx))),
                ..Default::default()
            },
            move |window, cx| {
                let pages = cx.new(|cx| Pages::new(client.clone(), "Laura".into(), window, cx));
                let session =
                    Session { state, directory: dir, client, pages, profiles: profiles.items, tokens, video: true };
                cx.new(|cx| LibraryWindow::with_session(session, window, cx))
            },
        )
        .expect("open the Library window");
    });
}
