//! The playback calls of the API client: the start request, the decision outcomes,
//! progress and stop, against a fake HTTP client.

mod common;

use common::{Log, fixture, json, jwt};
use futures::executor::block_on;
use relics::api::{Client, Decision, types};
use relics::playback::{Operation, Quality, Replan, Sample};
use serde_json::{Value, json};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

/// A fake Silo server at `https://silo.test/media/`. It answers the versions and capability
/// requests with server fixtures, and the start request with `start`, a fixture name.
fn fake(start: &'static str) -> (Client, Log) {
    fake_with(start, json!([]))
}

/// Like `fake`, and the effective read of the Server settings gives the items `settings`.
fn fake_with(start: &'static str, settings: Value) -> (Client, Log) {
    let (http, seen) = common::http(move |req| {
        let (status, body) = match (req.method.as_str(), req.path.as_str()) {
            ("GET", "settings/contract/capabilities") => (200, fixture("get_settings_contract_capabilities_ok.json")),
            ("GET", p) if p.starts_with("settings/values/effective?") => {
                (200, json!({ "revision": 17, "items": settings }).to_string())
            }
            ("GET", "catalog/items/movie:heat-1995/versions") => (200, fixture("list_catalog_item_versions_ok.json")),
            ("GET", "catalog/items/movie:two-files/versions") => (200, two_versions()),
            ("GET", "playback/capabilities") => (200, fixture("playback_capability_available.json")),
            ("GET", "markers/files/120") => (200, markers()),
            ("GET", "catalog/series/series:severance/seasons/1/episodes?image_size=small") => {
                (200, fixture("list_catalog_item_episodes_ok.json"))
            }
            ("GET", "catalog/series/series:severance/seasons/2/episodes?image_size=small") => (200, season_two()),
            // The server cannot transcode: it plays only the original file, and Auto picks it.
            _ if start == NO_TRANSCODE && transcodes(req) => (201, fixture(REFUSED)),
            ("POST", "playback/start") if start == NO_TRANSCODE => (201, fixture("playback_start_opaque_ids.json")),
            ("POST", "playback/start") => (201, fixture(start)),
            ("POST", "auth/refresh") => (
                200,
                json!({ "access_token": jwt(1_900_000_000), "refresh_token": "ref2", "expires_in": 28800 }).to_string(),
            ),
            ("POST", p) if p.ends_with("/replan") => (200, fixture("../own/playback_replan_failure_recovery.json")),
            ("POST", p) if p.ends_with("/progress") => (200, fixture("playback_progress_applied.json")),
            ("DELETE", _) => (200, fixture("playback_stop_completed.json")),
            _ => (404, String::new()),
        };
        json(status, body)
    });
    let client = common::client(http, "https://silo.test/media/");
    client.set_profile(Some("p-owner".into()));
    (client, seen)
}

/// The start fixture of a fake server that cannot transcode.
const NO_TRANSCODE: &str = "no transcode";
const REFUSED: &str = "../own/playback_start_adaptation_unavailable.json";

/// A start or replan request that asks for a fixed resolution.
fn transcodes(req: &common::Seen) -> bool {
    let body: Value = serde_json::from_str(&req.body).unwrap_or_default();
    req.method == "POST"
        && (req.path == "playback/start" || req.path.ends_with("/replan"))
        && !matches!(body["quality_preference"].as_str(), Some("auto" | "original"))
}

/// The Markers of file 120: two intros (a cold open and the title sequence) and the credits.
fn markers() -> String {
    let mut body: Value = serde_json::from_str(&fixture("get_file_markers_ok.json")).unwrap();
    body["marker_segments"] = json!([
        {"kind": "intro", "start_seconds": 0.0, "end_seconds": 12.0},
        {"kind": "intro", "start_seconds": 95.0, "end_seconds": 140.5},
        {"kind": "credits", "start_seconds": 2600.0, "end_seconds": 2700.0},
    ]);
    body.to_string()
}

/// The versions fixture with a second file, 121.
fn two_versions() -> String {
    let mut body: Value = serde_json::from_str(&fixture("list_catalog_item_versions_ok.json")).unwrap();
    let mut second = body["items"][0].clone();
    second["file_id"] = json!("121");
    second["resolution"] = json!("1080p");
    body["items"].as_array_mut().unwrap().push(second);
    body.to_string()
}

