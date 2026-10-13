//! The playback controller. It holds the current Playback session and the mpv handle, feeds the session the
//! mpv events and a tick each second, and carries out the commands of the session. The video shows in the
//! Player view of the Library window (`player_view.rs`). The app plays at most one Item.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedReceiver};
use gpui::{AppContext as _, Context, EventEmitter, Task};
use mpris_server::PlaybackStatus;

use crate::api::types::Marker;
use crate::api::{Client, wire};
use crate::home::Play;
use crate::mpris::{self, Track};
use crate::playback::tracks::{self, Carry};
use crate::playback::{Command, Input, Session};
use crate::settings::LocalSettings;
use crate::video::Surface;
use crate::{artwork, mpv};

/// What the controller needs to play an Item.
#[derive(Debug, Clone)]
pub struct PlayItem {
    content_id: String,
    /// The runtime in seconds.
    runtime: Option<f64>,
    /// The series id and the season number of an episode. The next episode lookup needs them.
    season: Option<(String, i64)>,
    /// The Item is an episode of a series card with no season, so the player reads the season of the episode.
    find_season: bool,
    /// Start at the beginning instead of the saved position.
    pub start_over: bool,
    /// The file version from the Version picker. `None` plays the first version.
    pub file_id: Option<String>,
    /// Start at this source position, for a version switch. It wins over `start_over`.
    start: Option<f64>,
    /// The id of the server shuffle that picked the Item, and the title of its next pick when it has one.
    shuffle: Option<(String, Option<String>)>,
}

impl From<&wire::CatalogItem> for PlayItem {
    /// A series or season card plays its `play_content_id`, an episode.
    fn from(item: &wire::CatalogItem) -> Self {
        let content_id = item.play_content_id.clone().unwrap_or_else(|| item.content_id.clone());
        let season = item.series_id.clone().zip(item.season_number);
        Self {
            find_season: season.is_none() && content_id != item.content_id,
            content_id,
            runtime: item.duration_seconds.or(item.runtime.map(|minutes| minutes as f64 * 60.0)),
            season,
            start_over: false,
            file_id: None,
            start: None,
            shuffle: None,
        }
    }
}

impl From<&wire::CatalogItemDetail> for PlayItem {
    /// A series plays its `play_content_id`, the Next up episode, in season `play_season_number`.
    fn from(item: &wire::CatalogItemDetail) -> Self {
        match &item.play_content_id {
            Some(episode) if item.type_ == "series" => Self {
                content_id: episode.clone(),
                runtime: None,
                season: item.play_season_number.map(|season| (item.content_id.clone(), season)),
                find_season: item.play_season_number.is_none(),
                start_over: false,
                file_id: None,
                start: None,
                shuffle: None,
            },
            _ => Self {
                content_id: item.content_id.clone(),
                runtime: item.duration_seconds.or(item.runtime.map(|minutes| minutes as f64 * 60.0)),
                season: item.series_id.clone().zip(item.season_number),
                find_season: false,
                start_over: false,
                file_id: None,
                start: None,
                shuffle: None,
            },
        }
    }
}

impl PlayItem {
    pub fn episode(series_id: String, episode: wire::Episode) -> Self {
        Self {
            content_id: episode.content_id,
            runtime: Some(episode.runtime as f64 * 60.0),
            season: Some((series_id, episode.season_number)),
            find_season: false,
            start_over: false,
            file_id: None,
            start: None,
            shuffle: None,
        }
    }

    /// The current pick of a shuffle. A pick plays from the beginning, as in the web app.
    pub fn shuffled(shuffle: &wire::Shuffle) -> Self {
        let next = &shuffle.next;
        let next = (next.content_id != shuffle.current.content_id)
            .then(|| next_label(next.season_number, next.episode_number, &next.title));
        Self { start_over: true, shuffle: Some((shuffle.id.to_string(), next)), ..Self::from(&shuffle.current) }
    }

