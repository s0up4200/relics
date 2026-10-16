//! Home: the Home sections of the active Profile as rows that scroll sideways. A Featured section shows
//! as a hero above the other rows. The Recommended tab of a Library uses the same view for its rows.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ListAlignment,
    ListOffset, ListState, ParentElement, Render, RenderImage, ScrollHandle, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, list, prelude::FluentBuilder as _, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, Theme, button};

use crate::api::{Client, HomeSection, wire};
use crate::artwork::{self, ArtworkCache, Kind, Rung};
use crate::click::Clickable as _;
use crate::keys::Key;
use crate::pages::freshness::{Deps, Fresh, Reload};
use crate::pages::loading::{Failure, Loader};
use crate::sideways::Row;

/// The user pressed p on a card, or Play on the detail page.
pub struct Play(pub crate::playback::playing::PlayItem);

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
    fresh: Loader,
    /// The page list: one item for each section, and the status line as the last item.
    page: ListState,
    row_scrolls: Vec<ScrollHandle>,
    /// The decoded Thumbhash of each card, in the order of `sections`.
    thumbs: Vec<Vec<Option<Arc<RenderImage>>>>,
    artwork: Entity<ArtworkCache>,
    /// The window scale of the loaded list. Each section has the rung for its cards at this scale.
    scale: f32,
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

/// The focus after a reload from `old` to `new` sections. A focused card moves with its section and Item, and "View
/// all" with its section. When the Item is gone, the focus keeps its place in the row.
pub fn kept_focus(focus: Focus, old: &[HomeSection], new: &[HomeSection]) -> Focus {
    let Focus::Card { row, col } = focus else {
        return focus;
    };
    let Some(section) = old.get(row) else {
        return focus;
    };
    let Some(row) = crate::selection::matching(old, row, new, |s| s.id.clone()) else {
        return focus;
    };
    let col = match section.items.get(col) {
        Some(_) => {
            crate::selection::matching(&section.items, col, &new[row].items, |i| i.content_id.clone()).unwrap_or(col)
        }
        None => new[row].items.len(),
    };
    Focus::Card { row, col }
}

/// The scroll position of the page after a reload of its sections from `old` to `new`. The section at the top of
/// the view stays there, also when a section above it comes or goes. When that section goes, the section that takes
/// its place shows from its start.
pub fn kept_top(top: ListOffset, old: &[HomeSection], new: &[HomeSection]) -> ListOffset {
    let Some(_) = old.get(top.item_ix) else {
        return top;
    };
    match crate::selection::matching(old, top.item_ix, new, |s| s.id.clone()) {
        Some(item_ix) => ListOffset { item_ix, ..top },
        None => ListOffset { offset_in_item: px(0.), ..top },
    }
}

impl Reload for Home {
    fn deps(&self) -> Vec<Deps> {
        vec![match &self.rows {
            Rows::Home => Deps::Every,
            Rows::Library(id) => Deps::Library(id.clone()),
        }]
    }

    fn fresh(&mut self, _: usize) -> &mut Fresh {
        &mut self.fresh
    }

