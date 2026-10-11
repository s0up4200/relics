//! The Playback session: feed inputs and the time to `step`, and check the commands.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use silo_omarchy::api::types::{Decision, Marker, MarkerKind};
use silo_omarchy::playback::{Access, Command, Headers, Input, Replan, Sample, Session};
use silo_omarchy::settings::{IntroSkipMode, ServerSettings, SubtitleMode};

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

const HOUR: u64 = 3600;

/// The media headers with a token that expires 8 hours after the start.
fn access() -> Access {
    Access { headers: headers(), expires: Some(at(8 * HOUR)) }
}

/// A session whose file is loaded at time 0. `runtime` is the Item runtime in seconds.
fn playing(runtime: Option<f64>) -> Session {
    let mut session = Session::new(runtime, ServerSettings::default());
    session.step(Input::Started { decision: decision(|_| {}), access: access() }, at(0));
    session.step(Input::FileLoaded, at(0));
    session
}

fn stop(position: f64, sequence: u64) -> Command {
    Command::Stop { session_id: SESSION.into(), sample: Some(Sample { sequence, position, paused: false }) }
}

#[test]
fn a_playable_decision_loads_the_stream_with_the_headers_at_the_plan_start() {
    let mut session = Session::new(None, ServerSettings::default());
    let commands = session.step(Input::Started { decision: decision(|_| {}), access: access() }, at(0));
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
    session.step(Input::Started { decision: decision(|_| {}), access: access() }, at(0));
    // A position from the file that this Item replaced.
    session.step(Input::Position(900.0), at(1));
    assert_eq!(session.step(Input::Tick, at(30)), vec![]);
}