    /// Where playback starts: `None` resumes at the saved position.
    pub fn start_position(&self) -> Option<f64> {
        self.start.or(self.start_over.then_some(0.))
    }
}

/// The title of the next Item on the Up Next card: `S1:E2 · Title` for an episode.
pub fn next_label(season: Option<i64>, episode: Option<i64>, title: &str) -> String {
    match season.zip(episode) {
        Some((season, episode)) => format!("S{season}:E{episode} · {title}"),
        None => title.to_string(),
    }
}

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

/// The text of a playback error, for the Library window.
pub struct PlaybackError(pub String);

/// The mpv handle closed: the video in the Player view stopped.
pub struct PlayerClosed;

/// The Player view closed: playback ended, or the video cannot show.
pub struct ViewClosed;

pub struct Controller {
    client: Option<Arc<Client>>,
    session: Option<Session>,
    mpv: Option<mpv::Player>,
    /// Changes when a handle opens or closes, so that the late events of a closed handle are dropped.
    mpv_id: u64,
    starting: Option<Task<()>>,
    /// The next episode of the Item that plays.
    next: Option<PlayItem>,
    /// The previous episode of the Item that plays.
    previous: Option<PlayItem>,
    /// Looks up the next and the previous episode.
    finding: Option<Task<()>>,
    /// The MPRIS service. It runs while the mpv handle is open.
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
    _mpv_events: Option<Task<()>>,
    _ticks: Task<()>,
    /// The video surface of the Library window. `None` when the window has none, so no Item can play.
    video: Option<Rc<Surface>>,
    /// The Player view shows: from `play` until playback ends.
    in_view: bool,
    /// The player position, for the Player view.
    position: Option<f64>,
    /// The label of the Skip prompt that shows. Empty when none shows.
    prompt: String,
    /// The Markers of the file in player time, for the seek bar of the Player view.
    markers: Vec<Marker>,
    /// The series of the episode that plays. The Track picker choice carries to the next Item of the same series.
    series: Option<String>,
    /// The Item that plays. A version switch plays it again with another file.
    item: Option<PlayItem>,
    /// The file versions of the Item that plays, for the Quality list of the Player view.
    versions: Vec<wire::FileVersion>,
    /// The seek thumbnails of the file that plays. `None` when the server has none.
    trickplay: Option<wire::WatchTrickplay>,
    /// Reads the seek thumbnails again before their URLs expire.
    _trickplay: Option<Task<()>>,
    /// The seconds left on the Up Next card, while it shows.
    up_next: Option<u64>,
    /// The Still watching prompt shows.
    still_watching: bool,
    /// The title of the next episode or shuffle pick, for the Up Next card.
    next_title: Option<String>,
    /// How many Items in a row played by themselves up to the next Item that plays.
    auto_plays: u32,
}

impl EventEmitter<PlaybackError> for Controller {}
impl EventEmitter<PlayerClosed> for Controller {}
impl EventEmitter<ViewClosed> for Controller {}