    fn reload(&mut self, _: usize, cx: &mut Context<Self>) {
        self.load(cx);
    }
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
        // A larger scale loads the list again only when the rung of a section goes up.
        let scale = cx.observe_window_bounds(window, |this, window, cx| {
            let (old, new) = (this.scale, window.scale_factor());
            if new > old {
                this.scale = new;
                let rung = |s: &HomeSection, scale| rung(&s.section_type, s.featured, scale);
                if this.sections.iter().any(|s| rung(s, new) != rung(s, old)) {
                    this.load(cx);
                }
            }
        });
        cx.on_release(|this, cx| {
            for mut row in this.thumbs.drain(..) {
                artwork::replace_previews(&mut row, Vec::new(), cx);
            }
        })
        .detach();
        let mut home = Self {
            client,
            rows,
            sections: Vec::new(),
            thumbs: Vec::new(),
            artwork: artwork::global(cx),
            scale: window.scale_factor(),
            status: None,
            failed: false,
            focus: Focus::Outside,
            active: false,
            fresh: Loader::default(),
            // The overdraw measures the rows near the viewport, so that the focus can scroll to them.
            page: ListState::new(1, ListAlignment::Top, px(800.)),
            row_scrolls: Vec::new(),
            _scale: scale,
        };
        home.load(cx);
        home
    }

    /// Loads the Home sections, with the artwork URLs of each section in the rung for its cards.
    fn load(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let (scale, rows) = (self.scale, self.rows.clone());
        Loader::run(
            self,
            cx,
            |this| &mut this.fresh,
            Failure::Retain,
            async move {
                let size = |entry: &wire::SectionLayoutEntry| rung(&entry.section_type, entry.featured, scale);
                match rows {
                    Rows::Home => client.home(size).await,
                    Rows::Library(id) => client.library_rows(&id, size).await,
                }
            },
            move |this, result, cx| match result {
                Ok(sections) => {
                    this.replace(sections, cx);
                }
                Err(err) => {
                    this.status = Some(err.to_string());
                    this.failed = true;
                    let status = this.sections.len();
                    this.page.remeasure_items(status..status + 1);
                }
            },
            Self::load,
        );
    }

    fn replace(&mut self, mut sections: Vec<HomeSection>, cx: &mut Context<Self>) {
        sections.sort_by_key(|s| !s.featured);
        let first_load = self.sections.is_empty();
        let thumbs = sections.iter().map(|s| s.items.iter().map(|i| thumb(s, i)).collect()).collect();
        for mut row in std::mem::replace(&mut self.thumbs, thumbs) {
            artwork::replace_previews(&mut row, Vec::new(), cx);
        }
        // A reload keeps the scroll position of each section and the focus.
        self.row_scrolls = sections
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let old = crate::selection::matching(&sections, i, &self.sections, |s| s.id.clone());
                old.and_then(|i| self.row_scrolls.get(i).cloned()).unwrap_or_else(ScrollHandle::new)
            })
            .collect();
        let focus = kept_focus(self.focus, &self.sections, &sections);
        let old_top = self.page.logical_scroll_top();
        let top = kept_top(old_top, &self.sections, &sections);
        if let (Focus::Card { col: old, .. }, Focus::Card { row, col }) = (self.focus, focus)
            && old != col
        {
            cards(&sections[row]).reveal(&self.row_scrolls[row], col);
        }
        self.focus = focus;
        self.sections = sections;
        self.status = None;
        self.failed = false;
        let rows = self.row_lengths();
        let lost = match self.focus {
            Focus::Card { row, col } => rows.get(row).is_none_or(|&cards| col >= cards),
            Focus::Outside => first_load,
        };
        if lost {
            self.focus = move_focus(Focus::Outside, Key::Right, &rows);
        }
        self.artwork.update(cx, |artwork, _| artwork.retry_failed());
        let count = self.sections.len() + 1;
        if self.page.item_count() == count
            && (top.item_ix, top.offset_in_item) == (old_top.item_ix, old_top.offset_in_item)
        {
            self.page.remeasure();
        } else {
            self.page.reset(count);
            self.page.scroll_to(top);
        }
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
        self.page.scroll_to_reveal_item(row);
        cards(&self.sections[row]).reveal(&self.row_scrolls[row], col);
        self.focus = focus;
        cx.notify();
        true
    }

    /// Makes a card a focus target. A click focuses the card and opens it, like Enter.
    fn card_target(&self, row: usize, col: usize, card: impl IntoElement, cx: &Context<Self>) -> AnyElement {
        div()
            .id(("home-card", row * 10_000 + col))
            .debug_selector(|| format!("home-card-{}-{col}", self.sections[row].id))
            .flex_none()
            .clickable()
            .child(card)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focus = Focus::Card { row, col };
                this.key(Key::Other("enter"), cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The row of a section. `viewport` is the width of the window.
    fn row(&self, index: usize, section: &HomeSection, viewport: f32, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let (width, height) = card_size(&section.section_type, section.featured);
        let card = |col: usize| {
            let Some(item) = section.items.get(col) else {
                return self.view_all_card(index, col, (width, height), cx);
            };
            let focused = self.active && self.focus == Focus::Card { row: index, col };
            let url = art_url(section, item).and_then(|url| self.client.server().url.join(url).ok());
            let thumb = self.thumbs.get(index).and_then(|row| row.get(col)).cloned().flatten();
            if section.featured {
                let art = art(url.map(String::from), thumb, (width, height), focused, t).child(
                    div().absolute().inset_0().flex().items_end().p(px(14.)).child(
                        div()
                            .text_size(px(32.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.bright)
                            .child(item.title.clone()),
                    ),
                );
                return self.card_target(index, col, art, cx);
            }
            self.card_target(index, col, card(item, url.map(String::from), thumb, (width, height), focused, t), cx)
        };
        let count = section.items.len() + usize::from(section.view_all);
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
            .child(cards(section).build(("home-row", index), &self.row_scrolls[index], count, viewport, card))
            .into_any_element()
    }

    /// The "View all" card at the end of a row, the same size as the other cards.
    fn view_all_card(&self, index: usize, col: usize, (width, height): (f32, f32), cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let focused = self.active && self.focus == Focus::Card { row: index, col };
        div()
            .id(("view-all", index))
            .clickable()
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
            .into_any_element()
    }

    /// The last item of the page: the status line with Retry after a failed load, and the bottom margin.
    fn status(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        div()
            .pb(px(80.))
            .children(self.fresh.status(self.status.as_deref(), self.failed).map(|(status, failed)| {
                let retry = button("home-retry", "Retry", ButtonVariant::Secondary, cx)
                    .debug_selector(|| "home-retry".into())
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx)));
                div()
                    .px(px(40.))
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .text_color(if failed { t.danger } else { t.secondary })
                    .child(status.to_owned())
                    .children(failed.then_some(retry))
            }))
            .into_any_element()
    }
}

