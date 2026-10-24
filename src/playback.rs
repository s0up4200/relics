//! The Playback session: one viewing of one Item, from the start request to the stop
//! report. It is a pure state machine with one entry point, `step`. The caller gives
//! it each input with the wall-clock time and carries out the commands that it
//! returns. It holds no clock and does no I/O, so the tests reach it without mpv.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::api::types::{Decision, Delivery, Marker, MarkerKind};
use crate::api::wire;
use crate::settings::{IntroSkipMode, ServerSettings, SubtitleAppearance};

pub mod playing;
pub mod subtitles;
pub mod tracks;

use tracks::{Carry, Choice, Kind, Track};

/// The time between two progress reports while the file is open, playing or paused.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// An end of file this close to the duration, in seconds, is a Normal end.
const NORMAL_END_SECONDS: f64 = 8.0;
/// An end of file at this fraction of the duration or more is a Normal end.
const NORMAL_END_FRACTION: f64 = 0.985;
/// How long the Skip prompt shows.
const PROMPT_TIME: Duration = Duration::from_secs(5);
/// Credits that end this close to the end of the file, in seconds, run to the end of the file.
const CREDITS_TO_END_SECONDS: f64 = 1.0;
/// How long the Up Next card counts down before the next Item plays. Without end credits, the card shows this long
/// before the end of the file.
const UP_NEXT_TIME: Duration = Duration::from_secs(10);
/// After this many Items in a row play by themselves with no input of the user, the Still watching prompt shows in
/// place of the next one. It is the default of the `player.passout_threshold` setting of the settings contract.
// ponytail: a fixed count; read the client_local setting when the app gets a Local setting for it.
pub const STILL_WATCHING_AFTER: u32 = 3;
/// Before each load, the app refreshes a token that has less time than this left.
const REFRESH_BEFORE_LOAD: Duration = Duration::from_secs(4 * 3600);
/// When the token in mpv has this much time left, the app refreshes it and reloads at the position.
const RENEW_BEFORE_EXPIRY: Duration = Duration::from_secs(600);
/// A reloaded stream that plays this many seconds is healthy again, and the next failure starts a new chain.
// ponytail: a fixed distance; a stream that fails every few seconds restarts the chain only by Enter.
const HEALTHY_SECONDS: f64 = 10.0;
/// The prompt text that the Player view shows when the recovery chain gave up.
const LOST: &str = "Lost connection to the server";

/// HTTP headers as (name, value).
pub type Headers = Vec<(String, String)>;

/// The headers of the media requests, and when the access token in them expires.
#[derive(Debug, Clone, PartialEq)]
pub struct Access {
    pub headers: Headers,
    pub expires: Option<SystemTime>,
}

/// What a `replan` request needs to know about the plan that failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Replan {
    pub session_id: String,
    /// The `playback_attempt_id` of the start request.
    pub attempt_id: String,
    pub failed_plan_id: String,
    pub plan_attempt_key: String,
    /// The keys of each plan that failed in this Playback session, the failed plan last.
    pub attempted_plan_keys: Vec<String>,
    /// The `selected_tracks` of the failed plan.
    pub selected_tracks: serde_json::Value,
    /// The last good source position.
    pub position: f64,
    pub operation: Operation,
    /// The quality of the Playback session.
    pub quality: Quality,
}

/// The quality that a start request or a replan asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Quality {
    /// The `quality_preference`: the preferred quality, or the step label of a Quality ladder pick.
    pub preference: String,
    /// The `bandwidth_cap_kbps`. `None` is no limit.
    pub bandwidth_cap_kbps: Option<u64>,
}

/// The step of the Quality ladder that plays in `plan`, for the quality preference `quality` of the session.
///
/// - A step with the label of the preference plays.
/// - Else, when the server keeps the source video, the source step plays.
/// - Else the step at the bitrate of the transcode plays. When two steps have that bitrate, the step at the height
///   of the transcode plays. With `auto` or a plain resolution, the server encodes at the bitrate of the Medium step
///   of the resolution.
// ponytail: an exact bitrate match. Under a bitrate cap the server encodes below each step, so no step is marked.
// Read a step label from the plan when the server sends one.
pub fn current_step<'a>(plan: &'a wire::PlaybackPlan, quality: &str) -> Option<&'a wire::AvailableQualityV3> {
    let steps = &plan.available_qualities;
    if let Some(step) = steps.iter().find(|step| step.label == quality) {
        return Some(step);
    }
    if Delivery::from(plan.delivery.clone()) != Delivery::ServerTranscodeHls {
        return steps.iter().find(|step| step.preserves_source);
    }
    let delivered = &plan.effective_recipe;
    let at_bitrate = || {
        steps
            .iter()
            .filter(|s| !s.preserves_source && delivered.bitrate_kbps.is_some_and(|b| s.bitrate_kbps == Some(b)))
    };
    at_bitrate().find(|step| step.height == delivered.height).or_else(|| at_bitrate().next())
}

/// Why the app asks for a new plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Operation {
    /// The plan failed: `failure_recovery`.
    FailureRecovery,
    /// The user picked a step on the Quality ladder: `quality_change`.
    QualityChange,
    /// The subtitle track that the Server draws into the video changes: `track_change`.
    TrackChange,
}

