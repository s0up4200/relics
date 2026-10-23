//! The Playback session: feed inputs and the time to `step`, and check the commands.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relics::api::types::{Decision, Marker, MarkerKind};
use relics::playback::tracks::{self, Choice, Kind, Track};
use relics::playback::{
    Access, Command, Headers, Input, Operation, Quality, Replan, STILL_WATCHING_AFTER, Sample, Session,
};
use relics::settings::{IntroSkipMode, ServerSettings, SubtitleMode};
use serde_json::Value;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
}

/// The plan of the server fixture: original_http, source duration 7200 s, start at 12.5 s.
fn decision(edit: impl FnOnce(&mut Value)) -> Decision {
    let path = format!("{}/tests/fixtures/silo/playback_start_opaque_ids.json", env!("CARGO_MANIFEST_DIR"));
    let mut body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    edit(&mut body);
    Decision::from(serde_json::from_value::<relics::api::wire::PlaybackDecision>(body).unwrap())
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
    session.step(Input::Started { decision: decision(|_| {}), access: access(), item: None }, at(0));
    session.step(Input::FileLoaded, at(0));
    session
}

fn stop(position: f64, sequence: u64) -> Command {
    Command::Stop { session_id: SESSION.into(), sample: Some(Sample { sequence, position, paused: false }) }
}

#[test]
fn a_playable_decision_loads_the_stream_with_the_headers_at_the_plan_start() {
    let mut session = Session::new(None, ServerSettings::default());
    let commands = session.step(Input::Started { decision: decision(|_| {}), access: access(), item: None }, at(0));
    assert_eq!(
        commands,
        vec![Command::Load { url: format!("/api/v2/stream/{SESSION}"), headers: headers(), start: 12.5, aid: Some(1) }]
    );
}

#[test]
fn direct_play_plays_the_audio_track_that_the_server_picked() {
    let mut session = Session::new(None, ServerSettings::default());
    let decision = decision(|body| body["playback_plan"]["selected_tracks"]["audio"]["index"] = 2.into());
    let commands = session.step(Input::Started { decision, access: access(), item: None }, at(0));
    // mpv counts the audio tracks from 1, the server from 0.
    assert!(matches!(&commands[..], [Command::Load { aid: Some(3), .. }]), "{commands:?}");
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
    session.step(Input::Started { decision: decision(|_| {}), access: access(), item: None }, at(0));
    // A position from the file that this Item replaced.
    session.step(Input::Position(900.0), at(1));
    assert_eq!(session.step(Input::Tick, at(30)), vec![]);
}

#[test]
fn the_position_in_a_report_is_the_source_position() {
    let mut session = Session::new(None, ServerSettings::default());
    let decision = decision(|body| body["playback_plan"]["timeline"]["timeline_offset_seconds"] = 600.into());
    session.step(Input::Started { decision, access: access(), item: None }, at(0));
    session.step(Input::FileLoaded, at(0));
    session.step(Input::Position(30.0), at(1));
    assert_eq!(session.step(Input::Shutdown, at(2)), vec![stop(630.0, 1), Command::ClosePlayer]);
}

#[test]
fn an_mpv_shutdown_sends_the_stop_report_and_closes_the_player() {
    let mut session = playing(None);
    session.step(Input::Position(300.0), at(1));
    assert_eq!(session.step(Input::Shutdown, at(2)), vec![stop(300.0, 1), Command::ClosePlayer]);
    assert_eq!(session.step(Input::Tick, at(60)), vec![], "an ended session sends nothing more");
}

// Back in the Player view and a closed Library window both end the session with `Input::End`, then the
// controller closes the player.
#[test]
fn back_in_the_player_view_and_a_closed_library_window_send_the_stop_report_once() {
    let mut session = playing(None);
    session.step(Input::Position(300.0), at(1));
    session.step(Input::Paused(true), at(2));
    let paused = Sample { sequence: 1, position: 300.0, paused: true };
    let stop = Command::Stop { session_id: SESSION.into(), sample: Some(paused) };
    assert_eq!(session.step(Input::End, at(3)), vec![stop]);
    assert_eq!(session.step(Input::Tick, at(60)), vec![], "an ended session sends nothing more");
    assert_eq!(session.step(Input::End, at(61)), vec![], "the controller ends the session once");
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
        session.step(Input::Started { decision, access: access(), item: None }, at(0)),
        vec![Command::ShowError("Transcoding is off".into()), Command::ClosePlayer]
    );
    let mut session = Session::new(None, ServerSettings::default());
    let commands = session
        .step(Input::Started { decision: Decision::Unknown("queued".into()), access: access(), item: None }, at(0));
    assert!(matches!(&commands[..], [Command::ShowError(text), Command::ClosePlayer] if text.contains("queued")));
}

/// Starts a session with `settings` on the fixture plan (direct play), whose audio is in `audio_language`, and
/// loads the file. Returns the subtitle commands.
fn load_with(settings: ServerSettings, audio_language: &str) -> Vec<Command> {
    load_with_delivery(settings, audio_language, "original_http")
}