impl Render for Home {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A deferred call runs when the window has drawn the frame.
        if matches!(self.rows, Rows::Home) && !self.sections.is_empty() && crate::startup::on() {
            window.defer(cx, |_, _| crate::startup::mark("home-ready"));
        }
        let viewport = window.viewport_size().width.to_f64() as f32;
        // The list draws only the sections near the viewport. Each row keeps its own sideways scroll.
        let page = list(
            self.page.clone(),
            cx.processor(move |this, index: usize, _, cx| {
                let item = match this.sections.get(index) {
                    Some(section) => div().pb(px(36.)).child(this.row(index, section, viewport, cx)).into_any_element(),
                    None => this.status(cx),
                };
                div().when(index == 0, |d| d.pt(px(32.))).child(item).into_any_element()
            }),
        );
        div().id("home-page").size_full().child(page.size_full())
    }
}

/// The size of the row of a section: its cards, the gap between them, and the page margin at each end.
fn cards(section: &HomeSection) -> Row {
    let (width, _) = card_size(&section.section_type, section.featured);
    Row { width, gap: 14., pad: 40. }
}

/// Continue watching and Next up use 16:9 cards. Other sections use 2:3 posters.
fn is_wide(section_type: &str) -> bool {
    matches!(section_type, "continue_watching" | "next_up")
}

/// The width and height of the cards of a section: a hero, a 16:9 card, or a poster.
fn card_size(section_type: &str, featured: bool) -> (f32, f32) {
    match (featured, is_wide(section_type)) {
        (true, _) => (560., 300.),
        (false, true) => (290., 163.),
        (false, false) => (150., 225.),
    }
}

/// The rung of the artwork of a section at the window `scale`. A hero or a 16:9 card shows a backdrop, or the
/// still of an episode.
pub fn rung(section_type: &str, featured: bool, scale: f32) -> Rung {
    let (width, _) = card_size(section_type, featured);
    if featured || is_wide(section_type) {
        Rung::fit(Kind::Backdrop, width, scale).max(Rung::fit(Kind::Still, width, scale))
    } else {
        Rung::fit(Kind::Poster, width, scale)
    }
}

/// The artwork URL of a card: the backdrop (the still for an episode) on a 16:9 card
/// or a hero, else the poster.
fn art_url<'a>(section: &HomeSection, item: &'a wire::CatalogItem) -> Option<&'a str> {
    let url = if section.featured || is_wide(&section.section_type) { &item.backdrop_url } else { &item.poster_url };
    url.as_deref().filter(|url| !url.is_empty())
}

