//! The Playback session: one viewing of one Item, from the start request to the stop
//! report. It is a pure state machine with one entry point, `step`. The caller gives
//! it each input with the wall-clock time and carries out the commands that it
//! returns. It holds no clock and does no I/O, so the tests reach it without mpv.

use std::time::{Duration, SystemTime};

use crate::api::types::Decision;

/// The time between two progress reports while the file is open, playing or paused.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// An end of file this close to the duration, in seconds, is a Normal end.
const NORMAL_END_SECONDS: f64 = 8.0;
/// An end of file at this fraction of the duration or more is a Normal end.
const NORMAL_END_FRACTION: f64 = 0.985;

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
}

pub struct Session {
    /// The Item runtime in seconds. The end rule uses it when mpv has no duration.
    runtime: Option<f64>,
    plan: Option<Plan>,
    ended: bool,
    loaded: bool,
    position: Option<f64>,
    duration: Option<f64>,
    paused: bool,
    sequence: u64,
    last_report: Option<SystemTime>,
}

struct Plan {
    session_id: String,
    /// Add it to a player position to get the source position.
    offset: f64,
}

impl Session {
    pub fn new(runtime: Option<f64>) -> Self {
        Self {
            runtime,
            plan: None,
            ended: false,
            loaded: false,
            position: None,
            duration: None,
            paused: false,
            sequence: 0,
            last_report: None,
        }
    }

    pub fn step(&mut self, input: Input, now: SystemTime) -> Vec<Command> {
        if self.ended {
            return Vec::new();
        }
        match input {
            Input::Started { decision, headers } => match decision {
                Decision::Playable { session_id, plan } => {
                    let load = Command::Load {
                        url: plan.stream.url.clone(),
                        headers,
                        start: plan.timeline.player_start_seconds,
                    };
                    self.plan = Some(Plan { session_id, offset: plan.timeline.timeline_offset_seconds });
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
                Vec::new()
            }
            Input::Position(position) if self.loaded => {
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
            Input::Tick => self.report(now),
            Input::EndOfFile => {
                let position = self.end_position();
                self.finish(position, true)
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

    /// The final position at an end of file: the duration on a Normal end, else the last
    /// position. An Early end never carries the duration, so it never marks the Item watched.
    fn end_position(&self) -> Option<f64> {
        let position = self.source_position();
        let offset = self.plan.as_ref().map_or(0.0, |plan| plan.offset);
        let duration = self.duration.map(|duration| duration + offset).or(self.runtime);
        match (position, duration) {
            (Some(position), Some(duration))
                if duration - position <= NORMAL_END_SECONDS || position >= duration * NORMAL_END_FRACTION =>
            {
                Some(duration)
            }
            _ => position,
        }
    }
}
