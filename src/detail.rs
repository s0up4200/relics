//! The detail page of an Item: a hero with the backdrop, the poster, the title, the metadata line, the quality
//! badges, the overview and the buttons. A movie shows its cast. A series shows its seasons and a grid of 16:9
//! episode cards. The page loads `large` artwork.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Render,
    RenderImage, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Task, Window, div,
    linear_color_stop, linear_gradient, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, Error, wire};
use crate::artwork::{self, ArtworkCache};
use crate::grid::Move;
use crate::home::{Play, artwork_image, progress, progress_bar};
use crate::player::PlayItem;
use crate::search::OpenPerson;

/// The rung of the Artwork ladder of the detail page.
const IMAGE_SIZE: &str = "large";
const EPISODE_WIDTH: f32 = 250.;
const EPISODE_HEIGHT: f32 = 140.;
const GAP: f32 = 14.;
/// The left and right margin of the page.
const MARGIN: f32 = 48.;

pub struct Detail {
    client: Arc<Client>,
    /// The id of the Item.
    id: String,
    item: Option<wire::CatalogItemDetail>,
    seasons: Vec<wire::Season>,
    /// The index of the chosen season in `seasons`.
    season: usize,
    /// The season number to choose when the seasons load, from an episode or season card.
    wanted_season: Option<i64>,
    episodes: Vec<wire::Episode>,
    focus: Focus,
    /// The page has the keyboard focus, so the focused control shows the focus ring.
    pub active: bool,
    columns: usize,
    /// The error of the last request, shown with Retry.
    error: Option<String>,
    /// The Thumbhash of the backdrop, the poster and each cast photo.
    thumbs: Vec<Option<Arc<RenderImage>>>,
    episode_thumbs: Vec<Option<Arc<RenderImage>>>,
    scroll: ScrollHandle,
    artwork: Entity<ArtworkCache>,
    _loading: Task<()>,
    _episodes: Task<()>,
    _watchlist: Task<()>,
}

impl EventEmitter<Play> for Detail {}
impl EventEmitter<OpenPerson> for Detail {}

