//! The Server settings of the active Profile, read with the API client from a fake server.

mod common;

use common::{Log, json};
use futures::executor::block_on;
use relics::api::Client;
use relics::settings::{self, IntroSkipMode, LocalSettings, Origin, ServerSettings, SubtitleAppearance, SubtitleMode};
use relics::settings_page::Row;
use serde_json::json;

const CAPABILITIES: &str = "settings/contract/capabilities";

/// The settings contract capabilities of a server at `revision`.
fn capabilities(revision: u32) -> String {
    let mut body: serde_json::Value =
        serde_json::from_str(&common::fixture("get_settings_contract_capabilities_ok.json")).unwrap();
    body["manifest_revision"] = revision.into();
    body.to_string()
}

/// A fake server at `revision` of the settings contract. It answers each other request with `body`.
fn fake_at(revision: u32, body: serde_json::Value) -> (Client, Log) {
    common::fake(move |request| {
        if request.path == CAPABILITIES { json(200, capabilities(revision)) } else { json(200, body.to_string()) }
    })
}

/// A fake server whose capability read fails. It answers each other request with 404.
fn fake_without_capabilities() -> (Client, Log) {
    common::fake(|request| if request.path == CAPABILITIES { json(500, String::new()) } else { common::not_found() })
}

/// A fake server at revision 17 whose capabilities have the `changed` fields. It accepts each write, and it
/// answers each other request with no settings.
fn fake_capabilities(changed: serde_json::Value) -> (Client, Log) {
    common::fake(move |request| {
        if request.path == CAPABILITIES {
            let mut body: serde_json::Value = serde_json::from_str(&capabilities(17)).unwrap();
            for (field, value) in changed.as_object().unwrap() {
                body[field] = value.clone();
            }
            json(200, body.to_string())
        } else if request.method == "PUT" {
            common::ok("update_setting_value_ok.json")
        } else {
            json(200, json!({ "revision": 17, "items": [] }).to_string())
        }
    })
}

/// A fake server at revision 17 of the settings contract.
fn fake(body: serde_json::Value) -> (Client, Log) {
    fake_at(17, body)
}

/// The keys that an effective read asks for.
fn asked_keys(seen: &common::Seen) -> Vec<String> {
    let query = seen.url.split_once('?').map_or("", |(_, query)| query);
    query.split('&').filter_map(|pair| pair.strip_prefix("keys=")).map(String::from).collect()
}

fn value(key: &str, value: serde_json::Value) -> serde_json::Value {
    json!({ "key": key, "value": value, "source": "profile", "definition_revision": 17 })
}

#[test]
fn opening_a_profile_reads_the_server_settings() {
    let (client, seen) = fake(json!({
        "revision": 17,
        "items": [
            value("playback.audio_language", json!("ja")),
            value("playback.subtitle_language", json!("en")),
            value("playback.subtitle_mode", json!("always")),
            value("playback.show_forced_subtitles", json!(false)),
            value("playback.intro_skip_mode", json!("always")),
            value("playback.auto_skip_credits", json!(true)),
            value("playback.auto_skip_recap", json!(true)),
            value("playback.auto_play_next", json!(false)),
        ],
    }));
    assert_eq!(client.server_settings(), ServerSettings::default());
    block_on(client.load_server_settings()).unwrap();
    assert_eq!(
        ServerSettings { origins: Default::default(), ..client.server_settings() },
        ServerSettings {
            audio_language: Some("ja".into()),
            subtitle_language: Some("en".into()),
            subtitle_mode: SubtitleMode::Always,
            show_forced_subtitles: false,
            intro_skip_mode: IntroSkipMode::Always,
            auto_skip_credits: true,
            auto_skip_recap: true,
            auto_play_next: false,
            subtitle_appearance: SubtitleAppearance::default(),
            manifest_revision: Some(17),
            origins: Default::default(),
        }
    );
    let seen = seen.lock().unwrap();
    let (url, profile) = (&seen[1].url, seen[1].header("X-Profile-Id"));
    let keys = [
        "playback.intro_skip_mode",
        "playback.auto_skip_recap",
        "playback.auto_skip_credits",
        "playback.auto_play_next",
        "playback.audio_language",
        "playback.subtitle_language",
        "playback.subtitle_mode",
        "playback.show_forced_subtitles",
        "playback.subtitle_appearance",
    ];
    let query = keys.map(|key| format!("keys={key}")).join("&");
    assert_eq!(url, &format!("https://silo.test/api/v2/settings/values/effective?{query}"));
    assert_eq!(profile, Some("p-owner"));
}

#[test]
fn a_missing_key_or_a_value_of_another_type_keeps_the_default() {
    let (client, _) = fake(json!({
        "revision": 17,
        "items": [
            value("playback.subtitle_mode", json!("from_a_newer_server")),
            value("playback.auto_play_next", json!("yes")),
            value("playback.audio_language", json!(null)),
        ],
    }));
    block_on(client.load_server_settings()).unwrap();
    let settings = ServerSettings { origins: Default::default(), ..client.server_settings() };
    assert_eq!(settings, ServerSettings { manifest_revision: Some(17), ..ServerSettings::default() });
}

#[test]
fn the_source_and_the_cap_fields_reach_the_store() {
    let (client, _) = fake(json!({
        "revision": 17,
        "items": [
            { "key": "playback.auto_skip_recap", "value": true, "source": "profile_device", "definition_revision": 1 },
            {
                "key": "playback.subtitle_mode", "value": "auto", "source": "profile", "definition_revision": 1,
                "constrained": true, "stored_value": "always", "permitted_values": ["auto", "off"],
            },
        ],
    }));
    block_on(client.load_server_settings()).unwrap();
    let origins = client.server_settings().origins;
    assert_eq!(origins["playback.auto_skip_recap"], Origin { source: "profile_device".into(), ..Origin::default() });
    assert_eq!(
        origins["playback.subtitle_mode"],
        Origin {
            source: "profile".into(),
            constrained: true,
            stored_value: Some(json!("always")),
            permitted_values: vec![json!("auto"), json!("off")],
        }
    );
    assert!(!origins.contains_key("playback.audio_language"), "a key that the server did not send has no origin");
}