fn load_with_delivery(settings: ServerSettings, audio_language: &str, delivery: &str) -> Vec<Command> {
    let mut session = Session::new(None, settings);
    let mut decision = decision(|body| body["playback_plan"]["delivery"] = delivery.into());
    let Decision::Playable { audio_language: language, .. } = &mut decision else { unreachable!() };
    *language = Some(audio_language.into());
    session.step(Input::Started { decision, access: access(), item: None }, at(0));
    let commands = session.step(Input::FileLoaded, at(0)).into_iter();
    commands.filter(|c| matches!(c, Command::AddSubtitle { .. } | Command::SelectSubtitle(_))).collect()
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
    // The track is in the file that mpv plays, so mpv shows it from there: the first embedded subtitle track.
    assert_eq!(load_with(settings.clone(), "en"), vec![Command::SelectSubtitle(Some(1))]);
    let settings = ServerSettings { show_forced_subtitles: false, ..settings };
    assert_eq!(load_with(settings, "en"), vec![]);
}

#[test]
fn a_stream_without_the_embedded_tracks_adds_the_sidecar_of_an_embedded_track() {
    let settings = ServerSettings { audio_language: Some("en".into()), ..ServerSettings::default() };
    assert_eq!(load_with_delivery(settings, "en", "server_transcode_hls"), vec![subtitle(1)]);
}

#[test]
fn a_track_that_needs_a_burn_in_shows_from_the_file_in_direct_play_and_else_not_at_all() {
    let settings = ServerSettings {
        subtitle_mode: SubtitleMode::Always,
        subtitle_language: Some("fr".into()),
        ..ServerSettings::default()
    };
    // French is the third embedded track of the fixture.
    assert_eq!(load_with(settings.clone(), "en"), vec![Command::SelectSubtitle(Some(3))]);
    assert_eq!(load_with_delivery(settings, "en", "server_transcode_hls"), vec![]);
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
    session.step(Input::Started { decision, access: access(), item: None }, at(0));
    session.step(Input::FileLoaded, at(0));
    session.step(Input::Duration(1000.0), at(0));
    session
}

fn intro_with(mode: IntroSkipMode) -> Session {
    let settings = ServerSettings { intro_skip_mode: mode, ..ServerSettings::default() };
    with_markers(settings, vec![marker(MarkerKind::Intro, 110.0, 150.0)])
}

#[test]
fn the_player_view_gets_the_markers_in_player_time() {
    let mut session = Session::new(None, ServerSettings::default());
    let mut decision = decision(|body| body["playback_plan"]["timeline"]["timeline_offset_seconds"] = 100.into());
    let Decision::Playable { markers, .. } = &mut decision else { unreachable!() };
    *markers = vec![marker(MarkerKind::Intro, 110.0, 150.0), marker(MarkerKind::Credits, 1000.0, 1100.0)];
    session.step(Input::Started { decision, access: access(), item: None }, at(0));
    session.step(Input::FileLoaded, at(0));
    assert_eq!(session.markers(), [marker(MarkerKind::Intro, 10.0, 50.0), marker(MarkerKind::Credits, 900.0, 1000.0)]);
}

#[test]
fn ask_offers_skip_intro_and_the_action_seeks_to_the_intro_end() {
    let mut session = intro_with(IntroSkipMode::Ask);
    assert_eq!(session.step(Input::Position(9.0), at(1)), vec![]);
    assert_eq!(session.step(Input::Position(10.0), at(1)), vec![]);
    assert_eq!(session.prompt(), "Skip intro");
    assert_eq!(session.step(Input::Position(11.0), at(2)), vec![], "the prompt shows once");
    assert_eq!(session.step(Input::Action, at(3)), vec![Command::Seek(50.0)]);
    assert_eq!(session.prompt(), "");
    session.step(Input::Position(50.0), at(3));
    assert_eq!(session.step(Input::Position(20.0), at(4)), vec![], "a skipped intro is not offered again");
}

#[test]
fn ask_hides_the_prompt_after_five_seconds_and_offers_it_again_on_the_next_entry() {
    let mut session = intro_with(IntroSkipMode::Ask);
    session.step(Input::Position(10.0), at(1));
    assert_eq!(session.step(Input::Tick, at(5)), vec![]);
    assert_eq!(session.step(Input::Tick, at(6)), vec![]);
    assert_eq!(session.prompt(), "");
    assert_eq!(session.step(Input::Action, at(7)), vec![], "no prompt, so the action does nothing");
    assert_eq!(session.step(Input::Position(60.0), at(8)), vec![]);
    assert_eq!(session.step(Input::Position(30.0), at(9)), vec![]);
    assert_eq!(session.prompt(), "Skip intro");
}

#[test]
fn ask_hides_the_prompt_when_playback_leaves_the_intro() {
    let mut session = intro_with(IntroSkipMode::Ask);
    session.step(Input::Position(10.0), at(1));
    assert_eq!(session.step(Input::Position(50.0), at(2)), vec![]);
    assert_eq!(session.prompt(), "");
}

#[test]
fn never_shows_no_prompt() {
    let mut session = intro_with(IntroSkipMode::Never);
    assert_eq!(session.step(Input::Position(10.0), at(1)), vec![]);
}