#[derive(Debug)]
pub enum Input {
    /// The server answered the start request. The headers of `access` go on each media request. `item` is the
    /// Item detail when it arrived in time. Its subtitle values go over the Server settings of the session.
    Started {
        decision: Decision,
        access: Access,
        item: Option<Arc<wire::CatalogItemDetail>>,
    },
    /// A token refresh ended. `None` means that it failed.
    Refreshed(Option<Access>),
    /// A replan ended. `None` means that the request failed.
    Replanned(Option<Decision>),
    /// The start request failed. The value is the text for the user.
    StartFailed(String),
    /// mpv opened the file of this session.
    FileLoaded,
    /// The mpv `time-pos`, in player time.
    Position(f64),
    /// The mpv `duration`, in player time.
    Duration(f64),
    Paused(bool),
    /// mpv reached the end of the file.
    EndOfFile,
    /// mpv could not open or read the file.
    LoadFailed,
    /// mpv quit, or the user sent the MPRIS Stop.
    Shutdown,
    /// The app ends the session: Back in the Player view, a new Item replaces it, or the Library window closes.
    End,
    /// The periodic tick of the caller.
    Tick,
    /// The user pressed Enter or clicked the button that shows: the Skip prompt, Play now on the Up Next card,
    /// Continue on the Still watching prompt, or the retry after `LOST`.
    Action,
    /// The Item has a next episode.
    NextEpisode,
    /// The Item is a pick of a server shuffle with a next pick. The next pick plays at the end, also when the Server
    /// settings do not auto-play the next episode.
    Shuffle,
    /// The Item has a previous episode.
    PreviousEpisode,
    /// mpv ended a seek at this player position, or started the file there.
    Seeked(f64),
    /// MPRIS Play.
    Play,
    /// MPRIS Pause.
    Pause,
    /// MPRIS PlayPause.
    PlayPause,
    /// MPRIS Seek: move this many seconds from the position.
    SeekBy(f64),
    /// MPRIS SetPosition: go to this player position.
    SetPosition(f64),
    /// MPRIS Next, or Skip in the Player view.
    Next,
    /// MPRIS Previous.
    Previous,
    /// mpv reports the tracks of the file.
    Tracks(Vec<Track>),
    /// The user picked a track of `kind` in the Track picker. `None` turns the subtitles off.
    PickTrack {
        kind: Kind,
        id: Option<i64>,
    },
    /// The user picked the subtitle track with this `combined_index` of the plan, which the Server must draw
    /// into the video.
    PickBurnIn(i64),
    /// The user picked this step label of the Quality ladder.
    PickQuality(String),
    /// The user picked the file version with this file id.
    SwitchVersion(String),
    /// The user cancelled the Up Next card: the next Item does not play at the end.
    CancelUpNext,
    /// The user pressed a key, clicked, scrolled or moved the mouse in the Player view.
    Active,
}

impl Input {
    /// The input comes from the user, so the user is not away.
    fn is_user_input(&self) -> bool {
        matches!(
            self,
            Input::Active
                | Input::Action
                | Input::Play
                | Input::Pause
                | Input::PlayPause
                | Input::SeekBy(_)
                | Input::SetPosition(_)
                | Input::Next
                | Input::Previous
                | Input::PickTrack { .. }
                | Input::PickBurnIn(_)
                | Input::PickQuality(_)
                | Input::SwitchVersion(_)
                | Input::CancelUpNext
        )
    }
}

/// One progress sample. `position` is the source position in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub sequence: u64,
    pub position: f64,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Open `url` in mpv with these headers on each media request, at `start` seconds of player time. `aid` is the
    /// mpv audio track to play, when mpv must pick it.
    Load { url: String, headers: Headers, start: f64, aid: Option<i64> },
    /// Send `updatePlaybackProgress`.
    Progress { session_id: String, sample: Sample },
    /// Send `stopPlayback`, with the final sample when the session has one.
    Stop { session_id: String, sample: Option<Sample> },
    /// Destroy the mpv handle, which closes the Player view.
    ClosePlayer,
    /// Show this text to the user.
    ShowError(String),
    /// Add the subtitle file at `url` to mpv and show it.
    AddSubtitle { url: String, title: String, language: String },
    /// Show the subtitle track with this mpv track id. `None` hides the subtitles.
    SelectSubtitle(Option<i64>),
    /// Play the audio track with this mpv track id.
    SelectAudio(i64),
    /// Seek to this player position.
    Seek(f64),
    /// Play the next episode in the same Player view. The session has already ended.
    PlayNext,
    /// Play the previous episode in the same Player view. The session has already ended.
    PlayPrevious,
    /// Set the mpv `pause` property.
    SetPause(bool),
    /// Refresh the access token and send `Input::Refreshed`.
    Refresh,
    /// Ask the server for a new plan and send `Input::Replanned`.
    Replan(Replan),
    /// Start the file version `file_id` of the same Item at this source position, in the same Player view. The
    /// session has already ended. The replan request names no file, so a version switch is a new start.
    PlayVersion { file_id: String, position: Option<f64> },
}

/// The recovery chain: a failure starts it, and each next failure goes one step on.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Chain {
    Healthy,
    /// The refresh after the failure runs.
    Refreshing,
    /// The app reloaded the plan with the refreshed token.
    Reloaded,
    /// The replan runs.
    Replanning,
    /// The app loaded the new plan.
    Replanned,
    /// The Player view shows the `LOST` text as the prompt. Enter starts the chain again.
    Lost,
}

/// A replan that the user asked for, while it runs.
enum Change {
    /// A step of the Quality ladder, with its label.
    Quality(String),
    /// The subtitle track that the Server draws into the video, or `None` for no burn-in.
    Subtitle(Option<i64>),
}

/// What the app does when a refresh outside the recovery chain ends.
enum AfterRefresh {
    /// Load the plan at this player position, also when the refresh failed.
    Load(f64),
    /// Reload at the position that plays then, only when the refresh worked.
    Renew,
}

