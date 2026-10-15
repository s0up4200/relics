//! Home: the Home sections of the active Profile as rows that scroll sideways. A Featured section shows
//! as a hero above the other rows. The Recommended tab of a Library uses the same view for its rows.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ListAlignment,
    ListOffset, ListState, ParentElement, Render, RenderImage, ScrollHandle, StatefulInteractiveElement as _, Styled,
    Subscription, Task, Window, div, list, prelude::FluentBuilder as _, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, Theme, button};

use crate::api::{Client, HomeSection, wire};
use crate::artwork::{self, ArtworkCache, Kind, Rung};
use crate::click::Clickable as _;
use crate::keys::Key;
use crate::pages::freshness::{Deps, Fresh, Reload};
use crate::sideways::Row;

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
    fresh: Fresh,
    /// The page list: one item for each section, and the status line as the last item.
    page: ListState,
    row_scrolls: Vec<ScrollHandle>,
    /// The decoded Thumbhash of each card, in the order of `sections`.
    thumbs: Vec<Vec<Option<Arc<RenderImage>>>>,
    artwork: Entity<ArtworkCache>,
    /// The window scale of the loaded list. Each section has the rung for its cards at this scale.
    scale: f32,
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

/// The focus after a reload from `old` to `new` sections. A focused card moves with its section and Item, and "View
/// all" with its section. When the Item is gone, the focus keeps its place in the row.
pub fn kept_focus(focus: Focus, old: &[HomeSection], new: &[HomeSection]) -> Focus {
    let Focus::Card { row, col } = focus else {
        return focus;
    };
    let Some(section) = old.get(row) else {
        return focus;
    };
    let Some(row) = new.iter().position(|s| s.id == section.id) else {
        return focus;
    };
    let col = match section.items.get(col) {
        Some(item) => new[row].items.iter().position(|i| i.content_id == item.content_id).unwrap_or(col),
        None => new[row].items.len(),
    };
    Focus::Card { row, col }
}

/// The scroll position of the page after a reload of its sections from `old` to `new`. The section at the top of
/// the view stays there, also when a section above it comes or goes. When that section goes, the section that takes
/// its place shows from its start.
pub fn kept_top(top: ListOffset, old: &[HomeSection], new: &[HomeSection]) -> ListOffset {
    let Some(section) = old.get(top.item_ix) else {
        return top;
    };
    match new.iter().position(|s| s.id == section.id) {
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
            scale: window.scale_factor(),
            status: Some("Loading…".into()),
            failed: false,
            focus: Focus::Outside,
            active: false,
            fresh: Fresh::default(),
            // The overdraw measures the rows near the viewport, so that the focus can scroll to them.
            page: ListState::new(1, ListAlignment::Top, px(800.)),
            row_scrolls: Vec::new(),
            _loading: Task::ready(()),
            _scale: scale,
        };
        home.load(cx);
        home
    }

    /// Loads the Home sections, with the artwork URLs of each section in the rung for its cards.
    fn load(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let (scale, rows) = (self.scale, self.rows.clone());
        let ticket = self.fresh.begin();
        self._loading = cx.spawn(async move |this, cx| {
            let size = |entry: &wire::SectionLayoutEntry| rung(&entry.section_type, entry.featured, scale);
            let result = match rows {
                Rows::Home => client.home(size).await,
                Rows::Library(id) => client.library_rows(&id, size).await,
            };
            this.update(cx, |this, cx| {
                match result {
                    _ if !this.fresh.end(ticket) => {}
                    Ok(mut sections) => {
                        sections.sort_by_key(|s| !s.featured);
                        let first_load = this.sections.is_empty();
                        let thumbs = sections.iter().map(|s| s.items.iter().map(|i| thumb(s, i)).collect()).collect();
                        for old in std::mem::replace(&mut this.thumbs, thumbs).into_iter().flatten().flatten() {
                            cx.drop_image(old, None);
                        }
                        // A reload keeps the scroll position of each section and the focus.
                        this.row_scrolls = sections
                            .iter()
                            .map(|section| {
                                let old = this.sections.iter().position(|s| s.id == section.id);
                                old.and_then(|i| this.row_scrolls.get(i).cloned()).unwrap_or_else(ScrollHandle::new)
                            })
                            .collect();
                        let focus = kept_focus(this.focus, &this.sections, &sections);
                        let old_top = this.page.logical_scroll_top();
                        let top = kept_top(old_top, &this.sections, &sections);
                        if let (Focus::Card { row, col }, true) = (focus, focus != this.focus) {
                            cards(&sections[row]).reveal(&this.row_scrolls[row], col);
                        }
                        this.focus = focus;
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
                        let count = this.sections.len() + 1;
                        if this.page.item_count() == count
                            && (top.item_ix, top.offset_in_item) == (old_top.item_ix, old_top.offset_in_item)
                        {
                            this.page.remeasure();
                        } else {
                            this.page.reset(count);
                            this.page.scroll_to(top);
                        }
                    }
                    // The page keeps what it shows, with no error. The Library window shows Offline or the sign-in
                    // screen.
                    Err(err) if !this.sections.is_empty() => eprintln!("relics: cannot reload the rows: {err}"),
                    Err(err) => {
                        this.status = Some(err.to_string());
                        this.failed = true;
                        let status = this.sections.len();
                        this.page.remeasure_items(status..status + 1);
                    }
                }
                if this.fresh.ready() {
                    this.load(cx);
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
            .into_any_element()
    }
}

impl Render for Home {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
}