#[test]
fn the_capability_read_comes_before_the_effective_read() {
    let (client, seen) = fake(json!({ "revision": 17, "items": [] }));
    block_on(client.load_server_settings()).unwrap();
    let paths: Vec<_> = seen.lock().unwrap().iter().map(|request| request.path.clone()).collect();
    assert_eq!(paths[0], CAPABILITIES);
    assert!(paths[1].starts_with("settings/values/effective?"), "{paths:?}");
    assert_eq!(paths.len(), 2);
}

#[test]
fn a_server_before_revision_7_is_not_asked_for_the_intro_skip_mode() {
    let (client, seen) = fake_at(
        6,
        json!({
            "revision": 6,
            "items": [
                value("playback.audio_language", json!("ja")),
                value("playback.subtitle_language", json!("en")),
                value("playback.subtitle_mode", json!("always")),
                value("playback.show_forced_subtitles", json!(false)),
                value("playback.auto_skip_credits", json!(true)),
                value("playback.auto_skip_recap", json!(true)),
                value("playback.auto_play_next", json!(false)),
            ],
        }),
    );
    block_on(client.load_server_settings()).unwrap();
    assert_eq!(
        ServerSettings { origins: Default::default(), ..client.server_settings() },
        ServerSettings {
            audio_language: Some("ja".into()),
            subtitle_language: Some("en".into()),
            subtitle_mode: SubtitleMode::Always,
            show_forced_subtitles: false,
            intro_skip_mode: IntroSkipMode::Ask,
            auto_skip_credits: true,
            auto_skip_recap: true,
            auto_play_next: false,
            subtitle_appearance: SubtitleAppearance::default(),
            manifest_revision: Some(6),
            origins: Default::default(),
        }
    );
    let asked = asked_keys(&seen.lock().unwrap()[1]);
    assert_eq!(asked.len(), 8, "{asked:?}");
    assert!(!asked.iter().any(|key| key == "playback.intro_skip_mode"), "{asked:?}");
    let shown: Vec<_> = relics::settings_page::sections(Some(&client.server_settings()), 0)
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .collect();
    assert!(
        shown.iter().any(|row| matches!(row, Row::Server(definition) if definition.key == "playback.auto_skip_recap"))
    );
    assert!(
        !shown.iter().any(|row| matches!(row, Row::Server(definition) if definition.key == "playback.intro_skip_mode"))
    );
}

#[test]
fn a_key_that_the_server_does_not_know_is_not_written() {
    let (client, seen) = fake_at(6, json!({ "revision": 6, "items": [] }));
    block_on(client.load_server_settings()).unwrap();
    // A Settings page from before the capability read can still show the row.
    let err = block_on(client.set_server_setting("playback.intro_skip_mode", json!("always"))).unwrap_err();
    assert_eq!(err.to_string(), "Relics cannot reach the settings of the Server");
    assert!(!seen.lock().unwrap().iter().any(|request| request.method == "PUT"), "the client wrote an unknown key");
}

#[test]
fn a_new_revision_applies_to_writes_also_when_the_effective_read_fails() {
    use std::sync::atomic::{AtomicU32, Ordering};
    // The server goes back from revision 17 to revision 6, and its effective read then fails.
    let revision = std::sync::Arc::new(AtomicU32::new(17));
    let server = revision.clone();
    let (client, seen) = common::fake(move |request| match server.load(Ordering::SeqCst) {
        revision if request.path == CAPABILITIES => json(200, capabilities(revision)),
        17 => json(200, json!({ "revision": 17, "items": [] }).to_string()),
        _ => json(500, String::new()),
    });
    block_on(client.load_server_settings()).unwrap();
    revision.store(6, Ordering::SeqCst);
    assert!(block_on(client.load_server_settings()).is_err());
    assert!(block_on(client.set_server_setting("playback.intro_skip_mode", json!("always"))).is_err());
    assert!(!seen.lock().unwrap().iter().any(|request| request.method == "PUT"), "the client wrote an unknown key");
}

/// Two loads of one Profile overlap. The first capability read gives revision 17, and the second one fails.
/// `hold` is the path of the reply of the first load that comes last: its capability read or its effective read.
fn overlapping_loads(hold: &'static str) -> Log {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use futures::channel::oneshot;
    use http_client::FakeHttpClient;

    let (release, held) = oneshot::channel::<()>();
    let held = Arc::new(Mutex::new(Some(held)));
    let capability_reads = Arc::new(AtomicUsize::new(0));
    let seen: Log = Arc::default();
    let log = seen.clone();
    let http = FakeHttpClient::create(move |req| {
        let path = req.uri().path().trim_start_matches("/api/v2/").to_string();
        log.lock().unwrap().push(common::Seen {
            method: req.method().to_string(),
            url: req.uri().to_string(),
            path: path.clone(),
            headers: Vec::new(),
            body: String::new(),
        });
        let first = if path == CAPABILITIES { capability_reads.fetch_add(1, Ordering::SeqCst) == 0 } else { true };
        let reply = match path.as_str() {
            CAPABILITIES if first => (200, capabilities(17)),
            CAPABILITIES => (500, String::new()),
            _ if hold != CAPABILITIES => (200, json!({ "revision": 17, "items": [] }).to_string()),
            _ => (500, String::new()),
        };
        let wait = (first && path.starts_with(hold)).then(|| held.lock().unwrap().take()).flatten();
        Box::pin(async move {
            if let Some(wait) = wait {
                wait.await.unwrap();
            }
            Ok(common::json(reply.0, reply.1).unwrap())
        }) as futures::future::BoxFuture<'static, _>
    });
    let client = common::client(http, "https://silo.test/");
    client.set_profile(Some("p-owner".into()));
    block_on(async {
        let mut first = Box::pin(client.load_server_settings());
        assert!(futures::poll!(first.as_mut()).is_pending());
        client.load_server_settings().await.unwrap();
        release.send(()).unwrap();
        first.await.ok();
    });
    block_on(client.set_server_setting("playback.intro_skip_mode", json!("always"))).ok();
    seen
}