#[test]
fn always_skips_the_intro_and_offers_watch_intro() {
    let mut session = intro_with(IntroSkipMode::Always);
    assert_eq!(session.step(Input::Position(10.0), at(1)), vec![Command::Seek(50.0)]);
    assert_eq!(session.prompt(), "Watch intro");
    assert_eq!(session.step(Input::Position(50.0), at(1)), vec![], "the undo prompt stays after the seek");
    assert_eq!(session.step(Input::Action, at(2)), vec![Command::Seek(10.0)]);
    assert_eq!(session.prompt(), "");
    assert_eq!(session.step(Input::Position(10.0), at(2)), vec![], "the intro the user chose to watch plays");
}

#[test]
fn always_does_not_skip_again_after_the_undo_prompt_times_out() {
    let mut session = intro_with(IntroSkipMode::Always);
    session.step(Input::Position(10.0), at(1));
    session.step(Input::Position(50.0), at(1));
    assert_eq!(session.step(Input::Tick, at(6)), vec![]);
    assert_eq!(session.prompt(), "");
    assert_eq!(session.step(Input::Position(20.0), at(7)), vec![]);
}

#[test]
fn credits_and_recap_skip_only_when_the_server_settings_say_so() {
    let markers = vec![marker(MarkerKind::Recap, 100.0, 130.0), marker(MarkerKind::Credits, 500.0, 520.0)];
    let mut session = with_markers(ServerSettings::default(), markers.clone());
    assert_eq!(session.step(Input::Position(0.5), at(1)), vec![]);
    assert_eq!(session.prompt(), "Skip recap");
    assert_eq!(session.step(Input::Action, at(2)), vec![Command::Seek(30.0)]);
    assert_eq!(session.prompt(), "");
    assert_eq!(session.step(Input::Position(400.0), at(3)), vec![]);
    assert_eq!(session.prompt(), "Skip credits");
    assert_eq!(session.step(Input::Action, at(4)), vec![Command::Seek(420.0)]);
    assert_eq!(session.prompt(), "");
    let settings = ServerSettings { auto_skip_credits: true, auto_skip_recap: true, ..ServerSettings::default() };
    let mut session = with_markers(settings, markers);
    assert_eq!(session.step(Input::Position(0.5), at(1)), vec![Command::Seek(30.0)]);
    assert_eq!(session.step(Input::Position(400.0), at(1)), vec![Command::Seek(420.0)]);
}

#[test]
fn a_preview_marker_offers_skip_preview_and_the_action_seeks_to_its_end() {
    let settings = ServerSettings { auto_skip_credits: true, auto_skip_recap: true, ..ServerSettings::default() };
    let mut session = with_markers(settings, vec![marker(MarkerKind::Preview, 200.0, 220.0)]);
    assert_eq!(session.step(Input::Position(99.0), at(1)), vec![]);
    assert_eq!(session.step(Input::Position(100.0), at(1)), vec![], "no automatic skip");
    assert_eq!(session.prompt(), "Skip preview");
    assert_eq!(session.step(Input::Action, at(2)), vec![Command::Seek(120.0)]);
    assert_eq!(session.prompt(), "");
    session.step(Input::Position(120.0), at(2));
    assert_eq!(session.step(Input::Position(105.0), at(3)), vec![], "a skipped preview is not offered again");
}

#[test]
fn skip_preview_hides_when_playback_leaves_the_preview_or_after_five_seconds() {
    let mut session = with_markers(ServerSettings::default(), vec![marker(MarkerKind::Preview, 200.0, 220.0)]);
    session.step(Input::Position(100.0), at(1));
    assert_eq!(session.step(Input::Position(130.0), at(2)), vec![]);
    assert_eq!(session.prompt(), "");
    session.step(Input::Position(110.0), at(3));
    assert_eq!(session.step(Input::Tick, at(8)), vec![]);
    assert_eq!(session.prompt(), "");
}

#[test]
fn credits_to_the_end_with_no_next_episode_offer_skip_credits() {
    let mut session = end_credits(ServerSettings::default());
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![]);
    assert_eq!(session.prompt(), "Skip credits");
    assert_eq!(session.step(Input::Action, at(2)), vec![Command::Seek(1000.0)]);
    assert_eq!(session.prompt(), "");
}

fn end_credits(settings: ServerSettings) -> Session {
    with_markers(settings, vec![marker(MarkerKind::Credits, 1040.0, 1100.0)])
}

#[test]
fn credits_to_the_end_of_the_file_offer_the_next_episode_when_autoplay_is_off() {
    let mut session = end_credits(ServerSettings { auto_play_next: false, ..ServerSettings::default() });
    session.step(Input::NextEpisode, at(0));
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![]);
    assert_eq!(session.prompt(), "Next episode");
    assert_eq!(session.step(Input::Action, at(2)), vec![stop(1050.0, 1), Command::PlayNext]);
    assert_eq!(session.prompt(), "");
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
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![]);
    assert_eq!(session.prompt(), "Next episode");
}