impl Controller {
    pub fn new(video: Option<Rc<Surface>>, cx: &mut Context<Self>) -> Self {
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
            previous: None,
            finding: None,
            mpris: None,
            _mpris: None,
            track: None,
            art: None,
            duration: None,
            describing: None,
            stopping: None,
            generation: 0,
            _mpv_events: None,
            _ticks: ticks,
            video,
            in_view: false,
            position: None,
            prompt: String::new(),
            markers: Vec::new(),
            series: None,
            item: None,
            versions: Vec::new(),
            trickplay: None,
            _trickplay: None,
            up_next: None,
            still_watching: false,
            next_title: None,
            auto_plays: 0,
        }
    }

    /// Plays `item`. A new Item ends the Playback session that plays now, and plays in the
    /// same Player view. Resume is the default: the server starts at the saved position.
    pub fn play(&mut self, client: Arc<Client>, item: PlayItem, cx: &mut Context<Self>) {
        self.step(Input::End, cx);
        self.generation += 1;
        self.in_view = self.video.is_some();
        (self.position, self.prompt, self.markers) = (None, String::new(), Vec::new());
        (self.up_next, self.still_watching) = (None, false);
        let series = item.season.as_ref().map(|(id, _)| id.clone());
        // A version switch plays the same Item again, so the choice carries too.
        let same = self.item.as_ref().is_some_and(|old| old.content_id == item.content_id);
        let carry = match &self.session {
            Some(session) if same || series.is_some() && series == self.series => session.carry(),
            _ => {
                // The subtitle delay carries as the Track picker choice does. mpv keeps it, so another Item
                // resets it.
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.set_sub_delay(0.0)) {
                    eprintln!("silo-omarchy: cannot reset the subtitle delay: {err}");
                }
                Carry::default()
            }
        };
        self.series = series;
        self.item = Some(item.clone());
        (self.versions, self.trickplay, self._trickplay) = (Vec::new(), None, None);
        // Only the next Item of an automatic advance keeps the count; any other start is the user's.
        let auto_plays = std::mem::take(&mut self.auto_plays);
        self.session =
            Some(Session::new(item.runtime, client.server_settings()).with_carry(carry).with_auto_plays(auto_plays));
        self.client = Some(client.clone());
        // A new Item drops these tasks, so an older answer never reaches the new session.
        self.next = None;
        self.previous = None;
        // A shuffle replaces the series order: Next plays the next pick, and Previous does nothing.
        self.next_title = None;
        self.finding = match &item.shuffle {
            Some((_, next)) => {
                if next.is_some() {
                    self.step(Input::Shuffle, cx);
                }
                self.next_title.clone_from(next);
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
                Err(err) => return eprintln!("silo-omarchy: cannot read the versions of {content_id}: {err}"),
            }
            .ok();
        }));
    }

    /// Reads the seek thumbnails of the file that plays, and reads them again a minute before their URLs expire.
    /// A file with no thumbnails (an error, for example a 404) keeps `trickplay` at `None`.
    fn load_trickplay(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Some(item), Some(plan)) = (self.client.clone(), &self.item, self.plan()) else {
            return;
        };
        let (content_id, file_id) = (item.content_id.clone(), plan.effective_media_file_id.to_string());
        self._trickplay = Some(cx.spawn(async move |this, cx| {
            loop {
                let trickplay = match client.trickplay(&content_id, &file_id).await {
                    Ok(trickplay) => trickplay,
                    Err(err) => return eprintln!("silo-omarchy: no seek thumbnails for {content_id}: {err}"),
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
                        eprintln!("silo-omarchy: cannot read the season of {content_id}: {err}");
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
            let next_title = next.as_ref().ok().and_then(Option::as_ref);
            let next_title = next_title.map(|e| next_label(Some(e.season_number), Some(e.episode_number), &e.title));
            let episode = |found: Result<Option<wire::Episode>, _>| match found {
                Ok(found) => found.map(|episode| PlayItem::episode(series_id.clone(), episode)),
                Err(err) => {
                    eprintln!("silo-omarchy: cannot find the episode before or after {content_id}: {err}");
                    None
                }
            };
            let (next, previous) = (episode(next), episode(previous));
            this.update(cx, |this, cx| {
                this.series = Some(series_id);
                if let Some(next) = next {
                    this.next = Some(next);
                    this.next_title = next_title;
                    this.step(Input::NextEpisode, cx);
                }
                if let Some(previous) = previous {
                    this.previous = Some(previous);
                    this.step(Input::PreviousEpisode, cx);
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
                Err(err) => return eprintln!("silo-omarchy: cannot read {content_id} for MPRIS: {err}"),
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
                Err(err) => eprintln!("silo-omarchy: cannot download the poster for MPRIS: {err}"),
            }
        })
    }

    /// Takes the MPRIS bus name, and sends the MPRIS controls to the session until the mpv handle closes.
    fn start_mpris(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded();
        self._mpris = Some(cx.spawn(async move |this, cx| {
            let player = match mpris::start(move |input| drop(tx.unbounded_send(input))).await {
                Ok(player) => Rc::new(player),
                Err(err) => return eprintln!("silo-omarchy: the media keys do not work: {err}"),
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
        let (Some(player), Some(session)) = (self.mpris.clone(), &self.session) else {
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
                eprintln!("silo-omarchy: cannot update MPRIS: {err}");
            }
        })
        .detach();
    }

    /// Ends the Playback session and closes the Player view. The task ends when the
    /// server has the stop report. The Library window waits for it before the app quits.
    pub fn quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        (self.starting, self._trickplay, self.auto_plays) = (None, None, 0);
        self.step(Input::End, cx);
        self.close_mpv(cx);
        self.stopping.take().unwrap_or_else(|| Task::ready(()))
    }

    /// Back in the Player view: ends the Playback session with the stop report, and closes the view.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        self.quit(cx).detach();
    }

    /// Gives `input` to the Playback session, for example a control of the Player view.
    pub fn step(&mut self, input: Input, cx: &mut Context<Self>) {
        if let Input::Position(position) | Input::Seeked(position) = input {
            self.position = Some(position);
        }
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
        let Some(session) = &mut self.session else {
            return;
        };
        for command in session.step(input, SystemTime::now()) {
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
            Command::Load { url, headers, start, aid } => match self.open_mpv(cx) {
                Ok(mpv) => {
                    if let Err(err) = mpv.load(&url, &headers, start, aid) {
                        eprintln!("silo-omarchy: mpv cannot play {url}: {err}");
                        self.step(Input::LoadFailed, cx);
                    }
                }
                // The session ends with the stop report, and the Library window shows the error on the page.
                Err(err) => {
                    cx.emit(PlaybackError(format!("The player cannot start: {err}")));
                    self.step(Input::End, cx);
                    self.close_mpv(cx);
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
            // The client keeps the position of a failed report and sends it with syncProgress later.
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
            Command::SelectSubtitle(sid) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.select_subtitle(sid)) {
                    eprintln!("silo-omarchy: cannot show subtitle track {sid:?}: {err}");
                }
            }
            Command::SelectAudio(aid) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.select_audio(aid)) {
                    eprintln!("silo-omarchy: cannot play audio track {aid}: {err}");
                }
            }
            Command::ShowMarkers(markers) => self.markers = markers,
            Command::ShowPrompt(label) => self.prompt = label,
            Command::Seek(position) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.seek(position)) {
                    eprintln!("silo-omarchy: cannot seek to {position}: {err}");
                }
            }
            Command::PlayNext => {
                self.auto_plays = self.session.as_ref().map_or(0, Session::auto_plays);
                match self.item.as_ref().and_then(|i| Some((i.shuffle.clone()?.0, i.content_id.clone()))) {
                    Some((id, from)) => self.advance(client, id, from, cx),
                    None => {
                        if let Some(next) = self.next.take() {
                            self.play(client, next, cx);
                        }
                    }
                }
            }
            Command::PlayPrevious => {
                if let Some(previous) = self.previous.take() {
                    self.play(client, previous, cx);
                }
            }
            Command::PlayVersion { file_id, position } => {
                if let Some(item) = self.item.clone() {
                    self.play(client, PlayItem { file_id: Some(file_id), start: position, ..item }, cx);
                }
            }
            Command::SetPause(paused) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.set_pause(paused)) {
                    eprintln!("silo-omarchy: cannot pause or play: {err}");
                }
            }
            Command::UpNext(left) => self.up_next = left,
            Command::StillWatching(shows) => self.still_watching = shows,
            Command::ClosePlayer => self.close_mpv(cx),
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
                    this.close_mpv(cx);
                }
            })
            .ok();
        }));
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
        self.session.as_ref().is_some_and(Session::can_go_next)
    }

    /// The mpv handle is open.
    pub fn is_open(&self) -> bool {
        self.mpv.is_some()
    }

    /// The Player view shows.
    pub fn in_view(&self) -> bool {
        self.in_view
    }

    /// The player position in seconds, once the file plays.
    pub fn position(&self) -> Option<f64> {
        self.position
    }

    /// The duration of the file in seconds, in player time.
    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn paused(&self) -> bool {
        self.session.as_ref().is_some_and(Session::paused)
    }

    /// The title of the Item that plays, once it is known.
    pub fn title(&self) -> Option<&str> {
        self.track.as_ref().map(|track| track.title.as_str())
    }

    /// The label of the Skip prompt that shows, or an empty text.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// The seconds left before the next Item plays, while the Up Next card shows.
    pub fn up_next(&self) -> Option<u64> {
        self.up_next
    }

    /// The title of the next episode or shuffle pick, once it is known.
    pub fn next_title(&self) -> Option<&str> {
        self.next_title.as_deref()
    }

    /// The Item that plays is a shuffle pick.
    pub fn in_shuffle(&self) -> bool {
        self.item.as_ref().is_some_and(|item| item.shuffle.is_some())
    }

    /// The Still watching prompt shows.
    pub fn still_watching(&self) -> bool {
        self.still_watching
    }

    /// The Markers of the file, in player time.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// The tracks of the file, for the Track picker.
    pub fn tracks(&self) -> &[tracks::Track] {
        self.session.as_ref().map_or(&[], Session::tracks)
    }

    /// The plan that plays, for the Playback info panel and the Quality ladder.
    pub fn plan(&self) -> Option<&wire::PlaybackPlan> {
        self.session.as_ref()?.plan()
    }

    /// The step label of the Quality ladder that plays.
    pub fn quality(&self) -> &str {
        self.session.as_ref().map_or(crate::playback::ORIGINAL, Session::quality)
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
        self.mpv.as_ref()?.volume().ok()
    }

    pub fn add_volume(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.add_volume(delta)) {
            eprintln!("silo-omarchy: cannot change the volume: {err}");
        }
        cx.notify();
    }

    /// The subtitle delay in seconds.
    pub fn sub_delay(&self) -> Option<f64> {
        self.mpv.as_ref()?.sub_delay().ok()
    }

    pub fn add_sub_delay(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.add_sub_delay(delta)) {
            eprintln!("silo-omarchy: cannot change the subtitle delay: {err}");
        }
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(Err(err)) = self.mpv.as_ref().map(mpv::Player::toggle_mute) {
            eprintln!("silo-omarchy: cannot mute: {err}");
        }
        cx.notify();
    }

    fn close_mpv(&mut self, cx: &mut Context<Self>) {
        // Dropping the MPRIS player and its task gives up the bus name.
        self.mpris = None;
        self._mpris = None;
        // The render thread stops before mpv, and before the window can close its Wayland connection.
        if let Some(video) = &self.video {
            video.stop();
        }
        if self.mpv.take().is_some() {
            cx.emit(PlayerClosed);
        }
        self.mpv_id += 1;
        if std::mem::take(&mut self.in_view) {
            cx.emit(ViewClosed);
        }
    }

    /// Returns the mpv handle, and opens one in the Player view first when there is none. An error tells why the
    /// video cannot show.
    fn open_mpv(&mut self, cx: &mut Context<Self>) -> Result<&mpv::Player, String> {
        if self.mpv.is_none() {
            let video = self.video.as_ref().ok_or("the window has no video surface (it needs Wayland)")?;
            // Each new mpv handle reads the hardware decoding Local setting again.
            let hwdec = LocalSettings::load(&LocalSettings::dir()).hwdec();
            let (player, mut rx) = open_in_view(video, hwdec)?;
            self.mpv = Some(player);
            self.mpv_id += 1;
            self.start_mpris(cx);
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

/// Opens mpv with no window, and starts the render thread of the Player view on it.
fn open_in_view(video: &Surface, hwdec: &str) -> Result<(mpv::Player, UnboundedReceiver<Input>), String> {
    let (tx, rx) = mpsc::unbounded();
    let player = mpv::Player::open(hwdec, &[("vo", "libmpv")], move |input| drop(tx.unbounded_send(input)))
        .map_err(|err| err.to_string())?;
    video.start(player.video())?;
    Ok((player, rx))
}

#[cfg(test)]
mod tests {
    use super::{PlayItem, next_label};
    use crate::api::wire;

    fn detail(fields: serde_json::Value) -> wire::CatalogItemDetail {
        let mut item: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/get_catalog_item_ok.json")).unwrap();
        item.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(item).unwrap()
    }

    #[test]
    fn a_series_page_plays_its_next_up_episode_with_the_season_so_that_next_episode_works() {
        let series = detail(serde_json::json!({
            "content_id": "series:severance", "type": "series",
            "play_content_id": "episode:severance-s01e02", "play_season_number": 1,
        }));
        let item = PlayItem::from(&series);
        assert_eq!(item.content_id, "episode:severance-s01e02");
        assert_eq!(item.season, Some(("series:severance".into(), 1)));
        assert!(!item.start_over);
    }

    #[test]
    fn a_series_card_plays_its_episode_and_looks_up_the_season_of_it() {
        let page: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/query_catalog_items_ok.json")).unwrap();
        let mut card = page["items"][0].clone();
        let series = serde_json::json!({
            "content_id": "series:severance", "type": "series", "play_content_id": "episode:severance-s01e02",
            "series_id": null, "season_number": null,
        });
        card.as_object_mut().unwrap().extend(series.as_object().unwrap().clone());
        let card: wire::CatalogItem = serde_json::from_value(card).unwrap();
        let item = PlayItem::from(&card);
        assert_eq!((item.content_id.as_str(), item.season.clone()), ("episode:severance-s01e02", None));
        assert!(item.find_season, "Next episode needs the season of the episode");
        let movie = PlayItem::from(&detail(serde_json::json!({"content_id": "movie:heat-1995"})));
        assert!(!movie.find_season);
    }

    #[test]
    fn a_movie_page_plays_the_movie_and_start_over_starts_at_zero() {
        let movie = detail(serde_json::json!({"content_id": "movie:heat-1995", "runtime": 170}));
        let item = PlayItem::from(&movie);
        assert_eq!(
            (item.content_id.as_str(), item.runtime, item.season.clone()),
            ("movie:heat-1995", Some(10200.), None)
        );
        assert_eq!(item.start_position(), None, "the server resumes at the saved position");
        assert_eq!(PlayItem { start_over: true, ..item }.start_position(), Some(0.));
    }

    fn shuffle(next: usize) -> wire::Shuffle {
        let page: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/query_catalog_items_ok.json")).unwrap();
        serde_json::from_value(serde_json::json!({
            "id": "s1", "created_at": "2026-01-01T00:00:00.000Z", "updated_at": "2026-01-01T00:00:00.000Z",
            "scope": {"kind": "library", "id": "1", "title": "Movies"},
            "current": page["items"][0], "next": page["items"][next],
        }))
        .unwrap()
    }

    #[test]
    fn a_shuffle_pick_plays_from_the_beginning_and_knows_if_a_next_pick_exists() {
        let item = PlayItem::shuffled(&shuffle(1));
        assert_eq!(item.content_id, shuffle(1).current.content_id);
        assert_eq!(item.start_position(), Some(0.), "a pick does not resume");
        assert_eq!(item.shuffle, Some(("s1".into(), Some(shuffle(1).next.title))));
        let only = PlayItem::shuffled(&shuffle(0));
        assert_eq!(only.shuffle, Some(("s1".into(), None)), "a scope with one Item has no next pick");
    }

    #[test]
    fn the_up_next_title_of_an_episode_has_its_season_and_number() {
        assert_eq!(next_label(Some(1), Some(2), "Half Loop"), "S1:E2 · Half Loop");
        assert_eq!(next_label(None, None, "Heat"), "Heat");
    }
}