#[test]
fn an_older_effective_read_does_not_undo_a_newer_failed_capability_read() {
    let seen = overlapping_loads("settings/values/effective");
    assert!(!seen.lock().unwrap().iter().any(|request| request.method == "PUT"), "the client wrote");
}

#[test]
fn an_older_capability_read_does_not_undo_a_newer_failed_one() {
    let seen = overlapping_loads(CAPABILITIES);
    assert!(!seen.lock().unwrap().iter().any(|request| request.method == "PUT"), "the client wrote");
}

#[test]
fn a_server_at_revision_17_is_asked_for_each_key() {
    let (client, seen) = fake(json!({ "revision": 17, "items": [] }));
    block_on(client.load_server_settings()).unwrap();
    let asked = asked_keys(&seen.lock().unwrap()[1]);
    let mut all: Vec<_> = settings::DEFINITIONS.iter().map(|definition| definition.key.to_string()).collect();
    all.dedup();
    assert_eq!(asked, all);
}

#[test]
fn without_the_capabilities_the_rows_stay_and_each_change_is_refused() {
    let (client, seen) = fake_without_capabilities();
    block_on(client.load_server_settings()).unwrap();
    let settings = client.server_settings();
    assert_eq!(settings.manifest_revision, None);
    let rows = relics::settings_page::sections(Some(&settings), 0).into_iter().flat_map(|(_, rows)| rows);
    assert_eq!(rows.filter(|row| matches!(row, Row::Server(_))).count(), settings::DEFINITIONS.len());
    let err = block_on(client.set_server_setting("playback.auto_play_next", json!(false))).unwrap_err();
    assert_eq!(err.to_string(), "Relics cannot reach the settings of the Server");
    assert!(client.server_settings().auto_play_next);
    let paths: Vec<_> = seen.lock().unwrap().iter().map(|request| request.path.clone()).collect();
    assert_eq!(paths, [CAPABILITIES], "the client read or wrote without the capabilities");
}

#[test]
fn another_api_version_an_unavailable_state_or_a_refusal_is_like_a_failed_capability_read() {
    for changed in [
        json!({ "api_version": 2 }),
        json!({ "state": "disabled" }),
        json!({ "state": "not_configured" }),
        json!({ "state": "unsupported" }),
        json!({ "state": "from_a_newer_server" }),
        json!({ "allowed": false }),
    ] {
        let (client, seen) = fake_capabilities(changed.clone());
        block_on(client.load_server_settings()).unwrap();
        assert_eq!(client.server_settings().manifest_revision, None, "{changed}");
        let err = block_on(client.set_server_setting("playback.auto_play_next", json!(false))).unwrap_err();
        assert_eq!(err.to_string(), "Relics cannot reach the settings of the Server", "{changed}");
        let paths: Vec<_> = seen.lock().unwrap().iter().map(|request| request.path.clone()).collect();
        assert_eq!(paths, [CAPABILITIES], "{changed}: the client read or wrote the settings");
    }
}

#[test]
fn without_batched_reads_the_keys_after_revision_1_are_unknown() {
    let (client, seen) = fake_capabilities(json!({ "supports_batched_effective": false }));
    block_on(client.load_server_settings()).unwrap();
    let asked = asked_keys(&seen.lock().unwrap()[1]);
    assert_eq!(asked.len(), 8, "{asked:?}");
    assert!(!asked.iter().any(|key| key == "playback.intro_skip_mode"), "{asked:?}");
    let shown = relics::settings_page::sections(Some(&client.server_settings()), 0);
    let rows: Vec<_> = shown.into_iter().flat_map(|(_, rows)| rows).collect();
    assert!(
        !rows.iter().any(|row| matches!(row, Row::Server(definition) if definition.key == "playback.intro_skip_mode"))
    );
    block_on(client.set_server_setting("playback.auto_play_next", json!(false))).unwrap();
}

#[test]
fn each_read_reads_the_capabilities_again() {
    use std::sync::atomic::{AtomicU32, Ordering};
    // The server goes back from revision 17 to revision 6, and then its capability read fails.
    let revision = std::sync::Arc::new(AtomicU32::new(17));
    let server = revision.clone();
    let (client, seen) = common::fake(move |request| match server.load(Ordering::SeqCst) {
        0 if request.path == CAPABILITIES => json(500, String::new()),
        revision if request.path == CAPABILITIES => json(200, capabilities(revision)),
        _ if request.method == "PUT" => common::ok("update_setting_value_ok.json"),
        _ => json(200, json!({ "revision": 17, "items": [] }).to_string()),
    });
    block_on(client.load_server_settings()).unwrap();
    assert_eq!(client.server_settings().manifest_revision, Some(17));
    revision.store(6, Ordering::SeqCst);
    block_on(client.load_server_settings()).unwrap();
    assert_eq!(client.server_settings().manifest_revision, Some(6));
    assert!(!asked_keys(seen.lock().unwrap().last().unwrap()).iter().any(|key| key == "playback.intro_skip_mode"));
    revision.store(0, Ordering::SeqCst);
    block_on(client.load_server_settings()).unwrap();
    assert!(block_on(client.set_server_setting("playback.auto_play_next", json!(false))).is_err());
    let capability_reads = seen.lock().unwrap().iter().filter(|request| request.path == CAPABILITIES).count();
    assert_eq!(capability_reads, 3);
    assert!(!seen.lock().unwrap().iter().any(|request| request.method == "PUT"));
}