#[test]
fn a_normal_end_plays_the_next_episode_when_autoplay_is_on() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::NextEpisode, at(0));
    session.step(Input::Position(998.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(1100.0, 1), Command::PlayNext]);
    assert_eq!(session.up_next(), None);

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

// Shuffle (#62).

#[test]
fn a_shuffled_item_plays_the_next_pick_at_its_end_also_with_autoplay_off() {
    let settings = ServerSettings { auto_play_next: false, ..ServerSettings::default() };
    let mut session = with_markers(settings, vec![]);
    session.step(Input::Shuffle, at(0));
    session.step(Input::Position(998.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![stop(1100.0, 1), Command::PlayNext]);
    assert_eq!(session.up_next(), None);
}

#[test]
fn skip_ends_the_shuffled_item_at_its_position_and_plays_the_next_pick() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::Shuffle, at(0));
    session.step(Input::Position(300.0), at(1));
    assert_eq!(session.step(Input::Next, at(2)), vec![stop(400.0, 1), Command::PlayNext]);
    assert_eq!(session.step(Input::Tick, at(30)), vec![], "the session ended");
}

#[test]
fn credits_to_the_end_of_a_shuffled_item_show_up_next_also_with_autoplay_off() {
    let mut session = end_credits(ServerSettings { auto_play_next: false, ..ServerSettings::default() });
    session.step(Input::Shuffle, at(0));
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![]);
    assert_eq!(session.up_next(), Some(10));
    assert_eq!(session.step(Input::Action, at(2)), vec![stop(1050.0, 1), Command::PlayNext]);
    assert_eq!(session.up_next(), None);
}

// Up Next and Still watching (#52).

/// A session at the end credits (player time 940 to 1000) of an episode with a next episode, with autoplay on.
fn credits_with_next() -> Session {
    let mut session = end_credits(ServerSettings::default());
    session.step(Input::NextEpisode, at(0));
    session
}

#[test]
fn up_next_counts_down_from_ten_seconds_at_the_end_credits_and_then_plays_the_next_episode() {
    let mut session = credits_with_next();
    assert_eq!(session.step(Input::Position(950.0), at(1)), vec![], "no Skip prompt with it");
    assert_eq!(session.up_next(), Some(10));
    assert_eq!(session.step(Input::Position(951.0), at(2)), vec![], "the card shows once");
    assert_eq!(session.step(Input::Tick, at(2)), vec![]);
    assert_eq!(session.up_next(), Some(9));
    // A progress report is due at 10 s.
    assert_eq!(session.step(Input::Tick, at(9)), vec![]);
    assert_eq!(session.up_next(), Some(2));
    assert_eq!(session.step(Input::Tick, at(11)), vec![stop(1051.0, 1), Command::PlayNext]);
    assert_eq!(session.up_next(), None);
    assert_eq!(session.auto_plays(), 1, "the countdown played the next episode, not the user");
}

#[test]
fn play_now_on_the_up_next_card_plays_the_next_episode_at_once() {
    let mut session = credits_with_next();
    session.step(Input::Position(950.0), at(1));
    assert_eq!(session.step(Input::Action, at(3)), vec![stop(1050.0, 1), Command::PlayNext]);
    assert_eq!(session.up_next(), None);
    assert_eq!(session.auto_plays(), 0, "the user played it");
}

#[test]
fn a_cancelled_up_next_card_does_not_play_the_next_episode_at_the_end() {
    let mut session = credits_with_next();
    session.step(Input::Position(950.0), at(1));
    assert_eq!(session.step(Input::CancelUpNext, at(2)), vec![]);
    assert_eq!(session.up_next(), None);
    assert_eq!(session.step(Input::Tick, at(30)).iter().filter(|c| **c == Command::PlayNext).count(), 0);
    assert_eq!(session.step(Input::Position(990.0), at(31)), vec![], "the card does not come back");
    session.step(Input::Position(999.0), at(32));
    assert!(session.step(Input::EndOfFile, at(33)).ends_with(&[Command::ClosePlayer]));
}

#[test]
fn up_next_hides_when_playback_leaves_the_credits_and_starts_again_on_the_next_entry() {
    let mut session = credits_with_next();
    session.step(Input::Position(950.0), at(1));
    assert_eq!(session.step(Input::Position(500.0), at(3)), vec![]);
    assert_eq!(session.up_next(), None);
    assert_eq!(session.step(Input::Tick, at(20)).iter().filter(|c| **c == Command::PlayNext).count(), 0);
    assert_eq!(session.step(Input::Position(960.0), at(21)), vec![]);
    assert_eq!(session.up_next(), Some(10));
}

#[test]
fn without_end_credits_up_next_shows_ten_seconds_before_the_end() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::NextEpisode, at(0));
    assert_eq!(session.step(Input::Position(989.0), at(1)), vec![]);
    assert_eq!(session.step(Input::Position(990.0), at(2)), vec![]);
    assert_eq!(session.up_next(), Some(10));
}