/// Season 2 of the fixture series: episode 1 has only an unreadable file, episode 2 plays.
fn season_two() -> String {
    let mut body: Value = serde_json::from_str(&fixture("list_catalog_item_episodes_ok.json")).unwrap();
    for (index, episode) in body["items"].as_array_mut().unwrap().iter_mut().enumerate() {
        let number = index + 1;
        episode["season_number"] = json!(2);
        episode["content_id"] = json!(format!("episode:severance-s02e0{number}"));
        episode["files"][0]["unreadable"] = json!(number == 1);
    }
    body.to_string()
}

fn body_of(seen: &Log, method: &str, suffix: &str) -> Value {
    let seen = seen.lock().unwrap();
    let request = seen.iter().find(|s| s.method == method && s.path.ends_with(suffix)).expect("no such request");
    serde_json::from_str(&request.body).unwrap()
}

#[test]
fn the_start_request_matches_the_snapshot() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    let mut body = body_of(&seen, "POST", "playback/start");
    let attempt = body["playback_attempt_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(attempt).is_ok(), "the attempt id {attempt} is not a UUID");
    body["playback_attempt_id"] = json!("<uuid>");
    body["client_playback_context"]["app_version"] = json!("<version>");
    let snapshot: Value = serde_json::from_str(&fixture("../../snapshots/start_request.json")).unwrap();
    assert_eq!(body, snapshot, "the start request changed:\n{}", serde_json::to_string_pretty(&body).unwrap());
}

#[test]
fn with_no_stored_quality_the_start_asks_for_auto_and_no_bandwidth_cap() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    let body = body_of(&seen, "POST", "playback/start");
    assert_eq!(body["quality_preference"], json!("auto"));
    assert!(body.get("bandwidth_cap_kbps").is_none(), "{body}");
}

#[test]
fn the_start_asks_for_the_effective_quality_and_bitrate_also_under_a_household_cap() {
    let capped = json!({
        "key": "playback.preferred_quality", "value": "1080p", "source": "profile", "definition_revision": 1,
        "constrained": true, "stored_value": "2160p", "permitted_values": ["auto", "480p", "720p", "1080p"],
    });
    let bitrate =
        json!({ "key": "playback.max_bitrate_kbps", "value": 6000, "source": "profile", "definition_revision": 1 });
    let (client, seen) = fake_with("playback_start_opaque_ids.json", json!([capped, bitrate]));
    block_on(client.load_server_settings()).unwrap();
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    let body = body_of(&seen, "POST", "playback/start");
    assert_eq!((&body["quality_preference"], &body["bandwidth_cap_kbps"]), (&json!("1080p"), &json!(6000)));
}

#[test]
fn the_start_request_asks_for_the_chosen_version_and_keeps_it() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
    let auto = Quality { preference: "auto".into(), bandwidth_cap_kbps: None };
    block_on(client.start_version("movie:two-files", Some("121"), None, &auto)).unwrap();
    let body = body_of(&seen, "POST", "playback/start");
    assert_eq!((&body["file_id"], &body["allow_alternate_versions"]), (&json!("121"), &json!(false)));
    // With no choice, the first version plays and the server can pick another one.
    let (client, seen) = fake("playback_start_opaque_ids.json");
    block_on(client.start_version("movie:two-files", None, None, &auto)).unwrap();
    let body = body_of(&seen, "POST", "playback/start");
    assert_eq!((&body["file_id"], body.get("allow_alternate_versions")), (&json!("120"), None));
}

#[test]
fn resume_leaves_out_the_start_position_and_start_over_sends_zero() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    assert_eq!(body_of(&seen, "POST", "playback/start").get("start_position"), None);
    let (client, seen) = fake("playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", Some(0.0))).unwrap();
    assert_eq!(body_of(&seen, "POST", "playback/start")["start_position"], json!(0.0));
}

#[test]
fn a_direct_play_decision_has_an_absolute_stream_url_below_the_base_url() {
    let (client, _) = fake("playback_start_opaque_ids.json");
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
    let (client, seen) = fake("playback_start_opaque_ids.json");
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
    let (client, _) = fake("../own/playback_start_transcode.json");
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
    let (client, _) = fake("../own/playback_start_adaptation_unavailable.json");
    let decision = block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    assert!(
        matches!(&decision, Decision::Unavailable { reason, message }
            if reason == "transcoding_disabled" && message.starts_with("The server does not allow transcoding")),
        "{decision:?}"
    );
}