pub struct Session {
    /// The Item runtime in seconds. The end rule uses it when mpv has no duration.
    runtime: Option<f64>,
    /// The Server settings of the Profile when the session started, with the subtitle values of the Item over them.
    settings: ServerSettings,
    /// The subtitle track that the web player remembers for the series of the Item, from the Item detail.
    remembered_subtitle: Option<wire::WatchSubtitleSignature>,
    plan: Option<Plan>,
    ended: bool,
    /// mpv opened the file of the last load. The session drops the events of the old file until then.
    loaded: bool,
    position: Option<f64>,
    duration: Option<f64>,
    paused: bool,
    sequence: u64,
    last_report: Option<SystemTime>,
    /// The index of the Marker that holds the position.
    inside: Option<usize>,
    /// The Markers that the user or an automatic skip already decided on. They do nothing more.
    resolved: Vec<usize>,
    prompt: Option<Prompt>,
    has_next: bool,
    /// The next Item is the next pick of a shuffle.
    shuffle: bool,
    has_previous: bool,
    /// The headers for the next load.
    access: Option<Access>,
    /// When the token in the file that mpv plays expires.
    mpv_expires: Option<SystemTime>,
    /// The token in mpv was renewed or its renewal failed. A load resets it.
    renewed: bool,
    after_refresh: Option<AfterRefresh>,
    chain: Chain,
    /// The player position of the last load.
    load_start: f64,
    /// The language of the audio that the server picked at the start. A replan keeps it.
    audio_language: Option<String>,
    /// The keys of the plans that failed.
    failed_keys: Vec<String>,
    /// The tracks of the file, as mpv last reported them.
    tracks: Vec<Track>,
    /// The track choice of the Track picker, in this session or carried from the last episode.
    carry: Carry,
    /// The burn-in of the last subtitle pick in this session: the picked track, or `None` for Off or a track of
    /// mpv.
    burn_pick: Option<Option<i64>>,
    /// The part of `carry` that mpv has not played yet in the file that loads.
    pending: Carry,
    /// The quality preference of the session: the preferred quality, or the step of a Quality ladder pick.
    quality: String,
    /// The replan of the user that runs.
    changing: Option<Change>,
    /// The Up Next card shows, the next Item plays at this time, and the card shows this many seconds left.
    up_next: Option<(SystemTime, u64)>,
    /// The user cancelled the Up Next card, so the next Item does not play by itself.
    cancelled: bool,
    /// How many Items in a row played by themselves, with no input of the user, up to this one.
    auto_plays: u32,
    /// The Still watching prompt shows. The value is the final position for the stop report.
    still_watching: Option<Option<f64>>,
}

/// The Skip prompt that shows now.
struct Prompt {
    label: String,
    action: Action,
    until: SystemTime,
    /// The prompt stays when the position leaves its Marker. The "Watch intro" prompt shows after the skip.
    stays: bool,
}

enum Action {
    /// Seek to this player position and resolve the Marker.
    Seek {
        to: f64,
        marker: usize,
    },
    Next,
}

struct Plan {
    session_id: String,
    attempt_id: String,
    plan_id: String,
    plan_attempt_key: String,
    selected_tracks: serde_json::Value,
    url: String,
    /// The mpv audio track to play.
    aid: Option<i64>,
    /// Add it to a player position to get the source position.
    offset: f64,
    /// The command that shows the subtitle track of the subtitle choice, when the file loads.
    subtitle: Option<Command>,
    /// The subtitle track that the Server must draw into the video for the start choice, when no pick of the user
    /// says otherwise: the track that this plan burns in, or the choice when it needs a burn-in.
    burn_in: Option<i64>,
    /// The app asked the Server to fix the burn-in of this plan, so it does not ask again. A pick asks again.
    asked: bool,
    /// The Markers in player time.
    markers: Vec<Marker>,
    /// The plan as the server sent it, for the Playback info panel.
    wire: wire::PlaybackPlan,
}

impl Session {
    pub fn new(runtime: Option<f64>, settings: ServerSettings) -> Self {
        Self {
            runtime,
            quality: settings.preferred_quality.clone(),
            settings,
            remembered_subtitle: None,
            plan: None,
            ended: false,
            loaded: false,
            position: None,
            duration: None,
            paused: false,
            sequence: 0,
            last_report: None,
            inside: None,
            resolved: Vec::new(),
            prompt: None,
            has_next: false,
            shuffle: false,
            has_previous: false,
            access: None,
            mpv_expires: None,
            renewed: false,
            after_refresh: None,
            chain: Chain::Healthy,
            load_start: 0.0,
            audio_language: None,
            failed_keys: Vec::new(),
            tracks: Vec::new(),
            carry: Carry::default(),
            burn_pick: None,
            pending: Carry::default(),
            changing: None,
            up_next: None,
            cancelled: false,
            auto_plays: 0,
            still_watching: None,
        }
    }

    /// Starts the session after `count` Items in a row played by themselves, with no input of the user.
    pub fn with_auto_plays(mut self, count: u32) -> Self {
        self.auto_plays = count;
        self
    }

    /// How many Items in a row played by themselves, with no input of the user. The next Item starts with it.
    pub fn auto_plays(&self) -> u32 {
        self.auto_plays
    }

    /// Starts the session with the track choice of the last episode of the same series.
    pub fn with_carry(mut self, carry: Carry) -> Self {
        self.carry = carry;
        self
    }

    /// The track choice to carry to the next episode.
    pub fn carry(&self) -> Carry {
        self.carry.clone()
    }

    /// The Subtitle appearance of the Server settings that the session started with.
    pub fn subtitle_appearance(&self) -> &SubtitleAppearance {
        &self.settings.subtitle_appearance
    }

    /// The tracks of the file, for the Track picker.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The plan that plays, for the Playback info panel and the Quality ladder.
    pub fn plan(&self) -> Option<&wire::PlaybackPlan> {
        self.plan.as_ref().map(|plan| &plan.wire)
    }

    /// The quality preference of the session: the preferred quality, or the step of a Quality ladder pick.
    pub fn quality(&self) -> &str {
        &self.quality
    }

    /// What a replan asks for: the quality preference of the session and the bitrate cap that it started with.
    fn replan_quality(&self) -> Quality {
        Quality { preference: self.quality.clone(), ..self.settings.quality() }
    }