#[test]
fn up_next_shows_only_when_the_next_episode_plays_by_itself() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    assert_eq!(session.step(Input::Position(995.0), at(1)), vec![], "no next episode");
    let mut session = with_markers(ServerSettings { auto_play_next: false, ..ServerSettings::default() }, vec![]);
    session.step(Input::NextEpisode, at(0));
    assert_eq!(session.step(Input::Position(995.0), at(1)), vec![], "autoplay is off");
}

/// `credits_with_next` after `count` episodes in a row played by themselves.
fn after_auto_plays(count: u32) -> Session {
    let mut session = Session::new(None, ServerSettings::default()).with_auto_plays(count);
    let mut decision = decision(|body| body["playback_plan"]["timeline"]["timeline_offset_seconds"] = 100.into());
    let Decision::Playable { markers, .. } = &mut decision else { unreachable!() };
    *markers = vec![marker(MarkerKind::Credits, 1040.0, 1100.0)];
    session.step(Input::Started { decision, access: access(), item: None }, at(0));
    session.step(Input::FileLoaded, at(0));
    session.step(Input::Duration(1000.0), at(0));
    session.step(Input::NextEpisode, at(0));
    session
}

#[test]
fn after_three_episodes_with_no_input_still_watching_pauses_in_place_of_the_next_episode() {
    let mut session = after_auto_plays(STILL_WATCHING_AFTER - 1);
    session.step(Input::Position(950.0), at(1));
    assert!(session.step(Input::Tick, at(11)).ends_with(&[Command::PlayNext]), "the third episode plays");
    assert_eq!(session.auto_plays(), STILL_WATCHING_AFTER);

    let mut session = after_auto_plays(STILL_WATCHING_AFTER);
    session.step(Input::Position(950.0), at(1));
    assert_eq!(session.step(Input::Tick, at(11)), vec![Command::SetPause(true)]);
    assert_eq!((session.up_next(), session.still_watching()), (None, true));
    session.step(Input::Paused(true), at(11));
    assert!(!session.step(Input::Tick, at(30)).contains(&Command::PlayNext), "playback waits for the answer");
    assert_eq!(session.step(Input::Position(951.0), at(12)), vec![], "no Up Next card while it asks");
}

#[test]
fn continue_on_still_watching_plays_the_next_episode_and_starts_the_count_again() {
    let mut session = after_auto_plays(STILL_WATCHING_AFTER);
    session.step(Input::Position(950.0), at(1));
    session.step(Input::Tick, at(11));
    session.step(Input::Paused(true), at(11));
    let paused = Sample { sequence: 1, position: 1050.0, paused: true };
    assert_eq!(
        session.step(Input::Action, at(20)),
        vec![
            Command::SetPause(false),
            Command::Stop { session_id: SESSION.into(), sample: Some(paused) },
            Command::PlayNext
        ]
    );
    assert!(!session.still_watching());
    assert_eq!(session.auto_plays(), 0);
}

#[test]
fn still_watching_at_the_end_of_the_file_keeps_the_duration_for_the_stop_report_on_back() {
    let mut session = after_auto_plays(STILL_WATCHING_AFTER);
    session.step(Input::Position(999.0), at(1));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![Command::SetPause(true)]);
    assert_eq!((session.up_next(), session.still_watching()), (None, true));
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![], "mpv can report the end twice");
    assert_eq!(session.step(Input::End, at(3)), vec![stop(1100.0, 1)]);
    assert!(!session.still_watching());
}

#[test]
fn any_input_of_the_user_starts_the_count_of_episodes_again() {
    for input in [Input::Active, Input::PlayPause, Input::SeekBy(10.0), Input::CancelUpNext] {
        let mut session = after_auto_plays(STILL_WATCHING_AFTER);
        session.step(input, at(1));
        assert_eq!(session.auto_plays(), 0);
    }
    let mut session = after_auto_plays(1);
    session.step(Input::Tick, at(1));
    session.step(Input::Position(10.0), at(2));
    assert_eq!(session.auto_plays(), 1, "the time and mpv events are not input of the user");
}

#[test]
fn the_end_of_a_session_hides_its_prompt() {
    let mut session = intro_with(IntroSkipMode::Ask);
    session.step(Input::Position(10.0), at(1));
    assert_eq!(session.step(Input::End, at(2)), vec![stop(110.0, 1)]);
    assert_eq!(session.prompt(), "");
}

// Token rules and the recovery chain (#27).

/// A load of `url`. The direct play stream of the fixture plays audio track 1, and a transcode has no track choice.
fn load(url: &str, headers: Headers, start: f64) -> Command {
    Command::Load { url: url.into(), headers, start, aid: (url == stream()).then_some(1) }
}

fn stream() -> String {
    format!("/api/v2/stream/{SESSION}")
}

/// The headers after a refresh, with a token that expires 8 hours after `refreshed_at` seconds.
fn refreshed(refreshed_at: u64) -> Access {
    Access { headers: vec![("Authorization".into(), "Bearer new".into())], expires: Some(at(refreshed_at + 8 * HOUR)) }
}

const LOST: &str = "Lost connection to the server";

