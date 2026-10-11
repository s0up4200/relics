//! The Server settings of the active Profile, read with the API client from a fake server.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use serde_json::json;
use silo_omarchy::api::{Client, Device, Server, Tokens};
use silo_omarchy::settings::{IntroSkipMode, ServerSettings, SubtitleMode};

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
