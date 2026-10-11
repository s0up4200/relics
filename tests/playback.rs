//! The Playback session: feed inputs and the time to `step`, and check the commands.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use silo_omarchy::api::types::Decision;
use silo_omarchy::playback::{Command, Headers, Input, Sample, Session};
use silo_omarchy::settings::{ServerSettings, SubtitleMode};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
}

/// The plan of the server fixture: original_http, source duration 7200 s, start at 12.5 s.
fn decision(edit: impl FnOnce(&mut Value)) -> Decision {
    let path = format!("{}/tests/fixtures/silo/playback_start_opaque_ids.json", env!("CARGO_MANIFEST_DIR"));
    let mut body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    edit(&mut body);
    Decision::from(serde_json::from_value::<silo_omarchy::api::wire::PlaybackDecision>(body).unwrap())
}

fn headers() -> Headers {
    vec![("Authorization".into(), "Bearer tok".into()), ("X-Profile-Id".into(), "p-owner".into())]
}

/// A session whose file is loaded at time 0. `runtime` is the Item runtime in seconds.
fn playing(runtime: Option<f64>) -> Session {
    let mut session = Session::new(runtime, ServerSettings::default());
    session.step(Input::Started { decision: decision(|_| {}), headers: headers() }, at(0));
    session.step(Input::FileLoaded, at(0));
    session
}

fn stop(position: f64, sequence: u64) -> Command {
    Command::Stop { session_id: SESSION.into(), sample: Some(Sample { sequence, position, paused: false }) }
}

#[test]
fn a_playable_decision_loads_the_stream_with_the_headers_at_the_plan_start() {
    let mut session = Session::new(None, ServerSettings::default());
    let commands = session.step(Input::Started { decision: decision(|_| {}), headers: headers() }, at(0));
    assert_eq!(
        commands,
        vec![Command::Load { url: format!("/api/v2/stream/{SESSION}"), headers: headers(), start: 12.5 }]
    );
}

#[test]
fn progress_goes_to_the_server_every_ten_seconds() {
    let mut session = playing(None);
    session.step(Input::Position(30.0), at(1));
    assert_eq!(session.step(Input::Tick, at(9)), vec![]);
    assert_eq!(
        session.step(Input::Tick, at(10)),
        vec![Command::Progress {
            session_id: SESSION.into(),
            sample: Sample { sequence: 1, position: 30.0, paused: false },
        }]
    );
    session.step(Input::Position(40.0), at(12));
    session.step(Input::Paused(true), at(13));
    assert_eq!(session.step(Input::Tick, at(15)), vec![]);
    assert_eq!(
        session.step(Input::Tick, at(20)),
        vec![Command::Progress {
            session_id: SESSION.into(),
            sample: Sample { sequence: 2, position: 40.0, paused: true },
        }]
    );
}

#[test]
fn no_progress_goes_before_the_file_is_loaded() {
    let mut session = Session::new(None, ServerSettings::default());
    session.step(Input::Started { decision: decision(|_| {}), headers: headers() }, at(0));
    // A position from the file that this Item replaced.
    session.step(Input::Position(900.0), at(1));
    assert_eq!(session.step(Input::Tick, at(30)), vec![]);
}

#[test]
fn the_position_in_a_report_is_the_source_position() {
    let mut session = Session::new(None, ServerSettings::default());
    let decision = decision(|body| body["playback_plan"]["timeline"]["timeline_offset_seconds"] = 600.into());
    session.step(Input::Started { decision, headers: headers() }, at(0));
    session.step(Input::FileLoaded, at(0));
    session.step(Input::Position(30.0), at(1));
    assert_eq!(session.step(Input::Shutdown, at(2)), vec![stop(630.0, 1), Command::ClosePlayer]);
}

#[test]
fn a_closed_window_sends_the_stop_report_and_closes_the_player() {
    let mut session = playing(None);
    session.step(Input::Position(300.0), at(1));
    assert_eq!(session.step(Input::Shutdown, at(2)), vec![stop(300.0, 1), Command::ClosePlayer]);
    assert_eq!(session.step(Input::Tick, at(60)), vec![], "an ended session sends nothing more");
}

#[test]
fn the_app_ends_the_session_with_a_stop_report_and_keeps_the_player() {
    let mut session = playing(None);
    session.step(Input::Position(300.0), at(1));
    assert_eq!(session.step(Input::End, at(2)), vec![stop(300.0, 1)]);
}

