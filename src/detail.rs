//! The detail page of an Item: a hero with the backdrop, the poster, the type and the studio or network, the title
//! logo or the title, the tagline, the metadata line, the quality badges, the ratings, the overview, the Directed by
//! and Written by line, and the buttons. A series shows its season cards and a grid of 16:9 episode cards. An
//! episode shows the other episodes of its season under More Episodes. Then come the Cast, Crew, Trailers and More
//! Like This sections. Each request asks for the artwork rung that the page needs at the window scale.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ListAlignment,
    ListState, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, RenderImage, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled, Task, Window, anchored, canvas, deferred, div,
    linear_color_stop, linear_gradient, list, px, relative,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, Error, wire};
use crate::artwork::{self, ArtworkCache, Kind, Rung};
use crate::click::Clickable as _;
use crate::grid::Move;
use crate::home::{Open, Play, badge_label, card, progress, progress_bar};
use crate::keys::Key;
use crate::player::{PlayItem, start_shuffle};
use crate::search::OpenPerson;
use crate::sideways::Row;

const EPISODE_WIDTH: f32 = 250.;
const EPISODE_HEIGHT: f32 = 140.;
const GAP: f32 = 14.;
/// The left and right margin of the page.
const MARGIN: f32 = 48.;
const POSTER_WIDTH: f32 = 200.;
const SEASON_WIDTH: f32 = 130.;
const CAST_WIDTH: f32 = 110.;
const TRAILER_WIDTH: f32 = 220.;
const LOGO_WIDTH: f32 = 400.;
const LOGO_HEIGHT: f32 = 110.;

pub struct Detail {
    client: Arc<Client>,
    /// The id of the Item.
    id: String,
    item: Option<wire::CatalogItemDetail>,
    /// The file versions of a movie. A series has none here.
    versions: Vec<wire::FileVersion>,
    /// The index of the chosen version in `versions`.
    version: usize,
    seasons: Vec<wire::Season>,
    /// The index of the chosen season in `seasons`.
    season: usize,
    /// The season number to choose when the seasons load, from an episode or season card.
    wanted_season: Option<i64>,
    episodes: Vec<wire::Episode>,
    focus: Focus,
    /// The page has the keyboard focus, so the focused control shows the focus ring.
    pub active: bool,
    /// A refresh came while the page did not show. The page reloads when it shows again.
    stale: bool,
    columns: usize,
    /// The error of the last request, shown with Retry.
    error: Option<String>,
    /// The error of the last write action, for example a favorite that did not save. It shows at the bottom of the
    /// window.
    notice: Option<String>,
    /// The season or episode card whose menu is open.
    menu: Option<Mark>,
    /// Where a right-click opened the menu, in window coordinates. The `o` key opens it below the card.
    menu_at: Option<Point<Pixels>>,
    /// The Thumbhash of the backdrop, the poster and each cast photo.
    thumbs: Vec<Option<Arc<RenderImage>>>,
    episode_thumbs: Vec<Option<Arc<RenderImage>>>,
    season_thumbs: Vec<Option<Arc<RenderImage>>>,
    /// The Items of More Like This, from the similar call, and their Thumbhashes.
    similar: Vec<wire::CatalogItem>,
    similar_thumbs: Vec<Option<Arc<RenderImage>>>,
    /// The page list. It draws only the items near the viewport: the hero, one item for each row of cards, and one
    /// for each line of the episode grid.
    page: ListState,
    /// The items of the page list when it last drew, from `Rows::items`.
    items: Vec<(usize, usize)>,
    /// The sideways scroll of each row of cards, by the row number of `Focus::place`.
    row_scrolls: [ScrollHandle; 9],
    /// The width of the window when the page last drew.
    viewport: f32,
    /// The height of the hero content when the page last drew.
    content_height: f32,
    artwork: Entity<ArtworkCache>,
    /// The artwork rung of the Item and of the episodes, at the window scale when the page opened.
    // ponytail: read once; a move to a larger scale keeps it until the page opens again.
    image_size: Rung,
    episode_size: Rung,
    _loading: Task<()>,
    _episodes: Task<()>,
    _similar: Task<()>,
    _shuffle: Task<()>,
}

impl EventEmitter<Play> for Detail {}
impl EventEmitter<OpenPerson> for Detail {}
impl EventEmitter<Open> for Detail {}

impl Detail {
    pub fn new(client: Arc<Client>, id: String, season: Option<i64>, window: &Window, cx: &mut Context<Self>) -> Self {
        let scale = window.scale_factor();
        // The backdrop fills the page next to the sidebar.
        let viewport = window.viewport_size().width.to_f64() as f32;
        let width = viewport - crate::pages::sidebar_width(viewport, true);
        cx.on_release(|this, cx| {
            let thumbs = this
                .thumbs
                .drain(..)
                .chain(this.episode_thumbs.drain(..))
                .chain(this.season_thumbs.drain(..))
                .chain(this.similar_thumbs.drain(..));
            for thumb in thumbs.flatten() {
                cx.drop_image(thumb, None);
            }
        })
        .detach();
        let mut detail = Self {
            client,
            id,
            item: None,
            versions: Vec::new(),
            version: 0,
            seasons: Vec::new(),
            season: 0,
            wanted_season: season,
            episodes: Vec::new(),
            focus: Focus::Action(0),
            active: false,
            stale: false,
            columns: 1,
            error: None,
            notice: None,
            menu: None,
            menu_at: None,
            thumbs: Vec::new(),
            episode_thumbs: Vec::new(),
            season_thumbs: Vec::new(),
            similar: Vec::new(),
            similar_thumbs: Vec::new(),
            page: ListState::new(0, ListAlignment::Top, px(800.)),
            items: Vec::new(),
            row_scrolls: std::array::from_fn(|_| ScrollHandle::new()),
            viewport,
            content_height: 0.,
            artwork: artwork::global(cx),
            image_size: Rung::fit(Kind::Backdrop, width, scale)
                .max(Rung::fit(Kind::Poster, POSTER_WIDTH, scale))
                .max(Rung::fit(Kind::Profile, CAST_WIDTH, scale)),
            episode_size: Rung::fit(Kind::Still, EPISODE_WIDTH, scale),
            _loading: Task::ready(()),
            _episodes: Task::ready(()),
            _similar: Task::ready(()),
            _shuffle: Task::ready(()),
        };
        detail.load(cx);
        detail
    }

    /// The Item of the page, and the series of an episode.
    pub fn shows(&self) -> (&str, Option<&str>) {
        (&self.id, self.item.as_ref().and_then(|item| item.series_id.as_deref()))
    }

    /// Reloads the page now when it shows. Otherwise the page reloads when it shows again.
    pub fn refresh(&mut self, shows: bool, cx: &mut Context<Self>) {
        self.stale = true;
        if shows {
            self.shown(cx);
        }
    }

