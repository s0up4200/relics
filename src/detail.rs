//! The detail page of an Item: a hero with the backdrop, the poster, the type and the studio or network, the title
//! logo or the title, the tagline, the metadata line, the quality badges, the ratings, the overview, the Directed by
//! and Written by line, and the buttons. A series shows its seasons and a grid of 16:9 episode cards. Then come the
//! Cast, Crew and Trailers sections. Each request asks for the artwork rung that the page needs at the window scale.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Render,
    RenderImage, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Task, Window, div,
    linear_color_stop, linear_gradient, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, Error, wire};
use crate::artwork::{self, ArtworkCache, Kind, Rung};
use crate::click::Clickable as _;
use crate::grid::Move;
use crate::home::{Play, progress, progress_bar};
use crate::keys::Key;
use crate::player::PlayItem;
use crate::search::OpenPerson;

const EPISODE_WIDTH: f32 = 250.;
const EPISODE_HEIGHT: f32 = 140.;
const GAP: f32 = 14.;
/// The left and right margin of the page.
const MARGIN: f32 = 48.;
const POSTER_WIDTH: f32 = 200.;
const CAST_WIDTH: f32 = 110.;
const TRAILER_WIDTH: f32 = 220.;
const HERO_HEIGHT: f32 = 500.;
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
    columns: usize,
    /// The error of the last request, shown with Retry.
    error: Option<String>,
    /// The Thumbhash of the backdrop, the poster and each cast photo.
    thumbs: Vec<Option<Arc<RenderImage>>>,
    episode_thumbs: Vec<Option<Arc<RenderImage>>>,
    scroll: ScrollHandle,
    artwork: Entity<ArtworkCache>,
    /// The artwork rung of the Item and of the episodes, at the window scale when the page opened.
    // ponytail: read once; a move to a larger scale keeps it until the page opens again.
    image_size: Rung,
    episode_size: Rung,
    _loading: Task<()>,
    _episodes: Task<()>,
    _watchlist: Task<()>,
}

impl EventEmitter<Play> for Detail {}
impl EventEmitter<OpenPerson> for Detail {}

