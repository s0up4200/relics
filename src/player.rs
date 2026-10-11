//! The Player window controller. It holds the current Playback session and the mpv
//! handle, feeds the session the mpv events and a tick each second, and carries out
//! the commands of the session. The app has at most one Player window.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{AppContext as _, Context, EventEmitter, Hsla, Rgba, Task};
use gpui_omarchy::ActiveTheme as _;

use crate::api::{Client, wire};
use crate::mpv;
use crate::playback::{Command, Input, Session};

// ponytail: a fixed value until #36 adds the hardware decoding Local setting.
const HWDEC: &str = "auto-safe";

/// The text of a playback error, for the Library window.
pub struct PlaybackError(pub String);

pub struct PlayerWindow {
    client: Option<Arc<Client>>,
    session: Option<Session>,
    mpv: Option<mpv::Player>,
    /// Changes when a handle opens or closes, so that the late events of a closed handle are dropped.
    mpv_id: u64,
    starting: Option<Task<()>>,
    /// The last stop report. Quit waits for it.
    stopping: Option<Task<()>>,
    _mpv_events: Option<Task<()>>,
    _ticks: Task<()>,
}

impl EventEmitter<PlaybackError> for PlayerWindow {}

impl PlayerWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let ticks = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.step(Input::Tick, cx)).is_err() {
                    return;
                }
            }
        });
        Self {
            client: None,
            session: None,
            mpv: None,
            mpv_id: 0,
            starting: None,
            stopping: None,
            _mpv_events: None,
            _ticks: ticks,
        }
    }

    /// Plays `item`. A new Item ends the Playback session that plays now, and plays in the
    /// same Player window. Resume is the default: the server starts at the saved position.
    pub fn play(&mut self, client: Arc<Client>, item: &wire::CatalogItem, cx: &mut Context<Self>) {
        self.step(Input::End, cx);
        let runtime = item.duration_seconds.or(item.runtime.map(|minutes| minutes as f64 * 60.0));
        self.session = Some(Session::new(runtime));
        self.client = Some(client.clone());
        let content_id = item.play_content_id.clone().unwrap_or_else(|| item.content_id.clone());
        // A new Item drops this task, so the reply of an older start never reaches the new session.
        self.starting = Some(cx.spawn(async move |this, cx| {
            let input = match client.start_playback(&content_id, None).await {
                Ok(decision) => Input::Started { decision, headers: client.media_headers() },
                Err(err) => Input::StartFailed(err.to_string()),
            };
            this.update(cx, |this, cx| this.step(input, cx)).ok();
        }));
    }

    /// Ends the Playback session and closes the Player window. The task ends when the
    /// server has the stop report. The Library window waits for it before the app quits.
    pub fn quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.starting = None;
        self.step(Input::End, cx);
        self.close_mpv();
        self.stopping.take().unwrap_or_else(|| Task::ready(()))
    }

    fn step(&mut self, input: Input, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        for command in session.step(input, SystemTime::now()) {
            self.execute(command, cx);
        }
    }

    fn execute(&mut self, command: Command, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        match command {
            Command::Load { url, headers, start } => {
                let loaded = self.open_mpv(cx).and_then(|mpv| mpv.load(&url, &headers, start));
                if let Err(err) = loaded {
                    eprintln!("silo-omarchy: mpv cannot play {url}: {err}");
                    self.step(Input::LoadFailed, cx);
                }
            }
            // ponytail: a failed report is lost; #33 keeps it and sends it with syncProgress.
            Command::Progress { session_id, sample } => cx
                .background_spawn(async move {
                    if let Err(err) = client.report_progress(&session_id, &sample).await {
                        eprintln!("silo-omarchy: progress report failed: {err}");
                    }
                })
                .detach(),
            Command::Stop { session_id, sample } => {
                let stop = cx.background_spawn(async move {
                    if let Err(err) = client.stop_playback(&session_id, sample.as_ref()).await {
                        eprintln!("silo-omarchy: stop report failed: {err}");
                    }
                });
                if let Some(older) = self.stopping.replace(stop) {
                    older.detach();
                }
            }
            Command::ClosePlayer => self.close_mpv(),
            Command::ShowError(text) => cx.emit(PlaybackError(text)),
            Command::SendTheme => {
                let theme = cx.omarchy();
                let [accent, foreground, background] = [theme.accent, theme.foreground, theme.background].map(hex);
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.send_theme(&accent, &foreground, &background)) {
                    eprintln!("silo-omarchy: cannot send the theme to the player: {err}");
                }
            }
        }
    }

    fn close_mpv(&mut self) {
        self.mpv = None;
        self.mpv_id += 1;
    }

    /// Returns the mpv handle, and opens a Player window first when there is none.
    fn open_mpv(&mut self, cx: &mut Context<Self>) -> Result<&mpv::Player, mpv::Error> {
        if self.mpv.is_none() {
            let (tx, mut rx) = mpsc::unbounded();
            let controls = mpv::find_controls(|name| std::env::var_os(name));
            let options = match controls.as_deref().and_then(|path| path.to_str()) {
                Some(path) => vec![("scripts", path)],
                None => {
                    eprintln!("silo-omarchy: silo-controls.lua not found, so the player has no controls");
                    vec![]
                }
            };
            self.mpv = Some(mpv::Player::open(HWDEC, &options, move |input| {
                tx.unbounded_send(input).ok();
            })?);
            self.mpv_id += 1;
            let id = self.mpv_id;
            self._mpv_events = Some(cx.spawn(async move |this, cx| {
                while let Some(input) = rx.next().await {
                    let current = this.update(cx, |this, cx| {
                        if this.mpv_id == id {
                            this.step(input, cx);
                        }
                    });
                    if current.is_err() {
                        return;
                    }
                }
            }));
        }
        Ok(self.mpv.as_ref().expect("opened above"))
    }
}

/// A color as `#RRGGBB`.
fn hex(color: Hsla) -> String {
    let Rgba { r, g, b, .. } = color.into();
    let byte = |c: f32| (c * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}