#[test]
fn a_normal_end_within_eight_seconds_sends_the_duration() {
    let mut session = playing(None);
    session.step(Input::Duration(7200.0), at(0));
    session.step(Input::Position(7192.5), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(7200.0, 1), Command::ClosePlayer]);
}

#[test]
fn a_normal_end_at_98_point_5_percent_sends_the_duration() {
    let mut session = playing(None);
    session.step(Input::Duration(1000.0), at(0));
    session.step(Input::Position(985.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(1000.0, 1), Command::ClosePlayer]);
}

#[test]
fn an_early_end_sends_only_the_last_position() {
    let mut session = playing(None);
    session.step(Input::Duration(7200.0), at(0));
    session.step(Input::Position(3600.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(3600.0, 1), Command::ClosePlayer]);
}

#[test]
fn without_an_mpv_duration_the_end_rule_uses_the_item_runtime() {
    let mut session = playing(Some(7200.0));
    session.step(Input::Position(7195.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(7200.0, 1), Command::ClosePlayer]);
    let mut session = playing(None);
    session.step(Input::Position(7195.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(7195.0, 1), Command::ClosePlayer]);
}

#[test]
fn a_load_failure_sends_the_stop_report_and_shows_an_error() {
    let mut session = playing(None);
    assert_eq!(
        session.step(Input::LoadFailed, at(1)),
        vec![
            Command::Stop { session_id: SESSION.into(), sample: None },
            Command::ShowError("Lost connection to the server".into()),
            Command::ClosePlayer,
        ]
    );
}

#[test]
fn an_unplayable_item_shows_the_server_message() {
    let mut session = Session::new(None, ServerSettings::default());
    let decision =
        Decision::Unavailable { reason: "transcoding_disabled".into(), message: "Transcoding is off".into() };
    assert_eq!(
        session.step(Input::Started { decision, headers: headers() }, at(0)),
        vec![Command::ShowError("Transcoding is off".into()), Command::ClosePlayer]
    );
    let mut session = Session::new(None, ServerSettings::default());
    let commands =
        session.step(Input::Started { decision: Decision::Unknown("queued".into()), headers: headers() }, at(0));
    assert!(matches!(&commands[..], [Command::ShowError(text), Command::ClosePlayer] if text.contains("queued")));
}

#[test]
fn the_controller_gets_the_theme_when_it_is_ready() {
    let mut session = playing(None);
    assert_eq!(session.step(Input::ControlsReady, at(1)), vec![Command::SendTheme]);
}

/// Starts a session with `settings` on the fixture plan, whose audio is in `audio_language`, and loads the file.
fn load_with(settings: ServerSettings, audio_language: &str) -> Vec<Command> {
    let mut session = Session::new(None, settings);
    let Decision::Playable { session_id, plan, .. } = decision(|_| {}) else { unreachable!() };
    let decision = Decision::Playable { session_id, plan, audio_language: Some(audio_language.into()) };
    session.step(Input::Started { decision, headers: headers() }, at(0));
    session.step(Input::FileLoaded, at(0))
}

/// The command that adds track 0 (English, external) or 1 (English signs, forced) of the fixture inventory.
fn subtitle(index: usize) -> Command {
    let (path, title) = [
        (
            "0.vtt?file_id=42&external_subtitle_key=f903db1c5624c46c4a2c20f2fd8cd247fc0d46e6471c91d4ddb651ae4e039448",
            "English",
        ),
        ("1.ass?file_id=42&embedded_stream_index=0", "English (Signs)"),
    ][index];
    Command::AddSubtitle {
        url: format!("/api/v2/stream/{SESSION}/subtitles/{path}"),
        title: title.into(),
        language: "eng".into(),
    }
}

#[test]
fn foreign_audio_shows_the_subtitles_of_the_profile_language_when_the_file_loads() {
    // With no languages set, the profile language is English.
    assert_eq!(load_with(ServerSettings::default(), "ja"), vec![subtitle(0)]);
}

#[test]
fn audio_in_the_profile_language_shows_only_the_forced_track() {
    let settings = ServerSettings { audio_language: Some("en".into()), ..ServerSettings::default() };
    assert_eq!(load_with(settings.clone(), "en"), vec![subtitle(1)]);
    let settings = ServerSettings { show_forced_subtitles: false, ..settings };
    assert_eq!(load_with(settings, "en"), vec![]);
}

#[test]
fn a_track_that_needs_a_burn_in_shows_no_subtitles() {
    let settings = ServerSettings {
        subtitle_mode: SubtitleMode::Always,
        subtitle_language: Some("fr".into()),
        ..ServerSettings::default()
    };
    assert_eq!(load_with(settings, "en"), vec![]);
}
