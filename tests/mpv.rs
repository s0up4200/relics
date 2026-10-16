//! The mpv adapter with real libmpv (`vo=null ao=null`) against a local HTTP stub on
//! 127.0.0.1. The stub serves the test video from tests/media/ (scripts/make-test-video.sh).
#![allow(clippy::disallowed_types)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use relics::mpv::Player;
use relics::playback::Input;
use relics::playback::tracks::Kind;

/// One request that the stub got: the path, the `Authorization` header and the device name header.
#[derive(Debug, Clone)]
struct Request {
    path: String,
    authorization: Option<String>,
    device_name: Option<String>,
}

struct Stub {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

/// Starts the stub. After `ok_requests` requests, it answers each request with 401.
fn stub(ok_requests: usize) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    let count = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (log, count) = (log.clone(), count.clone());
            std::thread::spawn(move || serve(stream, &log, count.fetch_add(1, Ordering::SeqCst) >= ok_requests));
        }
    });
    Stub { url, requests }
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<Request>>, refuse: bool) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split_whitespace().nth(1).unwrap_or_default().to_string();
    let (mut authorization, mut device_name, mut range) = (None, None, None);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or_default();
        match name.to_ascii_lowercase().as_str() {
            "authorization" => authorization = Some(value.trim().to_string()),
            "x-silo-device-name" => device_name = Some(value.trim().to_string()),
            "range" => range = Some(value.trim().to_string()),
            _ => {}
        }
    }
    log.lock().unwrap().push(Request { path: path.clone(), authorization, device_name });
    let file = format!("{}/tests/media{path}", env!("CARGO_MANIFEST_DIR"));
    let reply = match std::fs::read(&file) {
        _ if refuse => b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        Err(_) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        Ok(bytes) => {
            // A range is `bytes=start-` or `bytes=start-end`.
            let first = range
                .as_deref()
                .and_then(|r| r.strip_prefix("bytes="))
                .and_then(|r| r.split('-').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0usize)
                .min(bytes.len());
            let (status, extra) = match range {
                Some(_) => (
                    "206 Partial Content",
                    format!("Content-Range: bytes {first}-{}/{}\r\n", bytes.len() - 1, bytes.len()),
                ),
                None => ("200 OK", String::new()),
            };
            let mut reply = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n{extra}Connection: close\r\n\r\n",
                bytes.len() - first
            )
            .into_bytes();
            reply.extend_from_slice(&bytes[first..]);
            reply
        }
    };
    stream.write_all(&reply).ok();
}

fn headers() -> Vec<(String, String)> {
    vec![
        ("Authorization".into(), "Bearer test-token".into()),
        ("X-Profile-Id".into(), "p-owner".into()),
        ("X-Silo-Device-Name".into(), "box, with a comma".into()),
    ]
}

fn open() -> (Player, Receiver<Input>) {
    let (tx, rx) = mpsc::channel();
    let player = Player::open("no", &[("vo", "null"), ("ao", "null")], move |input| {
        tx.send(input).ok();
    })
    .unwrap();
    (player, rx)
}

/// Waits up to 15 s for the first input that `want` accepts.
fn wait_for(rx: &Receiver<Input>, mut want: impl FnMut(&Input) -> bool) -> Input {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let input = rx.recv_timeout(left).expect("mpv sent no such input in time");
        if want(&input) {
            return input;
        }
    }
}

fn assert_each_request_has_the_header(stub: &Stub) {
    let requests = stub.requests.lock().unwrap();
    assert!(!requests.is_empty());
    for request in requests.iter() {
        assert_eq!(request.authorization.as_deref(), Some("Bearer test-token"), "{request:?} in {requests:?}");
        assert_eq!(request.device_name.as_deref(), Some("box, with a comma"), "{request:?}");
    }
}

