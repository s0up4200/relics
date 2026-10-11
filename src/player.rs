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

/// What the Player window needs to play an Item.
#[derive(Debug, Clone)]
pub struct PlayItem {
    content_id: String,
    /// The runtime in seconds.
    runtime: Option<f64>,
    /// The series id and the season number of an episode. The next episode lookup needs them.
    season: Option<(String, i64)>,
}

impl From<&wire::CatalogItem> for PlayItem {
    /// A series or season card plays its `play_content_id`, an episode.
    // ponytail: a series card has no season number, so its episode has no next episode. The detail
    // page (#30) can play the episode with `PlayItem::episode` instead.
    fn from(item: &wire::CatalogItem) -> Self {
        Self {
            content_id: item.play_content_id.clone().unwrap_or_else(|| item.content_id.clone()),
            runtime: item.duration_seconds.or(item.runtime.map(|minutes| minutes as f64 * 60.0)),
            season: item.series_id.clone().zip(item.season_number),
        }
    }
}

impl PlayItem {
    pub fn episode(series_id: String, episode: wire::Episode) -> Self {
        Self {
            content_id: episode.content_id,
            runtime: Some(episode.runtime as f64 * 60.0),
            season: Some((series_id, episode.season_number)),
        }
    }
}

/// The text of a playback error, for the Library window.
pub struct PlaybackError(pub String);

pub struct PlayerWindow {
    client: Option<Arc<Client>>,
    session: Option<Session>,
    mpv: Option<mpv::Player>,
    /// Changes when a handle opens or closes, so that the late events of a closed handle are dropped.
    mpv_id: u64,
    starting: Option<Task<()>>,
    /// The next episode of the Item that plays.
    next: Option<PlayItem>,
    finding_next: Option<Task<()>>,
    /// The last stop report. Quit waits for it.
    stopping: Option<Task<()>>,
    /// Changes with each new Item, so that the answer of a refresh or replan of an older session is dropped.
    generation: u64,
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
            next: None,
            finding_next: None,
            stopping: None,
            generation: 0,
            _mpv_events: None,
            _ticks: ticks,
        }
    }

    /// Plays `item`. A new Item ends the Playback session that plays now, and plays in the
    /// same Player window. Resume is the default: the server starts at the saved position.
    pub fn play(&mut self, client: Arc<Client>, item: PlayItem, cx: &mut Context<Self>) {
        self.step(Input::End, cx);
        self.generation += 1;
        self.session = Some(Session::new(item.runtime, client.server_settings()));
        self.client = Some(client.clone());
        self.next = None;
        // A new Item drops this task, so an older answer never reaches the new session.
        self.finding_next = item.season.map(|(series_id, season)| {
            let (client, content_id) = (client.clone(), item.content_id.clone());
            cx.spawn(async move |this, cx| match client.next_episode(&series_id, season, &content_id).await {
                Ok(Some(episode)) => {
                    let next = PlayItem::episode(series_id, episode);
                    this.update(cx, |this, cx| {
                        this.next = Some(next);
                        this.step(Input::NextEpisode, cx);
                    })
                    .ok();
                }
                Ok(None) => {}
                Err(err) => eprintln!("silo-omarchy: cannot find the next episode: {err}"),
            })
        });
        let content_id = item.content_id;
        // A new Item drops this task, so the reply of an older start never reaches the new session.
        self.starting = Some(cx.spawn(async move |this, cx| {
            let input = match client.start_playback(&content_id, None).await {
                Ok(decision) => Input::Started { decision, access: client.media_access() },
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
            Command::Load { url, headers, start } => match self.open_mpv(cx) {
                Ok(mpv) => {
                    if let Err(err) = mpv.load(&url, &headers, start) {
                        eprintln!("silo-omarchy: mpv cannot play {url}: {err}");
                        self.step(Input::LoadFailed, cx);
                    }
                }
                Err(err) => {
                    cx.emit(PlaybackError(format!("The player cannot start: {err}")));
                    self.step(Input::End, cx);
                }
            },
            Command::Refresh => self.answer(cx, async move {
                let access = client.refresh_access().await;
                Input::Refreshed(access.inspect_err(|err| eprintln!("silo-omarchy: token refresh failed: {err}")).ok())
            }),
            Command::Replan(replan) => self.answer(cx, async move {
                let decision = client.replan(&replan).await;
                Input::Replanned(decision.inspect_err(|err| eprintln!("silo-omarchy: replan failed: {err}")).ok())
            }),
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
            Command::AddSubtitle { url, title, language } => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.add_subtitle(&url, &title, &language)) {
                    eprintln!("silo-omarchy: cannot add the subtitles {url}: {err}");
                }
            }
            Command::ShowMarkers(markers) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.show_markers(&markers)) {
                    eprintln!("silo-omarchy: cannot send the markers to the player: {err}");
                }
            }
            Command::ShowPrompt(label) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.show_prompt(&label)) {
                    eprintln!("silo-omarchy: cannot send the skip prompt to the player: {err}");
                }
            }
            Command::Seek(position) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.seek(position)) {
                    eprintln!("silo-omarchy: cannot seek to {position}: {err}");
                }
            }
            Command::PlayNext => {
                if let Some(next) = self.next.take() {
                    self.play(client, next, cx);
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

    /// Runs `request` and gives its input to the session, unless a new Item started meanwhile. A new Item does
    /// not drop the task: a dropped refresh can lose the new refresh token.
    fn answer(&self, cx: &mut Context<Self>, request: impl Future<Output = Input> + 'static) {
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let input = request.await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.step(input, cx);
                }
            })
            .ok();
        })
        .detach();
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