    pub fn step(&mut self, input: Input, now: SystemTime) -> Vec<Command> {
        if self.ended {
            return Vec::new();
        }
        if input.is_user_input() {
            self.auto_plays = 0;
        }
        match input {
            Input::Started { decision, access, item } => {
                if let Some(item) = item {
                    self.settings.apply_item(&item);
                    self.remembered_subtitle = item.effective_subtitle_track_signature.clone();
                }
                match decision {
                    Decision::Playable {
                        session_id,
                        plan,
                        audio_language,
                        markers,
                        attempt_id,
                        quality_preference,
                    } => {
                        // The start request gives the quality of the session: `original` after a refused transcode.
                        if let Some(quality) = quality_preference {
                            self.quality = quality;
                        }
                        self.audio_language = audio_language;
                        self.access = Some(access);
                        let start = plan.timeline.player_start_seconds;
                        self.adopt(session_id, attempt_id, &plan, markers);
                        self.load(start, now)
                    }
                    Decision::Unavailable { message, .. } => self.fail(message),
                    Decision::Unknown(outcome) => {
                        self.fail(format!("The Silo server gave an answer that Relics does not know: {outcome}"))
                    }
                }
            }
            Input::StartFailed(message) => self.fail(message),
            Input::Refreshed(access) => {
                let worked = access.is_some();
                if access.is_some() {
                    self.access = access;
                }
                let after = self.after_refresh.take();
                if self.chain == Chain::Refreshing {
                    self.chain = Chain::Reloaded;
                    return self.load_now(self.position.unwrap_or(self.load_start));
                }
                match after {
                    Some(AfterRefresh::Load(start)) => self.load_now(start),
                    Some(AfterRefresh::Renew) if worked && self.chain == Chain::Healthy => {
                        self.load_now(self.position.unwrap_or(self.load_start))
                    }
                    _ => Vec::new(),
                }
            }
            Input::Replanned(decision) if self.chain == Chain::Replanning => match decision {
                Some(Decision::Playable { session_id, plan, attempt_id, quality_preference, .. }) => {
                    self.chain = Chain::Replanned;
                    // A recovery can end in a new start, with its own attempt and quality.
                    if let Some(quality) = quality_preference {
                        self.quality = quality;
                    }
                    self.play_replan(session_id, attempt_id, &plan)
                }
                _ => self.lose(),
            },
            Input::Replanned(decision) if self.changing.is_some() => {
                let change = self.changing.take();
                match decision {
                    Some(Decision::Playable { session_id, plan, attempt_id, .. }) => {
                        let commands = self.play_replan(session_id, attempt_id, &plan);
                        match change {
                            Some(Change::Quality(label)) => self.quality = label,
                            // If the Server did not do what the app asked, the app does not ask again. A pick
                            // while the replan ran still asks.
                            Some(Change::Subtitle(asked)) => {
                                let wanted = self.wanted_burn_in();
                                if let Some(plan) = &mut self.plan {
                                    plan.asked = wanted == asked;
                                }
                            }
                            None => {}
                        }
                        commands
                    }
                    // The old plan still plays.
                    _ => Vec::new(),
                }
            }
            Input::Replanned(_) => Vec::new(),
            Input::FileLoaded => {
                self.loaded = true;
                self.last_report = Some(now);
                // The user turned the subtitles off in the Track picker.
                let off = self.carry.subtitle == Some(Choice::Off);
                let subtitle = self.plan.as_ref().and_then(|plan| plan.subtitle.clone()).filter(|_| !off);
                let mut commands: Vec<Command> = subtitle.into_iter().collect();
                commands.extend(self.play_carry());
                commands.extend(self.ask_burn_in());
                commands
            }
            Input::Position(position) if self.loaded => {
                self.position = Some(position);
                if matches!(self.chain, Chain::Reloaded | Chain::Replanned)
                    && position >= self.load_start + HEALTHY_SECONDS
                {
                    self.chain = Chain::Healthy;
                }
                let mut commands = self.enter(position, now);
                // A recovery holds the ask for a burn-in until the stream is healthy again.
                commands.extend(self.ask_burn_in());
                if !self.ended {
                    self.count_down(position, now);
                }
                commands
            }
            Input::Seeked(position) if self.loaded => {
                self.position = Some(position);
                Vec::new()
            }
            Input::Duration(duration) if self.loaded => {
                self.duration = Some(duration);
                Vec::new()
            }
            Input::Paused(paused) => {
                self.paused = paused;
                Vec::new()
            }
            Input::Tick => {
                if let Some((until, _)) = self.up_next {
                    if now >= until {
                        return self.auto_advance(self.source_position());
                    }
                    let left = until.duration_since(now).unwrap_or_default().as_secs_f64().ceil() as u64;
                    self.up_next = Some((until, left));
                }
                if self.prompt.as_ref().is_some_and(|prompt| now >= prompt.until) {
                    self.prompt = None;
                }
                let mut commands = self.report(now);
                commands.extend(self.renew(now));
                commands
            }
            Input::Action if self.chain == Chain::Lost => {
                self.chain = Chain::Healthy;
                self.recover()
            }
            Input::Action if self.still_watching.is_some() => {
                let position = self.still_watching.unwrap_or_default();
                let mut commands = vec![Command::SetPause(false)];
                commands.extend(self.play_next(position));
                commands
            }
            Input::Action if self.up_next.is_some() => self.play_next(self.source_position()),
            Input::CancelUpNext => {
                if self.up_next.take().is_some() {
                    self.cancelled = true;
                }
                Vec::new()
            }
            Input::Action => match self.prompt.take() {
                Some(Prompt { action: Action::Seek { to, marker }, .. }) => {
                    self.resolved.push(marker);
                    vec![Command::Seek(to)]
                }
                Some(prompt @ Prompt { action: Action::Next, .. }) => {
                    // finish() hides the prompt.
                    self.prompt = Some(prompt);
                    self.play_next(self.source_position())
                }
                None => Vec::new(),
            },
            Input::NextEpisode => {
                self.has_next = true;
                Vec::new()
            }
            Input::Shuffle => {
                (self.has_next, self.shuffle) = (true, true);
                Vec::new()
            }
            Input::PreviousEpisode => {
                self.has_previous = true;
                Vec::new()
            }
            Input::Play => vec![Command::SetPause(false)],
            Input::Pause => vec![Command::SetPause(true)],
            Input::PlayPause => vec![Command::SetPause(!self.paused)],
            Input::SeekBy(seconds) => match self.position {
                Some(position) => vec![Command::Seek((position + seconds).max(0.0))],
                None => Vec::new(),
            },
            // MPRIS ignores a position outside the file.
            Input::SetPosition(position)
                if position >= 0.0 && self.player_duration().is_none_or(|duration| position <= duration) =>
            {
                vec![Command::Seek(position)]
            }
            Input::Next if self.has_next => self.play_next(self.stop_position()),
            Input::Previous if self.has_previous => {
                let mut commands = self.finish(self.source_position(), false);
                commands.push(Command::PlayPrevious);
                commands
            }
            // An end before the file loads is the end of the old file. mpv can send it after the next load returns.
            Input::EndOfFile if !self.loaded || self.still_watching.is_some() => Vec::new(),
            Input::EndOfFile => {
                let (position, normal) = self.end_position();
                if normal && self.auto_next() && !self.cancelled {
                    self.auto_advance(position)
                } else if normal {
                    self.finish(position, true)
                } else {
                    // An Early end: the stream usually lost access.
                    self.recover()
                }
            }
            Input::Tracks(tracks) => {
                self.tracks = tracks;
                if self.loaded { self.play_carry() } else { Vec::new() }
            }
            Input::PickTrack { kind, id } => {
                let track = id.and_then(|id| self.tracks.iter().find(|track| track.kind == kind && track.id == id));
                let picked = if id.is_none() { Some(Choice::Off) } else { track.map(Choice::of) };
                // A reload of the file shows the picked track of the file, not the sidecar of the start. A picked
                // external track is that sidecar.
                if let (Kind::Sub, Some(track), Some(plan)) = (kind, track, &mut self.plan)
                    && !track.external
                {
                    plan.subtitle = Some(Command::SelectSubtitle(Some(track.id)));
                }
                let (choice, pending) = match kind {
                    Kind::Audio => (&mut self.carry.audio, &mut self.pending.audio),
                    Kind::Sub => (&mut self.carry.subtitle, &mut self.pending.subtitle),
                    Kind::Other => return Vec::new(),
                };
                *pending = None;
                let picked_any = picked.is_some();
                if picked_any {
                    *choice = picked;
                }
                match kind {
                    Kind::Audio => id.map(Command::SelectAudio).into_iter().collect(),
                    _ => {
                        // On a transcode, the Server stops a burn-in, so that only the picked track shows. In direct
                        // play, the pick of a track of the file is the choice, and a later transcode can burn it in.
                        if picked_any && self.direct() {
                            self.burn_pick = None;
                        } else if picked_any {
                            self.pick_burn_in(None);
                        }
                        [vec![Command::SelectSubtitle(id)], self.ask_burn_in()].concat()
                    }
                }
            }
            Input::PickBurnIn(index) => {
                let Some(plan) = &self.plan else {
                    return Vec::new();
                };
                let Some(track) = plan.wire.subtitle.inventory.iter().find(|track| track.combined_index == index)
                else {
                    return Vec::new();
                };
                let burned = subtitles::burned_in(&plan.wire) == Some(index);
                let choice = Choice::Track { language: track.language.clone(), forced: track.forced };
                (self.carry.subtitle, self.pending.subtitle) = (Some(choice), None);
                self.pick_burn_in(Some(index));
                // The burn-in replaces the subtitles that mpv shows.
                let hide = (!burned).then_some(Command::SelectSubtitle(None));
                [hide.into_iter().collect(), self.ask_burn_in()].concat()
            }
            Input::PickQuality(label) => {
                let Some(plan) = &self.plan else {
                    return Vec::new();
                };
                let plays = current_step(&plan.wire, &self.quality).is_some_and(|step| step.label == label);
                if self.changing.is_some() || self.chain != Chain::Healthy {
                    return Vec::new();
                }
                // The step plays already, so it needs no replan. A later recovery still asks for it.
                if plays {
                    self.quality = label;
                    return Vec::new();
                }
                let (tracks, quality) =
                    (plan.selected_tracks.clone(), Quality { preference: label.clone(), ..self.settings.quality() });
                self.change(Change::Quality(label), Operation::QualityChange, tracks, quality)
            }
            Input::SwitchVersion(file_id) => {
                if self.plan.as_ref().is_none_or(|plan| *plan.wire.effective_media_file_id == file_id) {
                    return Vec::new();
                }
                let position = self.source_position();
                let mut commands = self.finish(position, false);
                commands.push(Command::PlayVersion { file_id, position });
                commands
            }
            Input::LoadFailed => self.recover(),
            Input::Shutdown => self.finish(self.stop_position(), true),
            Input::End => self.finish(self.stop_position(), false),
            Input::Active => Vec::new(),
            Input::Position(_)
            | Input::Duration(_)
            | Input::Seeked(_)
            | Input::SetPosition(_)
            | Input::Next
            | Input::Previous => Vec::new(),
        }
    }