#[test]
fn a_token_with_less_than_four_hours_left_is_refreshed_before_the_load() {
    let mut session = Session::new(None, ServerSettings::default());
    let access = Access { headers: headers(), expires: Some(at(4 * HOUR - 1)) };
    assert_eq!(
        session.step(Input::Started { decision: decision(|_| {}), access, item: None }, at(0)),
        vec![Command::Refresh]
    );
    assert_eq!(
        session.step(Input::Refreshed(Some(refreshed(1))), at(1)),
        vec![load(&stream(), refreshed(1).headers, 12.5)]
    );
}

#[test]
fn a_failed_refresh_before_the_load_loads_with_the_old_token() {
    let mut session = Session::new(None, ServerSettings::default());
    let access = Access { headers: headers(), expires: Some(at(HOUR)) };
    session.step(Input::Started { decision: decision(|_| {}), access, item: None }, at(0));
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
    session.step(Input::Started { decision, access: access(), item: None }, at(0));
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
        operation: Operation::FailureRecovery,
        quality: Quality { preference: "auto".into(), bandwidth_cap_kbps: None },
    })
}

/// The quality change replan of the session of `failed_at`, at `position`.
fn quality_change(label: &str, position: f64) -> Command {
    let Command::Replan(failure) = replan(position) else { unreachable!() };
    Command::Replan(Replan {
        attempted_plan_keys: vec![],
        operation: Operation::QualityChange,
        quality: Quality { preference: label.into(), bandwidth_cap_kbps: None },
        ..failure
    })
}

#[test]
fn a_quality_step_replans_at_the_position_and_plays_the_new_plan_in_the_same_session() {
    let mut session = failed_at(3600.0);
    assert_eq!(
        session.step(Input::PickQuality("1080p-medium".into()), at(2)),
        vec![quality_change("1080p-medium", 3600.0)]
    );
    assert_eq!(session.quality(), "auto", "the step plays only once the server answers");
    // The transcode starts at source 3600 s, which is player time 0.
    assert_eq!(session.step(Input::Replanned(Some(replanned())), at(3)), vec![load(&transcode(), headers(), 0.0)]);
    assert_eq!(session.quality(), "1080p-medium");
    session.step(Input::FileLoaded, at(4));
    session.step(Input::Position(30.0), at(5));
    assert_eq!(session.step(Input::Shutdown, at(6)), vec![stop(3630.0, 1), Command::ClosePlayer]);
}

#[test]
fn a_failed_quality_step_keeps_the_plan_that_plays() {
    let mut session = failed_at(3600.0);
    session.step(Input::PickQuality("720p-low".into()), at(2));
    assert_eq!(session.step(Input::Replanned(None), at(3)), vec![], "no Lost connection: the old stream still plays");
    assert_eq!(session.quality(), "auto");
    assert_eq!(session.step(Input::LoadFailed, at(4)), vec![Command::Refresh], "the recovery chain starts as usual");
}

#[test]
fn the_quality_that_plays_needs_no_replan_and_a_recovery_keeps_the_chosen_quality() {
    let mut session = failed_at(3600.0);
    assert_eq!(session.step(Input::PickQuality("original".into()), at(2)), vec![]);
    session.step(Input::PickQuality("1080p-medium".into()), at(2));
    session.step(Input::Replanned(Some(replanned())), at(3));
    session.step(Input::FileLoaded, at(4));
    session.step(Input::Position(30.0), at(5));
    session.step(Input::LoadFailed, at(6));
    session.step(Input::Refreshed(Some(refreshed(6))), at(7));
    let commands = session.step(Input::LoadFailed, at(8));
    let [Command::Replan(Replan { operation: Operation::FailureRecovery, quality, .. })] = &commands[..] else {
        panic!("{commands:?}");
    };
    assert_eq!(quality.preference, "1080p-medium");
}

#[test]
fn a_pick_of_the_step_that_plays_is_the_quality_of_a_later_recovery() {
    let mut session = failed_at(3600.0);
    assert_eq!(session.step(Input::PickQuality("original".into()), at(2)), vec![]);
    assert_eq!(session.quality(), "original");
    session.step(Input::LoadFailed, at(3));
    session.step(Input::Refreshed(Some(refreshed(3))), at(4));
    let commands = session.step(Input::LoadFailed, at(5));
    let [Command::Replan(Replan { operation: Operation::FailureRecovery, quality, .. })] = &commands[..] else {
        panic!("{commands:?}");
    };
    assert_eq!(quality.preference, "original");
}

#[test]
fn a_version_switch_ends_the_session_at_the_position_and_starts_the_other_file_there() {
    let mut session = playing(None);
    session.step(Input::Position(300.0), at(1));
    assert_eq!(session.step(Input::SwitchVersion("42".into()), at(2)), vec![], "file 42 plays already");
    assert_eq!(
        session.step(Input::SwitchVersion("43".into()), at(2)),
        vec![stop(300.0, 1), Command::PlayVersion { file_id: "43".into(), position: Some(300.0) }]
    );
    assert_eq!(session.step(Input::Tick, at(60)), vec![], "the new file plays in a new session");
}