#[test]
fn another_profile_starts_from_the_defaults() {
    let (client, _) = fake(json!({ "revision": 17, "items": [value("playback.subtitle_mode", json!("off"))] }));
    block_on(client.load_server_settings()).unwrap();
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Off);
    client.set_profile(Some("p-kid".into()));
    assert_eq!(client.server_settings(), ServerSettings::default());
}

#[test]
fn an_old_profiles_write_does_not_discard_the_new_profiles_settings() {
    use std::sync::{Arc, Mutex};

    use futures::channel::oneshot;
    use http_client::FakeHttpClient;

    let (finish_write, write) = oneshot::channel::<()>();
    let (finish_read, read) = oneshot::channel::<()>();
    let waits = Arc::new(Mutex::new((Some(write), Some(read))));
    let http = FakeHttpClient::create(move |req| {
        let writing = req.method() == "PUT";
        // The read of the old Profile, and each capability read, answer at once.
        let at_once = req.uri().path().ends_with("contract/capabilities")
            || req.headers().get("X-Profile-Id").is_some_and(|id| id == "p-owner");
        if at_once && !writing {
            let body = if req.uri().path().ends_with("contract/capabilities") {
                capabilities(17)
            } else {
                json!({"revision": 17, "items": []}).to_string()
            };
            return Box::pin(async move { Ok(common::json(200, body).unwrap()) })
                as futures::future::BoxFuture<'static, _>;
        }
        let wait = {
            let mut waits = waits.lock().unwrap();
            if writing { waits.0.take().unwrap() } else { waits.1.take().unwrap() }
        };
        Box::pin(async move {
            wait.await.unwrap();
            let body = if writing {
                common::fixture("update_setting_value_ok.json")
            } else {
                json!({"revision": 17, "items": [value("playback.subtitle_mode", json!("always"))]}).to_string()
            };
            Ok(common::json(200, body).unwrap())
        })
    });
    let client = common::client(http, "https://silo.test/");
    client.set_profile(Some("p-owner".into()));
    block_on(client.load_server_settings()).unwrap();
    block_on(async {
        let mut write = Box::pin(client.set_server_setting("playback.subtitle_mode", json!("off")));
        assert!(futures::poll!(write.as_mut()).is_pending());
        client.set_profile(Some("p-kid".into()));
        let mut read = Box::pin(client.load_server_settings());
        assert!(futures::poll!(read.as_mut()).is_pending());
        finish_write.send(()).unwrap();
        write.await.unwrap();
        finish_read.send(()).unwrap();
        read.await.unwrap();
    });
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Always);
}

/// A fake server at revision 17 that answers each write with `status`. The client has read the settings.
fn fake_write(status: u16) -> (Client, Log) {
    let (client, seen) = common::fake(move |request| match request.method.as_str() {
        "PUT" => {
            let reply = json!({
                "key": "playback.subtitle_mode", "scope": "profile", "profile_id": "p-owner",
                "value": "off", "revision": 2,
            });
            json(status, reply.to_string())
        }
        _ if request.path == CAPABILITIES => json(200, capabilities(17)),
        _ => json(200, json!({ "revision": 17, "items": [] }).to_string()),
    });
    block_on(client.load_server_settings()).unwrap();
    seen.lock().unwrap().clear();
    (client, seen)
}

/// The JSON body of a request.
fn body(seen: &common::Seen) -> serde_json::Value {
    serde_json::from_str(&seen.body).unwrap_or(serde_json::Value::Null)
}

#[test]
fn a_change_writes_at_the_profile_scope_and_the_store_keeps_it() {
    let (client, seen) = fake_write(200);
    block_on(client.set_server_setting("playback.subtitle_mode", json!("off"))).unwrap();
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Off);
    assert_eq!(
        seen.lock().unwrap().iter().map(|req| (req.method.as_str(), req.url.as_str(), body(req))).next().unwrap(),
        (
            "PUT",
            "https://silo.test/api/v2/settings/values/playback.subtitle_mode?scope=profile",
            json!({ "value": "off" }),
        )
    );
}

/// A fake server at revision 17 whose Effective value of the subtitle mode is "off" from the `source` row. It
/// accepts each write, and answers each delete with `delete_status`. After the first read it reads "always" from the
/// profile row. The client has read the settings.
fn fake_source(source: &'static str, delete_status: u16) -> (Client, Log) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let read = AtomicBool::new(false);
    let (client, seen) = common::fake(move |request| match request.method.as_str() {
        "PUT" => common::ok("update_setting_value_ok.json"),
        "DELETE" => json(delete_status, String::new()),
        _ if request.path == CAPABILITIES => json(200, capabilities(17)),
        _ => {
            let item = if read.swap(true, Ordering::SeqCst) {
                value("playback.subtitle_mode", json!("always"))
            } else {
                json!({ "key": "playback.subtitle_mode", "value": "off", "source": source, "definition_revision": 1 })
            };
            json(200, json!({ "revision": 17, "items": [item] }).to_string())
        }
    });
    block_on(client.load_server_settings()).unwrap();
    seen.lock().unwrap().clear();
    (client, seen)
}

