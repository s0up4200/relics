//! The playback controller. It holds the Item that plays (`playback::playing`) and the media handle, feeds the
//! Playback session the media events and a tick each second, and carries out the commands of the session. The
//! video shows in the Player view of the Library window (`player_view.rs`). The app plays at most one Item.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedReceiver};
use gpui::{App, AppContext as _, Context, EventEmitter, Task};
use mpris_server::PlaybackStatus;

use crate::api::types::Marker;
use crate::api::{Client, wire};
use crate::artwork;
use crate::home::Play;
use crate::mpris::{self, Track};
use crate::playback::playing::{Next, PlayItem, Playing, next_label};
use crate::playback::tracks;
use crate::playback::{Command, Input, Session};

/// Starts a server shuffle of the scope `kind` and `id` (a Library, a collection of a Library, a series or a
/// season) and emits `Play` with its first pick. A failure gives the text for the user to `failed`.
pub fn start_shuffle<T: EventEmitter<Play>>(
    client: Arc<Client>,
    kind: &'static str,
    id: String,
    cx: &mut Context<T>,
    failed: fn(&mut T, String),
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let shuffle = client.start_shuffle(kind, &id, artwork::Rung::Small).await;
        this.update(cx, |this, cx| {
            match shuffle {
                Ok(shuffle) => cx.emit(Play(PlayItem::shuffled(&shuffle))),
                Err(err) => failed(this, format!("Cannot shuffle: {err}")),
            }
            cx.notify();
        })
        .ok();
    })
}

/// The width of the poster that MPRIS shows, in pixels.
const MPRIS_POSTER_WIDTH: f32 = 512.0;

/// The media player of the Player view: libmpv in the app (`mpv::InView`), and a fake in the tests.
pub trait Media {
    /// Opens a player handle. The handle sends each of its events to the receiver as a Playback session input,
    /// and `Shutdown` last. The error is the text for the user.
    fn open(&self) -> Result<(Box<dyn Handle>, UnboundedReceiver<Input>), String>;
    /// The app takes the MPRIS bus name while a handle is open. The fake of the tests starts no D-Bus service.
    fn media_keys(&self) -> bool;
}

/// One open player handle. Drop it to close the handle and stop playback.
pub trait Handle {
    /// Opens `url` and plays it from `start` seconds of player time, with no subtitles, and with the audio track
    /// `aid` when it is set. Each media request of the file has `headers`.
    fn load(&self, url: &str, headers: &[(String, String)], start: f64, aid: Option<i64>) -> anyhow::Result<()>;
    /// Adds the subtitle file at `url` to the file that plays, and shows it.
    fn add_subtitle(&self, url: &str, title: &str, language: &str) -> anyhow::Result<()>;
    /// Shows the subtitle track `sid` of the file that plays. `None` hides the subtitles.
    fn select_subtitle(&self, sid: Option<i64>) -> anyhow::Result<()>;
    fn select_audio(&self, aid: i64) -> anyhow::Result<()>;
    /// Seeks to `position`, in player time.
    fn seek(&self, position: f64) -> anyhow::Result<()>;
    fn set_pause(&self, paused: bool) -> anyhow::Result<()>;
    /// Changes the volume by `delta` percent.
    fn add_volume(&self, delta: f64) -> anyhow::Result<()>;
    fn toggle_mute(&self) -> anyhow::Result<()>;
    /// The volume in percent, and whether the audio is muted.
    fn volume(&self) -> anyhow::Result<(f64, bool)>;
    /// The Subtitle delay in seconds. The handle keeps it from one file to the next.
    fn sub_delay(&self) -> anyhow::Result<f64>;
    fn add_sub_delay(&self, delta: f64) -> anyhow::Result<()>;
    fn set_sub_delay(&self, seconds: f64) -> anyhow::Result<()>;
}

/// The text of a playback error, for the Library window.
pub struct PlaybackError(pub String);

/// The media handle closed: the video in the Player view stopped.
pub struct PlayerClosed;

/// The Player view closed: playback ended, or the video cannot show.
pub struct ViewClosed;

/// A stop report completed, or failed. The Server has the position now, so the pages can reload.
pub struct Stopped;