impl Detail {
    pub fn new(client: Arc<Client>, id: String, season: Option<i64>, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| {
            for thumb in this.thumbs.drain(..).chain(this.episode_thumbs.drain(..)).flatten() {
                cx.drop_image(thumb, None);
            }
        })
        .detach();
        let mut detail = Self {
            client,
            id,
            item: None,
            seasons: Vec::new(),
            season: 0,
            wanted_season: season,
            episodes: Vec::new(),
            focus: Focus::Action(0),
            active: false,
            columns: 1,
            error: None,
            thumbs: Vec::new(),
            episode_thumbs: Vec::new(),
            scroll: ScrollHandle::new(),
            artwork: artwork::global(cx),
            _loading: Task::ready(()),
            _episodes: Task::ready(()),
            _watchlist: Task::ready(()),
        };
        detail.load(cx);
        detail
    }

    /// Loads the Item, and the seasons of a series.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let (client, id) = (self.client.clone(), self.id.clone());
        self._loading = cx.spawn(async move |this, cx| {
            let result: Result<_, Error> = async {
                let item = client.item(&id, IMAGE_SIZE).await?;
                let seasons = if item.type_ == "series" { client.seasons(&id).await? } else { Vec::new() };
                Ok((item, seasons))
            }
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((item, seasons)) => {
                        let thumbs = [&item.backdrop_thumbhash, &item.poster_thumbhash]
                            .into_iter()
                            .chain(item.cast.iter().map(|c| &c.photo_thumbhash))
                            .map(|hash| hash.as_deref().and_then(artwork::thumbhash))
                            .collect();
                        for old in std::mem::replace(&mut this.thumbs, thumbs).into_iter().flatten() {
                            cx.drop_image(old, None);
                        }
                        let number = this.wanted_season.take().or(item.play_season_number);
                        this.season = seasons.iter().position(|s| Some(s.season_number) == number).unwrap_or(0);
                        this.seasons = seasons;
                        this.item = Some(item);
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                        this.load_episodes(cx);
                    }
                    // The Library window shows Offline or the sign-in screen. The page shows Retry.
                    Err(err) => this.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Loads the episodes of the chosen season.
    fn load_episodes(&mut self, cx: &mut Context<Self>) {
        let Some(season) = self.seasons.get(self.season).map(|s| s.season_number) else {
            return;
        };
        for old in self.episode_thumbs.drain(..).flatten() {
            cx.drop_image(old, None);
        }
        self.episodes.clear();
        let (client, id) = (self.client.clone(), self.id.clone());
        self._episodes = cx.spawn(async move |this, cx| {
            let result = client.season_episodes(&id, season).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(episodes) => {
                        this.episode_thumbs = episodes
                            .iter()
                            .map(|e| e.still_thumbhash.as_deref().and_then(artwork::thumbhash))
                            .collect();
                        this.episodes = episodes;
                    }
                    Err(err) => this.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn rows(&self) -> Rows {
        Rows {
            actions: self.item.as_ref().map_or(0, |item| actions(item).len()),
            seasons: self.seasons.len(),
            season: self.season,
            cast: self.item.as_ref().filter(|item| item.type_ != "series").map_or(0, |item| item.cast.len()),
            episodes: self.episodes.len(),
            columns: self.columns,
        }
    }

    /// Handles a key. Returns false when the page does not use it, for example Esc or h at the left edge.
    pub fn key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(item) = &self.item else {
            return false;
        };
        match (self.focus, key) {
            (Focus::Action(i), "enter") => self.act(actions(item)[i], cx),
            (Focus::Season(i), "enter") => self.choose_season(i, cx),
            (Focus::Episode(i), "enter" | "p") => self.play_episode(i, cx),
            (Focus::Cast(i), "enter") => self.open_person(i, cx),
            (_, "p") => cx.emit(Play(item.into())),
            (focus, key) => match self.rows().step(focus, key) {
                Step::To(focus) => {
                    self.focus = focus;
                    // ponytail: scrolls to the row, not the card; a season with many rows of episodes can hide
                    // the focused card below the window.
                    self.scroll.scroll_to_item(match focus {
                        Focus::Action(_) => 0,
                        Focus::Season(_) | Focus::Cast(_) => 1,
                        Focus::Episode(_) => 2,
                    });
                }
                Step::OutLeft | Step::Stay => return false,
            },
        }
        cx.notify();
        true
    }

    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some(item) = &mut self.item else {
            return;
        };
        match action {
            Action::Play { .. } => cx.emit(Play(PlayItem::from(&*item))),
            Action::StartOver => {
                let mut play = PlayItem::from(&*item);
                play.start_over = true;
                cx.emit(Play(play));
            }
            Action::Watchlist => {
                let state = item.user_state.get_or_insert(wire::CatalogItemUserState {
                    in_watchlist: false,
                    is_favorite: false,
                    played: false,
                });
                state.in_watchlist = !state.in_watchlist;
                let (client, id, on) = (self.client.clone(), self.id.clone(), state.in_watchlist);
                self._watchlist = cx.spawn(async move |this, cx| {
                    if let Err(err) = client.set_watchlist(&id, on).await {
                        this.update(cx, |this, cx| {
                            if let Some(state) = this.item.as_mut().and_then(|item| item.user_state.as_mut()) {
                                state.in_watchlist = !on;
                            }
                            this.error = Some(err.to_string());
                            cx.notify();
                        })
                        .ok();
                    }
                });
            }
        }
        cx.notify();
    }

    fn choose_season(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Season(index);
        if index != self.season {
            self.season = index;
            self.load_episodes(cx);
        }
        cx.notify();
    }

    /// Plays an episode at its saved position, with the series, so that Next episode works.
    fn play_episode(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Episode(index);
        if let Some(episode) = self.episodes.get(index) {
            cx.emit(Play(PlayItem::episode(self.id.clone(), episode.clone())));
        }
        cx.notify();
    }

    /// Opens the Person grid of a cast member that the server knows.
    fn open_person(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Cast(index);
        let credit = self.item.as_ref().and_then(|item| item.cast.get(index));
        if let Some((id, name)) = credit.and_then(|c| Some((c.person_id.clone()?, c.name.clone()))) {
            cx.emit(OpenPerson { id, name });
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
        focus: Focus,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = cx.omarchy();
        let focused = self.active && self.focus == focus;
        div()
            .id(id)
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

    fn hero(&self, item: &wire::CatalogItemDetail, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let buttons = actions(item).into_iter().enumerate().map(|(i, action)| {
            let label: SharedString = match action {
                Action::Play { resume: true } => "▶ Resume".into(),
                Action::Play { resume: false } => "▶ Play".into(),
                Action::StartOver => "Start over".into(),
                Action::Watchlist if item.user_state.as_ref().is_some_and(|s| s.in_watchlist) => "✓ Watchlist".into(),
                Action::Watchlist => "+ Watchlist".into(),
            };
            self.pill(("detail-action", i), label, i == 0, Focus::Action(i), cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.focus = Focus::Action(i);
                    this.act(action, cx);
                },
            ))
        });
        let badges = quality_badges(item).into_iter().map(|badge| {
            div().px(px(8.)).py(px(2.)).rounded_full().bg(gpui::hsla(0., 0., 1., 0.1)).text_size(px(10.)).child(badge)
        });
        div()
            .relative()
            .w_full()
            .h(px(420.))
            .flex_none()
            .child(div().absolute().inset_0().child(artwork_image(self.url(&item.backdrop_url), self.thumb(0))))
            .child(div().absolute().inset_0().bg(linear_gradient(
                180.,
                linear_color_stop(gpui::hsla(0., 0., 0., 0.), 0.),
                linear_color_stop(t.background, 1.),
            )))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_end()
                    .gap(px(32.))
                    .px(px(MARGIN))
                    .pb(px(28.))
                    .child(
                        div()
                            .w(px(200.))
                            .h(px(300.))
                            .flex_none()
                            .rounded(px(12.))
                            .overflow_hidden()
                            .bg(t.surface)
                            .child(artwork_image(self.url(&item.poster_url), self.thumb(1))),
                    )
                    .child(
                        div()
                            .max_w(px(640.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(div().text_size(px(11.)).text_color(t.secondary).child(item.type_.to_uppercase()))
                            .child(
                                div()
                                    .text_size(px(46.))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .text_color(t.bright)
                                    .child(item.title.clone()),
                            )
                            .child(div().text_color(t.secondary).child(meta_line(item)))
                            .child(div().flex().gap(px(6.)).children(badges))
                            .children(item.overview.clone().map(|text| div().line_clamp(3).child(text)))
                            .child(div().pt(px(6.)).flex().gap(px(10.)).children(buttons)),
                    ),
            )
            .into_any_element()
    }

    fn season_pills(&self, cx: &Context<Self>) -> AnyElement {
        let pills = self.seasons.iter().enumerate().map(|(i, season)| {
            self.pill(("detail-season", i), season.title.clone().into(), i == self.season, Focus::Season(i), cx)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_season(i, cx)))
        });
        div().px(px(MARGIN)).flex().flex_wrap().gap(px(8.)).children(pills).into_any_element()
    }

    fn episode_grid(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let cards = self.episodes.iter().enumerate().map(|(i, episode)| {
            let focused = self.active && self.focus == Focus::Episode(i);
            let thumb = self.episode_thumbs.get(i).cloned().flatten();
            let progress = episode.user_data.as_ref().and_then(|d| progress(d.position_seconds, d.duration_seconds));
            let mut art = div()
                .relative()
                .w(px(EPISODE_WIDTH))
                .h(px(EPISODE_HEIGHT))
                .rounded(px(10.))
                .overflow_hidden()
                .bg(t.surface)
                .border_2()
                .border_color(if focused { t.accent } else { gpui::transparent_black() })
                .child(div().absolute().inset_0().child(artwork_image(self.url(&episode.still_url), thumb)));
            if let Some(progress) = progress {
                art = art.child(progress_bar(progress, t.accent));
            }
            div()
                .id(("detail-episode", i))
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
                .child(div().text_size(px(12.)).text_color(t.secondary).child(format!("{}m", episode.runtime)))
        });
        div().px(px(MARGIN)).flex().flex_wrap().gap(px(GAP)).children(cards).into_any_element()
    }

    fn cast(&self, item: &wire::CatalogItemDetail, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let cards = item.cast.iter().enumerate().map(|(i, credit)| {
            let focused = self.active && self.focus == Focus::Cast(i);
            div()
                .id(("detail-cast", i))
                .w(px(110.))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(4.))
                .on_click(cx.listener(move |this, _, _, cx| this.open_person(i, cx)))
                .child(
                    div()
                        .w(px(110.))
                        .h(px(150.))
                        .rounded(px(10.))
                        .overflow_hidden()
                        .bg(t.surface)
                        .border_2()
                        .border_color(if focused { t.accent } else { gpui::transparent_black() })
                        .child(artwork_image(self.url(&credit.photo_url), self.thumb(i + 2))),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(if focused { t.bright } else { t.foreground })
                        .truncate()
                        .child(credit.name.clone()),
                )
                .child(div().text_size(px(12.)).text_color(t.secondary).truncate().child(credit.character.clone()))
        });
        div()
            .px(px(MARGIN))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child("Cast"))
            .child(div().id("detail-cast").flex().gap(px(GAP)).overflow_x_scroll().children(cards))
            .into_any_element()
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
        // The children keep the order hero, seasons, episodes, so that `key` can scroll to them by index.
        let mut page = div()
            .image_cache(self.artwork.clone())
            .id("detail-page")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(24.))
            .pb(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if let Some(item) = &self.item {
            page = page.child(self.hero(item, cx));
            if item.type_ == "series" {
                page = page.child(self.season_pills(cx)).child(self.episode_grid(cx));
            } else if !item.cast.is_empty() {
                page = page.child(self.cast(item, cx));
            }
        }
        page.children(status)
    }
}