/// The body of each request whose path ends with `suffix`, in order.
fn bodies(seen: &Log, suffix: &str) -> Vec<Value> {
    let seen = seen.lock().unwrap();
    seen.iter().filter(|s| s.path.ends_with(suffix)).map(|s| serde_json::from_str(&s.body).unwrap()).collect()
}

/// The quality, the bitrate cap and the start position of each start request.
fn asked(seen: &Log) -> Vec<(Value, Value, Value)> {
    let starts = bodies(seen, "playback/start");
    starts
        .iter()
        .map(|b| (b["quality_preference"].clone(), b["bandwidth_cap_kbps"].clone(), b["start_position"].clone()))
        .collect()
}

fn hd() -> Quality {
    Quality { preference: "720p".into(), bandwidth_cap_kbps: Some(4000) }
}

#[test]
fn a_fixed_resolution_that_the_server_cannot_transcode_starts_again_at_the_original_with_the_same_cap() {
    let (client, seen) = fake(NO_TRANSCODE);
    let decision = block_on(client.start_version("movie:heat-1995", None, Some(60.0), &hd())).unwrap();
    let Decision::Playable { quality_preference: quality, attempt_id, .. } = decision else { panic!("{decision:?}") };
    assert_eq!(quality.as_deref(), Some("original"));
    assert_eq!(
        asked(&seen),
        vec![(json!("720p"), json!(4000), json!(60.0)), (json!("original"), json!(4000), json!(60.0))]
    );
    let starts = bodies(&seen, "playback/start");
    assert_ne!(starts[0]["playback_attempt_id"], starts[1]["playback_attempt_id"], "the server refuses a reused id");
    assert_eq!(starts[1]["playback_attempt_id"], json!(attempt_id));
}

#[test]
fn a_refusal_of_auto_the_original_or_the_second_start_shows_the_server_message() {
    for preference in ["auto", "original"] {
        let (client, seen) = fake(REFUSED);
        let quality = Quality { preference: preference.into(), bandwidth_cap_kbps: None };
        let decision = block_on(client.start_version("movie:heat-1995", None, None, &quality)).unwrap();
        assert!(matches!(decision, Decision::Unavailable { .. }), "{decision:?}");
        assert_eq!(asked(&seen).len(), 1, "{preference} asks for no transcode");
    }
    // The server refuses `original` too, for example under a server-wide bitrate cap.
    let (client, seen) = fake(REFUSED);
    let decision = block_on(client.start_version("movie:heat-1995", None, None, &hd())).unwrap();
    assert!(matches!(decision, Decision::Unavailable { .. }), "{decision:?}");
    assert_eq!(asked(&seen).len(), 2);
}

/// Starts the fixture session at Auto on a server that cannot transcode, and asks for a replan at 720p.
fn replan_at_hd(operation: Operation) -> (Decision, Log) {
    let (client, seen) = fake(NO_TRANSCODE);
    let Decision::Playable { attempt_id, .. } = block_on(client.start_playback("movie:heat-1995", None)).unwrap()
    else {
        panic!("not playable");
    };
    let replan = Replan {
        session_id: SESSION.into(),
        attempt_id,
        failed_plan_id: "plan:478677870860e5e5108c18bff749b34b".into(),
        plan_attempt_key: "v3:f0144c47fa349e3e".into(),
        attempted_plan_keys: vec![],
        selected_tracks: json!({}),
        position: 1800.0,
        operation,
        quality: hd(),
    };
    (block_on(client.replan(&replan)).unwrap(), seen)
}

#[test]
fn a_recovery_that_the_server_cannot_transcode_starts_again_at_the_original_at_the_position() {
    let (decision, seen) = replan_at_hd(Operation::FailureRecovery);
    assert!(
        matches!(&decision, Decision::Playable { quality_preference: Some(q), .. } if q == "original"),
        "{decision:?}"
    );
    assert_eq!(asked(&seen)[1..], [(json!("original"), json!(4000), json!(1800.0))]);
    assert!(body_of(&seen, "DELETE", SESSION)["stop_id"].is_string(), "the old Playback session stops");
}

#[test]
fn a_quality_pick_that_the_server_cannot_transcode_does_not_start_again() {
    let (decision, seen) = replan_at_hd(Operation::QualityChange);
    assert!(matches!(decision, Decision::Unavailable { .. }), "{decision:?}");
    assert_eq!(asked(&seen).len(), 1, "only the first start");
    assert!(!seen.lock().unwrap().iter().any(|s| s.method == "DELETE"), "the old plan keeps playing");
}