fn thumb(section: &HomeSection, item: &wire::CatalogItem) -> Option<Arc<RenderImage>> {
    let hash = if section.featured || is_wide(&section.section_type) {
        &item.backdrop_thumbhash
    } else {
        &item.poster_thumbhash
    };
    hash.as_deref().and_then(artwork::thumbhash)
}

/// The watched part of an Item, from 0 to 1, when the user started it.
pub(crate) fn progress(position: Option<f64>, duration: Option<f64>) -> Option<f32> {
    let (position, duration) = (position?, duration?);
    (position > 0. && duration > 0.).then(|| (position / duration).min(1.) as f32)
}

/// The artwork of a card of `size` (width, height), with the accent border when it has the focus.
pub(crate) fn art(
    url: Option<String>,
    thumb: Option<Arc<RenderImage>>,
    size: (f32, f32),
    focused: bool,
    t: &Theme,
) -> gpui::Div {
    div()
        .relative()
        .w(px(size.0))
        .h(px(size.1))
        .flex_none()
        .rounded(px(12.))
        .overflow_hidden()
        .bg(t.surface)
        .border_2()
        .border_color(if focused { t.accent } else { gpui::transparent_black() })
        .child(div().absolute().inset_0().child(artwork::image(url, thumb, size.0)))
}

/// A poster or 16:9 card of Home, a grid and More Like This: the artwork with the quality badge and the progress,
/// the title, and the short line.
pub(crate) fn card(
    item: &wire::CatalogItem,
    url: Option<String>,
    thumb: Option<Arc<RenderImage>>,
    size: (f32, f32),
    focused: bool,
    t: &Theme,
) -> gpui::Div {
    let badge = card_badge(item);
    let progress = progress(item.position_seconds, item.duration_seconds);
    let art = art(url, thumb, size, focused, t)
        .children((!badge.is_empty()).then(|| badge_label(badge).left(px(8.))))
        .children(progress.map(|progress| progress_bar(progress, t.accent)));
    div()
        .w(px(size.0))
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
}

