//! The Server settings of the active Profile, read with the API client from a fake server.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use serde_json::json;
use silo_omarchy::api::{Client, Device, Server, Tokens};
use silo_omarchy::settings::{self, IntroSkipMode, LocalSettings, ServerSettings, SubtitleMode};

/// The URL and the `X-Profile-Id` header of each request.
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A fake server that answers each request with `body`, and records the URL and the profile header.
fn fake(body: serde_json::Value) -> (Client, Seen) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let http: Arc<dyn HttpClient> = FakeHttpClient::create(move |req| {
        let profile = req.headers().get("X-Profile-Id").map(|v| v.to_str().unwrap().to_string());
        log.lock().unwrap().push((req.uri().to_string(), profile));
        let body = body.to_string();
        async move {
            Ok(Response::builder()
                .status(200)
                .header("Content-Type", "application/json")
                .body(AsyncBody::from(body))
                .unwrap())
        }
    });
    let server =
        Server { id: "server-1".into(), name: "silo.test".into(), url: Url::parse("https://silo.test/").unwrap() };
    let device = Device { id: "device-1".into(), name: "box".into() };
    let client = Client::new(http, server, device, Tokens { access_token: "tok".into(), refresh_token: "ref".into() });
    client.set_profile(Some("p-owner".into()));
    (client, seen)
}

fn value(key: &str, value: serde_json::Value) -> serde_json::Value {
    json!({ "key": key, "value": value, "source": "profile", "definition_revision": 17 })
}

#[test]
fn opening_a_profile_reads_the_eight_server_settings() {
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
        client.server_settings(),
        ServerSettings {
            audio_language: Some("ja".into()),
            subtitle_language: Some("en".into()),
            subtitle_mode: SubtitleMode::Always,
            show_forced_subtitles: false,
            intro_skip_mode: IntroSkipMode::Always,
            auto_skip_credits: true,
            auto_skip_recap: true,
            auto_play_next: false,
        }
    );
    let seen = seen.lock().unwrap();
    let (url, profile) = &seen[0];
    let keys = [
        "playback.audio_language",
        "playback.subtitle_language",
        "playback.subtitle_mode",
        "playback.show_forced_subtitles",
        "playback.intro_skip_mode",
        "playback.auto_skip_credits",
        "playback.auto_skip_recap",
        "playback.auto_play_next",
    ];
    let query = keys.map(|key| format!("keys={key}")).join("&");
    assert_eq!(url, &format!("https://silo.test/api/v2/settings/values/effective?{query}"));
    assert_eq!(profile.as_deref(), Some("p-owner"));
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
    assert_eq!(client.server_settings(), ServerSettings::default());
}

#[test]
fn another_profile_starts_from_the_defaults() {
    let (client, _) = fake(json!({ "revision": 17, "items": [value("playback.subtitle_mode", json!("off"))] }));
    block_on(client.load_server_settings()).unwrap();
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Off);
    client.set_profile(Some("p-kid".into()));
    assert_eq!(client.server_settings(), ServerSettings::default());
}

/// The method, URL and body of each request.
type Writes = Arc<Mutex<Vec<(String, String, serde_json::Value)>>>;

/// A fake server that answers each write with `status`, and records the method, the URL and the body.
fn fake_write(status: u16) -> (Client, Writes) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let http: Arc<dyn HttpClient> = FakeHttpClient::create(move |mut req| {
        let log = log.clone();
        async move {
            let mut body = String::new();
            futures::AsyncReadExt::read_to_string(req.body_mut(), &mut body).await.unwrap();
            let body = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
            log.lock().unwrap().push((req.method().to_string(), req.uri().to_string(), body));
            let reply = json!({
                "key": "playback.subtitle_mode", "scope": "profile", "profile_id": "p-owner",
                "value": "off", "revision": 2,
            });
            Ok(Response::builder()
                .status(status)
                .header("Content-Type", "application/json")
                .body(AsyncBody::from(reply.to_string()))
                .unwrap())
        }
    });
    let server =
        Server { id: "server-1".into(), name: "silo.test".into(), url: Url::parse("https://silo.test/").unwrap() };
    let device = Device { id: "device-1".into(), name: "box".into() };
    let client = Client::new(http, server, device, Tokens { access_token: "tok".into(), refresh_token: "ref".into() });
    client.set_profile(Some("p-owner".into()));
    (client, seen)
}

#[test]
fn a_change_writes_at_the_profile_scope_and_the_store_keeps_it() {
    let (client, seen) = fake_write(200);
    block_on(client.set_server_setting("playback.subtitle_mode", json!("off"))).unwrap();
    assert_eq!(client.server_settings().subtitle_mode, SubtitleMode::Off);
    assert_eq!(
        seen.lock().unwrap()[0],
        (
            "PUT".to_string(),
            "https://silo.test/api/v2/settings/values/playback.subtitle_mode?scope=profile".to_string(),
            json!({ "value": "off" }),
        )
    );
}

#[test]
fn no_preference_sends_null() {
    let (client, seen) = fake_write(200);
    block_on(client.set_server_setting("playback.audio_language", json!("ja"))).unwrap();
    assert_eq!(client.server_settings().audio_language.as_deref(), Some("ja"));
    block_on(client.set_server_setting("playback.audio_language", serde_json::Value::Null)).unwrap();
    assert_eq!(client.server_settings().audio_language, None);
    assert_eq!(seen.lock().unwrap()[1].2, json!({ "value": null }));
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
    LocalSettings { hardware_decoding: false }.save(&dir).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("settings.toml")).unwrap().trim(), "hardware_decoding = false");
    assert_eq!(LocalSettings::load(&dir).hwdec(), "no");
}

#[test]
fn a_broken_settings_file_gives_the_defaults() {
    let dir = config_dir("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.toml"), "hardware_decoding = [").unwrap();
    assert_eq!(LocalSettings::load(&dir), LocalSettings::default());
}