#[test]
fn the_position_in_a_report_is_the_source_position() {
    let mut session = Session::new(None, ServerSettings::default());
    let decision = decision(|body| body["playback_plan"]["timeline"]["timeline_offset_seconds"] = 600.into());
    session.step(Input::Started { decision, access: access() }, at(0));
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
fn without_an_mpv_duration_the_end_rule_uses_the_item_runtime() {
    let mut session = playing(Some(7200.0));
    session.step(Input::Position(7195.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(7200.0, 1), Command::ClosePlayer]);
    let mut session = playing(None);
    session.step(Input::Position(7195.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![Command::Refresh], "an Early end");
}

#[test]
fn an_unplayable_item_shows_the_server_message() {
    let mut session = Session::new(None, ServerSettings::default());
    let decision =
        Decision::Unavailable { reason: "transcoding_disabled".into(), message: "Transcoding is off".into() };
    assert_eq!(
        session.step(Input::Started { decision, access: access() }, at(0)),
        vec![Command::ShowError("Transcoding is off".into()), Command::ClosePlayer]
    );
    let mut session = Session::new(None, ServerSettings::default());
    let commands =
        session.step(Input::Started { decision: Decision::Unknown("queued".into()), access: access() }, at(0));
    assert!(matches!(&commands[..], [Command::ShowError(text), Command::ClosePlayer] if text.contains("queued")));
}

#[test]
fn the_controller_gets_the_theme_when_it_is_ready() {
    let mut session = playing(None);
    assert_eq!(session.step(Input::ControlsReady, at(1)), vec![Command::SendTheme, Command::ShowMarkers(vec![])]);
}

/// Starts a session with `settings` on the fixture plan, whose audio is in `audio_language`, and loads the file.
fn load_with(settings: ServerSettings, audio_language: &str) -> Vec<Command> {
    let mut session = Session::new(None, settings);
    let mut decision = decision(|_| {});
    let Decision::Playable { audio_language: language, .. } = &mut decision else { unreachable!() };
    *language = Some(audio_language.into());
    session.step(Input::Started { decision, access: access() }, at(0));
    session.step(Input::FileLoaded, at(0)).into_iter().filter(|c| matches!(c, Command::AddSubtitle { .. })).collect()
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

// The skip state machine.

fn marker(kind: MarkerKind, start: f64, end: f64) -> Marker {
    Marker { kind, start, end }
}

/// A session of a 1000 s file with these Markers (source time), with no position yet. The plan
/// has a timeline offset of 100 s, so player time = source time - 100.
fn with_markers(settings: ServerSettings, markers: Vec<Marker>) -> Session {
    let mut session = Session::new(None, settings);
    let mut decision = decision(|body| body["playback_plan"]["timeline"]["timeline_offset_seconds"] = 100.into());
    let Decision::Playable { markers: m, .. } = &mut decision else { unreachable!() };
    *m = markers;
    session.step(Input::Started { decision, access: access() }, at(0));
    session.step(Input::FileLoaded, at(0));
    session.step(Input::Duration(1000.0), at(0));
    session
}

fn intro_with(mode: IntroSkipMode) -> Session {
    let settings = ServerSettings { intro_skip_mode: mode, ..ServerSettings::default() };
    with_markers(settings, vec![marker(MarkerKind::Intro, 110.0, 150.0)])
}

fn prompt(label: &str) -> Command {
    Command::ShowPrompt(label.into())
}

#[test]
fn the_controller_gets_the_markers_in_player_time_when_the_file_loads() {
    let mut session = Session::new(None, ServerSettings::default());
    let mut decision = decision(|body| body["playback_plan"]["timeline"]["timeline_offset_seconds"] = 100.into());
    let Decision::Playable { markers, .. } = &mut decision else { unreachable!() };
    *markers = vec![marker(MarkerKind::Intro, 110.0, 150.0), marker(MarkerKind::Credits, 1000.0, 1100.0)];
    session.step(Input::Started { decision, access: access() }, at(0));
    let commands = session.step(Input::FileLoaded, at(0));
    let want =
        Command::ShowMarkers(vec![marker(MarkerKind::Intro, 10.0, 50.0), marker(MarkerKind::Credits, 900.0, 1000.0)]);
    assert!(commands.contains(&want), "{commands:?}");
}

#[test]
fn ask_offers_skip_intro_and_the_action_seeks_to_the_intro_end() {
    let mut session = intro_with(IntroSkipMode::Ask);
    assert_eq!(session.step(Input::Position(9.0), at(1)), vec![]);
    assert_eq!(session.step(Input::Position(10.0), at(1)), vec![prompt("Skip intro")]);
    assert_eq!(session.step(Input::Position(11.0), at(2)), vec![], "the prompt shows once");
    assert_eq!(session.step(Input::Action, at(3)), vec![Command::Seek(50.0), prompt("")]);
    session.step(Input::Position(50.0), at(3));
    assert_eq!(session.step(Input::Position(20.0), at(4)), vec![], "a skipped intro is not offered again");
}

#[test]
fn ask_hides_the_prompt_after_five_seconds_and_offers_it_again_on_the_next_entry() {
    let mut session = intro_with(IntroSkipMode::Ask);
    session.step(Input::Position(10.0), at(1));
    assert_eq!(session.step(Input::Tick, at(5)), vec![]);
    assert_eq!(session.step(Input::Tick, at(6)), vec![prompt("")]);
    assert_eq!(session.step(Input::Action, at(7)), vec![], "no prompt, so the action does nothing");
    assert_eq!(session.step(Input::Position(60.0), at(8)), vec![]);
    assert_eq!(session.step(Input::Position(30.0), at(9)), vec![prompt("Skip intro")]);
}

#[test]
fn ask_hides_the_prompt_when_playback_leaves_the_intro() {
    let mut session = intro_with(IntroSkipMode::Ask);
    session.step(Input::Position(10.0), at(1));
    assert_eq!(session.step(Input::Position(50.0), at(2)), vec![prompt("")]);
}

#[test]
fn never_shows_no_prompt() {
    let mut session = intro_with(IntroSkipMode::Never);
    assert_eq!(session.step(Input::Position(10.0), at(1)), vec![]);
}

#[test]
fn always_skips_the_intro_and_offers_watch_intro() {
    let mut session = intro_with(IntroSkipMode::Always);
    assert_eq!(session.step(Input::Position(10.0), at(1)), vec![Command::Seek(50.0), prompt("Watch intro")]);
    assert_eq!(session.step(Input::Position(50.0), at(1)), vec![], "the undo prompt stays after the seek");
    assert_eq!(session.step(Input::Action, at(2)), vec![Command::Seek(10.0), prompt("")]);
    assert_eq!(session.step(Input::Position(10.0), at(2)), vec![], "the intro the user chose to watch plays");
}

#[test]
fn always_does_not_skip_again_after_the_undo_prompt_times_out() {
    let mut session = intro_with(IntroSkipMode::Always);
    session.step(Input::Position(10.0), at(1));
    session.step(Input::Position(50.0), at(1));
    assert_eq!(session.step(Input::Tick, at(6)), vec![prompt("")]);
    assert_eq!(session.step(Input::Position(20.0), at(7)), vec![]);
}

#[test]
fn credits_and_recap_skip_only_when_the_server_settings_say_so() {
    let markers = vec![marker(MarkerKind::Recap, 100.0, 130.0), marker(MarkerKind::Credits, 500.0, 520.0)];
    let mut session = with_markers(ServerSettings::default(), markers.clone());
    assert_eq!(session.step(Input::Position(0.5), at(1)), vec![]);
    assert_eq!(session.step(Input::Position(400.0), at(1)), vec![]);
    let settings = ServerSettings { auto_skip_credits: true, auto_skip_recap: true, ..ServerSettings::default() };
    let mut session = with_markers(settings, markers);
    assert_eq!(session.step(Input::Position(0.5), at(1)), vec![Command::Seek(30.0)]);
    assert_eq!(session.step(Input::Position(400.0), at(1)), vec![Command::Seek(420.0)]);
}

#[test]
fn a_preview_marker_does_nothing() {
    let settings = ServerSettings { auto_skip_credits: true, auto_skip_recap: true, ..ServerSettings::default() };
    let mut session = with_markers(settings, vec![marker(MarkerKind::Preview, 200.0, 220.0)]);
    assert_eq!(session.step(Input::Position(105.0), at(1)), vec![]);
}

fn end_credits(settings: ServerSettings) -> Session {
    with_markers(settings, vec![marker(MarkerKind::Credits, 1040.0, 1100.0)])
}

#[test]
fn credits_to_the_end_of_the_file_offer_the_next_episode_when_one_exists() {
    let mut session = end_credits(ServerSettings::default());
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![], "no next episode, no prompt");
    let mut session = end_credits(ServerSettings::default());
    session.step(Input::NextEpisode, at(0));
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![prompt("Next episode")]);
    assert_eq!(session.step(Input::Action, at(2)), vec![prompt(""), stop(1050.0, 1), Command::PlayNext]);
    assert_eq!(session.step(Input::Tick, at(30)), vec![], "the session ended");
}

#[test]
fn auto_skip_of_credits_to_the_end_plays_the_next_episode_when_autoplay_is_on() {
    let settings = ServerSettings { auto_skip_credits: true, ..ServerSettings::default() };
    let mut session = end_credits(settings.clone());
    session.step(Input::NextEpisode, at(0));
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![stop(1050.0, 1), Command::PlayNext]);
    let mut session = end_credits(ServerSettings { auto_play_next: false, ..settings });
    session.step(Input::NextEpisode, at(0));
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![prompt("Next episode")]);
}