#[test]
fn an_mp4_plays_from_the_start_position_with_the_header_on_each_request() {
    let stub = stub(usize::MAX);
    let (player, rx) = open();
    player.load(&format!("{}/sample.mp4", stub.url), &headers(), 3.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    let Input::Position(position) = wait_for(&rx, |input| matches!(input, Input::Position(_))) else { unreachable!() };
    assert!((2.5..6.0).contains(&position), "playback started at {position}");
    wait_for(&rx, |input| matches!(input, Input::Duration(d) if (*d - 6.0).abs() < 0.5));
    assert_each_request_has_the_header(&stub);
}

#[test]
fn hls_plays_with_the_header_on_the_manifest_and_each_segment() {
    let stub = stub(usize::MAX);
    let (player, rx) = open();
    player.load(&format!("{}/hls/index.m3u8", stub.url), &headers(), 0.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    wait_for(&rx, |input| matches!(input, Input::Position(p) if *p > 0.0));
    drop(player);
    let paths: Vec<String> = stub.requests.lock().unwrap().iter().map(|r| r.path.clone()).collect();
    assert!(paths.iter().any(|p| p.ends_with(".m3u8")) && paths.iter().any(|p| p.ends_with(".ts")), "{paths:?}");
    assert_each_request_has_the_header(&stub);
}

#[test]
fn a_401_shows_as_a_load_failure() {
    let stub = stub(0);
    let (player, rx) = open();
    player.load(&format!("{}/sample.mp4", stub.url), &headers(), 0.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::LoadFailed));
    assert_each_request_has_the_header(&stub);
}

#[test]
fn the_end_of_a_file_shows_as_end_of_file() {
    let stub = stub(usize::MAX);
    let (player, rx) = open();
    player.load(&format!("{}/sample.mp4", stub.url), &headers(), 5.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::EndOfFile));
    assert_eq!(player.property("eof-reached").unwrap(), "yes", "mpv keeps the last frame");
}

#[test]
fn a_401_in_the_middle_of_an_hls_stream_ends_it_before_the_duration() {
    // The manifest and the first segment load, then each request gets a 401, as when the token expires.
    let stub = stub(2);
    let (player, rx) = open();
    player.load(&format!("{}/hls/index.m3u8", stub.url), &headers(), 0.0, None).unwrap();
    let mut position = 0.0;
    let end = wait_for(&rx, |input| {
        if let Input::Position(p) = input {
            position = *p;
        }
        matches!(input, Input::EndOfFile | Input::LoadFailed)
    });
    assert!(position < 5.0, "{end:?} at {position} of 6 s, so the end rule sees an Early end");
    // mpv stays open, so the Player view can show "Lost connection to the server".
    assert_eq!(player.property("idle-active").unwrap_or_default(), "no");
}

#[test]
fn closing_the_player_ends_with_shutdown() {
    let (tx, rx) = mpsc::channel();
    let player = Player::open("no", &[("vo", "null"), ("ao", "null")], move |input| {
        tx.send(input).ok();
    })
    .unwrap();
    drop(player);
    wait_for(&rx, |input| matches!(input, Input::Shutdown));
}

#[test]
fn the_player_view_changes_the_volume_and_mutes() {
    let (player, _rx) = open();
    let (volume, _) = player.volume().unwrap();
    player.add_volume(-5.0).unwrap();
    player.toggle_mute().unwrap();
    assert_eq!(player.volume().unwrap(), (volume - 5.0, true));
    player.toggle_mute().unwrap();
    assert!(!player.volume().unwrap().1);
}

#[test]
fn the_player_view_changes_and_resets_the_subtitle_delay() {
    let (player, _rx) = open();
    assert_eq!(player.sub_delay().unwrap(), 0.0);
    player.add_sub_delay(0.1).unwrap();
    player.add_sub_delay(0.1).unwrap();
    player.add_sub_delay(-0.5).unwrap();
    // mpv keeps the delay as a 32-bit float.
    let delay = player.sub_delay().unwrap();
    assert!((delay + 0.3).abs() < 1e-6, "{delay}");
    player.set_sub_delay(0.0).unwrap();
    assert_eq!(player.sub_delay().unwrap(), 0.0);
}

