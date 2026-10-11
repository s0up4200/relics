//! The Playback session: one viewing of one Item, from the start request to the stop
//! report. It is a pure state machine with one entry point, `step`. The caller gives
//! it each input with the wall-clock time and carries out the commands that it
//! returns. It holds no clock and does no I/O, so the tests reach it without mpv.

use std::time::{Duration, SystemTime};

use crate::api::types::{Decision, Marker, MarkerKind};
use crate::settings::{IntroSkipMode, ServerSettings};

pub mod subtitles;

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

/// HTTP headers as (name, value).
pub type Headers = Vec<(String, String)>;

#[derive(Debug)]
pub enum Input {
    /// The server answered the start request. `headers` go on each media request.
    Started {
        decision: Decision,
        headers: Headers,
    },
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
    /// The user closed the Player window or pressed q.
    Shutdown,
    /// The app ends the session: a new Item replaces it, or the Library window closes.
    End,
    /// The periodic tick of the caller.
    Tick,
    /// The Lua controller loaded and waits for the theme.
    ControlsReady,
    /// The user took the Skip prompt, with a click or Enter.
    Action,
    /// The Item has a next episode.
    NextEpisode,
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
    /// Open `url` in mpv with these headers on each media request, at `start` seconds of player time.
    Load { url: String, headers: Headers, start: f64 },
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
    /// Draw these Markers, in player time, on the seek bar.
    ShowMarkers(Vec<Marker>),
    /// Show the Skip prompt with this label. An empty label hides it.
    ShowPrompt(String),
    /// Seek to this player position.
    Seek(f64),
    /// Play the next episode in the same Player window. The session has already ended.
    PlayNext,
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
        }
    }

    pub fn step(&mut self, input: Input, now: SystemTime) -> Vec<Command> {
        if self.ended {
            return Vec::new();
        }
        match input {
            Input::Started { decision, headers } => match decision {
                Decision::Playable { session_id, plan, audio_language, markers } => {
                    let load = Command::Load {
                        url: plan.stream.url.clone(),
                        headers,
                        start: plan.timeline.player_start_seconds,
                    };
                    let tracks = &plan.subtitle.inventory;
                    let chosen = subtitles::choose(tracks, &self.settings, audio_language.as_deref());
                    // ponytail: a track without a URL needs a burn-in replan, so it shows no subtitles until #27
                    // adds replan.
                    let subtitle = tracks.iter().find(|track| Some(track.combined_index) == chosen).and_then(|track| {
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
                    self.plan = Some(Plan { session_id, offset, subtitle, markers });
                    vec![load]
                }
                Decision::Unavailable { message, .. } => self.fail(message),
                Decision::Unknown(outcome) => {
                    self.fail(format!("The Silo server gave an answer that silo-omarchy does not know: {outcome}"))
                }
            },
            Input::StartFailed(message) => self.fail(message),
            Input::FileLoaded => {
                self.loaded = true;
                self.last_report = Some(now);
                let subtitle = self.plan.as_ref().and_then(|plan| plan.subtitle.clone());
                [Some(self.show_markers()), subtitle].into_iter().flatten().collect()
            }
            Input::Position(position) if self.loaded => {
                self.position = Some(position);
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
                commands
            }
            Input::ControlsReady => vec![Command::SendTheme, self.show_markers()],
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
            Input::EndOfFile => {
                let (position, normal) = self.end_position();
                if normal && self.has_next && self.settings.auto_play_next {
                    self.play_next(position)
                } else {
                    self.finish(position, true)
                }
            }
            Input::LoadFailed => {
                // ponytail: #27 replaces this with the recovery chain (refresh, reload, replan).
                let mut commands = self.finish(self.source_position(), false);
                commands.extend([Command::ShowError("Lost connection to the server".into()), Command::ClosePlayer]);
                commands
            }
            Input::Shutdown => self.finish(self.source_position(), true),
            Input::End => self.finish(self.source_position(), false),
            Input::Position(_) | Input::Duration(_) => Vec::new(),
        }
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
            MarkerKind::Recap if self.settings.auto_skip_recap => {
                self.resolved.push(index);
                commands.push(Command::Seek(end));
            }
            _ => {}
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

    /// Ends the session at `position` and plays the next episode in the same Player window.
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
        if self.prompt.take().is_some() {
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
