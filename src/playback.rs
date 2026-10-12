//! The Playback session: one viewing of one Item, from the start request to the stop
//! report. It is a pure state machine with one entry point, `step`. The caller gives
//! it each input with the wall-clock time and carries out the commands that it
//! returns. It holds no clock and does no I/O, so the tests reach it without mpv.

use std::time::{Duration, SystemTime};

use crate::api::types::{Decision, Delivery, Marker, MarkerKind};
use crate::api::wire;
use crate::settings::{IntroSkipMode, ServerSettings};

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
/// Before each load, the app refreshes a token that has less time than this left.
const REFRESH_BEFORE_LOAD: Duration = Duration::from_secs(4 * 3600);
/// When the token in mpv has this much time left, the app refreshes it and reloads at the position.
const RENEW_BEFORE_EXPIRY: Duration = Duration::from_secs(600);
/// A reloaded stream that plays this many seconds is healthy again, and the next failure starts a new chain.
// ponytail: a fixed distance; a stream that fails every few seconds restarts the chain only by Enter.
const HEALTHY_SECONDS: f64 = 10.0;
/// The text of the controller when the recovery chain gave up.
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
}

#[derive(Debug)]
pub enum Input {
    /// The server answered the start request. The headers of `access` go on each media request.
    Started {
        decision: Decision,
        access: Access,
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
    /// The user closed the Player window, pressed q, or sent the MPRIS Stop.
    Shutdown,
    /// The app ends the session: Back in the Player view, a new Item replaces it, or the Library window closes.
    End,
    /// The periodic tick of the caller.
    Tick,
    /// The Lua controller loaded and waits for the theme.
    ControlsReady,
    /// The user took the Skip prompt, with a click or Enter.
    Action,
    /// The Item has a next episode.
    NextEpisode,
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
    /// MPRIS Next.
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
    /// Destroy the mpv handle, which closes the Player window.
    ClosePlayer,
    /// Show this text to the user.
    ShowError(String),
    /// Send the theme colors to the Lua controller.
    SendTheme,
    /// Add the subtitle file at `url` to mpv and show it.
    AddSubtitle { url: String, title: String, language: String },
    /// Show the subtitle track with this mpv track id. `None` hides the subtitles.
    SelectSubtitle(Option<i64>),
    /// Play the audio track with this mpv track id.
    SelectAudio(i64),
    /// Draw these Markers, in player time, on the seek bar.
    ShowMarkers(Vec<Marker>),
    /// Show the Skip prompt with this label. An empty label hides it.
    ShowPrompt(String),
    /// Seek to this player position.
    Seek(f64),
    /// Play the next episode in the same Player view or Player window. The session has already ended.
    PlayNext,
    /// Play the previous episode in the same Player view or Player window. The session has already ended.
    PlayPrevious,
    /// Set the mpv `pause` property.
    SetPause(bool),
    /// Refresh the access token and send `Input::Refreshed`.
    Refresh,
    /// Ask the server for a new plan and send `Input::Replanned`.
    Replan(Replan),
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
    /// The controller shows the `LOST` text. Enter starts the chain again.
    Lost,
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
    /// The Server settings of the Profile when the session started.
    settings: ServerSettings,
    plan: Option<Plan>,
    ended: bool,
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
    /// The part of `carry` that mpv has not played yet in the file that loads.
    pending: Carry,
}

/// The Skip prompt that shows now.
struct Prompt {
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
    /// The Markers in player time.
    markers: Vec<Marker>,
}

impl Session {
    pub fn new(runtime: Option<f64>, settings: ServerSettings) -> Self {
        Self {
            runtime,
            settings,
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
            pending: Carry::default(),
        }
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

    /// The tracks of the file, for the Track picker.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn step(&mut self, input: Input, now: SystemTime) -> Vec<Command> {
        if self.ended {
            return Vec::new();
        }
        match input {
            Input::Started { decision, access } => match decision {
                Decision::Playable { session_id, plan, audio_language, markers, attempt_id } => {
                    self.audio_language = audio_language;
                    self.access = Some(access);
                    let start = plan.timeline.player_start_seconds;
                    self.adopt(session_id, attempt_id, &plan, markers);
                    self.load(start, now)
                }
                Decision::Unavailable { message, .. } => self.fail(message),
                Decision::Unknown(outcome) => {
                    self.fail(format!("The Silo server gave an answer that silo-omarchy does not know: {outcome}"))
                }
            },
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
                Some(Decision::Playable { session_id, plan, .. }) => {
                    let Some(old) = &self.plan else {
                        return Vec::new();
                    };
                    let source = self.position.unwrap_or(self.load_start) + old.offset;
                    // The Markers come in source time. The plan keeps them in player time.
                    let markers = old
                        .markers
                        .iter()
                        .map(|marker| Marker {
                            start: marker.start + old.offset,
                            end: marker.end + old.offset,
                            ..marker.clone()
                        })
                        .collect();
                    let attempt_id = old.attempt_id.clone();
                    self.adopt(session_id, attempt_id, &plan, markers);
                    let start = (source - plan.timeline.timeline_offset_seconds).max(0.0);
                    self.position = Some(start);
                    self.chain = Chain::Replanned;
                    self.load_now(start)
                }
                _ => self.lose(),
            },
            Input::Replanned(_) => Vec::new(),
            Input::FileLoaded => {
                self.loaded = true;
                self.last_report = Some(now);
                // The user turned the subtitles off in the Track picker.
                let off = self.carry.subtitle == Some(Choice::Off);
                let subtitle = self.plan.as_ref().and_then(|plan| plan.subtitle.clone()).filter(|_| !off);
                let mut commands: Vec<Command> = [Some(self.show_markers()), subtitle].into_iter().flatten().collect();
                commands.extend(self.play_carry());
                commands
            }
            Input::Position(position) if self.loaded => {
                self.position = Some(position);
                if matches!(self.chain, Chain::Reloaded | Chain::Replanned)
                    && position >= self.load_start + HEALTHY_SECONDS
                {
                    self.chain = Chain::Healthy;
                }
                self.enter(position, now)
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
                let mut commands = Vec::new();
                if self.prompt.as_ref().is_some_and(|prompt| now >= prompt.until) {
                    self.prompt = None;
                    commands.push(Command::ShowPrompt(String::new()));
                }
                commands.extend(self.report(now));
                commands.extend(self.renew(now));
                commands
            }
            Input::ControlsReady => vec![Command::SendTheme, self.show_markers()],
            Input::Action if self.chain == Chain::Lost => {
                self.chain = Chain::Healthy;
                let mut commands = vec![Command::ShowPrompt(String::new())];
                commands.extend(self.recover());
                commands
            }
            Input::Action => match self.prompt.take() {
                Some(Prompt { action: Action::Seek { to, marker }, .. }) => {
                    self.resolved.push(marker);
                    vec![Command::Seek(to), Command::ShowPrompt(String::new())]
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
            Input::Next if self.has_next => self.play_next(self.source_position()),
            Input::Previous if self.has_previous => {
                let mut commands = self.finish(self.source_position(), false);
                commands.push(Command::PlayPrevious);
                commands
            }
            Input::EndOfFile => {
                let (position, normal) = self.end_position();
                if normal && self.has_next && self.settings.auto_play_next {
                    self.play_next(position)
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
                if picked.is_some() {
                    *choice = picked;
                }
                match kind {
                    Kind::Audio => id.map(Command::SelectAudio).into_iter().collect(),
                    _ => vec![Command::SelectSubtitle(id)],
                }
            }
            Input::LoadFailed => self.recover(),
            Input::Shutdown => self.finish(self.source_position(), true),
            Input::End => self.finish(self.source_position(), false),
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

    /// Makes `plan` the plan of the session. `markers` are in source time.
    fn adopt(&mut self, session_id: String, attempt_id: String, plan: &wire::PlaybackPlan, markers: Vec<Marker>) {
        let tracks = &plan.subtitle.inventory;
        let default = || subtitles::choose(tracks, &self.settings, self.audio_language.as_deref());
        let chosen = match &self.carry.subtitle {
            Some(choice @ Choice::Track { .. }) => subtitles::carried(tracks, choice).or_else(default),
            _ => default(),
        };
        // Direct play streams the file itself, with each of its tracks. mpv counts the tracks of a kind from 1.
        let direct = Delivery::from(plan.delivery.clone()) == Delivery::OriginalHttp;
        let aid = plan.selected_tracks.audio.as_ref().and_then(|audio| audio.index).filter(|_| direct).map(|i| i + 1);
        let subtitle = tracks.iter().find(|track| Some(track.combined_index) == chosen).and_then(|track| {
            if direct && track.source == "embedded" {
                // The file has the track, so a sidecar of it would show twice. The server lists the embedded
                // tracks in the order of the file.
                let before =
                    tracks.iter().filter(|t| t.source == "embedded" && t.combined_index < track.combined_index).count();
                return Some(Command::SelectSubtitle(Some(before as i64 + 1)));
            }
            // ponytail: a track without a URL needs a burn-in replan (operation track_change), so it shows no
            // subtitles.
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
            markers,
        });
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

    fn lose(&mut self) -> Vec<Command> {
        self.chain = Chain::Lost;
        self.prompt = None;
        vec![Command::ShowPrompt(LOST.into())]
    }

    fn show_markers(&self) -> Command {
        Command::ShowMarkers(self.plan.as_ref().map(|plan| plan.markers.clone()).unwrap_or_default())
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
            return self.hide_marker_prompt();
        };
        let Marker { kind, start, end } = markers[index].clone();
        let mut commands = self.hide_marker_prompt();
        let to_end = self.player_duration().is_some_and(|duration| end >= duration - CREDITS_TO_END_SECONDS);
        let skip = Action::Seek { to: end, marker: index };
        match kind {
            MarkerKind::Intro => match self.settings.intro_skip_mode {
                IntroSkipMode::Never => {}
                IntroSkipMode::Ask => commands.push(self.show("Skip intro", skip, false, now)),
                IntroSkipMode::Always => {
                    self.resolved.push(index);
                    let undo = Action::Seek { to: start, marker: index };
                    commands.extend([Command::Seek(end), self.show("Watch intro", undo, true, now)]);
                }
            },
            MarkerKind::Credits if to_end && self.has_next => {
                if self.settings.auto_skip_credits && self.settings.auto_play_next {
                    commands.extend(self.play_next(self.source_position()));
                } else {
                    commands.push(self.show("Next episode", Action::Next, false, now));
                }
            }
            MarkerKind::Credits if !to_end && self.settings.auto_skip_credits => {
                self.resolved.push(index);
                commands.push(Command::Seek(end));
            }
            MarkerKind::Credits => commands.push(self.show("Skip credits", skip, false, now)),
            MarkerKind::Recap if self.settings.auto_skip_recap => {
                self.resolved.push(index);
                commands.push(Command::Seek(end));
            }
            MarkerKind::Recap => commands.push(self.show("Skip recap", skip, false, now)),
            MarkerKind::Preview => commands.push(self.show("Skip preview", skip, false, now)),
            MarkerKind::Unknown(_) => {}
        }
        commands
    }

    /// Hides the prompt of the Marker that the position left.
    fn hide_marker_prompt(&mut self) -> Vec<Command> {
        if self.prompt.as_ref().is_some_and(|prompt| !prompt.stays) {
            self.prompt = None;
            return vec![Command::ShowPrompt(String::new())];
        }
        Vec::new()
    }

    // ponytail: the prompt timer runs on during a pause. The server design freezes it; add a pause
    // input to the timer if users miss the prompt.
    fn show(&mut self, label: &str, action: Action, stays: bool, now: SystemTime) -> Command {
        self.prompt = Some(Prompt { action, until: now + PROMPT_TIME, stays });
        Command::ShowPrompt(label.into())
    }

    /// Ends the session at `position` and plays the next episode in the same Player view or Player window.
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
        let Some(session_id) = self.plan.as_ref().map(|plan| plan.session_id.clone()) else {
            return Vec::new();
        };
        let mut commands = Vec::new();
        if self.prompt.take().is_some() || self.chain == Chain::Lost {
            commands.push(Command::ShowPrompt(String::new()));
        }
        let sample = position.map(|position| self.sample(position));
        commands.push(Command::Stop { session_id, sample });
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