/// The method and the path of each request.
fn requests(seen: &Log) -> Vec<(String, String)> {
    seen.lock().unwrap().iter().map(|req| (req.method.clone(), req.path.clone())).collect()
}

#[test]
fn a_save_over_a_device_value_deletes_the_device_row_of_this_device() {
    let (client, seen) = fake_source("profile_device", 204);
    block_on(client.set_server_setting("playback.subtitle_mode", json!("always"))).unwrap();
    assert_eq!(
        requests(&seen),
        [
            ("PUT".into(), "settings/values/playback.subtitle_mode?scope=profile".into()),
            ("DELETE".into(), "settings/values/playback.subtitle_mode?scope=profile_device".into()),
        ]
    );
    assert_eq!(seen.lock().unwrap()[1].header("X-Silo-Device-Id"), Some(common::DEVICE_ID));
    let settings = client.server_settings();
    assert_eq!(settings.subtitle_mode, SubtitleMode::Always);
    assert!(!settings.origins["playback.subtitle_mode"].is_device(), "the row still says Set for this device");
}

#[test]
fn a_save_over_a_profile_value_only_writes_the_profile_row() {
    let (client, seen) = fake_source("profile", 204);
    block_on(client.set_server_setting("playback.subtitle_mode", json!("always"))).unwrap();
    assert_eq!(requests(&seen), [("PUT".into(), "settings/values/playback.subtitle_mode?scope=profile".into())]);
}

#[test]
fn a_device_row_that_is_already_gone_is_a_success() {
    let (client, seen) = fake_source("profile_device", 404);
    block_on(client.set_server_setting("playback.subtitle_mode", json!("always"))).unwrap();
    assert_eq!(requests(&seen).len(), 2);
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Always);
}

#[test]
fn a_profile_switch_during_the_write_keeps_the_device_row_of_the_new_profile() {
    use std::sync::{Arc, Mutex};

    use futures::channel::oneshot;
    use http_client::FakeHttpClient;

    let (finish_write, write) = oneshot::channel::<()>();
    let write = Mutex::new(Some(write));
    let deletes = Arc::new(Mutex::new(Vec::new()));
    let http = FakeHttpClient::create({
        let deletes = deletes.clone();
        move |req| {
            let body = match req.method().as_str() {
                "PUT" => {
                    let write = write.lock().unwrap().take().unwrap();
                    return Box::pin(async move {
                        write.await.unwrap();
                        Ok(common::json(200, common::fixture("update_setting_value_ok.json")).unwrap())
                    }) as futures::future::BoxFuture<'static, _>;
                }
                "DELETE" => {
                    deletes.lock().unwrap().push(req.headers().get("X-Profile-Id").cloned());
                    String::new()
                }
                _ if req.uri().path().ends_with("contract/capabilities") => capabilities(17),
                _ => {
                    let item = json!({
                        "key": "playback.subtitle_mode", "value": "off", "source": "profile_device",
                        "definition_revision": 1,
                    });
                    json!({ "revision": 17, "items": [item] }).to_string()
                }
            };
            Box::pin(async move { Ok(common::json(200, body).unwrap()) })
        }
    });
    let client = common::client(http, "https://silo.test/");
    client.set_profile(Some("p-owner".into()));
    block_on(client.load_server_settings()).unwrap();
    block_on(async {
        let mut write = Box::pin(client.set_server_setting("playback.subtitle_mode", json!("always")));
        assert!(futures::poll!(write.as_mut()).is_pending());
        client.set_profile(Some("p-kid".into()));
        finish_write.send(()).unwrap();
        write.await.unwrap();
    });
    let deletes = deletes.lock().unwrap();
    assert!(deletes.is_empty(), "the write deleted a device row: {deletes:?}");
}

#[test]
fn a_profile_switch_during_a_refresh_sends_no_retry_for_the_new_profile() {
    use std::sync::{Arc, Mutex};

    use futures::channel::oneshot;
    use http_client::FakeHttpClient;

    let (finish_refresh, refresh) = oneshot::channel::<()>();
    let refresh = Mutex::new(Some(refresh));
    let deletes = Arc::new(Mutex::new(Vec::new()));
    let http = FakeHttpClient::create({
        let deletes = deletes.clone();
        move |req| {
            let reply = match req.method().as_str() {
                _ if req.uri().path().ends_with("auth/refresh") => {
                    let refresh = refresh.lock().unwrap().take().unwrap();
                    return Box::pin(async move {
                        refresh.await.unwrap();
                        let tokens = json!({
                            "access_token": common::jwt(1_900_000_000), "refresh_token": "ref2", "expires_in": 28800,
                        });
                        Ok(common::json(200, tokens.to_string()).unwrap())
                    }) as futures::future::BoxFuture<'static, _>;
                }
                "PUT" => common::ok("update_setting_value_ok.json"),
                "DELETE" => {
                    deletes.lock().unwrap().push(req.headers().get("X-Profile-Id").cloned());
                    common::problem(401, "token_refresh_required.json")
                }
                _ if req.uri().path().ends_with("contract/capabilities") => json(200, capabilities(17)),
                _ => {
                    let item = json!({
                        "key": "playback.subtitle_mode", "value": "off", "source": "profile_device",
                        "definition_revision": 1,
                    });
                    json(200, json!({ "revision": 17, "items": [item] }).to_string())
                }
            };
            Box::pin(async move { Ok(reply.unwrap()) })
        }
    });
    let client = common::client(http, "https://silo.test/");
    client.set_profile(Some("p-owner".into()));
    block_on(client.load_server_settings()).unwrap();
    block_on(async {
        let mut write = Box::pin(client.set_server_setting("playback.subtitle_mode", json!("always")));
        assert!(futures::poll!(write.as_mut()).is_pending());
        client.set_profile(Some("p-kid".into()));
        finish_refresh.send(()).unwrap();
        assert!(write.await.is_err());
    });
    let deletes = deletes.lock().unwrap();
    assert_eq!(deletes.len(), 1, "the retry went out for the new Profile: {deletes:?}");
}

