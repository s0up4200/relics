//! The Player window controller. It holds the current Playback session and the mpv
//! handle, feeds the session the mpv events and a tick each second, and carries out
//! the commands of the session. The app has at most one Player window.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{AppContext as _, Context, EventEmitter, Hsla, Rgba, Task};
use gpui_omarchy::ActiveTheme as _;
use mpris_server::PlaybackStatus;

use crate::api::{Client, wire};
use crate::mpris::{self, Track};
use crate::playback::{Command, Input, Session};
use crate::settings::LocalSettings;
use crate::{artwork, mpv};

/// What the Player window needs to play an Item.
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
            },
            _ => Self {
                content_id: item.content_id.clone(),
                runtime: item.duration_seconds.or(item.runtime.map(|minutes| minutes as f64 * 60.0)),
                season: item.series_id.clone().zip(item.season_number),
                find_season: false,
                start_over: false,
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
        }
    }

    /// Where playback starts: `None` resumes at the saved position.
    pub fn start_position(&self) -> Option<f64> {
        self.start_over.then_some(0.)
    }
}

/// The width of the poster that MPRIS shows, in pixels.
const MPRIS_POSTER_WIDTH: f32 = 512.0;

/// The text of a playback error, for the Library window.
pub struct PlaybackError(pub String);

/// The Player window closed.
pub struct PlayerClosed;

pub struct PlayerWindow {
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
    /// The MPRIS service. It runs while the Player window is open.
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
}

impl EventEmitter<PlaybackError> for PlayerWindow {}
impl EventEmitter<PlayerClosed> for PlayerWindow {}

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
        }
    }

    /// Plays `item`. A new Item ends the Playback session that plays now, and plays in the
    /// same Player window. Resume is the default: the server starts at the saved position.
    pub fn play(&mut self, client: Arc<Client>, item: PlayItem, cx: &mut Context<Self>) {
        self.step(Input::End, cx);
        self.generation += 1;
        self.session = Some(Session::new(item.runtime, client.server_settings()));
        self.client = Some(client.clone());
        // A new Item drops these tasks, so an older answer never reaches the new session.
        self.next = None;
        self.previous = None;
        self.finding = self.find_episodes(&client, &item, cx);
        (self.track, self.art, self.duration) = (None, None, None);
        self.describing = Some(self.describe(client.clone(), item.content_id.clone(), cx));
        let start = item.start_position();
        let content_id = item.content_id;
        // A new Item drops this task, so the reply of an older start never reaches the new session.
        self.starting = Some(cx.spawn(async move |this, cx| {
            let input = match client.start_playback(&content_id, start).await {
                Ok(decision) => Input::Started { decision, access: client.media_access() },
                Err(err) => Input::StartFailed(err.to_string()),
            };
            this.update(cx, |this, cx| this.step(input, cx)).ok();
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
            let episode = |found: Result<Option<wire::Episode>, _>| match found {
                Ok(found) => found.map(|episode| PlayItem::episode(series_id.clone(), episode)),
                Err(err) => {
                    eprintln!("silo-omarchy: cannot find the episode before or after {content_id}: {err}");
                    None
                }
            };
            let (next, previous) = (episode(next), episode(previous));
            this.update(cx, |this, cx| {
                if let Some(next) = next {
                    this.next = Some(next);
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
            let item = match client.item(&content_id, artwork::Rung::card(MPRIS_POSTER_WIDTH, 1.0)).await {
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
            let http = cx.update(|cx| cx.http_client());
            let file = cx.background_spawn(async move { artwork::fetch(&http, &artwork::cache_dir(), &url).await });
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

    /// Takes the MPRIS bus name, and sends the MPRIS controls to the session until the Player window closes.
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

    /// Ends the Playback session and closes the Player window. The task ends when the
    /// server has the stop report. The Library window waits for it before the app quits.
    pub fn quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.starting = None;
        self.step(Input::End, cx);
        self.close_mpv(cx);
        self.stopping.take().unwrap_or_else(|| Task::ready(()))
    }

    fn step(&mut self, input: Input, cx: &mut Context<Self>) {
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
                    eprintln!("silo-omarchy: cannot show subtitle track {sid}: {err}");
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
            Command::PlayPrevious => {
                if let Some(previous) = self.previous.take() {
                    self.play(client, previous, cx);
                }
            }
            Command::SetPause(paused) => {
                if let Some(Err(err)) = self.mpv.as_ref().map(|mpv| mpv.set_pause(paused)) {
                    eprintln!("silo-omarchy: cannot pause or play: {err}");
                }
            }
            Command::ClosePlayer => self.close_mpv(cx),
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

    /// The Player window is open.
    pub fn is_open(&self) -> bool {
        self.mpv.is_some()
    }

    fn close_mpv(&mut self, cx: &mut Context<Self>) {
        // Dropping the MPRIS player and its task gives up the bus name.
        self.mpris = None;
        self._mpris = None;
        if self.mpv.take().is_some() {
            cx.emit(PlayerClosed);
        }
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
            // Each new mpv handle reads the hardware decoding Local setting again.
            let hwdec = LocalSettings::load(&LocalSettings::dir()).hwdec();
            self.mpv = Some(mpv::Player::open(hwdec, &options, move |input| {
                tx.unbounded_send(input).ok();
            })?);
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

/// A color as `#RRGGBB`.
fn hex(color: Hsla) -> String {
    let Rgba { r, g, b, .. } = color.into();
    let byte = |c: f32| (c * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::PlayItem;
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
}