/// Waits up to 15 s until the mpv property `name` has the value `want`.
fn wait_for_property(player: &Player, name: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while player.property(name).ok().as_deref() != Some(want) {
        assert!(Instant::now() < deadline, "{name} is {:?}, not {want}", player.property(name).ok());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_added_subtitle_shows_and_its_request_has_the_header() {
    let stub = stub(usize::MAX);
    let (player, rx) = open();
    player.load(&format!("{}/sample.mp4", stub.url), &headers(), 0.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    assert_eq!(player.property("sid").unwrap(), "no");
    player.add_subtitle(&format!("{}/sample.vtt", stub.url), "English", "eng").unwrap();
    wait_for_property(&player, "sid", "1");
    assert_eq!(player.property("current-tracks/sub/lang").unwrap(), "eng");
    assert!(stub.requests.lock().unwrap().iter().any(|r| r.path == "/sample.vtt"));
    assert_each_request_has_the_header(&stub);
}

#[test]
fn a_seek_ends_in_seeked_with_the_new_position_and_set_pause_pauses() {
    let stub = stub(usize::MAX);
    let (player, rx) = open();
    player.load(&format!("{}/sample.mp4", stub.url), &headers(), 0.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    player.seek(3.0).unwrap();
    wait_for(&rx, |input| matches!(input, Input::Seeked(position) if (*position - 3.0).abs() < 0.5));
    player.set_pause(true).unwrap();
    wait_for(&rx, |input| matches!(input, Input::Paused(true)));
}

#[test]
fn direct_play_plays_the_chosen_audio_track_and_shows_an_embedded_subtitle_track() {
    let stub = stub(usize::MAX);
    let (player, rx) = open();
    player.load(&format!("{}/tracks.mkv", stub.url), &headers(), 0.0, Some(2)).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    wait_for_property(&player, "current-tracks/audio/lang", "jpn");
    assert_eq!(player.property("sid").unwrap(), "no");
    player.select_subtitle(Some(1)).unwrap();
    wait_for_property(&player, "current-tracks/sub/lang", "eng");
    assert_eq!(player.property("track-list/count").unwrap(), "4", "no second copy of the subtitle track");
}

/// The (id, language, selected) of each track of `kind`.
fn tracks_of(input: &Input, kind: Kind) -> Vec<(i64, String, bool)> {
    let Input::Tracks(tracks) = input else { return vec![] };
    let of_kind = tracks.iter().filter(|track| track.kind == kind);
    of_kind.map(|track| (track.id, track.lang.clone().unwrap_or_default(), track.selected)).collect()
}

#[test]
fn mpv_reports_the_track_list_and_a_pick_by_id_selects_the_audio_and_subtitle_track() {
    let stub = stub(usize::MAX);
    let (player, rx) = open();
    player.load(&format!("{}/tracks.mkv", stub.url), &headers(), 0.0, None).unwrap();
    let audio = |eng, jpn| vec![(1, "eng".to_string(), eng), (2, "jpn".to_string(), jpn)];
    let first = wait_for(&rx, |input| tracks_of(input, Kind::Audio) == audio(true, false));
    let Input::Tracks(tracks) = first else { unreachable!() };
    let sub = tracks.iter().find(|track| track.kind == Kind::Sub).expect("the embedded subtitle track");
    assert_eq!(
        (sub.id, sub.lang.as_deref(), sub.codec.as_deref(), sub.external),
        (1, Some("eng"), Some("subrip"), false)
    );
    let eng = tracks.iter().find(|track| track.kind == Kind::Audio).unwrap();
    assert_eq!((eng.codec.as_deref(), eng.channels), (Some("aac"), Some(1)));

    player.select_audio(2).unwrap();
    wait_for(&rx, |input| tracks_of(input, Kind::Audio) == audio(false, true));
    player.select_subtitle(Some(1)).unwrap();
    wait_for(&rx, |input| tracks_of(input, Kind::Sub) == [(1, "eng".to_string(), true)]);
    player.select_subtitle(None).unwrap();
    wait_for(&rx, |input| tracks_of(input, Kind::Sub) == [(1, "eng".to_string(), false)]);

    player.add_subtitle(&format!("{}/sample.vtt", stub.url), "English", "eng").unwrap();
    let added = wait_for(&rx, |input| tracks_of(input, Kind::Sub).len() == 2);
    let Input::Tracks(tracks) = added else { unreachable!() };
    let external = tracks.iter().find(|track| track.kind == Kind::Sub && track.id == 2).unwrap();
    assert!(external.external && external.selected, "{external:?}");
    assert_eq!(external.title.as_deref(), Some("English"));
}

// Which load an event comes from (issue #115). The handle plays one file after the other, and libmpv2 6.0 drops the
// playlist entry id of `start-file` and `end-file`. So these tests read the raw `mpv_event` of the C API, with the
// layout of client.h of mpv client API 2.5.

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RawProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct RawEndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
}

unsafe extern "C" {
    fn mpv_wait_event(ctx: *mut c_void, timeout: f64) -> *mut RawEvent;
}

const SHUTDOWN: c_int = 1;
const START_FILE: c_int = 6;
const END_FILE: c_int = 7;
const FILE_LOADED: c_int = 8;
const PROPERTY_CHANGE: c_int = 22;
const FORMAT_STRING: c_int = 1;
const REASON_STOP: c_int = 2;
const REASON_ERROR: c_int = 4;
const REASON_REDIRECT: c_int = 5;

/// One raw mpv event, in the order that the event thread got it.
#[derive(Debug, Clone, PartialEq)]
enum Raw {
    /// Not an event: `loadfile` returned on the test thread, and its playlist entry has this id.
    Loadfile(i64),
    Start(i64),
    Loaded,
    End {
        id: i64,
        reason: c_int,
    },
    /// An observed property, as text. `None`: mpv had no value.
    Property(String, Option<String>),
}

/// An mpv handle with the options of `Player::open`. Its event thread logs each raw event with the time that the
/// event reached the thread.
struct Recorder {
    mpv: Arc<libmpv2::Mpv>,
    log: Arc<Mutex<Vec<(Instant, Raw)>>>,
    events: Option<std::thread::JoinHandle<()>>,
}

fn recorder() -> Recorder {
    let mpv = libmpv2::Mpv::with_initializer(|init| {
        for (name, value) in [
            ("hwdec", "no"),
            ("load-scripts", "no"),
            ("osc", "no"),
            ("osd-bar", "no"),
            ("reset-on-next-file", "aid,sid"),
            ("keep-open", "yes"),
            ("ytdl", "no"),
            ("vo", "null"),
            ("ao", "null"),
        ] {
            init.set_option(name, value)?;
        }
        Ok(())
    })
    .unwrap();
    for name in ["time-pos", "duration", "eof-reached", "track-list/count"] {
        mpv.observe_property(name, libmpv2::Format::String, 0).unwrap();
    }
    let (mpv, log) = (Arc::new(mpv), Arc::new(Mutex::new(Vec::new())));
    let events = std::thread::spawn({
        let (mpv, log) = (mpv.clone(), log.clone());
        move || {
            loop {
                let received;
                // SAFETY: the event and its data stay valid until the next wait on this handle, and only this thread
                // waits.
                let raw = unsafe {
                    let event = &*mpv_wait_event(mpv.ctx.as_ptr().cast(), -1.0);
                    received = Instant::now();
                    match event.event_id {
                        SHUTDOWN => return,
                        START_FILE => Raw::Start(*event.data.cast::<i64>()),
                        END_FILE => {
                            let end = &*event.data.cast::<RawEndFile>();
                            Raw::End { id: end.playlist_entry_id, reason: end.reason }
                        }
                        FILE_LOADED => Raw::Loaded,
                        PROPERTY_CHANGE => {
                            let property = &*event.data.cast::<RawProperty>();
                            let text = |data: *mut c_void| CStr::from_ptr(*data.cast::<*const c_char>());
                            Raw::Property(
                                CStr::from_ptr(property.name).to_string_lossy().into_owned(),
                                (property.format == FORMAT_STRING)
                                    .then(|| text(property.data).to_string_lossy().into_owned()),
                            )
                        }
                        _ => continue,
                    }
                };
                log.lock().unwrap().push((received, raw));
            }
        }
    });
    Recorder { mpv, log, events: Some(events) }
}

impl Recorder {
    /// Replaces the file with `url` at `start`, and returns the playlist entry id of the load. The `Loadfile` item has
    /// the time that the call returned, so each event after it in the log reached the event thread after the call
    /// returned. The id is correct only for a file that is not a playlist, because mpv can replace a playlist entry
    /// with new entries before the property read.
    fn load(&self, url: &str, start: f64) -> i64 {
        self.mpv.command("loadfile", &[url, "replace", "-1", &format!("start={start}")]).unwrap();
        let returned = Instant::now();
        // `replace` empties the playlist, so the new entry is the first one.
        let id = self.mpv.get_property("playlist/0/id").unwrap();
        self.log.lock().unwrap().push((returned, Raw::Loadfile(id)));
        id
    }

    /// The log in the order of time.
    fn log(&self) -> Vec<Raw> {
        let mut log = self.log.lock().unwrap().clone();
        log.sort_by_key(|(at, _)| *at);
        log.into_iter().map(|(_, raw)| raw).collect()
    }

    /// Waits up to 15 s for an event at or after the log index `from` that `want` accepts, and returns its index.
    fn wait(&self, from: usize, want: impl Fn(&Raw) -> bool) -> usize {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(at) = self.log().iter().skip(from).position(&want) {
                return from + at;
            }
            assert!(Instant::now() < deadline, "no such event in {}", show(&self.log()));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.mpv.command("quit", &[]).ok();
        if let Some(events) = self.events.take() {
            events.join().ok();
        }
    }
}

/// The log on one line, for the assertion messages.
fn show(log: &[Raw]) -> String {
    let token = |raw: &Raw| match raw {
        Raw::Property(name, value) => format!("{name}={}", value.as_deref().unwrap_or("none")),
        other => format!("{other:?}"),
    };
    log.iter().map(token).collect::<Vec<_>>().join(" ")
}

fn index(log: &[Raw], raw: &Raw) -> usize {
    log.iter().position(|r| r == raw).unwrap_or_else(|| panic!("no {raw:?} in {}", show(log)))
}

fn is_property(raw: &Raw, name: &str, value: &str) -> bool {
    matches!(raw, Raw::Property(n, Some(v)) if n == name && v == value)
}

#[test]
fn the_end_file_of_the_old_load_comes_after_the_next_load_returns_and_names_its_entry() {
    let stub = stub(usize::MAX);
    let rec = recorder();
    let a = rec.load(&format!("{}/sample.mp4", stub.url), 0.0);
    let at =
        rec.wait(0, |r| matches!(r, Raw::Property(n, Some(p)) if n == "time-pos" && p.parse::<f64>().unwrap() > 1.0));
    // A new Item or another version: another file.
    let b = rec.load(&format!("{}/tracks.mkv", stub.url), 0.0);
    let at = rec.wait(at, |r| *r == Raw::Start(b));
    let at = rec.wait(at, |r| *r == Raw::Loaded);
    // A reload: the same file again, at a position.
    let c = rec.load(&format!("{}/tracks.mkv", stub.url), 2.0);
    let at = rec.wait(at, |r| *r == Raw::Start(c));
    rec.wait(at, |r| *r == Raw::Loaded);
    let log = rec.log();
    for (old, new) in [(a, b), (b, c)] {
        let (load, end, start) = (
            index(&log, &Raw::Loadfile(new)),
            index(&log, &Raw::End { id: old, reason: REASON_STOP }),
            index(&log, &Raw::Start(new)),
        );
        assert!(old != new && load < end && end < start, "{}", show(&log));
    }
}

#[test]
fn a_file_at_its_end_reports_eof_reached_once_and_ends_with_stop_at_the_next_load() {
    let stub = stub(usize::MAX);
    let rec = recorder();
    let a = rec.load(&format!("{}/sample.mp4", stub.url), 5.0);
    let eof = rec.wait(0, |r| is_property(r, "eof-reached", "yes"));
    std::thread::sleep(Duration::from_millis(200));
    let b = rec.load(&format!("{}/tracks.mkv", stub.url), 0.0);
    let at = rec.wait(eof, |r| *r == Raw::Start(b));
    rec.wait(at, |r| *r == Raw::Loaded);
    let log = rec.log();
    // The adapter turns `eof-reached` into `EndOfFile`, and skips an end-file with the reason stop.
    let end = index(&log, &Raw::End { id: a, reason: REASON_STOP });
    assert!(eof < index(&log, &Raw::Loadfile(b)) && !log[end..].iter().any(|r| is_property(r, "eof-reached", "yes")));
    assert_eq!(log.iter().filter(|r| is_property(r, "eof-reached", "yes")).count(), 1, "{}", show(&log));
}

/// The file that a property value describes: 'a' is sample.mp4 from 4 s (2 tracks, 6.000 s, plays to its end), and
/// 'b' is tracks.mkv from 0 s (4 tracks, 6.023 s), which the test replaces before 1 s. `None` fits both files.
fn file_of(raw: &Raw) -> Option<char> {
    let Raw::Property(name, Some(value)) = raw else { return None };
    match (name.as_str(), value.as_str()) {
        ("duration", "6.000000") | ("track-list/count", "2") | ("eof-reached", "yes") => Some('a'),
        ("duration", "6.023000") | ("track-list/count", "4") => Some('b'),
        ("time-pos", position) => Some(if position.parse::<f64>().unwrap() >= 3.5 { 'a' } else { 'b' }),
        _ => None,
    }
}

#[test]
fn no_value_of_the_old_file_arrives_after_the_next_load_returns_and_no_end_before_the_file_loads() {
    let stub = stub(usize::MAX);
    let rec = recorder();
    let mut files = std::collections::HashMap::new();
    // File 'a' reaches its end about 2 s after it loads, so the replacements fall before, at and after its end.
    for delay in [0, 500, 1000, 1500, 1900, 1950, 2000, 2050, 2100, 2500] {
        let from = rec.log().len();
        files.insert(rec.load(&format!("{}/sample.mp4", stub.url), 4.0), 'a');
        rec.wait(from, |r| *r == Raw::Loaded);
        std::thread::sleep(Duration::from_millis(delay));
        let b = rec.load(&format!("{}/tracks.mkv", stub.url), 0.0);
        files.insert(b, 'b');
        let at = rec.wait(from, |r| *r == Raw::Start(b));
        rec.wait(at, |r| *r == Raw::Loaded);
    }
    let log = rec.log();
    let loads: Vec<(usize, i64)> = log
        .iter()
        .enumerate()
        .filter_map(|(at, r)| if let Raw::Loadfile(id) = r { Some((at, *id)) } else { None })
        .collect();
    for pair in loads.windows(2) {
        let ((_, old), (at, _)) = (pair[0], pair[1]);
        let until = loads.iter().find(|(next, _)| *next > at).map_or(log.len(), |(next, _)| *next);
        let late = log[at..until].iter().find(|r| file_of(r) == Some(files[&old]));
        assert!(late.is_none(), "{late:?} of entry {old} after the next loadfile in {}", show(&log));
    }
    for (at, raw) in log.iter().enumerate() {
        let Raw::Start(id) = raw else { continue };
        let loaded = at + log[at..].iter().position(|r| *r == Raw::Loaded).unwrap();
        let eof = log[at..loaded].iter().any(|r| is_property(r, "eof-reached", "yes"));
        assert!(!eof, "eof-reached of entry {id} before its file loaded in {}", show(&log));
    }
}

#[test]
fn an_hls_load_that_cannot_open_ends_three_more_entries_with_an_error() {
    // The stub answers the two manifest requests. Then the segment requests get a 401, so the HLS demuxer fails,
    // and mpv opens the manifest as a playlist of the three segments.
    let stub = stub(2);
    let rec = recorder();
    rec.load(&format!("{}/hls/index.m3u8", stub.url), 0.0);
    // The id of the manifest entry comes from its start-file, because the redirect can replace the first entry of the
    // playlist before a property read.
    rec.wait(0, |r| matches!(r, Raw::Start(_)));
    let a = rec.log().iter().find_map(|r| if let Raw::Start(id) = r { Some(*id) } else { None }).unwrap();
    // mpv documents only that the ids are unique, so the test does not compute the ids of the new entries.
    let error = |r: &Raw| matches!(r, Raw::End { reason: REASON_ERROR, .. });
    let at = rec.wait(0, error);
    let at = rec.wait(at + 1, error);
    rec.wait(at + 1, error);
    let log = rec.log();
    let ends: Vec<(i64, c_int)> =
        log.iter().filter_map(|r| if let Raw::End { id, reason } = r { Some((*id, *reason)) } else { None }).collect();
    let errors: std::collections::HashSet<i64> = ends[1..].iter().map(|(id, _)| *id).collect();
    assert_eq!(ends[0], (a, REASON_REDIRECT), "{}", show(&log));
    assert!(ends.len() == 4 && errors.len() == 3 && !errors.contains(&a), "{}", show(&log));
}