    /// The page shows again.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.stale) {
            self.load(cx);
        }
    }

    /// Loads More Like This. A failure leaves the row out, or keeps the row of the last load: the page works without
    /// it.
    fn load_similar(&mut self, cx: &mut Context<Self>) {
        let (client, id) = (self.client.clone(), self.id.clone());
        self._similar = cx.spawn(async move |this, cx| {
            let items = match client.similar(&id).await {
                Ok(items) => items,
                Err(err) => return eprintln!("relics: cannot read the Items like {id}: {err}"),
            };
            this.update(cx, |this, cx| {
                let thumbs = items.iter().map(|i| i.poster_thumbhash.as_deref().and_then(artwork::thumbhash)).collect();
                for old in std::mem::replace(&mut this.similar_thumbs, thumbs).into_iter().flatten() {
                    cx.drop_image(old, None);
                }
                this.similar = items;
                cx.notify();
            })
            .ok();
        });
    }

    /// Loads the Item, the seasons of a series, the episodes of the chosen season or of the season of an episode,
    /// and More Like This. A reload keeps the chosen season and what the page shows.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.load_similar(cx);
        let (client, id, image_size) = (self.client.clone(), self.id.clone(), self.image_size);
        self._loading = cx.spawn(async move |this, cx| {
            let result: Result<_, Error> = async {
                let item = client.item(&id, image_size).await?;
                let (seasons, versions) = if item.type_ == "series" {
                    (client.seasons(&id, image_size).await?, Vec::new())
                } else {
                    (Vec::new(), client.versions(&id).await?)
                };
                Ok((item, seasons, versions))
            }
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((item, seasons, versions)) => {
                        let thumbs = [&item.backdrop_thumbhash, &item.poster_thumbhash]
                            .into_iter()
                            .chain(item.cast.iter().map(|c| &c.photo_thumbhash))
                            .chain(item.crew.iter().map(|c| &c.photo_thumbhash))
                            .map(|hash| hash.as_deref().and_then(artwork::thumbhash))
                            .collect();
                        for old in std::mem::replace(&mut this.thumbs, thumbs).into_iter().flatten() {
                            cx.drop_image(old, None);
                        }
                        let posters = seasons
                            .iter()
                            .map(|s| s.poster_thumbhash.as_deref().and_then(artwork::thumbhash))
                            .collect();
                        for old in std::mem::replace(&mut this.season_thumbs, posters).into_iter().flatten() {
                            cx.drop_image(old, None);
                        }
                        let reload = this.item.is_some();
                        let chosen = this.seasons.get(this.season).map(|s| s.season_number);
                        let number = this.wanted_season.take().or(chosen).or(item.play_season_number);
                        this.season = seasons.iter().position(|s| Some(s.season_number) == number).unwrap_or(0);
                        this.seasons = seasons;
                        this.version = kept_version(&this.versions, this.version, &versions);
                        this.versions = versions;
                        this.item = Some(item);
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                        this.load_episodes(!reload, cx);
                    }
                    // The page keeps what it shows, with no error.
                    Err(err) if this.item.is_some() => eprintln!("relics: cannot reload {}: {err}", this.id),
                    // The Library window shows Offline or the sign-in screen. The page shows Retry.
                    Err(err) => this.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// The series id and the season number of the episodes that the page lists.
    fn listed_season(&self) -> Option<(String, i64)> {
        episode_list(self.item.as_ref()?, self.seasons.get(self.season).map(|s| s.season_number))
    }

    /// Loads the episodes of the chosen season, or of the season of an episode. With `clear`, the old episodes go
    /// at once. Otherwise they show until the new ones come.
    fn load_episodes(&mut self, clear: bool, cx: &mut Context<Self>) {
        let Some((id, season)) = self.listed_season() else {
            return;
        };
        if clear {
            for old in self.episode_thumbs.drain(..).flatten() {
                cx.drop_image(old, None);
            }
            self.episodes.clear();
        }
        let (client, image_size) = (self.client.clone(), self.episode_size);
        self._episodes = cx.spawn(async move |this, cx| {
            let result = client.season_episodes(&id, season, image_size).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(episodes) => {
                        let thumbs = episodes
                            .iter()
                            .map(|e| e.still_thumbhash.as_deref().and_then(artwork::thumbhash))
                            .collect();
                        for old in std::mem::replace(&mut this.episode_thumbs, thumbs).into_iter().flatten() {
                            cx.drop_image(old, None);
                        }
                        this.episodes = episodes;
                    }
                    Err(err) if !this.episodes.is_empty() => eprintln!("relics: cannot reload the episodes: {err}"),
                    Err(err) => this.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn rows(&self) -> Rows {
        Rows {
            actions: self.item.as_ref().map_or(0, |item| actions(item, &self.seasons).len()),
            versions: self.picker_len(),
            version: self.version,
            seasons: self.seasons.len(),
            season: self.season,
            shuffle: usize::from(self.shufflable_season().is_some()),
            episodes: self.episodes.len(),
            cast: self.item.as_ref().map_or(0, |item| item.cast.len()),
            crew: self.item.as_ref().map_or(0, |item| item.crew.len()),
            trailers: self.item.as_ref().map_or(0, |item| trailers(item).len()),
            similar: self.similar.len(),
            columns: self.columns,
        }
    }

    /// The pills of the Version picker: one for each version, or none for a single version.
    fn picker_len(&self) -> usize {
        if self.versions.len() > 1 { self.versions.len() } else { 0 }
    }

    /// The Item that Play starts, with the version from the Version picker.
    fn play_item(&self, item: &wire::CatalogItemDetail) -> PlayItem {
        let mut play = PlayItem::from(item);
        if self.picker_len() > 0 {
            play.file_id = self.versions.get(self.version).map(|v| v.file_id.to_string());
        }
        play
    }

    /// Handles a key. Returns false when the page does not use it, for example Esc or h at the left edge.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        let Some(item) = &self.item else {
            return false;
        };
        // The menu of a season or episode card takes each key.
        self.menu = self.open_menu();
        if let Some(mark) = self.menu {
            match key {
                Key::Other("enter") => self.mark(mark, cx),
                Key::Other("escape" | "backspace" | "o") => self.menu = None,
                _ => {}
            }
            cx.notify();
            return true;
        }
        match (self.focus, key) {
            (Focus::Action(i), Key::Other("enter")) => {
                if let Some(&action) = actions(item, &self.seasons).get(i) {
                    self.act(action, cx);
                }
            }
            (Focus::Season(i), Key::Other("o")) => self.show_menu(Mark::Season(i), None),
            (Focus::Episode(i), Key::Other("o")) => self.show_menu(Mark::Episode(i), None),
            (Focus::ShuffleSeason(_), Key::Other("enter")) => self.shuffle_season(cx),
            (Focus::Version(i), Key::Other("enter")) => self.choose_version(i, cx),
            (Focus::Season(i), Key::Other("enter")) => self.choose_season(i, cx),
            (Focus::Episode(i), Key::Other("enter" | "p")) => self.play_episode(i, cx),
            (Focus::Cast(_) | Focus::Crew(_), Key::Other("enter")) => self.open_person(self.focus, cx),
            (Focus::Trailer(i), Key::Other("enter")) => self.open_trailer(i, cx),
            (Focus::Similar(i), Key::Other("enter")) => self.open_similar(i, cx),
            (_, Key::Other("p")) => cx.emit(Play(self.play_item(item))),
            (focus, key) => match self.rows().step(focus, key) {
                Step::To(focus) => {
                    self.focus = focus;
                    let rows = self.rows();
                    self.page.scroll_to_reveal_item(rows.child(focus));
                    let (row, i) = focus.place();
                    if let Some(sideways) = sideways(row) {
                        sideways.reveal(&self.row_scrolls[row], i);
                    }
                }
                Step::OutLeft | Step::Stay => return false,
            },
        }
        cx.notify();
        true
    }

    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some(item) = &self.item else {
            return;
        };
        let mut play = self.play_item(item);
        let state = |flag: fn(&wire::CatalogItemUserState) -> bool| item.user_state.as_ref().is_some_and(flag);
        let (client, id) = (self.client.clone(), self.id.clone());
        match action {
            Action::Play { .. } => cx.emit(Play(play)),
            Action::StartOver => {
                play.start_over = true;
                cx.emit(Play(play));
            }
            Action::Shuffle => self.shuffle("series", id, cx),
            Action::Watchlist => {
                let on = !state(|s| s.in_watchlist);
                let call = async move { client.set_watchlist(&id, on).await };
                self.write(
                    call,
                    move |this| this.item.iter_mut().for_each(|item| user_state(item).in_watchlist = on),
                    cx,
                );
            }
            Action::Favorite => {
                let on = !state(|s| s.is_favorite);
                let call = async move { client.set_favorite(&id, on).await };
                self.write(
                    call,
                    move |this| this.item.iter_mut().for_each(|item| user_state(item).is_favorite = on),
                    cx,
                );
            }
            Action::Rate(stars) => {
                let rating = star_press(item.user_rating, stars);
                let call = async move { client.set_rating(&id, rating).await };
                self.write(
                    call,
                    move |this| this.item.iter_mut().for_each(|item| item.user_rating = rating.map(Into::into)),
                    cx,
                );
            }
            Action::Watched => self.mark(Mark::Item, cx),
        }
        cx.notify();
    }

    /// Marks the Item, a season or an episode watched, or unwatched when it is watched.
    fn mark(&mut self, mark: Mark, cx: &mut Context<Self>) {
        self.menu = None;
        let target = match mark {
            Mark::Item => {
                self.item.as_ref().map(|item| (self.id.clone(), !item.user_state.as_ref().is_some_and(|s| s.played)))
            }
            Mark::Season(i) => self.seasons.get(i).map(|s| (s.content_id.clone(), !season_watched(s))),
            Mark::Episode(i) => self.episodes.get(i).map(|e| (e.content_id.clone(), !episode_watched(e))),
        };
        let Some((id, on)) = target else {
            return;
        };
        let client = self.client.clone();
        let call = {
            let id = id.clone();
            async move { client.set_watched(&id, on).await }
        };
        self.write(
            call,
            move |this| {
                // The list can show another season by now.
                let listed = match mark {
                    Mark::Episode(i) => this.episodes.get(i).is_some_and(|e| e.content_id == id),
                    _ => true,
                };
                if let (Some(item), true) = (&mut this.item, listed) {
                    apply_mark(item, &mut this.seasons, this.season, &mut this.episodes, mark, on);
                }
            },
            cx,
        );
    }

    /// Opens the menu of a season or episode card, at the pointer `at` or below the card.
    fn show_menu(&mut self, mark: Mark, at: Option<Point<Pixels>>) {
        self.menu = Some(mark);
        self.menu_at = at;
    }

    /// The id of the chosen season, when it has 2 episodes or more to shuffle.
    fn shufflable_season(&self) -> Option<String> {
        self.seasons.get(self.season).filter(|s| s.episode_count >= 2).map(|s| s.content_id.clone())
    }

    /// Starts a shuffle of the chosen season.
    fn shuffle_season(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.shufflable_season() {
            self.shuffle("season", id, cx);
        }
    }

    /// The Shuffle season button, above the episodes of the chosen season.
    fn shuffle_season_button(&self, cx: &Context<Self>) -> AnyElement {
        let focus = Focus::ShuffleSeason(0);
        let pill = self.pill(("detail-shuffle-season", 0), "Shuffle season".into(), false, self.focus == focus, cx);
        div()
            .px(px(MARGIN))
            .flex()
            .child(pill.on_click(cx.listener(move |this, _, _, cx| {
                this.focus = focus;
                this.shuffle_season(cx);
            })))
            .into_any_element()
    }

    /// Starts a server shuffle of a series or a season. A failure shows in the notice.
    fn shuffle(&mut self, kind: &'static str, id: String, cx: &mut Context<Self>) {
        self.notice = None;
        self._shuffle = start_shuffle(self.client.clone(), kind, id, cx, |this, err| this.notice = Some(err));
    }

    /// The open menu of a season or episode card. A click on another control moves the focus, and that closes the
    /// menu.
    fn open_menu(&self) -> Option<Mark> {
        self.menu.filter(|mark| match (*mark, self.focus) {
            (Mark::Season(i), Focus::Season(j)) | (Mark::Episode(i), Focus::Episode(j)) => i == j,
            _ => false,
        })
    }

    /// A season or episode card with the right-click that opens its menu. The menu is a short list that floats over
    /// the page, so that the other cards do not move. A click outside the menu closes it.
    fn with_menu(
        &self,
        card: gpui::Stateful<gpui::Div>,
        (mark, focus): (Mark, Focus),
        watched: bool,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let card = card.on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.focus = focus;
                this.show_menu(mark, Some(event.position));
                cx.notify();
            }),
        );
        let menu = (self.open_menu() == Some(mark)).then(|| {
            let t = cx.omarchy();
            let label = if watched { "Mark unwatched" } else { "Mark watched" };
            let entry = div()
                .id("detail-mark")
                .cursor_pointer()
                .px(px(12.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(14.))
                .bg(if self.active { t.accent.alpha(0.25) } else { gpui::transparent_black() })
                .hover(|entry| entry.bg(t.accent.alpha(0.25)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.mark(mark, cx)));
            let panel = div()
                .min_w(px(160.))
                .p(px(4.))
                .flex()
                .flex_col()
                .rounded(px(8.))
                .border_1()
                .border_color(gpui::hsla(0., 0., 1., 0.1))
                .bg(t.surface)
                .shadow_lg()
                .child(entry)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    cx.notify();
                }));
            let menu = match self.menu_at {
                Some(at) => anchored().position(at),
                None => anchored(),
            };
            div().h_0().child(deferred(menu.snap_to_window().child(panel.mt(px(6.)))))
        });
        div().flex_none().flex().flex_col().items_start().child(card).children(menu)
    }

    /// Sends a write action. The page shows the new state only when the server saved it. A failure, for example
    /// while the app is Offline, shows an error and leaves the state as it was. Nothing is kept to send later.
    fn write(
        &mut self,
        call: impl Future<Output = Result<(), Error>> + 'static,
        apply: impl FnOnce(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.notice = None;
        let focused = match self.focus {
            Focus::Action(i) => self.item.as_ref().and_then(|item| actions(item, &self.seasons).get(i).copied()),
            _ => None,
        };
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        apply(this);
                        // A button can move or go, for example Resume after Mark watched. The focus stays on the
                        // button, or goes to the last one.
                        if let (Some(action), Some(item), Focus::Action(_)) = (focused, &this.item, this.focus) {
                            let actions = actions(item, &this.seasons);
                            let i = actions.iter().position(|a| *a == action).unwrap_or(actions.len() - 1);
                            this.focus = Focus::Action(i);
                        }
                    }
                    Err(err) => this.notice = Some(format!("Not saved: {err}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn choose_version(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Version(index);
        self.version = index;
        cx.notify();
    }

    fn choose_season(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Season(index);
        if index != self.season {
            self.season = index;
            self.load_episodes(true, cx);
        }
        cx.notify();
    }

    /// Plays an episode at its saved position, with the series, so that Next episode works.
    fn play_episode(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Episode(index);
        if let (Some(episode), Some((series, _))) = (self.episodes.get(index), self.listed_season()) {
            cx.emit(Play(PlayItem::episode(series, episode.clone())));
        }
        cx.notify();
    }

    /// Opens the Person page of the cast or crew member at `focus`, when the server knows the person.
    fn open_person(&mut self, focus: Focus, cx: &mut Context<Self>) {
        self.focus = focus;
        let person = self.item.as_ref().and_then(|item| match focus {
            Focus::Cast(i) => item.cast.get(i).map(|c| (&c.person_id, &c.name)),
            Focus::Crew(i) => item.crew.get(i).map(|c| (&c.person_id, &c.name)),
            _ => None,
        });
        if let Some((Some(id), name)) = person {
            cx.emit(OpenPerson { id: id.clone(), name: name.clone() });
        }
        cx.notify();
    }

    /// Opens a trailer in the default browser.
    fn open_trailer(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Trailer(index);
        if let Some((_, url)) = self.item.as_ref().and_then(|item| trailers(item).into_iter().nth(index)) {
            cx.open_url(&url);
        }
        cx.notify();
    }

    /// Opens the detail page of a More Like This Item.
    fn open_similar(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Similar(index);
        if let Some(item) = self.similar.get(index) {
            cx.emit(Open(item.clone()));
        }
        cx.notify();
    }

    fn url(&self, url: &Option<String>) -> Option<String> {
        let url = url.as_deref().filter(|url| !url.is_empty())?;
        self.client.server().url.join(url).ok().map(String::from)
    }

    fn thumb(&self, index: usize) -> Option<Arc<RenderImage>> {
        self.thumbs.get(index).cloned().flatten()
    }

    fn pill(
        &self,
        id: (&'static str, usize),
        label: SharedString,
        primary: bool,
        focused: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = cx.omarchy();
        let focused = self.active && focused;
        div()
            .id(id)
            .clickable()
            .px(px(18.))
            .py(px(9.))
            .rounded_full()
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .bg(if primary { t.foreground } else { gpui::hsla(0., 0., 1., 0.1) })
            .text_color(if primary { t.background } else { t.foreground })
            .border_2()
            .border_color(if focused { t.accent } else { gpui::transparent_black() })
            .child(label)
    }

    /// The hero, in a window of `window` (width, height) logical pixels. The hero clips the backdrop. Scrims in the
    /// theme background color fade its bottom edge into the page, as in the web UI.
    fn hero(&self, item: &wire::CatalogItemDetail, window: (f32, f32), cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let width = window.0 - crate::pages::sidebar_width(window.0, true);
        let height = hero_height(window, self.content_height);
        let crop = backdrop_box((width, height));
        let (poster, column) = hero_columns(width);
        let glow = artwork_color(item.backdrop_thumbhash.as_deref(), t.accent);
        let background = t.background;
        let (mut buttons, mut star_buttons) = (Vec::new(), Vec::new());
        for (i, action) in actions(item, &self.seasons).into_iter().enumerate() {
            let focused = self.focus == Focus::Action(i);
            let label: SharedString = action_label(item, action).into();
            let click = cx.listener(move |this, _, _, cx| {
                // A click on a star does not move the focus, so that the star does not stay in the accent color.
                if !matches!(action, Action::Rate(_)) {
                    this.focus = Focus::Action(i);
                }
                this.act(action, cx);
            });
            match action {
                // A bare star. The focused star shows in the accent color, with no ring.
                Action::Rate(_) => star_buttons.push(
                    div()
                        .id(("detail-action", i))
                        .clickable()
                        .px(px(2.))
                        .text_color(if self.active && focused { t.accent } else { t.foreground })
                        .child(label)
                        .on_click(click),
                ),
                _ => buttons.push(
                    self.pill(("detail-action", i), label, i == 0, focused, cx).on_click(click).into_any_element(),
                ),
            }
        }
        // The stars share one glass pill, so they wrap to the next line as one unit.
        buttons.push(
            div()
                .flex()
                .px(px(14.))
                .py(px(9.))
                .rounded_full()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .bg(gpui::hsla(0., 0., 1., 0.1))
                .border_2()
                .border_color(gpui::transparent_black())
                .children(star_buttons)
                .into_any_element(),
        );
        let labels = self.versions.get(self.version).map_or_else(|| quality_badges(item), version_labels);
        let title_text = {
            let (text, color) = (item.title.clone(), t.bright);
            move || {
                div()
                    .text_size(px(46.))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(color)
                    .child(text.clone())
                    .into_any_element()
            }
        };
        // The logo shows in place of the title. The title shows while the logo loads, and when it fails.
        let title = match self.url(&item.logo_url) {
            Some(url) => artwork::logo(url, (LOGO_WIDTH.min(column), LOGO_HEIGHT), title_text),
            None => title_text(),
        };
        let secondary = t.secondary;
        // The text lines stop at 640 px, so that they stay easy to read. The buttons can use the full width.
        let text = || div().max_w(px(640.));
        let line =
            |line: String| (!line.is_empty()).then(|| text().text_size(px(14.)).text_color(secondary).child(line));
        let badges = labels.into_iter().map(|badge| {
            div().px(px(8.)).py(px(2.)).rounded_full().bg(gpui::hsla(0., 0., 1., 0.1)).text_size(px(10.)).child(badge)
        });
        div()
            .relative()
            .w_full()
            .h(px(height))
            .flex_none()
            .overflow_hidden()
            // The page tint, from the web `body` background. It shows only where the hero has no backdrop.
            .child(div().absolute().top_0().left_0().right_0().h(px(window.1 / 3.)).bg(linear_gradient(
                180.,
                linear_color_stop(glow.alpha(0.1), 0.),
                linear_color_stop(glow.alpha(0.), 1.),
            )))
            .child(
                div()
                    .absolute()
                    .left(px(crop.origin.x))
                    .top(px(crop.origin.y))
                    .w(px(crop.size.width))
                    .h(px(crop.size.height))
                    .child(artwork::image(self.url(&item.backdrop_url), self.thumb(0), crop.size.width)),
            )
            // The scrims of the web UI. The left fade reaches 68% of the width, not 52%, so that it covers the text.
            // The other scrims are the web `detail-hero-scrim-under`, `detail-hero-ambient` and `detail-hero-scrim-over`.
            .children(fade(90., background, &[(0., 0.92), (0.24, 0.62), (0.47, 0.24), (0.68, 0.)]))
            .child(div().absolute().inset_0().bg(background.alpha(0.1)))
            .children(fade(0., glow, &[(0., 0.08), (0.3, 0.)]))
            .children(fade(0., background, &[(0., 1.), (0.14, 0.86), (0.34, 0.46), (0.54, 0.12), (0.7, 0.)]))
            .children(
                [0., 90., 180., 270.]
                    .into_iter()
                    .flat_map(|angle| fade(angle, gpui::black(), &[(0., 0.18), (0.15, 0.)])),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .items_end()
                    .gap(px(32.))
                    .px(px(MARGIN))
                    .pt(px(28.))
                    .pb(px(28.))
                    .child(self.measure(cx))
                    .children(poster.then(|| {
                        div()
                            .w(px(POSTER_WIDTH))
                            .h(px(300.))
                            .flex_none()
                            .rounded(px(12.))
                            .overflow_hidden()
                            .bg(t.surface)
                            .child(artwork::image(self.url(&item.poster_url), self.thumb(1), POSTER_WIDTH))
                    }))
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(div().text_size(px(11.)).text_color(t.secondary).child(kind_line(item)))
                            .child(title)
                            .children(
                                item.tagline
                                    .clone()
                                    .filter(|text| !text.is_empty())
                                    .map(|tagline| text().italic().child(tagline)),
                            )
                            .child(div().text_color(t.secondary).child(meta_line(item)))
                            .child(div().flex().flex_wrap().gap(px(6.)).children(badges))
                            .children(line(ratings_line(item)))
                            .children(item.overview.clone().map(|overview| text().line_clamp(3).child(overview)))
                            .children(line(credits_line(item)))
                            .child(div().pt(px(6.)).flex().flex_wrap().gap(px(10.)).children(buttons)),
                    ),
            )
            .into_any_element()
    }

    /// An element that fills the hero content and stores its height in `content_height` after layout. When the height
    /// changes, the page draws again.
    fn measure(&self, cx: &Context<Self>) -> AnyElement {
        let page = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                let height = bounds.size.height.to_f64() as f32;
                page.update(cx, |this, cx| {
                    if this.content_height != height {
                        this.content_height = height;
                        cx.notify();
                    }
                })
                .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// The Version picker: a pill for each file version, with its quality labels and size. A check marks the
    /// version that Play starts.
    fn version_pills(&self, cx: &Context<Self>) -> AnyElement {
        let pills = self.versions.iter().enumerate().map(|(i, version)| {
            let mark = if i == self.version { "✓ " } else { "" };
            let label = format!("{mark}{}", version_row(version)).into();
            self.pill(("detail-version", i), label, i == self.version, self.focus == Focus::Version(i), cx)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_version(i, cx)))
        });
        div().px(px(MARGIN)).flex().flex_wrap().gap(px(8.)).children(pills).into_any_element()
    }

    /// A card for each season: the poster, the title and the watched count. The chosen season has a light border.
    fn season_cards(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let card = |i: usize| {
            let season = &self.seasons[i];
            let focused = self.active && self.focus == Focus::Season(i);
            let border = match (focused, i == self.season) {
                (true, _) => t.accent,
                (false, true) => t.foreground,
                (false, false) => gpui::transparent_black(),
            };
            let card = div()
                .id(("detail-season", i))
                .clickable()
                .w(px(SEASON_WIDTH))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(4.))
                .on_click(cx.listener(move |this, _, _, cx| this.choose_season(i, cx)))
                .child(
                    div()
                        .w(px(SEASON_WIDTH))
                        .h(px(SEASON_WIDTH * 1.5))
                        .rounded(px(10.))
                        .overflow_hidden()
                        .bg(t.surface)
                        .border_2()
                        .border_color(border)
                        .child(artwork::image(
                            self.url(&season.poster_url),
                            self.season_thumbs.get(i).cloned().flatten(),
                            SEASON_WIDTH,
                        )),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if focused { t.bright } else { t.foreground })
                        .truncate()
                        .child(season.title.clone()),
                )
                .child(div().text_size(px(12.)).text_color(t.secondary).child(season_line(season)));
            self.with_menu(card, (Mark::Season(i), Focus::Season(i)), season_watched(season), cx).into_any_element()
        };
        let row = Focus::Season(0).place().0;
        let scroll = &self.row_scrolls[row];
        SEASONS.build("detail-seasons", scroll, self.seasons.len(), self.viewport, card).into_any_element()
    }

    /// The line `line` of the episode grid. On an episode page the first line has the More Episodes title.
    fn episode_line(&self, item: &wire::CatalogItemDetail, line: usize, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let start = line * self.columns;
        let end = (start + self.columns).min(self.episodes.len());
        let cards = (start..end).map(|i| {
            let episode = &self.episodes[i];
            let mark = (Mark::Episode(i), Focus::Episode(i));
            self.with_menu(self.episode_card(i, episode, cx), mark, episode_watched(episode), cx)
        });
        div()
            .px(px(MARGIN))
            .flex()
            .flex_col()
            .gap(px(12.))
            .children((line == 0 && item.type_ == "episode").then(|| {
                div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child("More Episodes")
            }))
            .child(div().flex().gap(px(GAP)).children(cards))
            .into_any_element()
    }

    /// An episode card: the still with the quality badge and the progress, the number and the title, the runtime
    /// and the air date, and two lines of the overview. The episode of an episode page has "Now viewing".
    fn episode_card(&self, i: usize, episode: &wire::Episode, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = cx.omarchy();
        let focused = self.active && self.focus == Focus::Episode(i);
        let thumb = self.episode_thumbs.get(i).cloned().flatten();
        let progress = episode.user_data.as_ref().and_then(|d| progress(d.position_seconds, d.duration_seconds));
        let badge = episode_badge(episode);
        let mut art = div()
            .relative()
            .w(px(EPISODE_WIDTH))
            .h(px(EPISODE_HEIGHT))
            .rounded(px(10.))
            .overflow_hidden()
            .bg(t.surface)
            .border_2()
            .border_color(if focused { t.accent } else { gpui::transparent_black() })
            .child(div().absolute().inset_0().child(artwork::image(self.url(&episode.still_url), thumb, EPISODE_WIDTH)))
            .children((!badge.is_empty()).then(|| badge_label(badge).left(px(8.))))
            .children((episode.content_id == self.id).then(|| badge_label("NOW VIEWING".into()).right(px(8.))));
        if let Some(progress) = progress {
            art = art.child(progress_bar(progress, t.accent));
        }
        div()
            .id(("detail-episode", i))
            .clickable()
            .w(px(EPISODE_WIDTH))
            .flex()
            .flex_col()
            .gap(px(4.))
            .on_click(cx.listener(move |this, _, _, cx| this.play_episode(i, cx)))
            .child(art)
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if focused { t.bright } else { t.foreground })
                    .truncate()
                    .child(format!("{}. {}", episode.episode_number, episode.title)),
            )
            .child(div().text_size(px(12.)).text_color(t.secondary).child(if episode_watched(episode) {
                format!("{} · ✓ Watched", episode_line(episode))
            } else {
                episode_line(episode)
            }))
            .children(
                episode
                    .overview
                    .clone()
                    .filter(|text| !text.is_empty())
                    .map(|text| div().text_size(px(12.)).text_color(t.secondary).line_clamp(2).child(text)),
            )
    }

    /// A cast or crew card: the photo, the name, and the character or the job. `thumb` is the index of its
    /// Thumbhash in `thumbs`.
    fn person_card(
        &self,
        focus: Focus,
        (name, line, photo): (&str, &str, &Option<String>),
        thumb: usize,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = cx.omarchy();
        let focused = self.active && self.focus == focus;
        let id = if matches!(focus, Focus::Crew(_)) { "detail-crew" } else { "detail-cast" };
        div()
            .id((id, focus.place().1))
            .clickable()
            .w(px(CAST_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .on_click(cx.listener(move |this, _, _, cx| this.open_person(focus, cx)))
            .child(
                div()
                    .w(px(CAST_WIDTH))
                    .h(px(150.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .bg(t.surface)
                    .border_2()
                    .border_color(if focused { t.accent } else { gpui::transparent_black() })
                    .child(artwork::image(self.url(photo), self.thumb(thumb), CAST_WIDTH)),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(if focused { t.bright } else { t.foreground })
                    .truncate()
                    .child(name.to_string()),
            )
            .child(div().text_size(px(12.)).text_color(t.secondary).truncate().child(line.to_string()))
    }

    /// The Cast, Crew, Trailers or More Like This section, by its row number in `Focus::place`. It has a title and
    /// a row of cards that scrolls sideways.
    fn section(&self, row: usize, item: &wire::CatalogItemDetail, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let videos = trailers(item);
        let (title, id, count): (&str, &str, usize) = match Focus::at(row, 0) {
            Focus::Cast(_) => ("Cast", "detail-cast", item.cast.len()),
            Focus::Crew(_) => ("Crew", "detail-crew", item.crew.len()),
            Focus::Trailer(_) => ("Trailers", "detail-trailers", videos.len()),
            _ => ("More Like This", "detail-similar", self.similar.len()),
        };
        let card = |i: usize| match Focus::at(row, i) {
            Focus::Cast(i) => {
                let credit = &item.cast[i];
                let card = (credit.name.as_str(), credit.character.as_str(), &credit.photo_url);
                self.person_card(Focus::Cast(i), card, 2 + i, cx).into_any_element()
            }
            Focus::Crew(i) => {
                let credit = &item.crew[i];
                let card = (credit.name.as_str(), credit.job.as_str(), &credit.photo_url);
                self.person_card(Focus::Crew(i), card, 2 + item.cast.len() + i, cx).into_any_element()
            }
            Focus::Trailer(i) => self.trailer_card(i, videos[i].0, cx).into_any_element(),
            _ => self.similar_card(i, &self.similar[i], cx).into_any_element(),
        };
        let cards = sideways(row).expect("a section scrolls sideways");
        div()
            .px(px(MARGIN))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child(title))
            .child(cards.build(id, &self.row_scrolls[row], count, self.viewport, card))
            .into_any_element()
    }

    /// A Trailers card: the name and the kind of the video.
    fn trailer_card(&self, i: usize, video: &wire::ItemVideoInfo, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = cx.omarchy();
        let focused = self.active && self.focus == Focus::Trailer(i);
        let official = if video.is_official { " · Official" } else { "" };
        div()
            .id(("detail-trailer", i))
            .clickable()
            .w(px(TRAILER_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(12.))
            .rounded(px(10.))
            .bg(t.surface)
            .border_2()
            .border_color(if focused { t.accent } else { gpui::transparent_black() })
            .on_click(cx.listener(move |this, _, _, cx| this.open_trailer(i, cx)))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(if focused { t.bright } else { t.foreground })
                    .line_clamp(2)
                    .child(format!("▶ {}", video.name.as_deref().unwrap_or(&video.kind))),
            )
            .child(div().text_size(px(12.)).text_color(t.secondary).child(format!("{}{official}", video.kind)))
    }

    /// A More Like This card, the same as a grid card.
    fn similar_card(&self, i: usize, item: &wire::CatalogItem, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let focused = self.active && self.focus == Focus::Similar(i);
        let size = (crate::grid::POSTER_WIDTH, crate::grid::POSTER_HEIGHT);
        let thumb = self.similar_thumbs.get(i).cloned().flatten();
        div()
            .id(("detail-similar", i))
            .clickable()
            .flex_none()
            .on_click(cx.listener(move |this, _, _, cx| this.open_similar(i, cx)))
            .child(card(item, self.url(&item.poster_url), thumb, size, focused, cx.omarchy()))
    }
}

impl Render for Detail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let viewport = window.viewport_size().width.to_f64() as f32;
        let width = viewport - crate::pages::sidebar_width(viewport, true) - 2. * MARGIN;
        self.columns = (((width + GAP) / (EPISODE_WIDTH + GAP)).floor() as usize).max(1);
        let status = match (&self.error, &self.item) {
            (Some(error), _) => Some(
                div()
                    .px(px(MARGIN))
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .text_color(t.danger)
                    .child(error.clone())
                    .child(
                        button("detail-retry", "Retry", ButtonVariant::Secondary, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
            ),
            (None, None) => Some(div().px(px(MARGIN)).text_color(t.secondary).child("Loading…")),
            _ => None,
        };
        let items = if self.item.is_some() { self.rows().items() } else { Vec::new() };
        if items != self.items {
            // Keep the scroll position when a row loads or the grid gets other columns.
            let top = self.page.logical_scroll_top();
            self.page.reset(items.len());
            self.page.scroll_to(top);
            self.items = items;
        }
        self.viewport = viewport;
        let window_size = (viewport, window.viewport_size().height.to_f64() as f32);
        let page = list(
            self.page.clone(),
            cx.processor(move |this, index: usize, _, cx| {
                let (Some(&(row, line)), Some(item)) = (this.items.get(index), this.item.as_ref()) else {
                    return div().into_any_element();
                };
                let child = match row {
                    0 => this.hero(item, window_size, cx),
                    1 => this.version_pills(cx),
                    2 => this.season_cards(cx),
                    3 => this.shuffle_season_button(cx),
                    EPISODE_ROW => this.episode_line(item, line, cx),
                    row => this.section(row, item, cx),
                };
                // The lines of the episode grid have the gap of the cards. The last item has the bottom margin.
                let gap = match this.items.get(index + 1) {
                    None => 80.,
                    Some(&(next, _)) if next == row => GAP,
                    Some(_) => 24.,
                };
                div().pb(px(gap)).child(child).into_any_element()
            }),
        )
        .flex_1();
        let notice = self.notice.clone().map(|notice| {
            div()
                .absolute()
                .bottom(px(24.))
                .left(px(MARGIN))
                .right(px(MARGIN))
                .p(px(12.))
                .rounded(px(10.))
                .bg(t.surface)
                .text_color(t.danger)
                .child(notice)
        });
        div().relative().size_full().flex().flex_col().children(status).child(page).children(notice)
    }
}

/// The place in `versions` of the file that `old[chosen]` plays. The first version when that file is gone.
pub fn kept_version(old: &[wire::FileVersion], chosen: usize, versions: &[wire::FileVersion]) -> usize {
    let file = old.get(chosen).map(|version| &version.file_id);
    versions.iter().position(|version| Some(&version.file_id) == file).unwrap_or(0)
}

/// The Item that Enter on a card opens, and the season to show: a season opens its series. An episode opens its own
/// page, with More Episodes.
pub fn target(item: &wire::CatalogItem) -> (String, Option<i64>) {
    match (&item.series_id, item.type_.as_str()) {
        (Some(series), "season") => (series.clone(), item.season_number),
        _ => (item.content_id.clone(), None),
    }
}

/// The series id and the season number of the episodes that the page lists: the chosen season `season` of a series,
/// or the season of an episode for its More Episodes row. A movie lists none.
pub fn episode_list(item: &wire::CatalogItemDetail, season: Option<i64>) -> Option<(String, i64)> {
    match item.type_.as_str() {
        "series" => season.map(|season| (item.content_id.clone(), season)),
        "episode" => item.series_id.clone().zip(item.season_number),
        _ => None,
    }
}

/// A fade with more than two stops, as a stack of two-stop strips end to end. The fade starts at the edge that
/// `angle` points away from. An angle of 0 starts at the bottom edge, 90 at the left, 180 at the top and 270 at the
/// right. Each stop is a position and the alpha of `color` at that position. The position is a fraction of the
/// parent, from the start edge.
fn fade(angle: f32, color: gpui::Hsla, stops: &[(f32, f32)]) -> Vec<gpui::Div> {
    stops
        .windows(2)
        .map(|pair| {
            let [(from, a), (to, b)] = [pair[0], pair[1]];
            let strip = div().absolute().bg(linear_gradient(
                angle,
                linear_color_stop(color.alpha(a), 0.),
                linear_color_stop(color.alpha(b), 1.),
            ));
            match angle as u32 {
                0 => strip.left_0().right_0().bottom(relative(from)).h(relative(to - from)),
                90 => strip.top_0().bottom_0().left(relative(from)).w(relative(to - from)),
                180 => strip.left_0().right_0().top(relative(from)).h(relative(to - from)),
                _ => strip.top_0().bottom_0().right(relative(from)).w(relative(to - from)),
            }
        })
        .collect()
}

/// The height of the hero on a window of `window` (width, height) logical pixels. The hero fills 72% of the window
/// height, or 60% when the window is narrower than 1024 px. The hero is never shorter than its `content`.
pub fn hero_height((width, height): (f32, f32), content: f32) -> f32 {
    let share = if width < 1024. { 0.6 } else { 0.72 };
    (height * share).max(content)
}

/// Whether the hero of `width` logical pixels shows the poster, and the width of the text column. The poster shows
/// only when the full logo fits next to it, so that a narrow window does not cut off the text.
pub fn hero_columns(width: f32) -> (bool, f32) {
    let column = width - 2. * MARGIN;
    let beside = column - POSTER_WIDTH - 32.;
    if beside >= LOGO_WIDTH { (true, beside) } else { (false, column) }
}

/// The box of the 16:9 backdrop in a hero of `hero` (width, height), relative to the hero. The box covers the hero.
/// It is centered from side to side. It keeps the upper part of the art: 20% of the vertical overflow is above the
/// hero, as with the web `object-position: center 20%`.
pub fn backdrop_box((width, height): (f32, f32)) -> gpui::Bounds<f32> {
    let box_width = width.max(height * 16. / 9.);
    let box_height = box_width * 9. / 16.;
    gpui::Bounds::new(
        gpui::point((width - box_width) / 2., (height - box_height) * 0.2),
        gpui::size(box_width, box_height),
    )
}

/// The artwork color: the average color of the backdrop Thumbhash `hash`, else `fallback`.
pub fn artwork_color(hash: Option<&str>, fallback: gpui::Hsla) -> gpui::Hsla {
    hash.and_then(artwork::thumbhash_average).map_or(fallback, Into::into)
}

/// The line below the title of an episode card: the runtime and the air date, for example "57m · Feb 18, 2022".
pub fn episode_line(episode: &wire::Episode) -> String {
    let aired = episode.air_date.as_deref().and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
    [Some(format!("{}m", episode.runtime)), aired.map(|date| date.format("%b %-d, %Y").to_string())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The quality badge of an episode, for example "4K DV", from the overlay that the server sends, else from the first
/// file. "" when neither has the resolution or the dynamic range.
pub fn episode_badge(episode: &wire::Episode) -> String {
    match (&episode.overlay_summary, episode.files.first()) {
        (Some(overlay), _) => overlay_badge(overlay),
        (None, Some(file)) => badge(file.resolution.as_deref(), file.hdr.then_some("HDR")),
        (None, None) => String::new(),
    }
}

/// The quality badge of a card from the overlay that the server sends, for example "4K DV". "" when the overlay has
/// neither the resolution nor the dynamic range.
pub fn overlay_badge(overlay: &wire::CatalogItemOverlay) -> String {
    let range = overlay.hdr.as_deref().map(|hdr| if hdr == "Dolby Vision" { "DV" } else { hdr });
    badge(overlay.resolution.as_deref(), range)
}

fn badge(size: Option<&str>, range: Option<&str>) -> String {
    [size.map(resolution), range.map(String::from)]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The line of a season card: how many episodes the Profile watched, for example "1 of 9 episodes", or only the
/// episode count when the server sends no watch data.
pub fn season_line(season: &wire::Season) -> String {
    let count = season.episode_count;
    let noun = if count == 1 { "episode" } else { "episodes" };
    match &season.user_data {
        Some(data) => format!("{} of {count} {noun}", data.watched_count),
        None => format!("{count} {noun}"),
    }
}

/// A button of the hero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Play, or Resume at the saved position.
    Play {
        resume: bool,
    },
    StartOver,
    /// Start a shuffle of the episodes of the series.
    Shuffle,
    Watchlist,
    Favorite,
    /// Mark watched, or Mark unwatched.
    Watched,
    /// Give the Item 1 to 5 stars, or clear the User rating when it has this number of stars.
    Rate(u8),
}

/// The buttons of the hero. Start over shows only when Resume does, and Shuffle only on a series of 2 episodes or
/// more. The 5 stars come last.
pub fn actions(item: &wire::CatalogItemDetail, seasons: &[wire::Season]) -> Vec<Action> {
    let resume = item.position_seconds.is_some_and(|position| position > 0.);
    let mut actions = vec![Action::Play { resume }];
    if resume {
        actions.push(Action::StartOver);
    }
    // The series detail sends no episode count, so the count comes from the seasons, as in the web client.
    if item.type_ == "series" && seasons.iter().map(|s| s.episode_count).sum::<i64>() >= 2 {
        actions.push(Action::Shuffle);
    }
    actions.extend([Action::Watchlist, Action::Favorite, Action::Watched]);
    actions.extend((1..=5).map(Action::Rate));
    actions
}

/// The User rating that a press of a star sends. The star of the current User rating sends no rating, which clears
/// it. Another star sends its own number.
pub fn star_press(user_rating: Option<i64>, stars: u8) -> Option<u8> {
    (user_rating != Some(stars.into())).then_some(stars)
}

/// The text of a hero button. A star is full up to the rating of the Item.
pub fn action_label(item: &wire::CatalogItemDetail, action: Action) -> String {
    let state = item.user_state.as_ref();
    let on = |flag: fn(&wire::CatalogItemUserState) -> bool| state.is_some_and(flag);
    match action {
        Action::Play { resume: true } => "▶ Resume",
        Action::Play { resume: false } => "▶ Play",
        Action::StartOver => "Start over",
        Action::Shuffle => "Shuffle",
        Action::Watchlist if on(|s| s.in_watchlist) => "✓ Watchlist",
        Action::Watchlist => "+ Watchlist",
        Action::Favorite if on(|s| s.is_favorite) => "♥ Favorite",
        Action::Favorite => "♡ Favorite",
        Action::Watched if on(|s| s.played) => "✓ Watched",
        Action::Watched => "Mark watched",
        Action::Rate(stars) if item.user_rating.is_some_and(|rating| i64::from(stars) <= rating) => "★",
        Action::Rate(_) => "☆",
    }
    .to_string()
}

/// What a Mark watched action marks: the Item of the page, a season card or an episode card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Item,
    Season(usize),
    Episode(usize),
}

/// The Profile watched each episode of the season.
pub fn season_watched(season: &wire::Season) -> bool {
    season.user_data.as_ref().is_some_and(|data| season.episode_count > 0 && data.watched_count >= season.episode_count)
}

pub fn episode_watched(episode: &wire::Episode) -> bool {
    episode.user_data.as_ref().is_some_and(|data| data.played)
}

/// Shows on the page that the server marked `mark` watched (`on`) or unwatched. `seasons[season]` is the season that
/// `episodes` lists, on a series. A mark and an unmark both drop the resume point on the server.
pub fn apply_mark(
    item: &mut wire::CatalogItemDetail,
    seasons: &mut [wire::Season],
    season: usize,
    episodes: &mut [wire::Episode],
    mark: Mark,
    on: bool,
) {
    let count = |season: &mut wire::Season, count: i64| watch_data(&mut season.user_data).watched_count = count;
    let mark_episode = |episode: &mut wire::Episode| {
        let data = watch_data(&mut episode.user_data);
        data.played = on;
        data.position_seconds = None;
    };
    let mark_item = |item: &mut wire::CatalogItemDetail| {
        user_state(item).played = on;
        item.position_seconds = None;
    };
    match mark {
        Mark::Item => {
            mark_item(item);
            if item.type_ == "series" {
                seasons.iter_mut().for_each(|s| count(s, if on { s.episode_count } else { 0 }));
                episodes.iter_mut().for_each(mark_episode);
            } else if let Some(episode) = episodes.iter_mut().find(|e| e.content_id == item.content_id) {
                mark_episode(episode);
            }
        }
        Mark::Season(i) => {
            if let Some(s) = seasons.get_mut(i) {
                count(s, if on { s.episode_count } else { 0 });
            }
            if i == season {
                episodes.iter_mut().for_each(mark_episode);
            }
        }
        Mark::Episode(i) => {
            let Some(episode) = episodes.get_mut(i) else {
                return;
            };
            let changed = episode_watched(episode) != on;
            mark_episode(episode);
            if episode.content_id == item.content_id {
                mark_item(item);
            }
            if let Some(s) = seasons.get_mut(season).filter(|_| changed) {
                let watched = s.user_data.as_ref().map_or(0, |d| d.watched_count) + if on { 1 } else { -1 };
                count(s, watched.clamp(0, s.episode_count));
            }
        }
    }
}

/// The watch state of the Item, with nothing set when the server sent none.
fn user_state(item: &mut wire::CatalogItemDetail) -> &mut wire::CatalogItemUserState {
    item.user_state.get_or_insert(wire::CatalogItemUserState { in_watchlist: false, is_favorite: false, played: false })
}

/// The watch data of a season or an episode, with nothing watched when the server sent none.
fn watch_data(data: &mut Option<wire::WatchRollup>) -> &mut wire::WatchRollup {
    data.get_or_insert(wire::WatchRollup {
        duration_seconds: None,
        in_progress_count: 0,
        is_in_progress: None,
        last_codec_video: None,
        last_edition_key: None,
        last_file_id: None,
        last_hdr: None,
        last_resolution: None,
        played: false,
        position_seconds: None,
        unplayed_count: 0,
        watched_count: 0,
    })
}

/// The focused control, by index: a hero button, a version pill, a season card, an episode card, a cast card, a
/// crew card, a trailer card or a More Like This card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Action(usize),
    Version(usize),
    Season(usize),
    /// The Shuffle season button. The row has one control.
    ShuffleSeason(usize),
    Episode(usize),
    Cast(usize),
    Crew(usize),
    Trailer(usize),
    Similar(usize),
}

impl Focus {
    /// The control at `index` in row `row` of the page, from the top.
    fn at(row: usize, index: usize) -> Self {
        [
            Focus::Action,
            Focus::Version,
            Focus::Season,
            Focus::ShuffleSeason,
            Focus::Episode,
            Focus::Cast,
            Focus::Crew,
            Focus::Trailer,
            Focus::Similar,
        ][row](index)
    }

    /// The row of the control, from the top, and its index in the row.
    fn place(self) -> (usize, usize) {
        match self {
            Focus::Action(i) => (0, i),
            Focus::Version(i) => (1, i),
            Focus::Season(i) => (2, i),
            Focus::ShuffleSeason(i) => (3, i),
            Focus::Episode(i) => (4, i),
            Focus::Cast(i) => (5, i),
            Focus::Crew(i) => (6, i),
            Focus::Trailer(i) => (7, i),
            Focus::Similar(i) => (8, i),
        }
    }
}

/// The row of the episode grid. The other rows go left and right only.
const EPISODE_ROW: usize = 4;

const SEASONS: Row = Row { width: SEASON_WIDTH, gap: GAP, pad: MARGIN };

/// The size of the cards of a row that scrolls sideways, by the row number of `Focus::place`.
fn sideways(row: usize) -> Option<Row> {
    let width = match Focus::at(row, 0) {
        Focus::Season(_) => return Some(SEASONS),
        Focus::Cast(_) | Focus::Crew(_) => CAST_WIDTH,
        Focus::Trailer(_) => TRAILER_WIDTH,
        Focus::Similar(_) => crate::grid::POSTER_WIDTH,
        Focus::Action(_) | Focus::Version(_) | Focus::ShuffleSeason(_) | Focus::Episode(_) => return None,
    };
    Some(Row { width, gap: GAP, pad: 0. })
}

/// Where a key takes the focus.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    To(Focus),
    /// h or Left at the left edge: the sidebar takes the focus.
    OutLeft,
    /// The page does not use the key.
    Stay,
}

/// The number of controls in each row of the page. The page shows a row only when it has a control.
#[derive(Debug, Clone, Copy)]
pub struct Rows {
    pub actions: usize,
    /// The pills of the Version picker. The page shows the picker only for more than one version.
    pub versions: usize,
    /// The index of the chosen version.
    pub version: usize,
    pub seasons: usize,
    /// The index of the chosen season.
    pub season: usize,
    /// The Shuffle season button: 1 when the chosen season has 2 episodes or more, else 0.
    pub shuffle: usize,
    pub episodes: usize,
    pub cast: usize,
    pub crew: usize,
    pub trailers: usize,
    /// The More Like This cards.
    pub similar: usize,
    /// The columns of the episode grid.
    pub columns: usize,
}

impl Rows {
    fn len(&self, row: usize) -> usize {
        let Rows { actions, versions, seasons, shuffle, episodes, cast, crew, trailers, similar, .. } = *self;
        [actions, versions, seasons, shuffle, episodes, cast, crew, trailers, similar][row]
    }

    /// The control that the focus goes to in `row`: the chosen version or season, the first card of the last
    /// episode row when the focus comes from below, else the first control.
    fn enter(&self, row: usize, from_below: bool) -> Focus {
        match row {
            1 => Focus::Version(self.version),
            2 => Focus::Season(self.season),
            EPISODE_ROW if from_below => Focus::Episode((self.episodes - 1) / self.columns * self.columns),
            row => Focus::at(row, 0),
        }
    }

    /// The items of the page list, as the row and the line of the episode grid: the hero, one item for each other
    /// row that is not empty, and one for each line of the episode grid.
    pub fn items(&self) -> Vec<(usize, usize)> {
        (0..9)
            .flat_map(|row| {
                let lines = match row {
                    0 => 1,
                    EPISODE_ROW => self.episodes.div_ceil(self.columns),
                    row => usize::from(self.len(row) > 0),
                };
                (0..lines).map(move |line| (row, line))
            })
            .collect()
    }

    /// The index of the page item that holds the control.
    pub fn child(&self, focus: Focus) -> usize {
        let (row, i) = focus.place();
        let line = if row == EPISODE_ROW { i / self.columns } else { 0 };
        self.items().iter().position(|&item| item == (row, line)).unwrap_or(0)
    }

    pub fn step(&self, focus: Focus, key: Key) -> Step {
        let (row, i) = focus.place();
        let leave = |down: bool| {
            let next =
                if down { (row + 1..9).find(|&r| self.len(r) > 0) } else { (0..row).rev().find(|&r| self.len(r) > 0) };
            next.map_or(Step::Stay, |next| Step::To(self.enter(next, !down)))
        };
        if row == EPISODE_ROW {
            return match crate::grid::move_focus(i, key, self.episodes, self.columns) {
                Move::To(i) => Step::To(Focus::Episode(i)),
                Move::OutTop => leave(false),
                Move::OutLeft => Step::OutLeft,
                Move::Stay if key == Key::Down => leave(true),
                Move::Stay => Step::Stay,
            };
        }
        match key {
            Key::Down => leave(true),
            Key::Up => leave(false),
            Key::Left if i == 0 => Step::OutLeft,
            Key::Left => Step::To(Focus::at(row, i - 1)),
            Key::Right if i + 1 < self.len(row) => Step::To(Focus::at(row, i + 1)),
            _ => Step::Stay,
        }
    }
}

/// The line below the title: the year, the runtime or the season count, the content rating and the genres.
pub fn meta_line(item: &wire::CatalogItemDetail) -> String {
    let length = match (item.type_.as_str(), item.season_count, item.runtime) {
        ("series", Some(1), _) => Some("1 Season".to_string()),
        ("series", Some(seasons), _) => Some(format!("{seasons} Seasons")),
        ("series", None, _) | (_, _, None) => None,
        (_, _, Some(minutes)) if minutes >= 60 => Some(format!("{}h {}m", minutes / 60, minutes % 60)),
        (_, _, Some(minutes)) => Some(format!("{minutes}m")),
    };
    let genres = (!item.genres.is_empty()).then(|| item.genres.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
    [item.year.map(|year| year.to_string()), length, item.content_rating.clone(), genres]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The line above the title: the type, and the network of a series or the first studio, for example
/// "SERIES · APPLE TV".
pub fn kind_line(item: &wire::CatalogItemDetail) -> String {
    let company = if item.type_ == "series" { item.networks.first() } else { None }.or(item.studios.first());
    [Some(&item.type_), company].into_iter().flatten().map(|part| part.to_uppercase()).collect::<Vec<_>>().join(" · ")
}

/// The ratings from each source that the server sends, in its order, for example "IMDb 8.3 · TMDB 7.9".
pub fn ratings_line(item: &wire::CatalogItemDetail) -> String {
    item.ratings.iter().map(|r| format!("{} {}", r.name, r.display)).collect::<Vec<_>>().join(" · ")
}

/// The crew jobs that the Written by part names.
const WRITER_JOBS: [&str; 4] = ["Writer", "Screenplay", "Story", "Teleplay"];

/// "Directed by … · Written by …", with each name once, or "" when the crew has no director and no writer.
pub fn credits_line(item: &wire::CatalogItemDetail) -> String {
    let names = |jobs: &[&str]| {
        let mut names: Vec<&str> = Vec::new();
        for credit in item.crew.iter().filter(|c| jobs.contains(&c.job.as_str())) {
            if !names.contains(&credit.name.as_str()) {
                names.push(&credit.name);
            }
        }
        names.join(", ")
    };
    [("Directed by", names(&["Director"])), ("Written by", names(&WRITER_JOBS))]
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(label, names)| format!("{label} {names}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The trailers and other videos that open in the browser, with their page. A video on another site, or with a key
/// that is not a plain id, is left out.
pub fn trailers(item: &wire::CatalogItemDetail) -> Vec<(&wire::ItemVideoInfo, String)> {
    item.videos
        .iter()
        .filter(|v| {
            !v.site_key.is_empty() && v.site_key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .filter_map(|video| {
            let url = match video.site.to_lowercase().as_str() {
                "youtube" => format!("https://www.youtube.com/watch?v={}", video.site_key),
                "vimeo" => format!("https://vimeo.com/{}", video.site_key),
                _ => return None,
            };
            Some((video, url))
        })
        .collect()
}

fn resolution(resolution: &str) -> String {
    match resolution {
        "2160p" | "4k" | "4K" => "4K".to_string(),
        other => other.to_string(),
    }
}

/// The quality badges of the version that plays, when the page has no version list: the resolution, HDR and
/// the video codec.
pub fn quality_badges(item: &wire::CatalogItemDetail) -> Vec<String> {
    let resolution = item.effective_version_resolution.as_deref().map(resolution);
    let hdr = (item.effective_version_hdr == Some(true)).then(|| "HDR".to_string());
    let codec = item.effective_version_codec_video.as_deref().map(str::to_uppercase);
    [resolution, hdr, codec].into_iter().flatten().collect()
}

/// The dynamic range of a version: "DV", "DV HDR10", "DV HDR10+", "DV HLG", "HDR10+", "HDR10", "HLG", "HDR"
/// for a file with only the `hdr` flag, or "" for SDR. The words and the rules are the same as in the web app.
pub fn video_range(version: &wire::FileVersion) -> String {
    let tracks = &version.video_tracks;
    let dolby_vision = tracks.iter().any(|t| {
        t.dolby_vision.as_deref().is_some_and(|dv| !dv.trim().is_empty())
            || t.dv_profile.is_some_and(|profile| profile > 0)
            || t.video_range_type.as_deref().is_some_and(|r| r.trim().starts_with("DOVI"))
    });
    let hdr_type = tracks.iter().find_map(|t| {
        let range = t.video_range_type.as_deref().unwrap_or_default().trim();
        let transfer = t.color_transfer.as_deref().unwrap_or_default().to_lowercase();
        if t.hdr10_plus == Some(true) || range.contains("HDR10Plus") {
            Some("HDR10+")
        } else if range == "HDR10" || range.ends_with("WithHDR10") || transfer.contains("smpte2084") {
            Some("HDR10")
        } else if range == "HLG" || range.ends_with("WithHLG") || transfer.contains("arib-std-b67") {
            Some("HLG")
        } else {
            None
        }
    });
    match (dolby_vision, hdr_type) {
        (true, Some(hdr)) => format!("DV {hdr}"),
        (true, None) => "DV".into(),
        (false, Some(hdr)) => hdr.into(),
        (false, None) if version.hdr => "HDR".into(),
        (false, None) => String::new(),
    }
}

/// The quality labels of a version: the resolution, the dynamic range and the video codec.
pub fn version_labels(version: &wire::FileVersion) -> Vec<String> {
    [resolution(&version.resolution), video_range(version), version.codec_video.to_uppercase()]
        .into_iter()
        .filter(|label| !label.is_empty())
        .collect()
}

/// The text of a row of the Version picker: the quality labels, the audio codec and the file size.
pub fn version_row(version: &wire::FileVersion) -> String {
    let mut parts = version_labels(version);
    parts.push(version.codec_audio.to_uppercase());
    parts.push(file_size(version.file_size));
    parts.retain(|part| !part.is_empty());
    parts.join(" · ")
}

/// A file size with 1024-based units and the GB, MB and KB labels of the web app, for example "10.6 GB".
pub fn file_size(bytes: i64) -> String {
    let bytes = bytes as f64;
    match [("GB", 3), ("MB", 2), ("KB", 1)].into_iter().find(|(_, power)| bytes >= 1024_f64.powi(*power)) {
        Some((unit, power)) => format!("{:.1} {unit}", bytes / 1024_f64.powi(power)),
        None if bytes > 0. => format!("{bytes} B"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn detail(fields: serde_json::Value) -> wire::CatalogItemDetail {
        let mut item: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/get_catalog_item_ok.json")).unwrap();
        item.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(item).unwrap()
    }

    fn card(fields: serde_json::Value) -> wire::CatalogItem {
        let mut item = json!({"content_id": "x", "type": "movie", "title": "x", "genres": [], "keywords": [], "status": "available"});
        item.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(item).unwrap()
    }

    #[test]
    fn enter_on_a_season_opens_its_series_with_that_season_and_an_episode_opens_its_page() {
        let movie = card(json!({"content_id": "movie:heat-1995"}));
        assert_eq!(target(&movie), ("movie:heat-1995".into(), None));
        let episode = card(json!({"content_id": "e", "type": "episode", "series_id": "series:s", "season_number": 2}));
        assert_eq!(target(&episode), ("e".into(), None));
        let season = card(json!({"content_id": "s2", "type": "season", "series_id": "series:s", "season_number": 2}));
        assert_eq!(target(&season), ("series:s".into(), Some(2)));
        let series = card(json!({"content_id": "series:s", "type": "series"}));
        assert_eq!(target(&series), ("series:s".into(), None));
    }

    #[test]
    fn a_started_item_offers_resume_and_start_over() {
        let fresh = detail(json!({}));
        assert!(actions(&fresh, &[]).starts_with(&[Action::Play { resume: false }, Action::Watchlist]));
        let started = detail(json!({"position_seconds": 600.0}));
        assert!(actions(&started, &[]).starts_with(&[
            Action::Play { resume: true },
            Action::StartOver,
            Action::Watchlist
        ]));
    }

    #[test]
    fn a_series_of_two_episodes_or_more_offers_shuffle_after_play_and_start_over() {
        let series = detail(json!({"type": "series", "position_seconds": 600.0}));
        let two = [season(json!({"episode_count": 1})), season(json!({"episode_count": 1}))];
        assert!(actions(&series, &two).starts_with(&[
            Action::Play { resume: true },
            Action::StartOver,
            Action::Shuffle
        ]));
        assert_eq!(action_label(&series, Action::Shuffle), "Shuffle");
        assert!(!actions(&series, &two[..1]).contains(&Action::Shuffle));
        let movie = detail(json!({"type": "movie"}));
        assert!(!actions(&movie, &two).contains(&Action::Shuffle));
    }

    #[test]
    fn the_metadata_line_has_the_year_the_runtime_the_rating_and_the_genres() {
        let movie = detail(json!({"runtime": 170, "content_rating": "R", "genres": ["Crime", "Drama"]}));
        assert_eq!(meta_line(&movie), "1995 · 2h 50m · R · Crime, Drama");
        let series = detail(json!({"type": "series", "year": 2022, "season_count": 2, "genres": ["Drama"]}));
        assert_eq!(meta_line(&series), "2022 · 2 Seasons · Drama");
        let short = detail(json!({"runtime": 45, "season_count": 1, "type": "series", "genres": []}));
        assert_eq!(meta_line(&short), "1995 · 1 Season");
    }

    #[test]
    fn hjkl_moves_between_the_buttons_the_seasons_and_the_episode_grid() {
        // 3 buttons, 2 seasons with season 1 chosen, 5 episodes in 2 columns.
        let rows = Rows {
            actions: 3,
            versions: 0,
            version: 0,
            seasons: 2,
            season: 1,
            shuffle: 0,
            episodes: 5,
            cast: 0,
            crew: 0,
            trailers: 0,
            similar: 0,
            columns: 2,
        };
        let to = |focus| Step::To(focus);
        assert_eq!(rows.step(Focus::Action(0), Key::Right), to(Focus::Action(1)));
        assert_eq!(rows.step(Focus::Action(2), Key::Right), Step::Stay);
        assert_eq!(rows.step(Focus::Action(0), Key::Left), Step::OutLeft, "h at the left goes to the sidebar");
        assert_eq!(rows.step(Focus::Action(2), Key::Down), to(Focus::Season(1)), "down goes to the chosen season");
        assert_eq!(rows.step(Focus::Season(1), Key::Up), to(Focus::Action(0)));
        assert_eq!(rows.step(Focus::Season(1), Key::Down), to(Focus::Episode(0)));
        assert_eq!(rows.step(Focus::Episode(1), Key::Down), to(Focus::Episode(3)));
        assert_eq!(rows.step(Focus::Episode(3), Key::Up), to(Focus::Episode(1)));
        assert_eq!(rows.step(Focus::Episode(1), Key::Up), to(Focus::Season(1)));
        assert_eq!(rows.step(Focus::Episode(2), Key::Left), Step::OutLeft);
        assert_eq!(rows.step(Focus::Action(0), Key::Up), Step::Stay);
        let movie = Rows {
            actions: 2,
            versions: 0,
            version: 0,
            seasons: 0,
            season: 0,
            shuffle: 0,
            episodes: 0,
            cast: 0,
            crew: 0,
            trailers: 0,
            similar: 0,
            columns: 4,
        };
        assert_eq!(movie.step(Focus::Action(1), Key::Down), Step::Stay);
        let loading = Rows { episodes: 0, ..rows };
        assert_eq!(loading.step(Focus::Season(0), Key::Down), Step::Stay);
        let shuffle = Rows { shuffle: 1, ..rows };
        assert_eq!(shuffle.step(Focus::Season(1), Key::Down), to(Focus::ShuffleSeason(0)));
        assert_eq!(shuffle.step(Focus::ShuffleSeason(0), Key::Down), to(Focus::Episode(0)));
        assert_eq!(shuffle.step(Focus::Episode(1), Key::Up), to(Focus::ShuffleSeason(0)));
        assert_eq!(shuffle.step(Focus::ShuffleSeason(0), Key::Up), to(Focus::Season(1)));
    }

    #[test]
    fn hjkl_moves_between_the_buttons_and_the_cast_of_a_movie() {
        let movie = Rows {
            actions: 2,
            versions: 0,
            version: 0,
            seasons: 0,
            season: 0,
            shuffle: 0,
            episodes: 0,
            cast: 3,
            crew: 0,
            trailers: 0,
            similar: 0,
            columns: 4,
        };
        let to = |focus| Step::To(focus);
        assert_eq!(movie.step(Focus::Action(1), Key::Down), to(Focus::Cast(0)));
        assert_eq!(movie.step(Focus::Cast(0), Key::Right), to(Focus::Cast(1)));
        assert_eq!(movie.step(Focus::Cast(2), Key::Right), Step::Stay);
        assert_eq!(movie.step(Focus::Cast(0), Key::Left), Step::OutLeft);
        assert_eq!(movie.step(Focus::Cast(1), Key::Up), to(Focus::Action(0)));
        assert_eq!(movie.step(Focus::Cast(1), Key::Down), Step::Stay);
    }

    fn version(fields: serde_json::Value) -> wire::FileVersion {
        let body: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/list_catalog_item_versions_ok.json")).unwrap();
        let mut version = body["items"][0].clone();
        version.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(version).unwrap()
    }

    fn range(hdr: bool, track: serde_json::Value) -> String {
        let mut track_fields =
            json!({"interlaced": false, "dv_bl_compat_id_present": false, "dv_config_present": false});
        track_fields.as_object_mut().unwrap().extend(track.as_object().unwrap().clone());
        video_range(&version(json!({"hdr": hdr, "video_tracks": [track_fields]})))
    }

    #[test]
    fn a_reload_keeps_the_chosen_version_by_its_file() {
        let versions = |ids: &[&str]| ids.iter().map(|id| version(json!({"file_id": id}))).collect::<Vec<_>>();
        let old = versions(&["1", "2"]);
        assert_eq!(kept_version(&old, 1, &versions(&["3", "1", "2"])), 2, "a new version comes first");
        assert_eq!(kept_version(&old, 1, &versions(&["2", "1"])), 0, "the order changed");
        assert_eq!(kept_version(&old, 1, &versions(&["1", "3"])), 0, "the chosen file is gone");
        assert_eq!(kept_version(&[], 0, &versions(&["1", "2"])), 0, "the first load");
    }

    #[test]
    fn the_video_range_names_dolby_vision_and_the_hdr_type() {
        assert_eq!(video_range(&version(json!({"hdr": false}))), "");
        assert_eq!(video_range(&version(json!({"hdr": true}))), "HDR", "a file probed before the range fields");
        assert_eq!(range(true, json!({"dolby_vision": "Profile 5"})), "DV");
        assert_eq!(range(true, json!({"dv_profile": 8})), "DV");
        assert_eq!(range(true, json!({"video_range_type": "DOVIWithSDR"})), "DV");
        assert_eq!(range(true, json!({"dolby_vision": "Profile 8", "video_range_type": "DOVIWithHDR10"})), "DV HDR10");
        assert_eq!(range(true, json!({"dolby_vision": "Profile 7", "color_transfer": "smpte2084"})), "DV HDR10");
        assert_eq!(range(true, json!({"video_range_type": "DOVIWithHLG"})), "DV HLG");
        assert_eq!(range(true, json!({"video_range_type": "DOVIWithELHDR10Plus"})), "DV HDR10+");
        assert_eq!(range(true, json!({"hdr10_plus": true})), "HDR10+");
        assert_eq!(range(true, json!({"video_range_type": "HDR10"})), "HDR10");
        assert_eq!(range(true, json!({"color_transfer": "arib-std-b67"})), "HLG");
    }

    #[test]
    fn a_version_row_has_the_labels_and_the_file_size() {
        let uhd = version(json!({
            "resolution": "2160p", "codec_video": "hevc", "codec_audio": "eac3", "hdr": true, "file_size": 11_381_663_334_i64,
            "video_tracks": [{"video_range_type": "DOVIWithHDR10", "interlaced": false,
                              "dv_bl_compat_id_present": false, "dv_config_present": false}],
        }));
        assert_eq!(version_labels(&uhd), ["4K", "DV HDR10", "HEVC"]);
        assert_eq!(version_row(&uhd), "4K · DV HDR10 · HEVC · EAC3 · 10.6 GB");
        let small = version(json!({"resolution": "720p", "codec_video": "h264", "file_size": 734_003_200}));
        assert_eq!(version_row(&small), "720p · H264 · 700.0 MB");
    }

    #[test]
    fn hjkl_moves_through_the_version_picker_of_a_movie() {
        let movie = Rows {
            actions: 2,
            versions: 3,
            version: 1,
            seasons: 0,
            season: 0,
            shuffle: 0,
            episodes: 0,
            cast: 2,
            crew: 0,
            trailers: 0,
            similar: 0,
            columns: 4,
        };
        let to = |focus| Step::To(focus);
        assert_eq!(movie.step(Focus::Action(0), Key::Down), to(Focus::Version(1)), "down goes to the chosen version");
        assert_eq!(movie.step(Focus::Version(1), Key::Right), to(Focus::Version(2)));
        assert_eq!(movie.step(Focus::Version(0), Key::Left), Step::OutLeft);
        assert_eq!(movie.step(Focus::Version(2), Key::Up), to(Focus::Action(0)));
        assert_eq!(movie.step(Focus::Version(2), Key::Down), to(Focus::Cast(0)));
        assert_eq!(movie.step(Focus::Cast(1), Key::Up), to(Focus::Version(1)));
    }

    #[test]
    fn the_quality_badges_come_from_the_version_that_plays() {
        assert!(quality_badges(&detail(json!({}))).is_empty());
        let uhd = detail(json!({
            "effective_version_resolution": "2160p", "effective_version_hdr": true, "effective_version_codec_video": "hevc",
        }));
        assert_eq!(quality_badges(&uhd), ["4K", "HDR", "HEVC"]);
        let hd = detail(json!({"effective_version_resolution": "1080p", "effective_version_hdr": false}));
        assert_eq!(quality_badges(&hd), ["1080p"]);
    }

    #[test]
    fn the_kind_line_names_the_network_of_a_series_and_the_studio_of_a_movie() {
        assert_eq!(kind_line(&detail(json!({}))), "MOVIE");
        let movie = detail(json!({"studios": ["Crossbow Productions", "Gruskoff"], "networks": []}));
        assert_eq!(kind_line(&movie), "MOVIE · CROSSBOW PRODUCTIONS");
        let series = detail(json!({"type": "series", "studios": ["AMC Studios"], "networks": ["Apple TV"]}));
        assert_eq!(kind_line(&series), "SERIES · APPLE TV");
    }

    #[test]
    fn the_ratings_line_has_each_source_in_the_server_order() {
        assert_eq!(ratings_line(&detail(json!({"ratings": []}))), "");
        let rated = detail(json!({"ratings": [
            {"source": "imdb", "name": "IMDb", "score": 83, "display": "8.3"},
            {"source": "rt_critic", "name": "Rotten Tomatoes", "score": 87, "display": "87%"},
        ]}));
        assert_eq!(ratings_line(&rated), "IMDb 8.3 · Rotten Tomatoes 87%");
    }

    #[test]
    fn the_credits_line_names_the_directors_and_the_writers_once() {
        assert_eq!(credits_line(&detail(json!({"crew": []}))), "");
        let crew = |job: &str, name: &str| json!({"job": job, "name": name});
        let movie = detail(json!({"crew": [
            crew("Director", "Mel Brooks"), crew("Screenplay", "Mel Brooks"), crew("Screenplay", "Gene Wilder"),
            crew("Writer", "Gene Wilder"), crew("Producer", "Michael Gruskoff"),
        ]}));
        assert_eq!(credits_line(&movie), "Directed by Mel Brooks · Written by Mel Brooks, Gene Wilder");
        let produced = detail(json!({"crew": [crew("Producer", "Michael Gruskoff")]}));
        assert_eq!(credits_line(&produced), "");
    }

    #[test]
    fn a_trailer_on_youtube_or_vimeo_opens_its_page() {
        let video =
            |site: &str, key: &str| json!({"is_official": true, "kind": "Trailer", "site": site, "site_key": key});
        let item = detail(json!({"videos": [
            video("YouTube", "dQw4w9WgXcQ"), video("Vimeo", "76979871"), video("Dailymotion", "x7"),
            video("YouTube", "a&b=c"), video("YouTube", ""),
        ]}));
        let urls: Vec<_> = trailers(&item).into_iter().map(|(_, url)| url).collect();
        assert_eq!(urls, ["https://www.youtube.com/watch?v=dQw4w9WgXcQ", "https://vimeo.com/76979871"]);
    }

    #[test]
    fn hjkl_moves_down_through_the_episodes_cast_crew_and_trailers_of_a_series() {
        // 2 seasons, 5 episodes in 2 columns, then 2 cast, 3 crew and 1 trailer.
        let series = Rows {
            actions: 2,
            versions: 0,
            version: 0,
            seasons: 2,
            season: 0,
            shuffle: 0,
            episodes: 5,
            cast: 2,
            crew: 3,
            trailers: 1,
            similar: 0,
            columns: 2,
        };
        let to = |focus| Step::To(focus);
        assert_eq!(series.step(Focus::Episode(3), Key::Down), to(Focus::Episode(4)));
        assert_eq!(
            series.step(Focus::Episode(4), Key::Down),
            to(Focus::Cast(0)),
            "down from the last row leaves the grid"
        );
        assert_eq!(series.step(Focus::Cast(1), Key::Up), to(Focus::Episode(4)), "up goes to the last row of the grid");
        assert_eq!(series.step(Focus::Cast(1), Key::Down), to(Focus::Crew(0)));
        assert_eq!(series.step(Focus::Crew(1), Key::Right), to(Focus::Crew(2)));
        assert_eq!(series.step(Focus::Crew(0), Key::Left), Step::OutLeft);
        assert_eq!(series.step(Focus::Crew(2), Key::Down), to(Focus::Trailer(0)));
        assert_eq!(series.step(Focus::Trailer(0), Key::Down), Step::Stay);
        assert_eq!(series.step(Focus::Trailer(0), Key::Up), to(Focus::Crew(0)));
        let no_crew = Rows { crew: 0, ..series };
        assert_eq!(no_crew.step(Focus::Trailer(0), Key::Up), to(Focus::Cast(0)), "an empty row is skipped");
        // The page items are the hero, one for each row that is not empty, and one for each line of episodes.
        assert_eq!(series.child(Focus::Action(1)), 0);
        assert_eq!(series.child(Focus::Season(1)), 1);
        assert_eq!(series.child(Focus::Episode(3)), 3, "the second line of the grid");
        assert_eq!(series.child(Focus::Episode(4)), 4);
        assert_eq!(
            series.child(Focus::Trailer(0)),
            7,
            "hero, seasons, three lines of episodes, cast, crew; a series has no versions"
        );
        assert_eq!(no_crew.child(Focus::Trailer(0)), 6);
    }

    #[test]
    fn hjkl_moves_down_from_the_trailers_into_more_like_this() {
        let movie = Rows {
            actions: 2,
            versions: 0,
            version: 0,
            seasons: 0,
            season: 0,
            shuffle: 0,
            episodes: 0,
            cast: 2,
            crew: 0,
            trailers: 1,
            similar: 3,
            columns: 4,
        };
        let to = |focus| Step::To(focus);
        assert_eq!(movie.step(Focus::Trailer(0), Key::Down), to(Focus::Similar(0)));
        assert_eq!(movie.step(Focus::Similar(0), Key::Right), to(Focus::Similar(1)));
        assert_eq!(movie.step(Focus::Similar(2), Key::Right), Step::Stay);
        assert_eq!(movie.step(Focus::Similar(0), Key::Left), Step::OutLeft);
        assert_eq!(movie.step(Focus::Similar(1), Key::Down), Step::Stay);
        assert_eq!(movie.step(Focus::Similar(1), Key::Up), to(Focus::Trailer(0)));
        let no_trailers = Rows { trailers: 0, ..movie };
        assert_eq!(no_trailers.step(Focus::Cast(1), Key::Down), to(Focus::Similar(0)));
        assert_eq!(movie.child(Focus::Similar(0)), 3, "hero, cast, trailers");
        let none = Rows { similar: 0, ..movie };
        assert_eq!(none.step(Focus::Trailer(0), Key::Down), Step::Stay, "no similar Items, no row");
    }

    fn episode(fields: serde_json::Value) -> wire::Episode {
        let body: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/list_season_episodes_ok.json")).unwrap();
        let mut episode = body["items"][0].clone();
        episode.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(episode).unwrap()
    }

    #[test]
    fn an_episode_row_has_the_runtime_the_air_date_and_a_quality_badge() {
        assert_eq!(episode_line(&episode(json!({}))), "57m · Feb 18, 2022");
        assert_eq!(episode_line(&episode(json!({"air_date": null}))), "57m");
        assert_eq!(
            episode_line(&episode(json!({"air_date": "soon"}))),
            "57m",
            "a date the app cannot read is left out"
        );
        let dv =
            episode(json!({"overlay_summary": {"resolution": "4K", "hdr": "Dolby Vision", "video_codec": "hevc"}}));
        assert_eq!(episode_badge(&dv), "4K DV");
        assert_eq!(
            episode_badge(&episode(json!({"overlay_summary": {"resolution": "2160p", "hdr": "HDR10"}}))),
            "4K HDR10"
        );
        assert_eq!(episode_badge(&episode(json!({}))), "1080p", "with no overlay, the first file gives the badge");
        let hdr_file = json!({"files": [{"file_id": "1", "resolution": "2160p", "hdr": true, "file_size": 1}]});
        assert_eq!(episode_badge(&episode(hdr_file)), "4K HDR");
        assert_eq!(episode_badge(&episode(json!({"files": []}))), "");
    }

    #[test]
    fn a_season_card_counts_the_watched_episodes() {
        let season = |fields: serde_json::Value| {
            let body: serde_json::Value =
                serde_json::from_str(include_str!("../tests/fixtures/silo/list_series_seasons_ok.json")).unwrap();
            let mut season = body["items"][0].clone();
            season.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
            serde_json::from_value::<wire::Season>(season).unwrap()
        };
        assert_eq!(season_line(&season(json!({}))), "1 of 9 episodes");
        assert_eq!(season_line(&season(json!({"user_data": null}))), "9 episodes");
        assert_eq!(season_line(&season(json!({"user_data": null, "episode_count": 1}))), "1 episode");
    }

    #[test]
    fn an_episode_page_lists_the_episodes_of_its_season() {
        let ep = detail(json!({"type": "episode", "content_id": "e", "series_id": "series:s", "season_number": 2}));
        assert_eq!(episode_list(&ep, None), Some(("series:s".into(), 2)));
        let series = detail(json!({"type": "series", "content_id": "series:s"}));
        assert_eq!(episode_list(&series, Some(3)), Some(("series:s".into(), 3)), "a series lists the chosen season");
        assert_eq!(episode_list(&series, None), None, "the seasons did not load");
        assert_eq!(episode_list(&detail(json!({})), None), None, "a movie has no episodes");
    }

    #[test]
    fn the_hero_has_favorite_mark_watched_and_ends_with_five_stars_that_fill_up_to_the_user_rating() {
        let stars = (1..=5).map(Action::Rate);
        let fresh = detail(json!({"user_state": null}));
        let expected: Vec<_> = [Action::Play { resume: false }, Action::Watchlist, Action::Favorite, Action::Watched]
            .into_iter()
            .chain(stars.clone())
            .collect();
        assert_eq!(actions(&fresh, &[]), expected);
        let labels: Vec<_> = actions(&fresh, &[]).into_iter().skip(2).map(|a| action_label(&fresh, a)).collect();
        assert_eq!(labels, ["♡ Favorite", "Mark watched", "☆", "☆", "☆", "☆", "☆"]);

        let rated = detail(
            json!({"user_rating": 3, "user_state": {"played": true, "is_favorite": true, "in_watchlist": false}}),
        );
        assert_eq!(actions(&rated, &[]), expected, "a User rating adds no Clear rating action");
        let labels: Vec<_> = actions(&rated, &[]).into_iter().skip(2).map(|a| action_label(&rated, a)).collect();
        assert_eq!(labels, ["♥ Favorite", "✓ Watched", "★", "★", "★", "☆", "☆"]);
    }

    #[test]
    fn a_star_press_sets_that_star_and_the_star_of_the_user_rating_clears_it() {
        assert_eq!(star_press(Some(3), 3), None);
        assert_eq!(star_press(Some(3), 1), Some(1));
        assert_eq!(star_press(Some(3), 5), Some(5));
        assert_eq!((1..=5).map(|stars| star_press(None, stars)).collect::<Vec<_>>(), [1, 2, 3, 4, 5].map(Some));
    }

    fn season(fields: serde_json::Value) -> wire::Season {
        let body: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/list_series_seasons_ok.json")).unwrap();
        let mut season = body["items"][0].clone();
        season.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(season).unwrap()
    }

    fn played(episodes: &[wire::Episode]) -> Vec<bool> {
        episodes.iter().map(episode_watched).collect()
    }

    #[test]
    fn a_mark_shows_on_the_episode_its_season_and_the_series() {
        let mut item = detail(json!({"type": "series", "content_id": "series:s", "user_state": null}));
        // Season 1 is listed: 1 of 3 episodes watched. Season 2 has none watched.
        let mut seasons = vec![
            season(
                json!({"episode_count": 3, "user_data": {"watched_count": 1, "unplayed_count": 2, "in_progress_count": 0, "played": false}}),
            ),
            season(json!({"season_number": 2, "episode_count": 2, "user_data": null})),
        ];
        let mut episodes = vec![
            episode(
                json!({"content_id": "e1", "user_data": {"watched_count": 1, "unplayed_count": 0, "in_progress_count": 0, "played": true}}),
            ),
            episode(
                json!({"content_id": "e2", "user_data": {"watched_count": 0, "unplayed_count": 1, "in_progress_count": 1, "played": false, "position_seconds": 600.0}}),
            ),
            episode(json!({"content_id": "e3", "user_data": null})),
        ];

        apply_mark(&mut item, &mut seasons, 0, &mut episodes, Mark::Episode(1), true);
        apply_mark(&mut item, &mut seasons, 0, &mut episodes, Mark::Episode(1), true);
        assert_eq!(played(&episodes), [true, true, false]);
        assert_eq!(season_line(&seasons[0]), "2 of 3 episodes", "a second mark of the same episode counts once");
        assert_eq!(episodes[1].user_data.as_ref().unwrap().position_seconds, None, "the resume point is gone");

        apply_mark(&mut item, &mut seasons, 0, &mut episodes, Mark::Episode(2), true);
        assert_eq!(season_line(&seasons[0]), "3 of 3 episodes");
        assert!(season_watched(&seasons[0]));

        apply_mark(&mut item, &mut seasons, 0, &mut episodes, Mark::Season(0), false);
        assert_eq!(played(&episodes), [false, false, false], "the listed season marks its episodes");
        assert_eq!(season_line(&seasons[0]), "0 of 3 episodes");

        apply_mark(&mut item, &mut seasons, 0, &mut episodes, Mark::Season(1), true);
        assert_eq!(played(&episodes), [false, false, false], "another season leaves the listed episodes");
        assert_eq!(season_line(&seasons[1]), "2 of 2 episodes");

        apply_mark(&mut item, &mut seasons, 0, &mut episodes, Mark::Item, true);
        assert!(item.user_state.as_ref().unwrap().played);
        assert_eq!(played(&episodes), [true, true, true]);
        assert_eq!(season_line(&seasons[0]), "3 of 3 episodes");
    }

    #[test]
    fn a_mark_drops_the_resume_point_of_the_item() {
        let mut item = detail(json!({"position_seconds": 600.0}));
        apply_mark(&mut item, &mut [], 0, &mut [], Mark::Item, false);
        assert_eq!(
            actions(&item, &[])[0],
            Action::Play { resume: false },
            "no Resume after the server cleared the progress"
        );
        assert_eq!(action_label(&item, Action::Watched), "Mark watched");

        // An episode page lists its own episode under More Episodes.
        let mut item = detail(json!({"type": "episode", "content_id": "e2", "user_state": null}));
        let mut episodes = vec![episode(json!({"content_id": "e1"})), episode(json!({"content_id": "e2"}))];
        apply_mark(&mut item, &mut [], 0, &mut episodes, Mark::Episode(1), true);
        assert_eq!(action_label(&item, Action::Watched), "✓ Watched");
        apply_mark(&mut item, &mut [], 0, &mut episodes, Mark::Item, false);
        assert!(!episode_watched(&episodes[1]));
    }

    #[test]
    fn the_hero_fills_72_percent_of_a_wide_window_and_60_percent_of_a_narrow_one() {
        assert_eq!(hero_height((1600., 1000.), 400.), 720.);
        assert_eq!(hero_height((900., 1000.), 400.), 600., "narrower than 1024 px");
        assert_eq!(hero_height((1600., 500.), 450.), 450., "the content is taller than 72%");
    }

    #[test]
    fn a_narrow_hero_hides_the_poster_so_that_the_text_fits() {
        assert_eq!(hero_columns(1336.), (true, 1008.));
        assert_eq!(hero_columns(728.), (true, 400.), "the logo just fits next to the poster");
        assert_eq!(hero_columns(700.), (false, 604.));
        assert_eq!(hero_columns(400.), (false, 304.), "narrower than the logo");
    }

    #[test]
    fn the_backdrop_covers_the_hero_and_keeps_the_upper_part_of_the_art() {
        // Wider than 16:9: the full width, 20% of the overflow above the hero.
        let wide = backdrop_box((1600., 450.));
        assert_eq!((wide.origin.x, wide.size.width, wide.size.height), (0., 1600., 900.));
        assert_eq!(wide.origin.y, -90.);
        // Taller than 16:9: the full height, centered from side to side.
        let tall = backdrop_box((800., 900.));
        assert_eq!((tall.origin.x, tall.origin.y, tall.size.width, tall.size.height), (-400., 0., 1600., 900.));
        let exact = backdrop_box((1600., 900.));
        assert_eq!((exact.origin.x, exact.origin.y, exact.size.width, exact.size.height), (0., 0., 1600., 900.));
    }

    #[test]
    fn the_artwork_color_is_the_average_of_the_thumbhash_or_the_accent() {
        use base64::Engine as _;
        let blue: Vec<u8> = [30u8, 60, 200, 255].repeat(16 * 16);
        let hash = base64::engine::general_purpose::STANDARD.encode(thumbhash::rgba_to_thumb_hash(16, 16, &blue));
        let accent = gpui::hsla(0.5, 0.5, 0.5, 1.);
        let color = gpui::Rgba::from(artwork_color(Some(&hash), accent));
        let near = |a: f32, b: u8| (a - b as f32 / 255.).abs() < 0.03;
        assert!(near(color.r, 30) && near(color.g, 60) && near(color.b, 200), "{color:?}");
        assert_eq!(artwork_color(None, accent), accent);
        assert_eq!(artwork_color(Some("not base64!"), accent), accent);
    }
}
