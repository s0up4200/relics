//! Home: the Home sections of the active Profile as rows that scroll sideways. A Featured section shows
//! as a hero above the other rows. The Recommended tab of a Library uses the same view for its rows.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, ImageSource, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement, Render, RenderImage, Resource, ScrollHandle, StatefulInteractiveElement as _, Styled,
    StyledImage as _, Subscription, Task, Window, div, img, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, HomeSection, wire};
use crate::artwork::{self, ArtworkCache, Rung};
use crate::keys::Key;

/// The user pressed p on a card, or Play on the detail page.
pub struct Play(pub crate::player::PlayItem);

/// The user pressed Enter on a card, or clicked it.
pub struct Open(pub wire::CatalogItem);

/// Open a page with the grid of `query`, for example "View all" of a row.
#[derive(Clone)]
pub struct OpenGrid {
    pub title: String,
    pub query: wire::CatalogQuery,
}

/// Where the rows come from.
#[derive(Debug, Clone)]
pub enum Rows {
    Home,
    /// The Recommended tab of the Library with this id.
    Library(String),
}

pub struct Home {
    client: Arc<Client>,
    rows: Rows,
    /// The sections in display order: the Featured sections first, then the others in layout order.
    sections: Vec<HomeSection>,
    status: Option<String>,
    /// The last load failed. The page shows Retry next to the error.
    failed: bool,
    focus: Focus,
    /// The page has the keyboard focus, so the focused card shows the focus ring.
    pub active: bool,
    page_scroll: ScrollHandle,
    row_scrolls: Vec<ScrollHandle>,
    /// The decoded Thumbhash of each card, in the order of `sections`.
    thumbs: Vec<Vec<Option<Arc<RenderImage>>>>,
    artwork: Entity<ArtworkCache>,
    /// The rung of the Artwork ladder of the loaded list.
    image_size: Rung,
    _loading: Task<()>,
    _scale: Subscription,
}

impl EventEmitter<Play> for Home {}
impl EventEmitter<Open> for Home {}
impl EventEmitter<OpenGrid> for Home {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// No card: the rows are empty, or h or Left left the first column.
    Outside,
    Card {
        row: usize,
        col: usize,
    },
}

/// Moves the focus for an arrow or hjkl key. `rows` holds the number of cards in each row.
pub fn move_focus(focus: Focus, key: Key, rows: &[usize]) -> Focus {
    let first_card_in = |mut range: Box<dyn Iterator<Item = usize>>, col: usize| {
        range.find(|&row| rows[row] > 0).map(|row| Focus::Card { row, col: col.min(rows[row] - 1) })
    };
    let moved = match (focus, key) {
        (Focus::Outside, Key::Right) => first_card_in(Box::new(0..rows.len()), 0),
        (Focus::Card { col: 0, .. }, Key::Left) => Some(Focus::Outside),
        (Focus::Card { row, col }, Key::Left) => Some(Focus::Card { row, col: col - 1 }),
        (Focus::Card { row, col }, Key::Right) if col + 1 < rows[row] => Some(Focus::Card { row, col: col + 1 }),
        (Focus::Card { row, col }, Key::Down) => first_card_in(Box::new(row + 1..rows.len()), col),
        (Focus::Card { row, col }, Key::Up) => first_card_in(Box::new((0..row).rev()), col),
        _ => None,
    };
    moved.unwrap_or(focus)
}

impl Home {
    pub fn new(client: Arc<Client>, rows: Rows, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A larger scale loads the list again only when the rung goes up.
        let scale = cx.observe_window_bounds(window, |this, window, cx| {
            let size = Rung::home(window.scale_factor());
            if size > this.image_size {
                this.image_size = size;
                this.load(cx);
            }
        });
        cx.on_release(|this, cx| {
            for thumb in this.thumbs.drain(..).flatten().flatten() {
                cx.drop_image(thumb, None);
            }
        })
        .detach();
        let mut home = Self {
            client,
            rows,
            sections: Vec::new(),
            thumbs: Vec::new(),
            artwork: artwork::global(cx),
            image_size: Rung::home(window.scale_factor()),
            status: Some("Loading…".into()),
            failed: false,
            focus: Focus::Outside,
            active: false,
            page_scroll: ScrollHandle::new(),
            row_scrolls: Vec::new(),
            _loading: Task::ready(()),
            _scale: scale,
        };
        home.load(cx);
        home
    }

