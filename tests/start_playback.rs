//! The playback calls of the API client: the start request, the decision outcomes,
//! progress and stop, against a fake HTTP client.

use std::sync::{Arc, Mutex};

use futures::AsyncReadExt as _;
use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use serde_json::{Value, json};
use silo_omarchy::api::{Client, Decision, Device, Server, Tokens, types};
use silo_omarchy::playback::Sample;

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
                ("POST", "playback/start") => (201, fixture(start)),
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
    let Decision::Playable { session_id, plan } = block_on(client.start_playback("movie:heat-1995", None)).unwrap()
    else {
        panic!("not playable");
    };
    assert_eq!(session_id, SESSION);
    assert_eq!(types::Delivery::from(plan.delivery), types::Delivery::OriginalHttp);
    assert_eq!(plan.stream.url, format!("https://silo.test/media/api/v2/stream/{SESSION}"));
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

#[test]
fn media_headers_carry_the_token_and_the_profile() {
    let (client, _) = fake("silo/playback_start_opaque_ids.json");
    let headers = client.media_headers();
    assert!(headers.contains(&("Authorization".into(), "Bearer tok".into())));
    assert!(headers.contains(&("X-Profile-Id".into(), "p-owner".into())));
    assert!(headers.contains(&("X-Silo-Device-Id".into(), "6c1f0d3e-0000-4000-8000-000000000001".into())));
}