#[test]
fn a_failed_delete_is_a_failed_save_and_reads_the_settings_again() {
    let (client, seen) = fake_source("profile_device", 500);
    assert!(block_on(client.set_server_setting("playback.subtitle_mode", json!("auto"))).is_err());
    let paths: Vec<_> = requests(&seen).into_iter().map(|(_, path)| path).collect();
    assert_eq!(paths[2], CAPABILITIES, "{paths:?}");
    assert!(paths[3].starts_with("settings/values/effective?"), "{paths:?}");
    // The read gives the value that really applies.
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Always);
}

#[test]
fn no_preference_sends_null() {
    let (client, seen) = fake_write(200);
    block_on(client.set_server_setting("playback.audio_language", json!("ja"))).unwrap();
    assert_eq!(client.server_settings().audio_language.as_deref(), Some("ja"));
    block_on(client.set_server_setting("playback.audio_language", serde_json::Value::Null)).unwrap();
    assert_eq!(client.server_settings().audio_language, None);
    assert_eq!(body(&seen.lock().unwrap()[1]), json!({ "value": null }));
}

#[test]
fn a_failed_write_keeps_the_old_value() {
    let (client, _) = fake_write(500);
    assert!(block_on(client.set_server_setting("playback.auto_play_next", json!(false))).is_err());
    assert!(client.server_settings().auto_play_next);
}

#[test]
fn the_language_list_is_the_web_list_after_no_preference() {
    let options = settings::options("playback.subtitle_language", &serde_json::Value::Null);
    assert_eq!(options.len(), 47);
    assert_eq!(options[0], (serde_json::Value::Null, "No preference".to_string()));
    assert_eq!(options[1], (json!("ar"), "Arabic".to_string()));
    assert_eq!(options[46], (json!("vi"), "Vietnamese".to_string()));
    assert!(options.contains(&(json!("pt-BR"), "Brazilian Portuguese".to_string())));
}

#[test]
fn a_stored_language_outside_the_list_stays_a_choice() {
    let options = settings::options("playback.audio_language", &json!("x-silo-original"));
    assert_eq!(options.len(), 48);
    assert_eq!(options[1], (json!("x-silo-original"), "Original language".to_string()));
}

#[test]
fn each_value_reads_back_as_the_value_that_was_set() {
    let mut store = ServerSettings::default();
    for (key, value) in [
        ("playback.audio_language", json!("ja")),
        ("playback.subtitle_language", json!("en-GB")),
        ("playback.subtitle_mode", json!("always")),
        ("playback.show_forced_subtitles", json!(false)),
        ("playback.intro_skip_mode", json!("never")),
        ("playback.auto_skip_credits", json!(true)),
        ("playback.auto_skip_recap", json!(true)),
        ("playback.auto_play_next", json!(false)),
    ] {
        store.set(key, &value);
        assert_eq!(store.value(key), value, "{key}");
    }
}

fn config_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("local-settings-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn hardware_decoding_is_auto_safe_until_the_user_turns_it_off() {
    let dir = config_dir("default");
    assert_eq!(LocalSettings::load(&dir).hwdec(), "auto-safe");
    LocalSettings { hardware_decoding: false, ..LocalSettings::default() }.save(&dir).unwrap();
    assert!(std::fs::read_to_string(dir.join("settings.toml")).unwrap().contains("hardware_decoding = false\n"));
    assert_eq!(LocalSettings::load(&dir).hwdec(), "no");
}

#[test]
fn the_release_check_is_on_until_the_user_turns_it_off() {
    let dir = config_dir("releases");
    std::fs::create_dir_all(&dir).unwrap();
    // A settings file from a build before the release check.
    std::fs::write(dir.join("settings.toml"), "hardware_decoding = false\n").unwrap();
    assert!(LocalSettings::load(&dir).check_releases);
    LocalSettings { check_releases: false, ..LocalSettings::default() }.save(&dir).unwrap();
    assert!(!LocalSettings::load(&dir).check_releases);
}

#[test]
fn a_broken_settings_file_gives_the_defaults() {
    let dir = config_dir("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.toml"), "hardware_decoding = [").unwrap();
    assert_eq!(LocalSettings::load(&dir), LocalSettings::default());
}

struct PageWindow(gpui::Entity<relics::pages::Pages>);
impl gpui::Render for PageWindow {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        gpui_omarchy::focus_scope("test-pages").size_full().child(self.0.clone())
    }
}

#[gpui::test]
fn a_settings_page_in_the_back_history_keeps_the_changes_of_a_newer_one(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let dir = config_dir("stacked");
    // Only this test in this file reads the Local settings through XDG_CONFIG_HOME.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    // The effective read fails, so the page shows no Server rows.
    let (client, _) =
        common::fake(
            |request| {
                if request.path == CAPABILITIES { json(200, capabilities(17)) } else { common::not_found() }
            },
        );
    let client = std::sync::Arc::new(client);
    let (_, cx) = cx.add_window_view(|window, cx| {
        PageWindow(cx.new(|cx| relics::pages::Pages::new(client, "Test".into(), window, cx)))
    });
    // A Settings page, and a second one on top of it. The second one turns the release check off.
    cx.simulate_keystrokes(", , j enter");
    cx.run_until_parked();
    assert!(!LocalSettings::load(&dir.join("relics")).check_releases);
    // Back to the first page, which turns hardware decoding off.
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    let saved = LocalSettings::load(&dir.join("relics"));
    assert!(!saved.hardware_decoding);
    assert!(!saved.check_releases, "the older page wrote back its stale release setting");
}