pub struct Controller {
    client: Option<Arc<Client>>,
    /// The Item that plays, and the state that the Player view shows.
    playing: Playing,
    /// Opens the media handles. `None` when the window has no video surface, so no Item can play.
    media: Option<Box<dyn Media>>,
    handle: Option<Box<dyn Handle>>,
    /// Changes when a handle opens or closes, so that the late events of a closed handle are dropped.
    handle_id: u64,
    starting: Option<Task<()>>,
    /// Looks up the next and the previous episode.
    finding: Option<Task<()>>,
    /// The MPRIS service. It runs while the media handle is open.
    mpris: Option<Rc<mpris_server::Player>>,
    _mpris: Option<Task<()>>,
    /// What MPRIS shows about the Item that plays, and the poster file.
    track: Option<Track>,
    art: Option<PathBuf>,
    duration: Option<f64>,
    describing: Option<Task<()>>,
    /// The last stop report. Quit waits for it.
    stopping: Option<Task<()>>,
    /// Changes with each new Item, so that the answer of a refresh or replan of an older session is dropped.
    generation: u64,
    _events: Option<Task<()>>,
    _ticks: Task<()>,
    /// The time of the Playback session, and the executor time when it was last read.
    clock: (SystemTime, Instant),
    /// The Player view shows: from `play` until playback ends.
    in_view: bool,
    /// The file versions of the Item that plays, for the Quality list of the Player view.
    versions: Vec<wire::FileVersion>,
    /// The seek thumbnails of the file that plays. `None` when the server has none.
    trickplay: Option<wire::WatchTrickplay>,
    /// Reads the seek thumbnails again before their URLs expire.
    _trickplay: Option<Task<()>>,
}

impl EventEmitter<PlaybackError> for Controller {}
impl EventEmitter<PlayerClosed> for Controller {}
impl EventEmitter<ViewClosed> for Controller {}
impl EventEmitter<Stopped> for Controller {}