#[test]
fn a_normal_end_plays_the_next_episode_when_autoplay_is_on() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::NextEpisode, at(0));
    session.step(Input::Position(998.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(1100.0, 1), Command::PlayNext]);

    let settings = ServerSettings { auto_play_next: false, ..ServerSettings::default() };
    let mut session = with_markers(settings, vec![]);
    session.step(Input::NextEpisode, at(0));
    session.step(Input::Position(998.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(1100.0, 1), Command::ClosePlayer]);

    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::NextEpisode, at(0));
    session.step(Input::Position(500.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![Command::Refresh], "an Early end");
}

#[test]
fn the_end_of_a_session_hides_its_prompt() {
    let mut session = intro_with(IntroSkipMode::Ask);
    session.step(Input::Position(10.0), at(1));
    assert_eq!(session.step(Input::End, at(2)), vec![prompt(""), stop(110.0, 1)]);
}

// Token rules and the recovery chain (#27).

fn load(url: &str, headers: Headers, start: f64) -> Command {
    Command::Load { url: url.into(), headers, start }
}

fn stream() -> String {
    format!("/api/v2/stream/{SESSION}")
}

/// The headers after a refresh, with a token that expires 8 hours after `refreshed_at` seconds.
fn refreshed(refreshed_at: u64) -> Access {
    Access { headers: vec![("Authorization".into(), "Bearer new".into())], expires: Some(at(refreshed_at + 8 * HOUR)) }
}

