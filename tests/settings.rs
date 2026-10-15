//! The Server settings of the active Profile, read with the API client from a fake server.

mod common;

use common::{Log, json};
use futures::executor::block_on;
use relics::api::Client;
use relics::settings::{self, IntroSkipMode, LocalSettings, ServerSettings, SubtitleMode};
use serde_json::json;

/// A fake server that answers each request with `body`.
fn fake(body: serde_json::Value) -> (Client, Log) {
    common::fake(move |_| json(200, body.to_string()))
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
    let (url, profile) = (&seen[0].url, seen[0].header("X-Profile-Id"));
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
/// A fake server that answers each write with `status`.
fn fake_write(status: u16) -> (Client, Log) {
    common::fake(move |_| {
        let reply = json!({
            "key": "playback.subtitle_mode", "scope": "profile", "profile_id": "p-owner",
            "value": "off", "revision": 2,
        });
        json(status, reply.to_string())
    })
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
    let (client, _) = common::fake(|_| common::not_found());
    let client = std::sync::Arc::new(client);
    let (_, cx) = cx.add_window_view(|window, cx| {
        PageWindow(cx.new(|cx| relics::pages::Pages::new(client, "Test".into(), window, cx)))
    });
    // A Settings page, and a second one on top of it. The second one turns the release check off.
    cx.simulate_keystrokes(", , j j j j j j j j j enter");
    cx.run_until_parked();
    assert!(!LocalSettings::load(&dir.join("relics")).check_releases);
    // Back to the first page, which turns hardware decoding off.
    cx.simulate_keystrokes("escape j j j j j j j j enter");
    cx.run_until_parked();
    let saved = LocalSettings::load(&dir.join("relics"));
    assert!(!saved.hardware_decoding);
    assert!(!saved.check_releases, "the older page wrote back its stale release setting");
}