#[gpui::test]
fn the_focus_stays_on_its_row_when_the_server_settings_appear(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        common::isolate(cx)
    });
    // The first read of the Server settings fails, and the retry reads them.
    let ready = std::sync::Arc::new(AtomicBool::new(false));
    let answer = ready.clone();
    let (client, seen) = common::fake(move |request| {
        if request.path == CAPABILITIES {
            json(200, capabilities(17))
        } else if request.path.starts_with("settings/values/effective") && answer.load(Ordering::SeqCst) {
            json(200, json!({ "revision": 17, "items": [] }).to_string())
        } else {
            common::not_found()
        }
    });
    let client = std::sync::Arc::new(client);
    let (_, cx) = cx.add_window_view(|window, cx| {
        PageWindow(cx.new(|cx| relics::pages::Pages::new(client, "Test".into(), window, cx)))
    });
    // Without the Server rows, j focuses the release check. The Server rows then appear above it.
    cx.simulate_keystrokes(", j");
    cx.run_until_parked();
    ready.store(true, Ordering::SeqCst);
    cx.simulate_keystrokes("r");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!LocalSettings::load(&dir).check_releases, "Enter did not act on the release check");
    assert!(!seen.lock().unwrap().iter().any(|request| request.method == "PUT"), "Enter changed a Server setting");
}

#[gpui::test]
fn without_the_capabilities_a_change_on_the_page_sends_no_write(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        common::isolate(cx)
    });
    let (client, seen) = fake_without_capabilities();
    let client = std::sync::Arc::new(client);
    let (_, cx) = cx.add_window_view(|window, cx| {
        PageWindow(cx.new(|cx| relics::pages::Pages::new(client, "Test".into(), window, cx)))
    });
    // The Server rows show with their current values. l changes the first one, Skip intros.
    cx.simulate_keystrokes(",");
    cx.run_until_parked();
    cx.simulate_keystrokes("l");
    cx.run_until_parked();
    let seen = seen.lock().unwrap();
    assert!(!seen.iter().any(|request| request.method == "PUT"), "the page wrote without the capabilities");
    assert!(!seen.iter().any(|request| request.path.starts_with("settings/values")), "the page read without them");
}

#[test]
fn a_sparse_subtitle_appearance_merges_over_the_defaults() {
    let (client, _) = fake(json!({
        "revision": 17,
        "items": [{
            "key": settings::APPEARANCE, "source": "profile_device", "definition_revision": 1,
            "value": { "fontSize": "small", "fontColor": "#facc15", "backgroundOpacity": 73, "position": "nowhere" },
        }],
    }));
    block_on(client.load_server_settings()).unwrap();
    assert_eq!(
        client.server_settings().subtitle_appearance,
        SubtitleAppearance {
            font_size: "small".into(),
            font_color: "#facc15".into(),
            background_opacity: 73,
            ..SubtitleAppearance::default()
        }
    );
    assert_eq!(
        SubtitleAppearance::default(),
        SubtitleAppearance {
            font_size: "large".into(),
            font_family: "sans-serif".into(),
            font_color: "#ffffff".into(),
            text_opacity: 100,
            background_style: "box".into(),
            background_color: "#000000".into(),
            background_opacity: 75,
            text_outline: false,
            text_outline_color: "#000000".into(),
            position: "bottom".into(),
        }
    );
}

#[test]
fn a_font_family_from_another_client_reads_as_sans_serif() {
    let mut store = ServerSettings::default();
    store.set(settings::APPEARANCE, &json!({ "fontFamily": "Helvetica Neue", "fontSize": "xxlarge" }));
    assert_eq!(store.subtitle_appearance.font_family, "sans-serif");
    assert_eq!(store.subtitle_appearance.font_size, "xxlarge");
}

#[test]
fn a_color_that_another_client_stored_in_capitals_is_its_swatch() {
    let mut store = ServerSettings::default();
    store.set(settings::APPEARANCE, &json!({ "fontColor": "#FACC15" }));
    let color = appearance_row("fontColor");
    assert_eq!(store.row_value(color), json!("#facc15"));
    assert_eq!(color.options(&store.row_value(color)).len(), 9, "no extra choice next to Yellow");
}

/// The definition of the Subtitle appearance row of `field`.
fn appearance_row(field: &str) -> &'static settings::Definition {
    settings::DEFINITIONS.iter().find(|definition| definition.field == Some(field)).unwrap()
}

#[test]
fn a_subtitle_appearance_save_writes_the_whole_value_for_this_device_and_deletes_no_row() {
    let (client, seen) = common::fake(|request| match request.method.as_str() {
        "PUT" => common::ok("update_setting_value_ok.json"),
        _ if request.path == CAPABILITIES => json(200, capabilities(17)),
        _ => {
            let item = json!({
                "key": settings::APPEARANCE, "source": "profile_device", "definition_revision": 1,
                "value": { "fontSize": "small" },
            });
            json(200, json!({ "revision": 17, "items": [item] }).to_string())
        }
    });
    block_on(client.load_server_settings()).unwrap();
    seen.lock().unwrap().clear();
    let mut store = client.server_settings();
    store.set_row(appearance_row("position"), json!("top"));
    block_on(client.set_server_setting(settings::APPEARANCE, store.value(settings::APPEARANCE))).unwrap();
    assert_eq!(
        requests(&seen),
        [("PUT".into(), format!("settings/values/{}?scope=profile_device", settings::APPEARANCE))]
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].header("X-Silo-Device-Id"), Some(common::DEVICE_ID));
    assert_eq!(
        body(&seen[0]),
        json!({ "value": {
            "fontSize": "small", "fontFamily": "sans-serif", "fontColor": "#ffffff", "textOpacity": 100,
            "backgroundStyle": "box", "backgroundColor": "#000000", "backgroundOpacity": 75, "textOutline": false,
            "textOutlineColor": "#000000", "position": "top",
        }})
    );
    let settings = client.server_settings();
    assert_eq!(settings.subtitle_appearance.position, "top");
    assert!(settings.origins[settings::APPEARANCE].is_device());
}