    /// mpv is paused, as far as the session knows.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// MPRIS `CanGoNext`: the session plays and the Item has a next episode.
    pub fn can_go_next(&self) -> bool {
        self.has_next && !self.ended
    }

    /// MPRIS `CanGoPrevious`: the session plays and the Item has a previous episode.
    pub fn can_go_previous(&self) -> bool {
        self.has_previous && !self.ended
    }

    /// The player position, once the file of this session plays.
    pub fn position(&self) -> Option<f64> {
        self.position
    }

    /// The Markers of the file in player time, for the seek bar of the Player view.
    pub fn markers(&self) -> &[Marker] {
        self.plan.as_ref().map_or(&[], |plan| &plan.markers)
    }

    /// The label of the Skip prompt that shows, or the `LOST` text when the recovery chain gave up. Empty when
    /// none shows.
    pub fn prompt(&self) -> &str {
        match &self.prompt {
            _ if self.ended => "",
            Some(prompt) => &prompt.label,
            None if self.chain == Chain::Lost => LOST,
            None => "",
        }
    }

    /// The seconds left before the next Item plays, while the Up Next card shows.
    pub fn up_next(&self) -> Option<u64> {
        self.up_next.map(|(_, left)| left)
    }

    /// The Still watching prompt shows.
    pub fn still_watching(&self) -> bool {
        self.still_watching.is_some()
    }