impl Controller {
    pub fn new(media: Option<Box<dyn Media>>, cx: &mut Context<Self>) -> Self {
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
            playing: Playing::default(),
            media,
            handle: None,
            handle_id: 0,
            starting: None,
            finding: None,
            mpris: None,
            _mpris: None,
            track: None,
            art: None,
            duration: None,
            describing: None,
            stopping: None,
            generation: 0,
            _events: None,
            _ticks: ticks,
            clock: (SystemTime::now(), cx.background_executor().now()),
            in_view: false,
            versions: Vec::new(),
            trickplay: None,
            _trickplay: None,
        }
    }

    /// Plays `item`. A new Item ends the Playback session that plays now, and plays in the
    /// same Player view. Resume is the default: the server starts at the saved position.
    pub fn play(&mut self, client: Arc<Client>, item: PlayItem, cx: &mut Context<Self>) {
        self.step(Input::End, cx);
        self.generation += 1;
        self.in_view = self.media.is_some();
        // The Subtitle delay carries as the Track picker choice does. The handle keeps it, so another Item resets
        // it.
        if !self.playing.start(item.clone(), client.server_settings())
            && let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.set_sub_delay(0.0))
        {
            eprintln!("relics: cannot reset the subtitle delay: {err}");
        }
        (self.versions, self.trickplay, self._trickplay) = (Vec::new(), None, None);
        self.client = Some(client.clone());
        // A new Item drops this task, so an older answer never reaches the new session.
        self.finding = match &item.shuffle {
            Some((_, next)) => {
                if next.is_some() {
                    self.step(Input::Shuffle, cx);
                }
                None
            }
            None => self.find_episodes(&client, &item, cx),
        };
        (self.track, self.art, self.duration) = (None, None, None);
        self.describing = Some(self.describe(client.clone(), item.content_id.clone(), cx));
        let start = item.start_position();
        let (content_id, file_id) = (item.content_id, item.file_id);
        // A new Item drops this task, so the reply of an older start never reaches the new session.
        self.starting = Some(cx.spawn(async move |this, cx| {
            let input = match client.start_version(&content_id, file_id.as_deref(), start).await {
                Ok(decision) => Input::Started { decision, access: client.media_access() },
                Err(err) => Input::StartFailed(err.to_string()),
            };
            this.update(cx, |this, cx| {
                this.step(input, cx);
                this.load_trickplay(cx);
            })
            .ok();
            match client.versions(&content_id).await {
                Ok(versions) => this.update(cx, |this, cx| {
                    this.versions = versions;
                    cx.notify();
                }),
                Err(err) => return eprintln!("relics: cannot read the versions of {content_id}: {err}"),
            }
            .ok();
        }));
    }

    /// Reads the seek thumbnails of the file that plays, and reads them again a minute before their URLs expire.
    /// A file with no thumbnails (an error, for example a 404) keeps `trickplay` at `None`.
    fn load_trickplay(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Some(item), Some(plan)) = (self.client.clone(), self.playing.item(), self.plan()) else {
            return;
        };
        let (content_id, file_id) = (item.content_id.clone(), plan.effective_media_file_id.to_string());
        self._trickplay = Some(cx.spawn(async move |this, cx| {
            loop {
                let trickplay = match client.trickplay(&content_id, &file_id).await {
                    Ok(trickplay) => trickplay,
                    Err(err) => return eprintln!("relics: no seek thumbnails for {content_id}: {err}"),
                };
                let wait = (trickplay.expires_at - chrono::Utc::now() - chrono::TimeDelta::minutes(1))
                    .to_std()
                    .unwrap_or_default()
                    .max(Duration::from_secs(60));
                let set = this.update(cx, |this, cx| {
                    this.trickplay = Some(trickplay);
                    // A sheet that failed with an expired URL loads again with the new one.
                    artwork::global(cx).update(cx, |cache, _| cache.retry_failed());
                    cx.notify();
                });
                if set.is_err() {
                    return;
                }
                cx.background_executor().timer(wait).await;
            }
        }));
    }

    /// Looks up the next and the previous episode of `item`, and gives each one that exists to the session.
    fn find_episodes(&self, client: &Arc<Client>, item: &PlayItem, cx: &mut Context<Self>) -> Option<Task<()>> {
        if item.season.is_none() && !item.find_season {
            return None;
        }
        let (client, content_id, season) = (client.clone(), item.content_id.clone(), item.season.clone());
        Some(cx.spawn(async move |this, cx| {
            let season = match season {
                Some(season) => Some(season),
                None => match client.item(&content_id, artwork::Rung::Small).await {
                    Ok(episode) => episode.series_id.zip(episode.season_number),
                    Err(err) => {
                        eprintln!("relics: cannot read the season of {content_id}: {err}");
                        None
                    }
                },
            };
            let Some((series_id, season)) = season else {
                return;
            };
            let (next, previous) = futures::join!(
                client.next_episode(&series_id, season, &content_id),
                client.previous_episode(&series_id, season, &content_id),
            );
            let episode = |found: Result<Option<wire::Episode>, _>| match found {
                Ok(found) => found,
                Err(err) => {
                    eprintln!("relics: cannot find the episode before or after {content_id}: {err}");
                    None
                }
            };
            let next = episode(next).map(|e| {
                let title = next_label(Some(e.season_number), Some(e.episode_number), &e.title);
                (PlayItem::episode(series_id.clone(), e), title)
            });
            let previous = episode(previous).map(|e| PlayItem::episode(series_id.clone(), e));
            this.update(cx, |this, cx| {
                for input in this.playing.found(series_id, next, previous) {
                    this.step(input, cx);
                }
            })
            .ok();
        }))
    }

    /// Reads the title of the Item and downloads its poster to the Artwork cache, for MPRIS.
    fn describe(&self, client: Arc<Client>, content_id: String, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let item = match client
                .item(&content_id, artwork::Rung::fit(artwork::Kind::Poster, MPRIS_POSTER_WIDTH, 1.0))
                .await
            {
                Ok(item) => item,
                Err(err) => return eprintln!("relics: cannot read {content_id} for MPRIS: {err}"),
            };
            let poster = item.poster_url.as_deref().filter(|url| !url.is_empty());
            let poster = poster.and_then(|url| client.server().url.join(url).ok()).map(String::from);
            let track = Track::from(&item);
            if this
                .update(cx, |this, cx| {
                    this.track = Some(track);
                    this.sync_mpris(cx);
                })
                .is_err()
            {
                return;
            }
            let Some(url) = poster else {
                return;
            };
            let target = MPRIS_POSTER_WIDTH as u32;
            let file = cx.update(|cx| artwork::global(cx).update(cx, |cache, cx| cache.cover(&url, target, cx)));
            match file.await {
                Ok(file) => {
                    this.update(cx, |this, cx| {
                        this.art = Some(file);
                        this.sync_mpris(cx);
                    })
                    .ok();
                }
                Err(err) => eprintln!("relics: cannot download the poster for MPRIS: {err}"),
            }
        })
    }

    /// Takes the MPRIS bus name, and sends the MPRIS controls to the session until the media handle closes.
    fn start_mpris(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded();
        self._mpris = Some(cx.spawn(async move |this, cx| {
            let player = match mpris::start(move |input| drop(tx.unbounded_send(input))).await {
                Ok(player) => Rc::new(player),
                Err(err) => return eprintln!("relics: the media keys do not work: {err}"),
            };
            let run = player.run();
            if this
                .update(cx, |this, cx| {
                    this.mpris = Some(player);
                    this.sync_mpris(cx);
                })
                .is_err()
            {
                return;
            }
            let controls = async {
                while let Some(input) = rx.next().await {
                    if this.update(cx, |this, cx| this.step(input, cx)).is_err() {
                        return;
                    }
                }
            };
            futures::future::join(run, controls).await;
        }));
    }

    /// Sends the state of the session and the metadata to MPRIS. Each setter sends a change only.
    fn sync_mpris(&self, cx: &mut Context<Self>) {
        let (Some(player), Some(session)) = (self.mpris.clone(), self.playing.session()) else {
            return;
        };
        let status = if session.paused() { PlaybackStatus::Paused } else { PlaybackStatus::Playing };
        let (next, previous) = (session.can_go_next(), session.can_go_previous());
        let metadata = mpris::metadata(self.track.as_ref(), self.duration, self.art.as_deref());
        cx.spawn(async move |_, _| {
            let result = async {
                player.set_playback_status(status).await?;
                player.set_can_go_next(next).await?;
                player.set_can_go_previous(previous).await?;
                player.set_metadata(metadata).await
            };
            if let Err(err) = result.await {
                eprintln!("relics: cannot update MPRIS: {err}");
            }
        })
        .detach();
    }

    /// Ends the Playback session and closes the Player view. The task ends when the
    /// server has the stop report. The Library window waits for it before the app quits.
    pub fn quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        (self.starting, self._trickplay) = (None, None);
        self.playing.reset_auto_plays();
        self.step(Input::End, cx);
        self.close(cx);
        self.stopping.take().unwrap_or_else(|| Task::ready(()))
    }

    /// Back in the Player view: ends the Playback session with the stop report, and closes the view.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        self.quit(cx).detach();
    }

    /// Gives `input` to the Playback session, for example a control of the Player view.
    pub fn step(&mut self, input: Input, cx: &mut Context<Self>) {
        // The tick each second redraws the Player view with the new position. A mouse move changes nothing to draw.
        if self.in_view && !matches!(input, Input::Position(_) | Input::Active) {
            cx.notify();
        }
        // MPRIS reads the position when it needs it. The other inputs can change what MPRIS shows.
        let sync = match input {
            Input::Position(position) => {
                if let Some(player) = &self.mpris {
                    player.set_position(mpris::time(position));
                }
                false
            }
            Input::Seeked(position) => {
                if let Some(player) = self.mpris.clone() {
                    player.set_position(mpris::time(position));
                    cx.spawn(async move |_, _| player.seeked(mpris::time(position)).await.ok()).detach();
                }
                false
            }
            Input::Duration(duration) => {
                self.duration = Some(duration);
                true
            }
            Input::Active => false,
            _ => true,
        };
        let now = self.now(cx);
        for command in self.playing.step(input, now) {
            self.execute(command, cx);
        }
        if sync {
            self.sync_mpris(cx);
        }
    }

    fn execute(&mut self, command: Command, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        match command {
            Command::Load { url, headers, start, aid } => match self.open(cx) {
                Ok(handle) => {
                    if let Err(err) = handle.load(&url, &headers, start, aid) {
                        eprintln!("relics: mpv cannot play {url}: {err}");
                        self.step(Input::LoadFailed, cx);
                    }
                }
                // The session ends with the stop report, and the Library window shows the error on the page.
                Err(err) => {
                    cx.emit(PlaybackError(format!("The player cannot start: {err}")));
                    self.step(Input::End, cx);
                    self.close(cx);
                }
            },
            Command::Refresh => self.answer(cx, async move {
                let access = client.refresh_access().await;
                Input::Refreshed(access.inspect_err(|err| eprintln!("relics: token refresh failed: {err}")).ok())
            }),
            Command::Replan(replan) => self.answer(cx, async move {
                let decision = client.replan(&replan).await;
                Input::Replanned(decision.inspect_err(|err| eprintln!("relics: replan failed: {err}")).ok())
            }),
            // The client keeps the position of a failed report and sends it with syncProgress later.
            Command::Progress { session_id, sample } => cx
                .background_spawn(async move {
                    if let Err(err) = client.report_progress(&session_id, &sample).await {
                        eprintln!("relics: progress report failed: {err}");
                    }
                })
                .detach(),
            Command::Stop { session_id, sample } => {
                let report = cx.background_spawn(async move {
                    if let Err(err) = client.stop_playback(&session_id, sample.as_ref()).await {
                        eprintln!("relics: stop report failed: {err}");
                    }
                });
                let stop = cx.spawn(async move |this, cx| {
                    report.await;
                    this.update(cx, |_, cx| cx.emit(Stopped)).ok();
                });
                if let Some(older) = self.stopping.replace(stop) {
                    older.detach();
                }
            }
            Command::AddSubtitle { url, title, language } => {
                if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.add_subtitle(&url, &title, &language))
                {
                    eprintln!("relics: cannot add the subtitles {url}: {err}");
                }
            }
            Command::SelectSubtitle(sid) => {
                if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.select_subtitle(sid)) {
                    eprintln!("relics: cannot show subtitle track {sid:?}: {err}");
                }
            }
            Command::SelectAudio(aid) => {
                if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.select_audio(aid)) {
                    eprintln!("relics: cannot play audio track {aid}: {err}");
                }
            }
            Command::Seek(position) => {
                if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.seek(position)) {
                    eprintln!("relics: cannot seek to {position}: {err}");
                }
            }
            Command::PlayNext => match self.playing.take_next() {
                Some(Next::Shuffle { id, from }) => self.advance(client, id, from, cx),
                Some(Next::Episode(next)) => self.play(client, next, cx),
                None => {}
            },
            Command::PlayPrevious => {
                if let Some(previous) = self.playing.take_previous() {
                    self.play(client, previous, cx);
                }
            }
            Command::PlayVersion { file_id, position } => {
                if let Some(item) = self.playing.version(file_id, position) {
                    self.play(client, item, cx);
                }
            }
            Command::SetPause(paused) => {
                if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.set_pause(paused)) {
                    eprintln!("relics: cannot pause or play: {err}");
                }
            }
            Command::ClosePlayer => self.close(cx),
            Command::ShowError(text) => cx.emit(PlaybackError(text)),
        }
    }

    /// Moves the shuffle `id` past the Item `from`, which ended, and plays the new pick in the same Player view.
    /// Back while the call runs drops it. A failed call closes the player.
    fn advance(&mut self, client: Arc<Client>, id: String, from: String, cx: &mut Context<Self>) {
        self.starting = Some(cx.spawn(async move |this, cx| {
            let shuffle = client.advance_shuffle(&id, &from, artwork::Rung::Small).await;
            this.update(cx, |this, cx| match shuffle {
                Ok(shuffle) => this.play(client, PlayItem::shuffled(&shuffle), cx),
                Err(err) => {
                    cx.emit(PlaybackError(format!("Cannot continue the shuffle: {err}")));
                    this.close(cx);
                }
            })
            .ok();
        }));
    }

    /// The time for the Playback session. It moves with the clock of the GPUI executor, which the tests control,
    /// and never falls behind the system clock, so that a suspend still counts toward the token expiry.
    fn now(&mut self, cx: &App) -> SystemTime {
        let executor = cx.background_executor().now();
        let (time, last) = self.clock;
        self.clock = ((time + executor.saturating_duration_since(last)).max(SystemTime::now()), executor);
        self.clock.0
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

    /// The Item has a next episode or a next shuffle pick, so Skip works.
    pub fn can_go_next(&self) -> bool {
        self.session().is_some_and(Session::can_go_next)
    }

    /// The media handle is open.
    pub fn is_open(&self) -> bool {
        self.handle.is_some()
    }

    fn session(&self) -> Option<&Session> {
        self.playing.session()
    }

    /// The Player view shows.
    pub fn in_view(&self) -> bool {
        self.in_view
    }

    /// The player position in seconds, once the file plays.
    pub fn position(&self) -> Option<f64> {
        self.session()?.position()
    }

    /// The duration of the file in seconds, in player time.
    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn paused(&self) -> bool {
        self.session().is_some_and(Session::paused)
    }

    /// The title of the Item that plays, once it is known.
    pub fn title(&self) -> Option<&str> {
        self.track.as_ref().map(|track| track.title.as_str())
    }

    /// The label of the Skip prompt that shows, or an empty text.
    pub fn prompt(&self) -> &str {
        self.session().map_or("", Session::prompt)
    }

    /// The seconds left before the next Item plays, while the Up Next card shows.
    pub fn up_next(&self) -> Option<u64> {
        self.session()?.up_next()
    }

    /// The title of the next episode or shuffle pick, once it is known.
    pub fn next_title(&self) -> Option<&str> {
        self.playing.next_title()
    }

    /// The Item that plays is a shuffle pick.
    pub fn in_shuffle(&self) -> bool {
        self.playing.in_shuffle()
    }

    /// The Still watching prompt shows.
    pub fn still_watching(&self) -> bool {
        self.session().is_some_and(Session::still_watching)
    }

    /// The Markers of the file, in player time.
    pub fn markers(&self) -> &[Marker] {
        self.session().map_or(&[], Session::markers)
    }

    /// The tracks of the file, for the Track picker.
    pub fn tracks(&self) -> &[tracks::Track] {
        self.session().map_or(&[], Session::tracks)
    }

    /// The plan that plays, for the Playback info panel and the Quality ladder.
    pub fn plan(&self) -> Option<&wire::PlaybackPlan> {
        self.session()?.plan()
    }

    /// The step label of the Quality ladder that plays.
    pub fn quality(&self) -> &str {
        self.session().map_or(crate::playback::ORIGINAL, Session::quality)
    }

    /// The file versions of the Item that plays.
    pub fn versions(&self) -> &[wire::FileVersion] {
        &self.versions
    }

    /// The seek thumbnails of the file that plays.
    pub fn trickplay(&self) -> Option<&wire::WatchTrickplay> {
        self.trickplay.as_ref()
    }

    /// The file version that plays, once the plan and the version list are known.
    pub fn version(&self) -> Option<&wire::FileVersion> {
        let plan = self.plan()?;
        self.versions.iter().find(|v| *v.file_id == *plan.effective_media_file_id)
    }

    /// The volume in percent, and whether the audio is muted.
    pub fn volume(&self) -> Option<(f64, bool)> {
        self.handle.as_ref()?.volume().ok()
    }

    pub fn add_volume(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.add_volume(delta)) {
            eprintln!("relics: cannot change the volume: {err}");
        }
        cx.notify();
    }

    /// The subtitle delay in seconds.
    pub fn sub_delay(&self) -> Option<f64> {
        self.handle.as_ref()?.sub_delay().ok()
    }

    pub fn add_sub_delay(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.add_sub_delay(delta)) {
            eprintln!("relics: cannot change the subtitle delay: {err}");
        }
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(Err(err)) = self.handle.as_ref().map(|handle| handle.toggle_mute()) {
            eprintln!("relics: cannot mute: {err}");
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        // Dropping the MPRIS player and its task gives up the bus name.
        self.mpris = None;
        self._mpris = None;
        if self.handle.take().is_some() {
            cx.emit(PlayerClosed);
        }
        self.handle_id += 1;
        if std::mem::take(&mut self.in_view) {
            cx.emit(ViewClosed);
        }
    }

    /// Returns the media handle, and opens one in the Player view first when there is none. An error tells why the
    /// video cannot show.
    fn open(&mut self, cx: &mut Context<Self>) -> Result<&dyn Handle, String> {
        if self.handle.is_none() {
            let media = self.media.as_ref().ok_or("the window has no video surface (it needs Wayland)")?;
            let (handle, mut rx) = media.open()?;
            let media_keys = media.media_keys();
            self.handle = Some(handle);
            self.handle_id += 1;
            if media_keys {
                self.start_mpris(cx);
            }
            let id = self.handle_id;
            self._events = Some(cx.spawn(async move |this, cx| {
                while let Some(input) = rx.next().await {
                    let current = this.update(cx, |this, cx| {
                        if this.handle_id == id {
                            this.step(input, cx);
                        }
                    });
                    if current.is_err() {
                        return;
                    }
                }
            }));
        }
        Ok(self.handle.as_deref().expect("opened above"))
    }
}