fn lost() -> Command {
    prompt("Lost connection to the server")
}

#[test]
fn a_token_with_less_than_four_hours_left_is_refreshed_before_the_load() {
    let mut session = Session::new(None, ServerSettings::default());
    let access = Access { headers: headers(), expires: Some(at(4 * HOUR - 1)) };
    assert_eq!(session.step(Input::Started { decision: decision(|_| {}), access }, at(0)), vec![Command::Refresh]);
    assert_eq!(
        session.step(Input::Refreshed(Some(refreshed(1))), at(1)),
        vec![load(&stream(), refreshed(1).headers, 12.5)]
    );
}

#[test]
fn a_failed_refresh_before_the_load_loads_with_the_old_token() {
    let mut session = Session::new(None, ServerSettings::default());
    let access = Access { headers: headers(), expires: Some(at(HOUR)) };
    session.step(Input::Started { decision: decision(|_| {}), access }, at(0));
    assert_eq!(session.step(Input::Refreshed(None), at(1)), vec![load(&stream(), headers(), 12.5)]);
}

#[test]
fn ten_minutes_before_the_token_in_mpv_expires_the_app_refreshes_it_and_reloads_at_the_position() {
    let mut session = playing(None);
    session.step(Input::Position(28_000.0), at(28_000));
    let renew_at = 8 * HOUR - 600;
    assert!(!session.step(Input::Tick, at(renew_at - 1)).contains(&Command::Refresh));
    assert!(session.step(Input::Tick, at(renew_at)).contains(&Command::Refresh));
    assert!(!session.step(Input::Tick, at(renew_at + 1)).contains(&Command::Refresh), "one refresh at a time");
    session.step(Input::Position(28_202.0), at(renew_at + 2));
    assert_eq!(
        session.step(Input::Refreshed(Some(refreshed(renew_at))), at(renew_at + 2)),
        vec![load(&stream(), refreshed(renew_at).headers, 28_202.0)]
    );
}

#[test]
fn a_failed_renew_keeps_the_stream_and_does_not_try_again() {
    let mut session = playing(None);
    session.step(Input::Position(100.0), at(1));
    let renew_at = 8 * HOUR - 600;
    assert!(session.step(Input::Tick, at(renew_at)).contains(&Command::Refresh));
    assert_eq!(session.step(Input::Refreshed(None), at(renew_at + 1)), vec![]);
    assert!(!session.step(Input::Tick, at(renew_at + 2)).contains(&Command::Refresh));
}

/// A session that plays the fixture plan (attempt `attempt-1`) and is at `position` seconds when it fails.
fn failed_at(position: f64) -> Session {
    let mut session = Session::new(None, ServerSettings::default());
    let mut decision = decision(|_| {});
    let Decision::Playable { attempt_id, .. } = &mut decision else { unreachable!() };
    *attempt_id = "attempt-1".into();
    session.step(Input::Started { decision, access: access() }, at(0));
    session.step(Input::FileLoaded, at(0));
    session.step(Input::Duration(7200.0), at(1));
    session.step(Input::Position(position), at(1));
    session
}

fn replan(position: f64) -> Command {
    Command::Replan(Replan {
        session_id: SESSION.into(),
        attempt_id: "attempt-1".into(),
        failed_plan_id: "plan:478677870860e5e5108c18bff749b34b".into(),
        plan_attempt_key: "v3:f0144c47fa349e3e".into(),
        attempted_plan_keys: vec!["v3:f0144c47fa349e3e".into()],
        selected_tracks: serde_json::json!({ "audio": { "id": "file:42:audio:0", "index": 0 } }),
        position,
    })
}