    /// Loads the Home sections, with artwork URLs for `image_size`.
    fn load(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let (image_size, rows) = (self.image_size, self.rows.clone());
        self._loading = cx.spawn(async move |this, cx| {
            let result = match rows {
                Rows::Home => client.home(image_size).await,
                Rows::Library(id) => client.library_rows(&id, image_size).await,
            };
            this.update(cx, |this, cx| {
                match result {
                    Ok(mut sections) => {
                        sections.sort_by_key(|s| !s.featured);
                        let first_load = this.sections.is_empty();
                        let thumbs = sections.iter().map(|s| s.items.iter().map(|i| thumb(s, i)).collect()).collect();
                        for old in std::mem::replace(&mut this.thumbs, thumbs).into_iter().flatten().flatten() {
                            cx.drop_image(old, None);
                        }
                        // A reload at a larger rung keeps the scroll positions and the focus.
                        this.row_scrolls.resize_with(sections.len(), ScrollHandle::new);
                        this.sections = sections;
                        this.status = None;
                        this.failed = false;
                        let rows = this.row_lengths();
                        let lost = match this.focus {
                            Focus::Card { row, col } => rows.get(row).is_none_or(|&cards| col >= cards),
                            Focus::Outside => first_load,
                        };
                        if lost {
                            this.focus = move_focus(Focus::Outside, Key::Right, &rows);
                        }
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                    }
                    // The page keeps what it shows. The Library window shows Offline or the sign-in screen.
                    Err(err) => {
                        this.status = Some(err.to_string());
                        this.failed = true;
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// The number of focus stops in each row: the cards, and "View all" after them.
    fn row_lengths(&self) -> Vec<usize> {
        self.sections.iter().map(|s| s.items.len() + usize::from(s.view_all)).collect()
    }

    /// Opens the full list of a row.
    fn view_all(&self, row: usize, cx: &mut Context<Self>) {
        let section = &self.sections[row];
        let library = match &self.rows {
            Rows::Home => None,
            Rows::Library(id) => Some(id.as_str()),
        };
        cx.emit(OpenGrid { title: section.title.clone(), query: crate::browse::section_query(&section.id, library) });
    }

    /// Handles a key. Returns false when Home does not use it, for example h or Left in the first column.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        if let (Focus::Card { row, col }, Key::Other(name @ ("p" | "enter"))) = (self.focus, key) {
            match (self.sections[row].items.get(col).cloned(), name) {
                (Some(item), "p") => cx.emit(Play((&item).into())),
                (Some(item), _) => cx.emit(Open(item)),
                (None, "enter") => self.view_all(row, cx),
                (None, _) => {}
            }
            return true;
        }
        let focus = move_focus(self.focus, key, &self.row_lengths());
        let Focus::Card { row, col } = focus else {
            return false;
        };
        if focus == self.focus {
            return false;
        }
        self.page_scroll.scroll_to_item(row);
        self.row_scrolls[row].scroll_to_item(col);
        self.focus = focus;
        cx.notify();
        true
    }

    fn row(&self, index: usize, section: &HomeSection, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let wide = is_wide(section);
        let (width, height) = match (section.featured, wide) {
            (true, _) => (560., 300.),
            (false, true) => (artwork::HOME_CARD_WIDTH, 163.),
            (false, false) => (150., 225.),
        };
        let cards = section.items.iter().enumerate().map(|(col, item)| {
            let focused = self.active && self.focus == Focus::Card { row: index, col };
            let url = art_url(section, item).and_then(|url| self.client.server().url.join(url).ok());
            let thumb = self.thumbs.get(index).and_then(|row| row.get(col)).cloned().flatten();
            let mut art = div()
                .relative()
                .w(px(width))
                .h(px(height))
                .flex_none()
                .rounded(px(12.))
                .overflow_hidden()
                .bg(t.surface)
                .border_2()
                .border_color(if focused { t.accent } else { gpui::transparent_black() })
                .child(div().absolute().inset_0().child(artwork_image(url.map(String::from), thumb)));
            if section.featured {
                art = art.child(
                    div().absolute().inset_0().flex().items_end().p(px(14.)).child(
                        div()
                            .text_size(px(32.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.bright)
                            .child(item.title.clone()),
                    ),
                );
                return art.into_any_element();
            }
            if wide && let Some(progress) = progress(item.position_seconds, item.duration_seconds) {
                art = art.child(progress_bar(progress, t.accent));
            }
            div()
                .w(px(width))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(art)
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if focused { t.bright } else { t.foreground })
                        .truncate()
                        .child(item.title.clone()),
                )
                .child(div().text_size(px(12.)).text_color(t.secondary).truncate().child(short_line(item)))
                .into_any_element()
        });
        let col = section.items.len();
        let view_all = section.view_all.then(|| {
            let focused = self.active && self.focus == Focus::Card { row: index, col };
            div()
                .id(("view-all", index))
                .w(px(width))
                .h(px(height))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(12.))
                .bg(t.surface)
                .border_2()
                .border_color(if focused { t.accent } else { gpui::transparent_black() })
                .text_color(if focused { t.bright } else { t.secondary })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.focus = Focus::Card { row: index, col };
                    this.view_all(index, cx);
                }))
                .child("View all")
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div().px(px(40.)).flex().items_baseline().gap(px(12.)).child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.bright)
                        .child(section.title.clone()),
                ),
            )
            .child(
                div()
                    .id(("home-row", index))
                    .px(px(40.))
                    .flex()
                    .gap(px(14.))
                    .overflow_x_scroll()
                    .track_scroll(&self.row_scrolls[index])
                    .children(cards)
                    .children(view_all),
            )
            .into_any_element()
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let rows: Vec<_> = self.sections.iter().enumerate().map(|(i, s)| self.row(i, s, cx)).collect();
        div()
            .image_cache(self.artwork.clone())
            .id("home-page")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(36.))
            .pt(px(32.))
            .pb(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .children(rows)
            .children(self.status.clone().map(|status| {
                let retry = button("home-retry", "Retry", ButtonVariant::Secondary, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx)));
                div()
                    .px(px(40.))
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .text_color(if self.failed { t.danger } else { t.secondary })
                    .child(status)
                    .children(self.failed.then_some(retry))
            }))
    }
}

