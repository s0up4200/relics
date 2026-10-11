//! The playback calls of the API client: the start request, the decision outcomes,
//! progress and stop, against a fake HTTP client.

use std::sync::{Arc, Mutex};

use futures::AsyncReadExt as _;
use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use serde_json::{Value, json};
use silo_omarchy::api::{Client, Decision, Device, Server, Tokens, types};
use silo_omarchy::playback::{Replan, Sample};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: String,
}

/// A fake Silo server at `https://silo.test/media/`. It answers the versions and capability
/// requests with server fixtures, and the start request with `start`, a fixture path.
fn fake(start: &'static str) -> (Client, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let http: Arc<dyn HttpClient> = FakeHttpClient::create(move |req| {
        let log = log.clone();
        async move {
            let method = req.method().to_string();
            let path = req.uri().to_string().strip_prefix("https://silo.test/media/api/v2/").unwrap().to_string();
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            log.lock().unwrap().push(Seen { method: method.clone(), path: path.clone(), body });
            let (status, body) = match (method.as_str(), path.as_str()) {
                ("GET", "catalog/items/movie:heat-1995/versions") => {
                    (200, fixture("silo/list_catalog_item_versions_ok.json"))
                }
                ("GET", "playback/capabilities") => (200, fixture("silo/playback_capability_available.json")),
                ("GET", "markers/files/120") => (200, markers()),
                ("GET", "catalog/series/series:severance/seasons/1/episodes") => {
                    (200, fixture("silo/list_catalog_item_episodes_ok.json"))
                }
                ("GET", "catalog/series/series:severance/seasons/2/episodes") => (200, season_two()),
                ("POST", "playback/start") => (201, fixture(start)),
                ("POST", "auth/refresh") => (
                    200,
                    json!({ "access_token": jwt(1_900_000_000), "refresh_token": "ref2", "expires_in": 28800 })
                        .to_string(),
                ),
                ("POST", p) if p.ends_with("/replan") => (200, fixture("own/playback_replan_failure_recovery.json")),
                ("POST", p) if p.ends_with("/progress") => (200, fixture("silo/playback_progress_applied.json")),
                ("DELETE", _) => (200, fixture("silo/playback_stop_completed.json")),
                _ => (404, String::new()),
            };
            Ok(Response::builder()
                .status(status)
                .header("Content-Type", "application/json")
                .body(AsyncBody::from(body))
                .unwrap())
        }
    });
    let server = Server {
        id: "server-1".into(),
        name: "silo.test".into(),
        url: Url::parse("https://silo.test/media/").unwrap(),
    };
    let device = Device { id: "6c1f0d3e-0000-4000-8000-000000000001".into(), name: "omarchy-box".into() };
    let client = Client::new(http, server, device, Tokens { access_token: "tok".into(), refresh_token: "ref".into() });
    client.set_profile(Some("p-owner".into()));
    (client, seen)
}

/// The Markers of file 120: two intros (a cold open and the title sequence) and the credits.
fn markers() -> String {
    let mut body: Value = serde_json::from_str(&fixture("silo/get_file_markers_ok.json")).unwrap();
    body["marker_segments"] = json!([
        {"kind": "intro", "start_seconds": 0.0, "end_seconds": 12.0},
        {"kind": "intro", "start_seconds": 95.0, "end_seconds": 140.5},
        {"kind": "credits", "start_seconds": 2600.0, "end_seconds": 2700.0},
    ]);
    body.to_string()
}

/// Season 2 of the fixture series: episode 1 has only an unreadable file, episode 2 plays.
fn season_two() -> String {
    let mut body: Value = serde_json::from_str(&fixture("silo/list_catalog_item_episodes_ok.json")).unwrap();
    for (index, episode) in body["items"].as_array_mut().unwrap().iter_mut().enumerate() {
        let number = index + 1;
        episode["season_number"] = json!(2);
        episode["content_id"] = json!(format!("episode:severance-s02e0{number}"));
        episode["files"][0]["unreadable"] = json!(number == 1);
    }
    body.to_string()
}

fn body_of(seen: &Arc<Mutex<Vec<Seen>>>, method: &str, suffix: &str) -> Value {
    let seen = seen.lock().unwrap();
    let request = seen.iter().find(|s| s.method == method && s.path.ends_with(suffix)).expect("no such request");
    serde_json::from_str(&request.body).unwrap()
}