    /// Makes `plan` the plan of the session. `markers` are in source time.
    fn adopt(&mut self, session_id: String, attempt_id: String, plan: &wire::PlaybackPlan, markers: Vec<Marker>) {
        let tracks = &plan.subtitle.inventory;
        let default = || {
            subtitles::choose(tracks, &self.settings, self.audio_language.as_deref(), self.remembered_subtitle.as_ref())
        };
        // The track that the Server draws into the video is the subtitle choice, so mpv shows no other.
        let chosen = match &self.carry.subtitle {
            _ if subtitles::burned_in(plan).is_some() => None,
            Some(choice @ Choice::Track { .. }) => subtitles::carried(tracks, choice).or_else(default),
            _ => default(),
        };
        // Direct play streams the file itself, with each of its tracks. mpv counts the tracks of a kind from 1.
        let direct = Delivery::from(plan.delivery.clone()) == Delivery::OriginalHttp;
        let aid = plan.selected_tracks.audio.as_ref().and_then(|audio| audio.index).filter(|_| direct).map(|i| i + 1);
        let chosen = tracks.iter().find(|track| Some(track.combined_index) == chosen);
        let burn_in = subtitles::burned_in(plan).or_else(|| {
            chosen.filter(|track| !direct && subtitles::needs_burn_in(track)).map(|track| track.combined_index)
        });
        let subtitle = chosen.and_then(|track| {
            if direct && track.source == "embedded" {
                // The file has the track, so a sidecar of it would show twice. The server lists the embedded
                // tracks in the order of the file.
                let before =
                    tracks.iter().filter(|t| t.source == "embedded" && t.combined_index < track.combined_index).count();
                return Some(Command::SelectSubtitle(Some(before as i64 + 1)));
            }
            Some(Command::AddSubtitle {
                url: track.url.clone()?,
                title: track.label.clone().unwrap_or_default(),
                language: track.language.clone().unwrap_or_default(),
            })
        });
        let offset = plan.timeline.timeline_offset_seconds;
        let markers = markers
            .into_iter()
            .map(|marker| Marker { start: marker.start - offset, end: marker.end - offset, ..marker })
            .collect();
        self.plan = Some(Plan {
            session_id,
            attempt_id,
            plan_id: plan.plan_id.clone(),
            plan_attempt_key: plan.plan_attempt_key.clone(),
            selected_tracks: serde_json::to_value(&plan.selected_tracks).unwrap_or_default(),
            url: plan.stream.url.clone(),
            aid,
            offset,
            subtitle,
            burn_in,
            asked: false,
            markers,
            wire: plan.clone(),
        });
    }

    /// Plays the new plan of a replan at the source position that plays now. The Markers keep their source time.
    fn play_replan(&mut self, session_id: String, attempt_id: String, plan: &wire::PlaybackPlan) -> Vec<Command> {
        let Some(old) = &self.plan else {
            return Vec::new();
        };
        let source = self.position.unwrap_or(self.load_start) + old.offset;
        // The Markers come in source time. The plan keeps them in player time.
        let markers = old
            .markers
            .iter()
            .map(|marker| Marker { start: marker.start + old.offset, end: marker.end + old.offset, ..marker.clone() })
            .collect();
        // A replan keeps the attempt. A new start after the session ended gives a new one.
        let attempt_id = if attempt_id.is_empty() { old.attempt_id.clone() } else { attempt_id };
        self.adopt(session_id, attempt_id, plan, markers);
        let start = (source - plan.timeline.timeline_offset_seconds).max(0.0);
        self.position = Some(start);
        self.load_now(start)
    }

    /// The subtitle track that the Server must draw into the video of the plan, or `None` for no burn-in: the
    /// last pick of the user, else no burn-in when the subtitles are off, else the burn-in of the start choice.
    /// Direct play shows the track from the file, so it wants no burn-in.
    fn wanted_burn_in(&self) -> Option<i64> {
        match self.burn_pick {
            _ if self.direct() => None,
            Some(pick) => pick,
            None if self.carry.subtitle == Some(Choice::Off) => None,
            None => self.plan.as_ref().and_then(|plan| plan.burn_in),
        }
    }

    /// The plan is direct play: mpv plays the file itself, with each of its tracks.
    fn direct(&self) -> bool {
        self.plan.as_ref().is_some_and(|plan| Delivery::from(plan.wire.delivery.clone()) == Delivery::OriginalHttp)
    }

    /// Records a subtitle pick of the user: `index` is the track that the Server must draw into the video, or
    /// `None` for no burn-in. The plan can ask again.
    fn pick_burn_in(&mut self, index: Option<i64>) {
        self.burn_pick = Some(index);
        if let Some(plan) = &mut self.plan {
            plan.asked = false;
        }
    }

    /// Asks the Server once for each plan to fix its burn-in when it is not the wanted burn-in. While another
    /// replan runs, the ask waits for the next load or position.
    fn ask_burn_in(&mut self) -> Vec<Command> {
        let wanted = self.wanted_burn_in();
        match &self.plan {
            Some(plan) if !plan.asked && wanted != subtitles::burned_in(&plan.wire) => self.change_subtitle(wanted),
            _ => Vec::new(),
        }
    }