/// A small label at the top of the artwork of a card, for example the quality badge. The caller sets the side.
pub(crate) fn badge_label(text: String) -> gpui::Div {
    div()
        .absolute()
        .top(px(8.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(gpui::hsla(0., 0., 0., 0.7))
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(text)
}

/// The progress bar inside the artwork of a card.
pub(crate) fn progress_bar(progress: f32, accent: gpui::Hsla) -> gpui::Div {
    div().absolute().inset_0().flex().flex_col().justify_end().p(px(8.)).child(
        div()
            .h(px(3.))
            .rounded_full()
            .bg(gpui::hsla(0., 0., 0., 0.45))
            .child(div().h_full().w(gpui::relative(progress)).rounded_full().bg(accent)),
    )
}

/// The line below the title: the series with the season and episode, or the year. A series adds its status and
/// network, for example "2022 · Returning · HBO". A started card adds the time left, for example "53 min left".
pub(crate) fn short_line(item: &wire::CatalogItem) -> String {
    let first = match (&item.series_title, item.season_number, item.episode_number) {
        (Some(series), Some(season), Some(episode)) => Some(format!("{series} · S{season} E{episode}")),
        _ => item.year.map(|year| year.to_string()),
    };
    let series = item.type_ == "series";
    let status = item.show_status.as_deref().filter(|_| series).map(show_status);
    let network = item.networks.first().filter(|_| series).cloned();
    let left = progress(item.position_seconds, item.duration_seconds).and(item.duration_seconds).map(|duration| {
        format!("{} min left", ((duration - item.position_seconds.unwrap_or_default()) / 60.).round() as i64)
    });
    [first, status, network, left].into_iter().flatten().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// The airing status of a series with the words of the web app: Returning, Ended, Cancelled or Upcoming.
fn show_status(status: &str) -> String {
    match status.to_lowercase().as_str() {
        "returning" | "returning series" | "continuing" | "in_production" | "in production" => "Returning".into(),
        "ended" => "Ended".into(),
        "cancelled" | "canceled" => "Cancelled".into(),
        "upcoming" | "planned" => "Upcoming".into(),
        _ => status.to_string(),
    }
}

/// The quality badge of a card, for example "4K DV", or "" when the server sends none.
pub(crate) fn card_badge(item: &wire::CatalogItem) -> String {
    item.overlay_summary.as_ref().map(crate::detail::overlay_badge).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Focus, card_badge, kept_focus, kept_top, move_focus, rung, short_line};
    use crate::api::HomeSection;
    use crate::api::wire;
    use crate::artwork::Rung;
    use crate::keys::Key;
    use gpui::{ListOffset, px};
    use serde_json::json;

    fn card(fields: serde_json::Value) -> wire::CatalogItem {
        let mut item = json!({"content_id": "x", "type": "movie", "title": "x", "genres": [], "keywords": [], "status": "available", "year": 2022});
        item.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(item).unwrap()
    }

    fn section(id: &str, items: &[&str]) -> HomeSection {
        HomeSection {
            id: id.into(),
            section_type: id.into(),
            title: id.into(),
            featured: false,
            items: items.iter().map(|item| card(json!({"content_id": item}))).collect(),
            view_all: true,
        }
    }

    #[test]
    fn a_reload_keeps_the_focus_on_the_same_card_or_view_all() {
        let old = [section("continue", &["a", "b", "c"]), section("new", &["x"])];
        let new = [section("featured", &[]), section("continue", &["c", "a"]), section("new", &["x"])];
        let at = |row, col| Focus::Card { row, col };
        assert_eq!(kept_focus(at(0, 2), &old, &new), at(1, 0), "the played Item moved to the front");
        assert_eq!(kept_focus(at(0, 1), &old, &new), at(1, 1), "a gone Item keeps its place");
        assert_eq!(kept_focus(at(1, 1), &old, &new), at(2, 1), "View all");
        assert_eq!(kept_focus(Focus::Outside, &old, &new), Focus::Outside);
    }

    #[test]
    fn a_reload_keeps_the_top_section_in_view_when_a_section_above_it_comes_or_goes() {
        let old = [section("continue", &["a"]), section("new", &["x"]), section("top", &["y"])];
        let new = [section("new", &["x"]), section("added", &["z"]), section("top", &["y"])];
        let top = |at: ListOffset| (at.item_ix, at.offset_in_item);
        let at = |item_ix, offset: f32| ListOffset { item_ix, offset_in_item: px(offset) };
        assert_eq!(top(kept_top(at(1, 40.), &old, &new)), top(at(0, 40.)), "Continue Watching went");
        assert_eq!(top(kept_top(at(2, 10.), &old, &new)), top(at(2, 10.)), "the same place");
        assert_eq!(top(kept_top(at(0, 40.), &old, &new)), top(at(0, 0.)), "a gone section");
        assert_eq!(top(kept_top(at(3, 5.), &old, &new)), top(at(3, 5.)), "the status line");
    }

    #[test]
    fn a_card_shows_the_quality_badge_that_the_server_sends() {
        let dv =
            card(json!({"overlay_summary": {"resolution": "2160p", "hdr": "Dolby Vision", "video_codec": "hevc"}}));
        assert_eq!(card_badge(&dv), "4K DV");
        assert_eq!(card_badge(&card(json!({"overlay_summary": {"resolution": "1080p"}}))), "1080p");
        assert_eq!(card_badge(&card(json!({}))), "", "no overlay, no badge");
    }

    #[test]
    fn a_series_card_shows_its_status_and_network_and_a_started_card_the_time_left() {
        assert_eq!(short_line(&card(json!({}))), "2022");
        let series = json!({"type": "series", "show_status": "Returning Series", "networks": ["HBO", "Max"]});
        assert_eq!(short_line(&card(series)), "2022 · Returning · HBO");
        let ended = json!({"type": "series", "show_status": "ended", "networks": []});
        assert_eq!(short_line(&card(ended)), "2022 · Ended");
        assert_eq!(short_line(&card(json!({"type": "series", "networks": ["AMC"]}))), "2022 · AMC");
        let started = json!({"type": "episode", "series_title": "Silo", "season_number": 1, "episode_number": 1,
            "position_seconds": 600.0, "duration_seconds": 3780.0});
        assert_eq!(short_line(&card(started)), "Silo · S1 E1 · 53 min left");
        let unstarted = json!({"position_seconds": 0.0, "duration_seconds": 3780.0});
        assert_eq!(short_line(&card(unstarted)), "2022", "a card that is not started has no time left");
    }

    #[test]
    fn each_section_gets_the_rung_for_its_cards() {
        assert_eq!(rung("recently_added", false, 1.0), Rung::Small, "a 150 px poster");
        assert_eq!(rung("recently_added", false, 3.0), Rung::Medium);
        assert_eq!(rung("continue_watching", false, 1.0), Rung::Small, "a 290 px card fits 300 px");
        assert_eq!(rung("next_up", false, 2.0), Rung::Large, "a 580 px still needs 780 px");
        assert_eq!(rung("recently_added", true, 1.0), Rung::Large, "a 560 px hero");
    }

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
    #[gpui::test]
    fn page_reload_clear_and_release_drop_rendered_previews(cx: &mut gpui::TestAppContext) {
        use crate::pages::freshness::Reload;
        use base64::Engine as _;
        use gpui::AppContext as _;
        use http_client::{AsyncBody, FakeHttpClient, Response};
        use std::sync::{Arc, Mutex};

        struct Root(Option<gpui::Entity<super::Home>>);
        impl gpui::Render for Root {
            fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
                use gpui::{ParentElement as _, Styled as _};
                gpui::div().size_full().children(self.0.clone())
            }
        }
        let hash = base64::engine::general_purpose::STANDARD.encode(thumbhash::rgba_to_thumb_hash(
            16,
            16,
            &vec![255; 16 * 16 * 4],
        ));
        let mut sections: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/list_home_sections_ok.json")).unwrap();
        for section in sections["sections"].as_array_mut().unwrap() {
            for item in section["items"].as_array_mut().unwrap() {
                item["poster_thumbhash"] = hash.clone().into();
                item["backdrop_thumbhash"] = hash.clone().into();
                item["poster_url"] = serde_json::Value::Null;
                item["backdrop_url"] = serde_json::Value::Null;
                item["still_url"] = serde_json::Value::Null;
            }
        }
        let response = Arc::new(Mutex::new(sections.clone()));
        let reply = response.clone();
        let http = FakeHttpClient::create(move |req| {
            let body = if req.uri().path().ends_with("home/layout") {
                include_str!("../tests/fixtures/silo/get_home_layout_ok.json").to_string()
            } else {
                reply.lock().unwrap().to_string()
            };
            async move { Ok(Response::builder().status(200).body(AsyncBody::from(body)).unwrap()) }
        });
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-state")
            .join(uuid::Uuid::new_v4().to_string());
        cx.update(|cx| {
            gpui_omarchy::Theme::tokyo_night().apply(cx);
            crate::artwork::install(http.clone(), dir.clone(), cx);
        });
        let client = Arc::new(crate::api::Client::new(
            http,
            crate::api::Server {
                id: "test".into(),
                name: "test".into(),
                url: crate::api::Url::parse("https://silo.test/").unwrap(),
            },
            crate::api::Device { id: "test".into(), name: "test".into() },
            crate::api::Tokens { access_token: "test".into(), refresh_token: "test".into() },
        ));
        let (root, cx) = cx.add_window_view(|window, cx| {
            Root(Some(cx.new(|cx| super::Home::new(client, super::Rows::Home, window, cx))))
        });
        cx.run_until_parked();
        let home = root.read_with(cx, |root, _| root.0.clone().unwrap());
        let preview = |cx: &mut gpui::VisualTestContext| {
            home.read_with(cx, |home, _| Arc::downgrade(home.thumbs.iter().flatten().flatten().next().unwrap()))
        };
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let old = preview(cx);
        home.update(cx, |home, cx| Reload::reload(home, 0, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(old.upgrade().is_none(), "replacement releases the old rendered image");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let old = preview(cx);
        *response.lock().unwrap() = serde_json::json!({"sections":[]});
        home.update(cx, |home, cx| Reload::reload(home, 0, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(old.upgrade().is_none(), "an empty result releases its previews");
        *response.lock().unwrap() = sections;
        home.update(cx, |home, cx| Reload::reload(home, 0, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let old = preview(cx);
        drop(home);
        root.update(cx, |root, cx| {
            root.0 = None;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(old.upgrade().is_none(), "page release drops its rendered image");
        std::fs::remove_dir_all(dir).ok();
    }
}