#[test]
fn the_start_request_matches_the_snapshot() {
    let (client, seen) = fake("silo/playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    let mut body = body_of(&seen, "POST", "playback/start");
    let attempt = body["playback_attempt_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(attempt).is_ok(), "the attempt id {attempt} is not a UUID");
    body["playback_attempt_id"] = json!("<uuid>");
    body["client_playback_context"]["app_version"] = json!("<version>");
    let snapshot: Value = serde_json::from_str(&fixture("../snapshots/start_request.json")).unwrap();
    assert_eq!(body, snapshot, "the start request changed:\n{}", serde_json::to_string_pretty(&body).unwrap());
}

#[test]
fn resume_leaves_out_the_start_position_and_start_over_sends_zero() {
    let (client, seen) = fake("silo/playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    assert_eq!(body_of(&seen, "POST", "playback/start").get("start_position"), None);
    let (client, seen) = fake("silo/playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", Some(0.0))).unwrap();
    assert_eq!(body_of(&seen, "POST", "playback/start")["start_position"], json!(0.0));
}

#[test]
fn a_direct_play_decision_has_an_absolute_stream_url_below_the_base_url() {
    let (client, _) = fake("silo/playback_start_opaque_ids.json");
    let Decision::Playable { session_id, plan, .. } = block_on(client.start_playback("movie:heat-1995", None)).unwrap()
    else {
        panic!("not playable");
    };
    assert_eq!(session_id, SESSION);
    assert_eq!(types::Delivery::from(plan.delivery), types::Delivery::OriginalHttp);
    assert_eq!(plan.stream.url, format!("https://silo.test/media/api/v2/stream/{SESSION}"));
}

#[test]
fn the_server_picks_the_audio_track_and_the_decision_carries_its_language() {
    let (client, seen) = fake("silo/playback_start_opaque_ids.json");
    let Decision::Playable { audio_language, plan, .. } =
        block_on(client.start_playback("movie:heat-1995", None)).unwrap()
    else {
        panic!("not playable");
    };
    let body = body_of(&seen, "POST", "playback/start");
    assert_eq!((body.get("audio_track_id"), body.get("audio_track_index")), (None, None));
    // The version has no audio track list, so the language is the effective audio language of the version.
    assert_eq!(audio_language.as_deref(), Some("en"));
    let url = plan.subtitle.inventory[0].url.as_deref().unwrap();
    assert!(url.starts_with(&format!("https://silo.test/media/api/v2/stream/{SESSION}/subtitles/0.vtt?")), "{url}");
}

#[test]
fn a_transcode_decision_plays_the_hls_manifest() {
    let (client, _) = fake("own/playback_start_transcode.json");
    let Decision::Playable { plan, .. } = block_on(client.start_playback("movie:heat-1995", None)).unwrap() else {
        panic!("not playable");
    };
    assert_eq!(types::Delivery::from(plan.delivery), types::Delivery::ServerTranscodeHls);
    assert_eq!(types::StreamProtocol::from(plan.stream.protocol), types::StreamProtocol::Hls);
    assert_eq!(plan.stream.url, format!("https://silo.test/media/api/v2/playback/transcode/{SESSION}/master.m3u8"));
    assert_eq!(plan.timeline.timeline_offset_seconds, 600.0);
}

#[test]
fn an_adaptation_unavailable_decision_carries_the_server_message() {
    let (client, _) = fake("own/playback_start_adaptation_unavailable.json");
    let decision = block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    assert!(
        matches!(&decision, Decision::Unavailable { reason, message }
            if reason == "transcoding_disabled" && message.starts_with("The server does not allow transcoding")),
        "{decision:?}"
    );
}

#[test]
fn an_unknown_outcome_decodes_as_unknown() {
    let (client, _) = fake("own/playback_start_unknown_outcome.json");
    let decision = block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    assert!(
        matches!(&decision, Decision::Unknown(outcome) if outcome == "outcome_from_a_newer_server"),
        "{decision:?}"
    );
}

#[test]
fn progress_and_stop_send_the_installation_id_and_the_sample() {
    let (client, seen) = fake("silo/playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    let sample = Sample { sequence: 3, position: 120.5, paused: true };
    block_on(client.report_progress(SESSION, &sample)).unwrap();
    assert_eq!(
        body_of(&seen, "POST", "/progress"),
        json!({
            "installation_id": "11111111-1111-4111-8111-111111111111",
            "sequence": 3,
            "position": 120.5,
            "is_paused": true,
        })
    );
    block_on(client.stop_playback(SESSION, Some(&Sample { sequence: 4, ..sample }))).unwrap();
    let stop = body_of(&seen, "DELETE", SESSION);
    assert!(uuid::Uuid::parse_str(stop["stop_id"].as_str().unwrap()).is_ok());
    assert_eq!(stop["installation_id"], json!("11111111-1111-4111-8111-111111111111"));
    assert_eq!((&stop["sequence"], &stop["position"], &stop["is_paused"]), (&json!(4), &json!(120.5), &json!(true)));
    let paths: Vec<String> = seen.lock().unwrap().iter().map(|s| format!("{} {}", s.method, s.path)).collect();
    assert!(!paths.iter().any(|p| p.contains("watched")), "the app never marks an Item watched: {paths:?}");
}

/// A JWT whose payload has this `exp`. The signature is not checked.
fn jwt(exp: u64) -> String {
    use base64::Engine as _;
    let part = |json: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.to_string());
    format!("{}.{}.sig", part(json!({ "alg": "HS256" })), part(json!({ "sub": "u1", "exp": exp })))
}

#[test]
fn a_refresh_gives_media_headers_with_the_new_token_and_its_expiry() {
    let (client, seen) = fake("silo/playback_start_opaque_ids.json");
    assert_eq!(client.media_access().expires, None, "the token of the fake is no JWT");
    let access = block_on(client.refresh_access()).unwrap();
    assert!(access.headers.contains(&("Authorization".into(), format!("Bearer {}", jwt(1_900_000_000)))));
    assert_eq!(access.expires, Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_000)));
    assert_eq!(body_of(&seen, "POST", "auth/refresh")["refresh_token"], json!("ref"));
}