/// The Item that Enter on a card opens, and the season to show: an episode or a season opens its series.
pub fn target(item: &wire::CatalogItem) -> (String, Option<i64>) {
    match (&item.series_id, item.type_.as_str()) {
        (Some(series), "episode" | "season") => (series.clone(), item.season_number),
        _ => (item.content_id.clone(), None),
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
    Watchlist,
}

/// The buttons of the hero. Start over shows only when Resume does.
pub fn actions(item: &wire::CatalogItemDetail) -> Vec<Action> {
    let resume = item.position_seconds.is_some_and(|position| position > 0.);
    let mut actions = vec![Action::Play { resume }];
    if resume {
        actions.push(Action::StartOver);
    }
    actions.push(Action::Watchlist);
    actions
}

/// The focused control: a hero button, a season pill, an episode card or a cast card, by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Action(usize),
    Season(usize),
    Episode(usize),
    Cast(usize),
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

/// The number of controls in each row of the page.
#[derive(Debug, Clone, Copy)]
pub struct Rows {
    pub actions: usize,
    pub seasons: usize,
    /// The index of the chosen season.
    pub season: usize,
    pub episodes: usize,
    /// The cast cards of a movie.
    pub cast: usize,
    /// The columns of the episode grid.
    pub columns: usize,
}

impl Rows {
    pub fn step(&self, focus: Focus, key: &str) -> Step {
        let along = |i: usize, len: usize, make: fn(usize) -> Focus| match key {
            "h" | "left" if i == 0 => Step::OutLeft,
            "h" | "left" => Step::To(make(i - 1)),
            "l" | "right" if i + 1 < len => Step::To(make(i + 1)),
            _ => Step::Stay,
        };
        let down = matches!(key, "j" | "down");
        let up = matches!(key, "k" | "up");
        match focus {
            Focus::Action(_) if down && self.seasons > 0 => Step::To(Focus::Season(self.season)),
            Focus::Action(_) if down && self.episodes > 0 => Step::To(Focus::Episode(0)),
            Focus::Action(_) if down && self.cast > 0 => Step::To(Focus::Cast(0)),
            Focus::Action(i) => along(i, self.actions, Focus::Action),
            Focus::Season(_) if up => Step::To(Focus::Action(0)),
            Focus::Season(_) if down && self.episodes > 0 => Step::To(Focus::Episode(0)),
            Focus::Season(i) => along(i, self.seasons, Focus::Season),
            Focus::Cast(_) if up => Step::To(Focus::Action(0)),
            Focus::Cast(i) => along(i, self.cast, Focus::Cast),
            Focus::Episode(i) => match crate::grid::move_focus(i, key, self.episodes, self.columns) {
                Move::To(i) => Step::To(Focus::Episode(i)),
                Move::OutTop if self.seasons > 0 => Step::To(Focus::Season(self.season)),
                Move::OutTop => Step::To(Focus::Action(0)),
                Move::OutLeft => Step::OutLeft,
                Move::Stay => Step::Stay,
            },
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

/// The quality badges of the version that plays: the resolution, HDR and the video codec.
pub fn quality_badges(item: &wire::CatalogItemDetail) -> Vec<String> {
    let resolution = item.effective_version_resolution.as_deref().map(|r| match r {
        "2160p" | "4k" | "4K" => "4K".to_string(),
        other => other.to_string(),
    });
    let hdr = (item.effective_version_hdr == Some(true)).then(|| "HDR".to_string());
    let codec = item.effective_version_codec_video.as_deref().map(str::to_uppercase);
    [resolution, hdr, codec].into_iter().flatten().collect()
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
    fn enter_on_an_episode_or_a_season_opens_its_series_with_that_season() {
        let movie = card(json!({"content_id": "movie:heat-1995"}));
        assert_eq!(target(&movie), ("movie:heat-1995".into(), None));
        let episode = card(json!({"content_id": "e", "type": "episode", "series_id": "series:s", "season_number": 2}));
        assert_eq!(target(&episode), ("series:s".into(), Some(2)));
        let season = card(json!({"content_id": "s2", "type": "season", "series_id": "series:s", "season_number": 2}));
        assert_eq!(target(&season), ("series:s".into(), Some(2)));
        let series = card(json!({"content_id": "series:s", "type": "series"}));
        assert_eq!(target(&series), ("series:s".into(), None));
    }

    #[test]
    fn a_started_item_offers_resume_and_start_over() {
        let fresh = detail(json!({}));
        assert_eq!(actions(&fresh), [Action::Play { resume: false }, Action::Watchlist]);
        let started = detail(json!({"position_seconds": 600.0}));
        assert_eq!(actions(&started), [Action::Play { resume: true }, Action::StartOver, Action::Watchlist]);
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
        let rows = Rows { actions: 3, seasons: 2, season: 1, episodes: 5, cast: 0, columns: 2 };
        let to = |focus| Step::To(focus);
        assert_eq!(rows.step(Focus::Action(0), "l"), to(Focus::Action(1)));
        assert_eq!(rows.step(Focus::Action(2), "right"), Step::Stay);
        assert_eq!(rows.step(Focus::Action(0), "h"), Step::OutLeft, "h at the left goes to the sidebar");
        assert_eq!(rows.step(Focus::Action(2), "j"), to(Focus::Season(1)), "down goes to the chosen season");
        assert_eq!(rows.step(Focus::Season(1), "k"), to(Focus::Action(0)));
        assert_eq!(rows.step(Focus::Season(1), "down"), to(Focus::Episode(0)));
        assert_eq!(rows.step(Focus::Episode(1), "j"), to(Focus::Episode(3)));
        assert_eq!(rows.step(Focus::Episode(3), "k"), to(Focus::Episode(1)));
        assert_eq!(rows.step(Focus::Episode(1), "up"), to(Focus::Season(1)));
        assert_eq!(rows.step(Focus::Episode(2), "h"), Step::OutLeft);
        assert_eq!(rows.step(Focus::Action(0), "k"), Step::Stay);
        let movie = Rows { actions: 2, seasons: 0, season: 0, episodes: 0, cast: 0, columns: 4 };
        assert_eq!(movie.step(Focus::Action(1), "j"), Step::Stay);
        let loading = Rows { episodes: 0, ..rows };
        assert_eq!(loading.step(Focus::Season(0), "j"), Step::Stay);
    }

    #[test]
    fn hjkl_moves_between_the_buttons_and_the_cast_of_a_movie() {
        let movie = Rows { actions: 2, seasons: 0, season: 0, episodes: 0, cast: 3, columns: 4 };
        let to = |focus| Step::To(focus);
        assert_eq!(movie.step(Focus::Action(1), "j"), to(Focus::Cast(0)));
        assert_eq!(movie.step(Focus::Cast(0), "l"), to(Focus::Cast(1)));
        assert_eq!(movie.step(Focus::Cast(2), "right"), Step::Stay);
        assert_eq!(movie.step(Focus::Cast(0), "h"), Step::OutLeft);
        assert_eq!(movie.step(Focus::Cast(1), "k"), to(Focus::Action(0)));
        assert_eq!(movie.step(Focus::Cast(1), "j"), Step::Stay);
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
}