/// The replan answer of the own fixture: a transcode from 3600 s.
fn replanned() -> Decision {
    let path = format!("{}/tests/fixtures/own/playback_replan_failure_recovery.json", env!("CARGO_MANIFEST_DIR"));
    let body = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    Decision::from(serde_json::from_value::<relics::api::wire::PlaybackDecision>(body).unwrap())
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
    assert_eq!(session.step(Input::LoadFailed, at(6)), vec![]);
    assert_eq!(session.prompt(), LOST);
    session.step(Input::Tick, at(60));
    assert_eq!(session.prompt(), LOST, "the message stays");
    assert_eq!(session.step(Input::Action, at(61)), vec![Command::Refresh]);
    assert_eq!(session.prompt(), "");
}

#[test]
fn a_failed_replan_shows_lost_connection_and_q_sends_the_last_good_position() {
    let mut session = failed_at(3600.0);
    session.step(Input::LoadFailed, at(2));
    session.step(Input::Refreshed(None), at(3));
    session.step(Input::LoadFailed, at(4));
    assert_eq!(session.step(Input::Replanned(None), at(5)), vec![]);
    assert_eq!(session.prompt(), LOST);
    assert_eq!(session.step(Input::Shutdown, at(6)), vec![stop(3600.0, 1), Command::ClosePlayer]);
    assert_eq!(session.prompt(), "");
}

#[test]
fn an_early_end_starts_the_same_chain_and_never_reports_the_duration() {
    let mut session = failed_at(3600.0);
    assert_eq!(session.step(Input::EndOfFile, at(2)), vec![Command::Refresh]);
    session.step(Input::Refreshed(Some(refreshed(2))), at(3));
    assert_eq!(session.step(Input::EndOfFile, at(4)), vec![replan(3600.0)]);
    assert_eq!(session.step(Input::Replanned(Some(Decision::Unknown("queued".into()))), at(5)), vec![]);
    assert_eq!(session.prompt(), LOST);
    assert_eq!(session.step(Input::Shutdown, at(6)), vec![stop(3600.0, 1), Command::ClosePlayer]);
    assert_eq!(session.prompt(), "");
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

// MPRIS controls. MPRIS Stop is `Input::Shutdown`, the same input as q.

#[test]
fn mpris_play_pause_and_play_pause_set_pause() {
    let mut session = playing(None);
    assert_eq!(session.step(Input::Pause, at(1)), vec![Command::SetPause(true)]);
    assert_eq!(session.step(Input::Play, at(1)), vec![Command::SetPause(false)]);
    assert_eq!(session.step(Input::PlayPause, at(1)), vec![Command::SetPause(true)], "mpv plays");
    session.step(Input::Paused(true), at(2));
    assert!(session.paused());
    assert_eq!(session.step(Input::PlayPause, at(3)), vec![Command::SetPause(false)], "mpv is paused");
}

#[test]
fn mpris_seek_moves_from_the_position_and_set_position_goes_to_a_position_in_the_file() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::Position(300.0), at(1));
    assert_eq!(session.step(Input::SeekBy(30.0), at(2)), vec![Command::Seek(330.0)]);
    assert_eq!(session.step(Input::SeekBy(-500.0), at(2)), vec![Command::Seek(0.0)], "not before the start");
    assert_eq!(session.step(Input::SetPosition(42.0), at(3)), vec![Command::Seek(42.0)]);
    assert_eq!(session.step(Input::SetPosition(1001.0), at(3)), vec![], "past the end of the file");
    assert_eq!(session.step(Input::SetPosition(-1.0), at(3)), vec![]);
}

#[test]
fn mpris_next_plays_the_next_episode_only_when_it_exists() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::Position(300.0), at(1));
    assert!(!session.can_go_next());
    assert_eq!(session.step(Input::Next, at(2)), vec![]);
    session.step(Input::NextEpisode, at(2));
    assert!(session.can_go_next());
    assert_eq!(session.step(Input::Next, at(3)), vec![stop(400.0, 1), Command::PlayNext]);
    assert!(!session.can_go_next(), "the session ended");
}

#[test]
fn mpris_previous_plays_the_previous_episode_only_when_it_exists() {
    let mut session = with_markers(ServerSettings::default(), vec![]);
    session.step(Input::Position(300.0), at(1));
    assert!(!session.can_go_previous());
    assert_eq!(session.step(Input::Previous, at(2)), vec![]);
    session.step(Input::PreviousEpisode, at(2));
    assert!(session.can_go_previous());
    assert_eq!(session.step(Input::Previous, at(3)), vec![stop(400.0, 1), Command::PlayPrevious]);
    assert!(!session.can_go_previous(), "the session ended");
}

// The Track picker.

/// The mpv track list of an episode: English audio (it plays) and Japanese audio, an English forced subtitle
/// track and a Japanese one.
/// `japanese_first` swaps the audio ids, as the next file can order its tracks in another way.
fn track_list(japanese_first: bool) -> Vec<Track> {
    let (eng, jpn) = if japanese_first { (2, 1) } else { (1, 2) };
    tracks::parse(&format!(
        r#"[{{"id":1,"type":"video","codec":"h264","selected":true}},
        {{"id":{eng},"type":"audio","lang":"eng","codec":"aac","selected":true}},
        {{"id":{jpn},"type":"audio","lang":"jpn","codec":"aac"}},
        {{"id":1,"type":"sub","lang":"eng","forced":true,"codec":"ass"}},
        {{"id":2,"type":"sub","lang":"jpn","codec":"subrip"}}]"#
    ))
}