#[test]
fn a_replan_sends_the_failed_plan_and_the_last_good_position() {
    let (client, seen) = fake("silo/playback_start_opaque_ids.json");
    let Decision::Playable { attempt_id, .. } = block_on(client.start_playback("movie:heat-1995", None)).unwrap()
    else {
        panic!("not playable");
    };
    let replan = Replan {
        session_id: SESSION.into(),
        attempt_id: attempt_id.clone(),
        failed_plan_id: "plan:478677870860e5e5108c18bff749b34b".into(),
        plan_attempt_key: "v3:f0144c47fa349e3e".into(),
        attempted_plan_keys: vec!["v3:f0144c47fa349e3e".into()],
        selected_tracks: json!({ "audio": { "id": "file:42:audio:0", "index": 0 } }),
        position: 3600.0,
    };
    let Decision::Playable { plan, .. } = block_on(client.replan(&replan)).unwrap() else {
        panic!("not playable");
    };
    assert_eq!(plan.stream.url, format!("https://silo.test/media/api/v2/playback/transcode/{SESSION}/master.m3u8"));
    let body = body_of(&seen, "POST", "/replan");
    assert_eq!(body["playback_attempt_id"], json!(attempt_id));
    assert_eq!(body_of(&seen, "POST", "playback/start")["playback_attempt_id"], json!(attempt_id));
    assert_eq!(body["operation"], json!("failure_recovery"));
    assert_eq!(body["failed_plan_id"], json!("plan:478677870860e5e5108c18bff749b34b"));
    assert_eq!(body["plan_attempt_key"], json!("v3:f0144c47fa349e3e"));
    assert_eq!(body["attempted_plan_keys"], json!(["v3:f0144c47fa349e3e"]));
    assert_eq!(body["attempt_count"], json!(1));
    assert_eq!(body["position_seconds"], json!(3600.0));
    assert_eq!(body["installation_id"], json!("11111111-1111-4111-8111-111111111111"));
    assert_eq!(body["selected_tracks"], replan.selected_tracks);
    assert!(body["failure"]["classification"].is_string());
    let start = body_of(&seen, "POST", "playback/start");
    assert_eq!(body["client_capabilities"], start["client_capabilities"]);
    assert_eq!(body["client_features"], start["client_features"]);
}

#[test]
fn media_headers_carry_the_token_and_the_profile() {
    let (client, _) = fake("silo/playback_start_opaque_ids.json");
    let headers = client.media_access().headers;
    assert!(headers.contains(&("Authorization".into(), "Bearer tok".into())));
    assert!(headers.contains(&("X-Profile-Id".into(), "p-owner".into())));
    assert!(headers.contains(&("X-Silo-Device-Id".into(), "6c1f0d3e-0000-4000-8000-000000000001".into())));
}

#[test]
fn the_decision_carries_each_marker_of_the_file() {
    let (client, _) = fake("silo/playback_start_opaque_ids.json");
    let Decision::Playable { markers, .. } = block_on(client.start_playback("movie:heat-1995", None)).unwrap() else {
        panic!("not playable");
    };
    let marker = |kind, start, end| types::Marker { kind, start, end };
    assert_eq!(
        markers,
        vec![
            marker(types::MarkerKind::Intro, 0.0, 12.0),
            marker(types::MarkerKind::Intro, 95.0, 140.5),
            marker(types::MarkerKind::Credits, 2600.0, 2700.0),
        ]
    );
}

fn next(season: i64, content_id: &str) -> Option<String> {
    let (client, _) = fake("silo/playback_start_opaque_ids.json");
    block_on(client.next_episode("series:severance", season, content_id)).unwrap().map(|episode| episode.content_id)
}

#[test]
fn the_next_episode_is_the_next_one_in_the_season() {
    assert_eq!(next(1, "episode:severance-s01e01").as_deref(), Some("episode:severance-s01e02"));
}

#[test]
fn after_the_last_episode_of_a_season_the_next_episode_is_the_first_playable_one_of_the_next_season() {
    assert_eq!(next(1, "episode:severance-s01e02").as_deref(), Some("episode:severance-s02e02"));
}

#[test]
fn the_last_episode_of_the_series_has_no_next_episode() {
    // The server has no season 3.
    assert_eq!(next(2, "episode:severance-s02e02"), None);
}