#[test]
fn an_unknown_outcome_decodes_as_unknown() {
    let (client, _) = fake("../own/playback_start_unknown_outcome.json");
    let decision = block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    assert!(
        matches!(&decision, Decision::Unknown(outcome) if outcome == "outcome_from_a_newer_server"),
        "{decision:?}"
    );
}

#[test]
fn progress_and_stop_send_the_installation_id_and_the_sample() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
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
fn a_refresh_gives_media_headers_with_the_new_token_and_its_expiry() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
    assert_eq!(client.media_access().expires, None, "the token of the fake is no JWT");
    let access = block_on(client.refresh_access()).unwrap();
    assert!(access.headers.contains(&("Authorization".into(), format!("Bearer {}", jwt(1_900_000_000)))));
    assert_eq!(access.expires, Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_000)));
    assert_eq!(body_of(&seen, "POST", "auth/refresh")["refresh_token"], json!("ref"));
}

#[test]
fn a_replan_sends_the_failed_plan_and_the_last_good_position() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
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
        operation: Operation::FailureRecovery,
        quality: Quality { preference: "auto".into(), bandwidth_cap_kbps: None },
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
fn a_quality_step_replans_with_its_label_and_no_failure() {
    let (client, seen) = fake("playback_start_opaque_ids.json");
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    let replan = Replan {
        session_id: SESSION.into(),
        attempt_id: "attempt".into(),
        failed_plan_id: "plan:478677870860e5e5108c18bff749b34b".into(),
        plan_attempt_key: "v3:f0144c47fa349e3e".into(),
        attempted_plan_keys: vec![],
        selected_tracks: json!({}),
        position: 1800.0,
        operation: Operation::QualityChange,
        quality: Quality { preference: "1080p-medium".into(), bandwidth_cap_kbps: Some(4000) },
    };
    block_on(client.replan(&replan)).unwrap();
    let body = body_of(&seen, "POST", "/replan");
    assert_eq!(body["operation"], json!("quality_change"));
    assert_eq!((&body["quality_preference"], &body["bandwidth_cap_kbps"]), (&json!("1080p-medium"), &json!(4000)));
    assert_eq!(body["position_seconds"], json!(1800.0));
    assert_eq!((&body["attempted_plan_keys"], &body["attempt_count"]), (&json!([]), &json!(1)));
    assert!(body.get("failure").is_none(), "nothing failed");
    assert_eq!(body_of(&seen, "POST", "playback/start")["quality_preference"], json!("auto"));
}

#[test]
fn media_headers_carry_the_token_and_the_profile() {
    let (client, _) = fake("playback_start_opaque_ids.json");
    let headers = client.media_access().headers;
    assert!(headers.contains(&("Authorization".into(), "Bearer acc".into())));
    assert!(headers.contains(&("X-Profile-Id".into(), "p-owner".into())));
    assert!(headers.contains(&("X-Silo-Device-Id".into(), "6c1f0d3e-0000-4000-8000-000000000001".into())));
}

#[test]
fn the_decision_carries_each_marker_of_the_file() {
    let (client, _) = fake("playback_start_opaque_ids.json");
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
    let (client, _) = fake("playback_start_opaque_ids.json");
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

fn previous(season: i64, content_id: &str) -> Option<String> {
    let (client, _) = fake("playback_start_opaque_ids.json");
    block_on(client.previous_episode("series:severance", season, content_id)).unwrap().map(|episode| episode.content_id)
}

#[test]
fn the_previous_episode_is_the_one_before_in_the_season() {
    assert_eq!(previous(1, "episode:severance-s01e02").as_deref(), Some("episode:severance-s01e01"));
}

#[test]
fn before_the_first_playable_episode_of_a_season_the_previous_episode_is_the_last_one_of_the_season_before() {
    // Episode 1 of season 2 has only an unreadable file.
    assert_eq!(previous(2, "episode:severance-s02e02").as_deref(), Some("episode:severance-s01e02"));
}

#[test]
fn the_first_episode_of_season_one_has_no_previous_episode() {
    // Season 0 holds the specials, so the lookup does not go there.
    assert_eq!(previous(1, "episode:severance-s01e01"), None);
}
