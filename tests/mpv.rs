//! The mpv adapter with real libmpv (`vo=null ao=null`) against a local HTTP stub on
//! 127.0.0.1. The stub serves the test video from tests/media/ (scripts/make-test-video.sh).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use silo_omarchy::api::types::{Marker, MarkerKind};
use silo_omarchy::mpv::{Player, find_controls};
use silo_omarchy::playback::Input;

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
    // The window stays, so the controller can show "Lost connection to the server".
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

fn open_with_controls() -> (Player, Receiver<Input>) {
    let (tx, rx) = mpsc::channel();
    let script = format!("{}/data/silo-controls.lua", env!("CARGO_MANIFEST_DIR"));
    let player = Player::open("no", &[("vo", "null"), ("ao", "null"), ("scripts", &script)], move |input| {
        tx.send(input).ok();
    })
    .unwrap();
    (player, rx)
}

#[test]
fn the_controller_says_that_it_is_ready() {
    let (_player, rx) = open_with_controls();
    wait_for(&rx, |input| matches!(input, Input::ControlsReady));
}

#[test]
fn the_controller_is_found_in_the_data_dirs_in_order() {
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("controls-search");
    std::fs::remove_dir_all(&root).ok();
    let script = |dir: &str| {
        let path = root.join(dir).join("silo-controls.lua");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        path
    };
    let own = script("own");
    let user = script("home/.local/share/silo-omarchy");
    let system = script("usr/share/silo-omarchy");
    std::fs::create_dir_all(root.join("empty")).unwrap();
    let dir = |d: &str| Some(root.join(d).into_os_string());
    let data_dirs = Some(format!("{0}/empty:{0}/usr/share", root.display()).into());

    let found = find_controls(|name| match name {
        "SILO_OMARCHY_DATA" => dir("own"),
        "HOME" => dir("home"),
        "XDG_DATA_DIRS" => data_dirs.clone(),
        _ => None,
    });
    assert_eq!(found, Some(own));
    // XDG_DATA_HOME is $HOME/.local/share when it is not set.
    let found = find_controls(|name| match name {
        "SILO_OMARCHY_DATA" => dir("empty"),
        "HOME" => dir("home"),
        "XDG_DATA_DIRS" => data_dirs.clone(),
        _ => None,
    });
    assert_eq!(found, Some(user));
    let found = find_controls(|name| match name {
        "XDG_DATA_HOME" => dir("empty"),
        "XDG_DATA_DIRS" => data_dirs.clone(),
        _ => None,
    });
    assert_eq!(found, Some(system));
    assert_eq!(find_controls(|name| (name == "XDG_DATA_HOME").then(|| dir("empty")).flatten()), None);
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
fn a_cycles_the_audio_track_only_for_the_file_that_plays() {
    let stub = stub(usize::MAX);
    let (player, rx) = open_with_controls();
    wait_for(&rx, |input| matches!(input, Input::ControlsReady));
    let url = format!("{}/sample.mp4", stub.url);
    player.load(&url, &headers(), 0.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    wait_for_property(&player, "aid", "1");
    // The test video has one audio track, so the next track is none.
    player.command(&["keypress", "a"]).unwrap();
    wait_for_property(&player, "aid", "no");
    player.load(&url, &headers(), 0.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    wait_for_property(&player, "aid", "1");
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
fn enter_takes_the_skip_prompt() {
    let (player, rx) = open_with_controls();
    wait_for(&rx, |input| matches!(input, Input::ControlsReady));
    player.show_prompt("Skip intro").unwrap();
    // The script message and the key reach the controller on different paths, so the key can come first.
    // Press Enter again until the controller sends the action.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        player.command(&["keypress", "ENTER"]).unwrap();
        if let Ok(Input::Action) = rx.recv_timeout(Duration::from_millis(200)) {
            return;
        }
        assert!(Instant::now() < deadline, "the controller sent no action");
    }
}

#[test]
fn seek_moves_the_position_after_the_controller_gets_the_markers() {
    let stub = stub(usize::MAX);
    let (player, rx) = open_with_controls();
    wait_for(&rx, |input| matches!(input, Input::ControlsReady));
    player.load(&format!("{}/sample.mp4", stub.url), &headers(), 0.0, None).unwrap();
    wait_for(&rx, |input| matches!(input, Input::FileLoaded));
    player.show_markers(&[Marker { kind: MarkerKind::Intro, start: -1.5, end: 2.0 }]).unwrap();
    player.seek(3.0).unwrap();
    wait_for(&rx, |input| matches!(input, Input::Position(position) if *position >= 3.0));
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
    player.select_subtitle(1).unwrap();
    wait_for_property(&player, "current-tracks/sub/lang", "eng");
    assert_eq!(player.property("track-list/count").unwrap(), "4", "no second copy of the subtitle track");
}