#[test]
fn a_pick_in_the_track_picker_selects_the_track_in_mpv_and_off_hides_the_subtitles() {
    let mut session = playing(None);
    session.step(Input::Tracks(track_list(false)), at(1));
    assert_eq!(session.step(Input::PickTrack { kind: Kind::Audio, id: Some(2) }, at(2)), vec![Command::SelectAudio(2)]);
    assert_eq!(
        session.step(Input::PickTrack { kind: Kind::Sub, id: Some(2) }, at(3)),
        vec![Command::SelectSubtitle(Some(2))]
    );
    assert_eq!(
        session.step(Input::PickTrack { kind: Kind::Sub, id: None }, at(4)),
        vec![Command::SelectSubtitle(None)]
    );
    assert_eq!(session.tracks(), &track_list(false)[..], "the picker lists what mpv reports");
}

/// The next episode after a session in which the user picked `picks`. The next plan is direct play with Japanese
/// audio. Returns the next session after its file loaded, and the subtitle commands of the load.
fn next_episode(picks: &[(Kind, Option<i64>)]) -> (Session, Vec<Command>) {
    let mut first = playing(None);
    first.step(Input::Tracks(track_list(false)), at(1));
    for (kind, id) in picks {
        first.step(Input::PickTrack { kind: *kind, id: *id }, at(2));
    }
    let mut next = Session::new(None, ServerSettings::default()).with_carry(first.carry());
    let mut decision = decision(|_| {});
    let Decision::Playable { audio_language, .. } = &mut decision else { unreachable!() };
    *audio_language = Some("ja".into());
    next.step(Input::Started { decision, access: access(), item: None }, at(10));
    let commands = next.step(Input::FileLoaded, at(10)).into_iter();
    let subtitles = commands.filter(|c| matches!(c, Command::AddSubtitle { .. } | Command::SelectSubtitle(_)));
    (next, subtitles.collect())
}

#[test]
fn the_audio_choice_carries_to_the_next_episode_by_language() {
    let (mut next, _) = next_episode(&[(Kind::Audio, Some(2))]);
    // In the next file, Japanese is track 1.
    assert_eq!(next.step(Input::Tracks(track_list(true)), at(11)), vec![Command::SelectAudio(1)]);
    assert_eq!(next.step(Input::Tracks(track_list(true)), at(12)), vec![], "the carry-over picks only once");
    // The episode after it gets the same choice, also with no new pick.
    assert_eq!(next.carry().audio, Some(Choice::Track { language: Some("jpn".into()), forced: false }));
}

#[test]
fn an_audio_track_that_already_plays_is_not_selected_again() {
    let (mut next, _) = next_episode(&[(Kind::Audio, Some(1))]);
    assert_eq!(next.step(Input::Tracks(track_list(false)), at(11)), vec![]);
}

#[test]
fn the_subtitle_choice_carries_to_the_next_episode_by_language_and_forced_state() {
    // The forced English track matches the forced English track of the next plan, its first embedded track.
    let (_, subtitles) = next_episode(&[(Kind::Sub, Some(1))]);
    assert_eq!(subtitles, vec![Command::SelectSubtitle(Some(1))]);
    // Japanese is the second embedded track of the next plan.
    let (_, subtitles) = next_episode(&[(Kind::Sub, Some(2))]);
    assert_eq!(subtitles, vec![Command::SelectSubtitle(Some(2))]);
}

#[test]
fn subtitles_turned_off_stay_off_in_the_next_episode() {
    // With Japanese audio, the default rule shows the English subtitles.
    let (_, subtitles) = next_episode(&[]);
    assert_eq!(subtitles.len(), 1, "{subtitles:?}");
    let (_, subtitles) = next_episode(&[(Kind::Sub, None)]);
    assert_eq!(subtitles, vec![]);
}

#[test]
fn a_reload_keeps_the_picked_tracks() {
    let mut session = playing(None);
    session.step(Input::Tracks(track_list(false)), at(1));
    session.step(Input::PickTrack { kind: Kind::Audio, id: Some(2) }, at(2));
    session.step(Input::PickTrack { kind: Kind::Sub, id: Some(2) }, at(2));
    session.step(Input::LoadFailed, at(3));
    let reload = session.step(Input::Refreshed(Some(access())), at(4));
    assert!(matches!(&reload[..], [Command::Load { .. }]), "{reload:?}");
    let loaded = session.step(Input::FileLoaded, at(5));
    assert_eq!(loaded, vec![Command::SelectSubtitle(Some(2))], "no sidecar of the start");
    // mpv reports the tracks of the reloaded file, with the tracks of the start.
    assert_eq!(
        session.step(Input::Tracks(track_list(false)), at(6)),
        vec![Command::SelectAudio(2), Command::SelectSubtitle(Some(2))]
    );
}