/// Continue watching and Next up use 16:9 cards. Other sections use 2:3 posters.
fn is_wide(section: &HomeSection) -> bool {
    matches!(section.section_type.as_str(), "continue_watching" | "next_up")
}

/// The artwork URL of a card: the backdrop (the still for an episode) on a 16:9 card
/// or a hero, else the poster.
fn art_url<'a>(section: &HomeSection, item: &'a wire::CatalogItem) -> Option<&'a str> {
    let url = if section.featured || is_wide(section) { &item.backdrop_url } else { &item.poster_url };
    url.as_deref().filter(|url| !url.is_empty())
}

fn thumb(section: &HomeSection, item: &wire::CatalogItem) -> Option<Arc<RenderImage>> {
    let hash = if section.featured || is_wide(section) { &item.backdrop_thumbhash } else { &item.poster_thumbhash };
    hash.as_deref().and_then(artwork::thumbhash)
}

/// The artwork of a card. The Thumbhash shows while the image loads and after it fails.
pub(crate) fn artwork_image(url: Option<String>, thumb: Option<Arc<RenderImage>>) -> AnyElement {
    let preview = move || match thumb.clone() {
        Some(thumb) => img(ImageSource::Render(thumb)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
        None => div().into_any_element(),
    };
    match url {
        Some(url) => img(ImageSource::Resource(Resource::Uri(url.into())))
            .size_full()
            .object_fit(ObjectFit::Cover)
            .with_loading(preview.clone())
            .with_fallback(preview)
            .into_any_element(),
        None => preview(),
    }
}

/// The watched part of an Item, from 0 to 1, when the user started it.
pub(crate) fn progress(position: Option<f64>, duration: Option<f64>) -> Option<f32> {
    let (position, duration) = (position?, duration?);
    (position > 0. && duration > 0.).then(|| (position / duration).min(1.) as f32)
}

/// The progress bar inside the artwork of a 16:9 card.
pub(crate) fn progress_bar(progress: f32, accent: gpui::Hsla) -> gpui::Div {
    div().absolute().inset_0().flex().flex_col().justify_end().p(px(8.)).child(
        div()
            .h(px(3.))
            .rounded_full()
            .bg(gpui::hsla(0., 0., 0., 0.45))
            .child(div().h_full().w(gpui::relative(progress)).rounded_full().bg(accent)),
    )
}

/// The line below the title: the series with the season and episode, or the year.
pub(crate) fn short_line(item: &wire::CatalogItem) -> String {
    match (&item.series_title, item.season_number, item.episode_number) {
        (Some(series), Some(season), Some(episode)) => format!("{series} · S{season} E{episode}"),
        _ => item.year.map(|year| year.to_string()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Focus, move_focus};
    use crate::keys::Key;

    #[test]
    fn hjkl_and_arrows_move_between_the_sidebar_and_the_cards() {
        let rows = [3, 0, 2];
        let card = |row, col| Focus::Card { row, col };
        assert_eq!(move_focus(Focus::Outside, Key::Right, &rows), card(0, 0));
        assert_eq!(move_focus(card(0, 0), Key::Left, &rows), Focus::Outside);
        assert_eq!(move_focus(card(0, 1), Key::Right, &rows), card(0, 2));
        assert_eq!(move_focus(card(0, 2), Key::Right, &rows), card(0, 2), "the last card stays");
        assert_eq!(move_focus(card(0, 2), Key::Down, &rows), card(2, 1), "an empty row is skipped");
        assert_eq!(move_focus(card(2, 1), Key::Up, &rows), card(0, 1));
        assert_eq!(move_focus(card(0, 1), Key::Up, &rows), card(0, 1));
        assert_eq!(move_focus(Focus::Outside, Key::Down, &rows), Focus::Outside);
        assert_eq!(move_focus(Focus::Outside, Key::Right, &[]), Focus::Outside);
    }
}