impl Detail {
    pub fn new(client: Arc<Client>, id: String, season: Option<i64>, window: &Window, cx: &mut Context<Self>) -> Self {
        let scale = window.scale_factor();
        // The backdrop fills the page next to the sidebar.
        let viewport = window.viewport_size().width.to_f64() as f32;
        let width = viewport - crate::pages::sidebar_width(viewport, true);
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
            versions: Vec::new(),
            version: 0,
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
            image_size: Rung::fit(Kind::Backdrop, width, scale)
                .max(Rung::fit(Kind::Poster, POSTER_WIDTH, scale))
                .max(Rung::fit(Kind::Profile, CAST_WIDTH, scale)),
            episode_size: Rung::fit(Kind::Still, EPISODE_WIDTH, scale),
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
        let (client, id, image_size) = (self.client.clone(), self.id.clone(), self.image_size);
        self._loading = cx.spawn(async move |this, cx| {
            let result: Result<_, Error> = async {
                let item = client.item(&id, image_size).await?;
                let (seasons, versions) = if item.type_ == "series" {
                    (client.seasons(&id).await?, Vec::new())
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
                        let number = this.wanted_season.take().or(item.play_season_number);
                        this.season = seasons.iter().position(|s| Some(s.season_number) == number).unwrap_or(0);
                        this.seasons = seasons;
                        this.version = this.version.min(versions.len().saturating_sub(1));
                        this.versions = versions;
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
        let (client, id, image_size) = (self.client.clone(), self.id.clone(), self.episode_size);
        self._episodes = cx.spawn(async move |this, cx| {
            let result = client.season_episodes(&id, season, image_size).await;
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
            versions: self.picker_len(),
            version: self.version,
            seasons: self.seasons.len(),
            season: self.season,
            episodes: self.episodes.len(),
            cast: self.item.as_ref().map_or(0, |item| item.cast.len()),
            crew: self.item.as_ref().map_or(0, |item| item.crew.len()),
            trailers: self.item.as_ref().map_or(0, |item| trailers(item).len()),
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
        match (self.focus, key) {
            (Focus::Action(i), Key::Other("enter")) => self.act(actions(item)[i], cx),
            (Focus::Version(i), Key::Other("enter")) => self.choose_version(i, cx),
            (Focus::Season(i), Key::Other("enter")) => self.choose_season(i, cx),
            (Focus::Episode(i), Key::Other("enter" | "p")) => self.play_episode(i, cx),
            (Focus::Cast(_) | Focus::Crew(_), Key::Other("enter")) => self.open_person(self.focus, cx),
            (Focus::Trailer(i), Key::Other("enter")) => self.open_trailer(i, cx),
            (_, Key::Other("p")) => cx.emit(Play(self.play_item(item))),
            (focus, key) => match self.rows().step(focus, key) {
                Step::To(focus) => {
                    self.focus = focus;
                    // ponytail: scrolls to the row, not the card; a season with many rows of episodes can hide
                    // the focused card below the window.
                    self.scroll.scroll_to_item(self.rows().child(focus));
                }
                Step::OutLeft | Step::Stay => return false,
            },
        }
        cx.notify();
        true
    }

    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        let play = self.item.as_ref().map(|item| self.play_item(item));
        let (Some(item), Some(mut play)) = (&mut self.item, play) else {
            return;
        };
        match action {
            Action::Play { .. } => cx.emit(Play(play)),
            Action::StartOver => {
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

    fn choose_version(&mut self, index: usize, cx: &mut Context<Self>) {
        self.focus = Focus::Version(index);
        self.version = index;
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

    /// Opens the Person grid of the cast or crew member at `focus`, when the server knows the person.
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

    /// The hero. The backdrop is `width` logical pixels wide.
    fn hero(&self, item: &wire::CatalogItemDetail, width: f32, cx: &Context<Self>) -> AnyElement {
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
            Some(url) => {
                div().w(px(LOGO_WIDTH)).h(px(LOGO_HEIGHT)).child(artwork::logo(url, title_text)).into_any_element()
            }
            None => title_text(),
        };
        let secondary = t.secondary;
        let line =
            |text: String| (!text.is_empty()).then(|| div().text_size(px(14.)).text_color(secondary).child(text));
        let badges = labels.into_iter().map(|badge| {
            div().px(px(8.)).py(px(2.)).rounded_full().bg(gpui::hsla(0., 0., 1., 0.1)).text_size(px(10.)).child(badge)
        });
        div()
            .relative()
            .w_full()
            .h(px(HERO_HEIGHT))
            .flex_none()
            .child(div().absolute().inset_0().child(artwork::image(self.url(&item.backdrop_url), self.thumb(0), width)))
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
                            .w(px(POSTER_WIDTH))
                            .h(px(300.))
                            .flex_none()
                            .rounded(px(12.))
                            .overflow_hidden()
                            .bg(t.surface)
                            .child(artwork::image(self.url(&item.poster_url), self.thumb(1), POSTER_WIDTH)),
                    )
                    .child(
                        div()
                            .max_w(px(640.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(div().text_size(px(11.)).text_color(t.secondary).child(kind_line(item)))
                            .child(title)
                            .children(
                                item.tagline
                                    .clone()
                                    .filter(|text| !text.is_empty())
                                    .map(|text| div().italic().child(text)),
                            )
                            .child(div().text_color(t.secondary).child(meta_line(item)))
                            .child(div().flex().gap(px(6.)).children(badges))
                            .children(line(ratings_line(item)))
                            .children(item.overview.clone().map(|text| div().line_clamp(3).child(text)))
                            .children(line(credits_line(item)))
                            .child(div().pt(px(6.)).flex().gap(px(10.)).children(buttons)),
                    ),
            )
            .into_any_element()
    }

    /// The Version picker: a pill for each file version, with its quality labels and size. A check marks the
    /// version that Play starts.
    fn version_pills(&self, cx: &Context<Self>) -> AnyElement {
        let pills = self.versions.iter().enumerate().map(|(i, version)| {
            let mark = if i == self.version { "✓ " } else { "" };
            let label = format!("{mark}{}", version_row(version)).into();
            self.pill(("detail-version", i), label, i == self.version, Focus::Version(i), cx)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_version(i, cx)))
        });
        div().px(px(MARGIN)).flex().flex_wrap().gap(px(8.)).children(pills).into_any_element()
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
                .child(div().absolute().inset_0().child(artwork::image(
                    self.url(&episode.still_url),
                    thumb,
                    EPISODE_WIDTH,
                )));
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
                .child(div().text_size(px(12.)).text_color(t.secondary).child(format!("{}m", episode.runtime)))
        });
        div().px(px(MARGIN)).flex().flex_wrap().gap(px(GAP)).children(cards).into_any_element()
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

    /// The Cast, Crew and Trailers sections that have cards. Each has a row of cards that scrolls sideways.
    fn sections(&self, item: &wire::CatalogItemDetail, cx: &Context<Self>) -> Vec<AnyElement> {
        let t = cx.omarchy();
        let section = |title: &'static str, id: &'static str, cards: Vec<gpui::Stateful<gpui::Div>>| {
            (!cards.is_empty()).then(|| {
                div()
                    .px(px(MARGIN))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(t.bright).child(title))
                    .child(div().id(id).flex().gap(px(GAP)).overflow_x_scroll().children(cards))
                    .into_any_element()
            })
        };
        let cast = item.cast.iter().enumerate().map(|(i, credit)| {
            let card = (credit.name.as_str(), credit.character.as_str(), &credit.photo_url);
            self.person_card(Focus::Cast(i), card, 2 + i, cx)
        });
        let crew = item.crew.iter().enumerate().map(|(i, credit)| {
            let card = (credit.name.as_str(), credit.job.as_str(), &credit.photo_url);
            self.person_card(Focus::Crew(i), card, 2 + item.cast.len() + i, cx)
        });
        let videos = trailers(item).into_iter().enumerate().map(|(i, (video, _))| {
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
        });
        [
            section("Cast", "detail-cast", cast.collect()),
            section("Crew", "detail-crew", crew.collect()),
            section("Trailers", "detail-trailers", videos.collect()),
        ]
        .into_iter()
        .flatten()
        .collect()
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
        // The children are the hero and one child for each row that is not empty, in the order of `Rows`, so that
        // `key` can scroll to them by index. A series has no versions and a movie has no seasons.
        let mut page = div()
            .id("detail-page")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(24.))
            .pb(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if let Some(item) = &self.item {
            page = page.child(self.hero(item, viewport - crate::pages::sidebar_width(viewport, true), cx));
            if self.picker_len() > 0 {
                page = page.child(self.version_pills(cx));
            }
            if !self.seasons.is_empty() {
                page = page.child(self.season_pills(cx));
            }
            if !self.episodes.is_empty() {
                page = page.child(self.episode_grid(cx));
            }
            page = page.children(self.sections(item, cx));
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

/// The focused control, by index: a hero button, a version pill, a season pill, an episode card, a cast card, a
/// crew card or a trailer card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Action(usize),
    Version(usize),
    Season(usize),
    Episode(usize),
    Cast(usize),
    Crew(usize),
    Trailer(usize),
}

impl Focus {
    /// The control at `index` in row `row` of the page, from the top.
    fn at(row: usize, index: usize) -> Self {
        [Focus::Action, Focus::Version, Focus::Season, Focus::Episode, Focus::Cast, Focus::Crew, Focus::Trailer][row](
            index,
        )
    }

    /// The row of the control, from the top, and its index in the row.
    fn place(self) -> (usize, usize) {
        match self {
            Focus::Action(i) => (0, i),
            Focus::Version(i) => (1, i),
            Focus::Season(i) => (2, i),
            Focus::Episode(i) => (3, i),
            Focus::Cast(i) => (4, i),
            Focus::Crew(i) => (5, i),
            Focus::Trailer(i) => (6, i),
        }
    }
}

/// The row of the episode grid. The other rows go left and right only.
const EPISODE_ROW: usize = 3;

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
    pub episodes: usize,
    pub cast: usize,
    pub crew: usize,
    pub trailers: usize,
    /// The columns of the episode grid.
    pub columns: usize,
}

impl Rows {
    fn len(&self, row: usize) -> usize {
        [self.actions, self.versions, self.seasons, self.episodes, self.cast, self.crew, self.trailers][row]
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

    /// The index of the page child that holds the control: the hero, then one child for each row that is not empty.
    pub fn child(&self, focus: Focus) -> usize {
        let (row, _) = focus.place();
        (0..row).filter(|&r| r == 0 || self.len(r) > 0).count()
    }

    pub fn step(&self, focus: Focus, key: Key) -> Step {
        let (row, i) = focus.place();
        let leave = |down: bool| {
            let next =
                if down { (row + 1..7).find(|&r| self.len(r) > 0) } else { (0..row).rev().find(|&r| self.len(r) > 0) };
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
fn file_size(bytes: i64) -> String {
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
        let rows = Rows {
            actions: 3,
            versions: 0,
            version: 0,
            seasons: 2,
            season: 1,
            episodes: 5,
            cast: 0,
            crew: 0,
            trailers: 0,
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
            episodes: 0,
            cast: 0,
            crew: 0,
            trailers: 0,
            columns: 4,
        };
        assert_eq!(movie.step(Focus::Action(1), Key::Down), Step::Stay);
        let loading = Rows { episodes: 0, ..rows };
        assert_eq!(loading.step(Focus::Season(0), Key::Down), Step::Stay);
    }

    #[test]
    fn hjkl_moves_between_the_buttons_and_the_cast_of_a_movie() {
        let movie = Rows {
            actions: 2,
            versions: 0,
            version: 0,
            seasons: 0,
            season: 0,
            episodes: 0,
            cast: 3,
            crew: 0,
            trailers: 0,
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
            episodes: 0,
            cast: 2,
            crew: 0,
            trailers: 0,
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
            episodes: 5,
            cast: 2,
            crew: 3,
            trailers: 1,
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
        // The page children are the hero and one for each row that is not empty.
        assert_eq!(series.child(Focus::Action(1)), 0);
        assert_eq!(series.child(Focus::Season(1)), 1);
        assert_eq!(series.child(Focus::Trailer(0)), 5, "hero, seasons, episodes, cast, crew; a series has no versions");
        assert_eq!(no_crew.child(Focus::Trailer(0)), 4);
    }
}