#[test]
fn before_revision_14_the_text_opacity_row_hides() {
    let (client, _) = fake_at(13, json!({ "revision": 13, "items": [] }));
    block_on(client.load_server_settings()).unwrap();
    let store = client.server_settings();
    assert!(!store.knows(appearance_row("textOpacity")));
    assert!(store.knows(appearance_row("fontSize")));
}

#[test]
fn the_subtitle_appearance_choices_keep_a_stored_value_outside_the_list() {
    let labels = |field: &str, current: serde_json::Value| -> Vec<String> {
        appearance_row(field).options(&current).into_iter().map(|(_, label)| label).collect()
    };
    assert_eq!(labels("fontSize", json!("large")), ["Small", "Medium", "Large", "X-Large", "XX-Large"]);
    assert_eq!(labels("backgroundStyle", json!("box")), ["Box", "Drop shadow", "Outline", "None"]);
    assert_eq!(labels("position", json!("top")), ["Bottom", "Lower third", "Top"]);
    assert_eq!(labels("backgroundColor", json!("#000000")), ["Black", "Dark Gray", "Navy", "Dark Red", "Dark Green"]);
    assert_eq!(labels("fontColor", json!("#123456"))[0], "#123456");
    assert_eq!(labels("textOutlineColor", json!("#ffffff")).len(), 9);
    let text = labels("textOpacity", json!(100));
    assert_eq!((text.len(), text[0].as_str(), text[9].as_str()), (10, "10%", "100%"));
    let back = labels("backgroundOpacity", json!(73));
    assert_eq!(back.len(), 12);
    assert_eq!(back[0], "0%");
    assert_eq!(back[7..10], ["70%", "73%", "80%"]);
    assert!(appearance_row("backgroundOpacity").allows(&json!(73)));
    assert!(!appearance_row("textOpacity").allows(&json!(0)));
    assert!(appearance_row("fontColor").allows(&json!("#AbCdEf")));
    assert!(!appearance_row("fontColor").allows(&json!("red")));
}

#[gpui::test]
fn a_slow_subtitle_appearance_save_does_not_undo_a_newer_change(cx: &mut gpui::TestAppContext) {
    use std::sync::{Arc, Mutex};

    use futures::channel::oneshot;
    use gpui::AppContext as _;
    use http_client::FakeHttpClient;
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
        common::isolate(cx)
    });
    // The server holds the first save until the test lets it go. Like a real server, it ends a save that it got
    // also when the client stops waiting. It keeps each saved value in the order that the saves end, so the last
    // one is the value that the server has.
    let (release, held) = oneshot::channel::<()>();
    let held = Mutex::new(Some(held));
    let saved = Arc::new(Mutex::new(Vec::new()));
    let server = Arc::new(Mutex::new(None));
    let http = FakeHttpClient::create({
        let (saved, server) = (saved.clone(), server.clone());
        move |req| {
            let (saved, server) = (saved.clone(), server.clone());
            let method = req.method().to_string();
            let path = req.uri().path().to_string();
            let held = if method == "PUT" { held.lock().unwrap().take() } else { None };
            Box::pin(async move {
                use futures::AsyncReadExt as _;
                let mut body = String::new();
                req.into_body().read_to_string(&mut body).await.unwrap();
                let reply = if method == "PUT" {
                    let value = serde_json::from_str::<serde_json::Value>(&body).unwrap()["value"].clone();
                    match held {
                        Some(held) => {
                            let (done, ended) = oneshot::channel();
                            *server.lock().unwrap() = Some(Box::pin(async move {
                                held.await.ok();
                                saved.lock().unwrap().push(value);
                                done.send(()).ok();
                            }) as futures::future::BoxFuture<'static, ()>);
                            ended.await.ok();
                        }
                        None => saved.lock().unwrap().push(value),
                    }
                    common::fixture("update_setting_value_ok.json")
                } else if path.ends_with("contract/capabilities") {
                    capabilities(17)
                } else {
                    json!({ "revision": 17, "items": [] }).to_string()
                };
                Ok(common::json(200, reply).unwrap())
            }) as futures::future::BoxFuture<'static, _>
        }
    });
    let client = Arc::new(common::client(http, "https://silo.test/"));
    client.set_profile(Some("p-owner".into()));
    let (_, cx) = cx.add_window_view(|window, cx| {
        PageWindow(cx.new(|cx| relics::pages::Pages::new(client, "Test".into(), window, cx)))
    });
    cx.simulate_keystrokes(",");
    cx.run_until_parked();
    // Font size is the ninth row. Its save waits at the server while the position changes.
    cx.simulate_keystrokes("j j j j j j j j l");
    cx.run_until_parked();
    cx.simulate_keystrokes("j j j j j j j j j l");
    cx.run_until_parked();
    release.send(()).unwrap();
    futures::executor::block_on(server.lock().unwrap().take().unwrap());
    cx.run_until_parked();
    let saved = saved.lock().unwrap();
    let last = saved.last().unwrap();
    assert_eq!((&last["fontSize"], &last["position"]), (&json!("xlarge"), &json!("lower-third")), "{saved:?}");
}