/// The replan answer of the own fixture: a transcode from 3600 s.
fn replanned() -> Decision {
    let path = format!("{}/tests/fixtures/own/playback_replan_failure_recovery.json", env!("CARGO_MANIFEST_DIR"));
    let body = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    Decision::from(serde_json::from_value::<silo_omarchy::api::wire::PlaybackDecision>(body).unwrap())
}

fn transcode() -> String {
    format!("/api/v2/playback/transcode/{SESSION}/master.m3u8")
}

#[test]
fn a_load_failure_refreshes_once_and_reloads_at_the_last_good_position() {
    let mut session = failed_at(3600.0);
    assert_eq!(session.step(Input::LoadFailed, at(2)), vec![Command::Refresh]);
    assert_eq!(session.step(Input::LoadFailed, at(2)), vec![], "the chain waits for the refresh");
    assert_eq!(
        session.step(Input::Refreshed(Some(refreshed(2))), at(3)),
        vec![load(&stream(), refreshed(2).headers, 3600.0)]
    );
}

#[test]
fn a_second_failure_replans_at_the_last_good_position_and_plays_the_new_plan() {
    let mut session = failed_at(3600.0);
    session.step(Input::LoadFailed, at(2));
    session.step(Input::Refreshed(Some(refreshed(2))), at(3));
    assert_eq!(session.step(Input::LoadFailed, at(4)), vec![replan(3600.0)]);
    // The transcode starts at source 3600 s, which is player time 0.
    assert_eq!(
        session.step(Input::Replanned(Some(replanned())), at(5)),
        vec![load(&transcode(), refreshed(2).headers, 0.0)]
    );
    session.step(Input::FileLoaded, at(6));
    session.step(Input::Position(30.0), at(7));
    assert_eq!(session.step(Input::Shutdown, at(8)), vec![stop(3630.0, 1), Command::ClosePlayer]);
}

#[test]
fn after_the_replan_fails_the_player_shows_lost_connection_and_enter_starts_the_chain_again() {
    let mut session = failed_at(3600.0);
    session.step(Input::LoadFailed, at(2));
    session.step(Input::Refreshed(Some(refreshed(2))), at(3));
    session.step(Input::LoadFailed, at(4));
    session.step(Input::Replanned(Some(replanned())), at(5));
    assert_eq!(session.step(Input::LoadFailed, at(6)), vec![lost()]);
    assert!(!session.step(Input::Tick, at(60)).contains(&prompt("")), "the message stays");
    assert_eq!(session.step(Input::Action, at(61)), vec![prompt(""), Command::Refresh]);
}

#[test]
fn a_failed_replan_shows_lost_connection_and_q_sends_the_last_good_position() {
    let mut session = failed_at(3600.0);
    session.step(Input::LoadFailed, at(2));
    session.step(Input::Refreshed(None), at(3));
    session.step(Input::LoadFailed, at(4));
    assert_eq!(session.step(Input::Replanned(None), at(5)), vec![lost()]);
    assert_eq!(session.step(Input::Shutdown, at(6)), vec![prompt(""), stop(3600.0, 1), Command::ClosePlayer]);
}

#[test]
fn an_early_end_starts_the_same_chain_and_never_reports_the_duration() {
    let mut session = failed_at(3600.0);
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![Command::Refresh]);
    session.step(Input::Refreshed(Some(refreshed(2))), at(3));
    assert_eq!(session.step(Input::EndOfFile, at(4)), vec![replan(3600.0)]);
    assert_eq!(session.step(Input::Replanned(Some(Decision::Unknown("queued".into()))), at(5)), vec![lost()]);
    assert_eq!(session.step(Input::Shutdown, at(6)), vec![prompt(""), stop(3600.0, 1), Command::ClosePlayer]);
}

#[test]
fn the_chain_starts_over_once_the_reloaded_stream_plays() {
    let mut session = failed_at(3600.0);
    session.step(Input::LoadFailed, at(2));
    session.step(Input::Refreshed(Some(refreshed(2))), at(3));
    session.step(Input::FileLoaded, at(4));
    session.step(Input::Position(3700.0), at(104));
    assert_eq!(session.step(Input::EndOfFile, at(105)), vec![Command::Refresh]);
}