    /// Asks the Server to draw the subtitle track `index` into the video, or to stop the burn-in with `None`.
    /// Nothing happens while another replan runs.
    fn change_subtitle(&mut self, index: Option<i64>) -> Vec<Command> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        if self.changing.is_some() || self.chain != Chain::Healthy {
            return Vec::new();
        }
        let mut tracks = plan.wire.selected_tracks.clone();
        // The server finds the track of the effective file by its index.
        tracks.subtitle = index.map(|index| wire::TrackIdentityV3 { id: String::new(), index: Some(index) });
        let tracks = serde_json::to_value(&tracks).unwrap_or_default();
        if let Some(plan) = &mut self.plan {
            plan.asked = true;
        }
        self.change(Change::Subtitle(index), Operation::TrackChange, tracks, self.replan_quality())
    }

    /// Asks for the replan `change` of the user at the position that plays.
    fn change(
        &mut self,
        change: Change,
        operation: Operation,
        tracks: serde_json::Value,
        quality: Quality,
    ) -> Vec<Command> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let replan = Replan {
            session_id: plan.session_id.clone(),
            attempt_id: plan.attempt_id.clone(),
            failed_plan_id: plan.plan_id.clone(),
            plan_attempt_key: plan.plan_attempt_key.clone(),
            // Nothing failed, so each plan is a choice again.
            attempted_plan_keys: Vec::new(),
            selected_tracks: tracks,
            position: self.position.unwrap_or(self.load_start) + plan.offset,
            operation,
            quality,
        };
        self.changing = Some(change);
        vec![Command::Replan(replan)]
    }

    /// Loads the plan at `start`, after a refresh when the token has less than 4 hours left.
    fn load(&mut self, start: f64, now: SystemTime) -> Vec<Command> {
        let expires = self.access.as_ref().and_then(|access| access.expires);
        if expires.is_some_and(|expires| expires < now + REFRESH_BEFORE_LOAD) {
            self.after_refresh = Some(AfterRefresh::Load(start));
            return vec![Command::Refresh];
        }
        self.load_now(start)
    }

    fn load_now(&mut self, start: f64) -> Vec<Command> {
        let (Some(plan), Some(access)) = (&self.plan, &self.access) else {
            return Vec::new();
        };
        // Drops the events of the file that this load replaces.
        self.loaded = false;
        self.load_start = start;
        self.mpv_expires = access.expires;
        self.renewed = false;
        // mpv resets the tracks on each load, so the Track picker choice must play again in the new file.
        self.pending = self.carry.clone();
        self.tracks.clear();
        vec![Command::Load { url: plan.url.clone(), headers: access.headers.clone(), start, aid: plan.aid }]
    }

    /// Refreshes the token in mpv when it has 10 minutes left. The reload follows the refresh.
    fn renew(&mut self, now: SystemTime) -> Vec<Command> {
        let due = self.mpv_expires.is_some_and(|expires| now + RENEW_BEFORE_EXPIRY >= expires);
        if !due || self.renewed || !self.loaded || self.chain != Chain::Healthy || self.after_refresh.is_some() {
            return Vec::new();
        }
        self.renewed = true;
        self.after_refresh = Some(AfterRefresh::Renew);
        vec![Command::Refresh]
    }

    /// The recovery chain after a load failure or an Early end: one refresh and one reload at the last good
    /// position, then one replan, then the `LOST` text. A failure while a step runs does nothing.
    fn recover(&mut self) -> Vec<Command> {
        let Some(plan) = &self.plan else {
            return self.finish(None, true);
        };
        let good = self.position.unwrap_or(self.load_start);
        match self.chain {
            Chain::Healthy => {
                self.chain = Chain::Refreshing;
                vec![Command::Refresh]
            }
            Chain::Reloaded => {
                self.chain = Chain::Replanning;
                self.failed_keys.push(plan.plan_attempt_key.clone());
                vec![Command::Replan(Replan {
                    session_id: plan.session_id.clone(),
                    attempt_id: plan.attempt_id.clone(),
                    failed_plan_id: plan.plan_id.clone(),
                    plan_attempt_key: plan.plan_attempt_key.clone(),
                    attempted_plan_keys: self.failed_keys.clone(),
                    selected_tracks: plan.selected_tracks.clone(),
                    position: good + plan.offset,
                    operation: Operation::FailureRecovery,
                    quality: self.replan_quality(),
                })]
            }
            Chain::Replanned => self.lose(),
            Chain::Refreshing | Chain::Replanning | Chain::Lost => Vec::new(),
        }
    }

    /// Plays the tracks of the Track picker choice that the loaded file has and that do not play yet. Each kind
    /// plays once per load, so that a later pick of the user stays.
    fn play_carry(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        for kind in [Kind::Audio, Kind::Sub] {
            let pending = if kind == Kind::Audio { &mut self.pending.audio } else { &mut self.pending.subtitle };
            let Some(choice) = pending else { continue };
            let found = self.tracks.iter().find(|t| t.kind == kind && choice.matches(t.lang.as_deref(), t.forced));
            let Some(track) = found else { continue };
            if !track.selected {
                commands.push(if kind == Kind::Audio {
                    Command::SelectAudio(track.id)
                } else {
                    Command::SelectSubtitle(Some(track.id))
                });
            }
            *pending = None;
        }
        commands
    }

    /// The recovery chain gave up. The Player view shows the `LOST` text as the prompt.
    fn lose(&mut self) -> Vec<Command> {
        self.chain = Chain::Lost;
        self.prompt = None;
        Vec::new()
    }

    /// The skip state machine. It acts when the position enters a Marker that is not resolved.
    fn enter(&mut self, position: f64, now: SystemTime) -> Vec<Command> {
        let markers = self.plan.as_ref().map_or(&[][..], |plan| &plan.markers);
        let inside = markers.iter().position(|marker| marker.start <= position && position < marker.end);
        if inside == self.inside {
            return Vec::new();
        }
        self.inside = inside;
        let Some(index) = inside.filter(|index| !self.resolved.contains(index)) else {
            self.hide_marker_prompt();
            return Vec::new();
        };
        let Marker { kind, start, end } = markers[index].clone();
        self.hide_marker_prompt();
        let mut commands = Vec::new();
        let to_end = self.player_duration().is_some_and(|duration| end >= duration - CREDITS_TO_END_SECONDS);
        let skip = Action::Seek { to: end, marker: index };
        match kind {
            MarkerKind::Intro => match self.settings.intro_skip_mode {
                IntroSkipMode::Never => {}
                IntroSkipMode::Ask => self.show("Skip intro", skip, false, now),
                IntroSkipMode::Always => {
                    self.resolved.push(index);
                    let undo = Action::Seek { to: start, marker: index };
                    commands.push(Command::Seek(end));
                    self.show("Watch intro", undo, true, now);
                }
            },
            // With autoplay, the Up Next card shows in these credits.
            MarkerKind::Credits if to_end && self.has_next && self.auto_next() => {
                if self.settings.auto_skip_credits {
                    commands.extend(self.auto_advance(self.source_position()));
                }
            }
            MarkerKind::Credits if to_end && self.has_next => self.show("Next episode", Action::Next, false, now),
            MarkerKind::Credits if !to_end && self.settings.auto_skip_credits => {
                self.resolved.push(index);
                commands.push(Command::Seek(end));
            }
            MarkerKind::Credits => self.show("Skip credits", skip, false, now),
            MarkerKind::Recap if self.settings.auto_skip_recap => {
                self.resolved.push(index);
                commands.push(Command::Seek(end));
            }
            MarkerKind::Recap => self.show("Skip recap", skip, false, now),
            MarkerKind::Preview => self.show("Skip preview", skip, false, now),
            MarkerKind::Unknown(_) => {}
        }
        commands
    }

    /// Hides the prompt of the Marker that the position left.
    fn hide_marker_prompt(&mut self) {
        if self.prompt.as_ref().is_some_and(|prompt| !prompt.stays) {
            self.prompt = None;
        }
    }

    // ponytail: the prompt timer runs on during a pause. The server design freezes it; add a pause
    // input to the timer if users miss the prompt.
    fn show(&mut self, label: &str, action: Action, stays: bool, now: SystemTime) {
        self.prompt = Some(Prompt { label: label.into(), action, until: now + PROMPT_TIME, stays });
    }

    /// The next Item plays at the end: the next pick of a shuffle, or the next episode when the Server settings
    /// auto-play it.
    fn auto_next(&self) -> bool {
        self.has_next && (self.shuffle || self.settings.auto_play_next)
    }

    /// Shows the Up Next card when the position enters the end of the file, and hides it when the position leaves.
    /// The end starts at the end credits, else `UP_NEXT_TIME` before the end of the file.
    fn count_down(&mut self, position: f64, now: SystemTime) {
        let markers = self.plan.as_ref().map_or(&[][..], |plan| &plan.markers);
        let start = self.player_duration().map(|duration| {
            let credits = markers.iter().filter(|marker| {
                matches!(marker.kind, MarkerKind::Credits) && marker.end >= duration - CREDITS_TO_END_SECONDS
            });
            credits.map(|marker| marker.start).fold(duration - UP_NEXT_TIME.as_secs_f64(), f64::min)
        });
        let inside = self.auto_next()
            && !self.cancelled
            && self.still_watching.is_none()
            && start.is_some_and(|start| position >= start);
        match (inside, self.up_next) {
            (true, None) => self.up_next = Some((now + UP_NEXT_TIME, UP_NEXT_TIME.as_secs())),
            (false, Some(_)) => self.up_next = None,
            _ => {}
        }
    }

    /// Plays the next Item with no input of the user: at the end of the file or of the Up Next countdown.
    /// After `STILL_WATCHING_AFTER` of them in a row, it pauses and shows the Still watching prompt instead.
    fn auto_advance(&mut self, position: Option<f64>) -> Vec<Command> {
        if self.auto_plays < STILL_WATCHING_AFTER {
            self.auto_plays += 1;
            return self.play_next(position);
        }
        (self.still_watching, self.up_next) = (Some(position), None);
        vec![Command::SetPause(true)]
    }

    /// The final position of a stop report: the position of the Still watching prompt while it shows.
    fn stop_position(&self) -> Option<f64> {
        self.still_watching.unwrap_or_else(|| self.source_position())
    }

    /// Ends the session at `position` and plays the next episode in the same Player view.
    fn play_next(&mut self, position: Option<f64>) -> Vec<Command> {
        let mut commands = self.finish(position, false);
        commands.push(Command::PlayNext);
        commands
    }

    fn player_duration(&self) -> Option<f64> {
        self.duration.or_else(|| Some(self.runtime? - self.plan.as_ref()?.offset))
    }

    fn fail(&mut self, message: String) -> Vec<Command> {
        self.ended = true;
        vec![Command::ShowError(message), Command::ClosePlayer]
    }

    fn report(&mut self, now: SystemTime) -> Vec<Command> {
        let due = self.last_report.is_some_and(|last| now.duration_since(last).unwrap_or_default() >= REPORT_INTERVAL);
        let (Some(plan), Some(position), true) = (&self.plan, self.source_position(), due) else {
            return Vec::new();
        };
        let session_id = plan.session_id.clone();
        self.last_report = Some(now);
        vec![Command::Progress { session_id, sample: self.sample(position) }]
    }

    /// Ends the session with a stop report that carries `position` as the final sample.
    fn finish(&mut self, position: Option<f64>, close_player: bool) -> Vec<Command> {
        self.ended = true;
        (self.prompt, self.up_next, self.still_watching) = (None, None, None);
        let Some(session_id) = self.plan.as_ref().map(|plan| plan.session_id.clone()) else {
            return Vec::new();
        };
        let sample = position.map(|position| self.sample(position));
        let mut commands = vec![Command::Stop { session_id, sample }];
        if close_player {
            commands.push(Command::ClosePlayer);
        }
        commands
    }

    fn sample(&mut self, position: f64) -> Sample {
        self.sequence += 1;
        Sample { sequence: self.sequence, position, paused: self.paused }
    }

    fn source_position(&self) -> Option<f64> {
        Some(self.position? + self.plan.as_ref()?.offset)
    }

    /// The final position at an end of file, and whether the end is a Normal end. It is the
    /// duration on a Normal end, else the last position. An Early end never carries the
    /// duration, so it never marks the Item watched.
    fn end_position(&self) -> (Option<f64>, bool) {
        let position = self.source_position();
        let offset = self.plan.as_ref().map_or(0.0, |plan| plan.offset);
        let duration = self.duration.map(|duration| duration + offset).or(self.runtime);
        match (position, duration) {
            (Some(position), Some(duration))
                if duration - position <= NORMAL_END_SECONDS || position >= duration * NORMAL_END_FRACTION =>
            {
                (Some(duration), true)
            }
            _ => (position, false),
        }
    }
}
