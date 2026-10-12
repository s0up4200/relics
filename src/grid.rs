//! The poster grid of a catalog query. It loads the next page with the cursor when the focus or the scroll
//! comes near the end. The Library page uses it, and Search and the Person grid can use it with their own query.

use std::sync::Arc;

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement,
    Render, RenderImage, ScrollStrategy, StatefulInteractiveElement as _, Styled, Subscription, Task,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::artwork::{self, ArtworkCache, Kind, Rung};
use crate::browse::Paging;
use crate::home::{Open, Play, short_line};
use crate::keys::Key;

pub const POSTER_WIDTH: f32 = 150.;
const POSTER_HEIGHT: f32 = 225.;
const GAP: f32 = 12.;

/// The number of grid columns in `width` pixels.
pub fn columns(width: f32) -> usize {
    (((width + GAP) / (POSTER_WIDTH + GAP)).floor() as usize).max(1)
}

/// Where an arrow or hjkl key takes the focus in a grid of `len` cards in `columns` columns.
#[derive(Debug, PartialEq, Eq)]
pub enum Move {
    To(usize),
    /// The key leaves the grid at the top (k or Up on the top row) or at the left (h or Left in the first
    /// column).
    OutTop,
    OutLeft,
    /// The key does nothing in the grid.
    Stay,
}

/// The indexes of the cards in grid row `row`.
pub fn row_cards(row: usize, columns: usize, len: usize) -> std::ops::Range<usize> {
    (row * columns).min(len)..((row + 1) * columns).min(len)
}

/// The scroll is within three posters of the end, so the next page must load. `scroll_top` and `max` are the
/// scrolled and the largest scroll distance.
fn near_end(scroll_top: f32, max: f32) -> bool {
    scroll_top > max - POSTER_HEIGHT * 3.
}

pub fn move_focus(index: usize, key: Key, len: usize, columns: usize) -> Move {
    match key {
        Key::Left if index.is_multiple_of(columns) => Move::OutLeft,
        Key::Left => Move::To(index - 1),
        Key::Right if index % columns + 1 < columns && index + 1 < len => Move::To(index + 1),
        Key::Up if index < columns => Move::OutTop,
        Key::Up => Move::To(index - columns),
        // Down from a column that the last row does not reach goes to the last card.
        Key::Down if index / columns < (len.max(1) - 1) / columns => Move::To((index + columns).min(len - 1)),
        _ => Move::Stay,
    }
}

pub struct Grid {
    client: Arc<Client>,
    /// The query of the first page.
    query: wire::CatalogQuery,
    paging: Paging,
    thumbs: Vec<Option<Arc<RenderImage>>>,
    /// The focused card.
    focus: usize,
    /// The grid has the keyboard focus, so the focused card shows the focus ring.
    pub active: bool,
    columns: usize,
    /// The error of the last load, shown with Retry.
    error: Option<String>,
    loading: Option<Task<()>>,
    image_size: Rung,
    scroll: UniformListScrollHandle,
    artwork: Entity<ArtworkCache>,
}

impl EventEmitter<Play> for Grid {}
impl EventEmitter<Open> for Grid {}

impl Grid {
    pub fn new(client: Arc<Client>, query: wire::CatalogQuery, window: &Window, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.drop_thumbs(cx)).detach();
        let mut grid = Self {
            client,
            query,
            paging: Paging::default(),
            thumbs: Vec::new(),
            focus: 0,
            active: false,
            columns: 1,
            error: None,
            loading: None,
            image_size: Rung::fit(Kind::Poster, POSTER_WIDTH, window.scale_factor()),
            scroll: UniformListScrollHandle::new(),
            artwork: artwork::global(cx),
        };
        grid.load_more(cx);
        grid
    }

    /// Starts again with a new query, for example after a filter change.
    pub fn set_query(&mut self, query: wire::CatalogQuery, cx: &mut Context<Self>) {
        self.drop_thumbs(cx);
        self.query = query;
        self.paging = Paging::default();
        self.focus = 0;
        self.loading = None;
        self.scroll.0.borrow().base_handle.set_offset(gpui::Point::default());
        self.load_more(cx);
    }

    fn drop_thumbs(&mut self, cx: &mut gpui::App) {
        for thumb in self.thumbs.drain(..).flatten() {
            cx.drop_image(thumb, None);
        }
    }

    /// The number of Items in the whole result, after the first page came.
    pub fn total(&self) -> Option<i64> {
        self.paging.total
    }

    /// The query loaded, and nothing matches it.
    pub fn is_empty(&self) -> bool {
        self.paging.items.is_empty() && self.paging.next_query(&self.query).is_none()
    }

    /// Loads the next page, unless a load runs or the last page is in.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.paging.next_query(&self.query) else {
            return;
        };
        if self.loading.is_some() {
            return;
        }
        self.error = None;
        let (client, image_size) = (self.client.clone(), self.image_size);
        self.loading = Some(cx.spawn(async move |this, cx| {
            let result = client.query_catalog(&query, image_size).await;
            this.update(cx, |this, cx| {
                this.loading = None;
                match result {
                    Ok(page) => {
                        this.thumbs.extend(
                            page.items.iter().map(|i| i.poster_thumbhash.as_deref().and_then(artwork::thumbhash)),
                        );
                        this.paging.add(page);
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                    }
                    // The Library window shows Offline or the sign-in screen. The grid keeps what it shows.
                    Err(err) => this.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn near_end(&self) -> bool {
        let scroll = &self.scroll.0.borrow().base_handle;
        near_end(-f32::from(scroll.offset().y), f32::from(scroll.max_offset().y))
    }

    /// Handles a key. Returns `Move::OutTop` or `Move::OutLeft` when the key leaves the grid, and `Move::Stay`
    /// when the grid does not use it.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> Move {
        let items = &self.paging.items;
        let Some(item) = items.get(self.focus) else {
            return match key {
                Key::Up => Move::OutTop,
                Key::Left => Move::OutLeft,
                _ => Move::Stay,
            };
        };
        match key {
            Key::Other("p") => cx.emit(Play(item.into())),
            Key::Other("enter") => cx.emit(Open(item.clone())),
            _ => match move_focus(self.focus, key, items.len(), self.columns) {
                Move::To(index) => {
                    self.focus = index;
                    self.scroll.scroll_to_item(index / self.columns, ScrollStrategy::Nearest);
                    if index + 2 * self.columns >= items.len() {
                        self.load_more(cx);
                    }
                    cx.notify();
                }
                other => return other,
            },
        }
        Move::To(self.focus)
    }

    fn card(&self, index: usize, item: &wire::CatalogItem, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let focused = self.active && index == self.focus;
        let url = item
            .poster_url
            .as_deref()
            .filter(|url| !url.is_empty())
            .and_then(|url| self.client.server().url.join(url).ok());
        let thumb = self.thumbs.get(index).cloned().flatten();
        div()
            .id(("grid-card", index))
            .w(px(POSTER_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focus = index;
                if let Some(item) = this.paging.items.get(index) {
                    cx.emit(Open(item.clone()));
                }
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .w(px(POSTER_WIDTH))
                    .h(px(POSTER_HEIGHT))
                    .rounded(px(12.))
                    .overflow_hidden()
                    .bg(t.surface)
                    .border_2()
                    .border_color(if focused { t.accent } else { gpui::transparent_black() })
                    .child(div().absolute().inset_0().child(artwork::image(
                        url.map(String::from),
                        thumb,
                        POSTER_WIDTH,
                    ))),
            )
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
}

impl Render for Grid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        // The grid fills the page between the sidebar and the 40 px page margins.
        let viewport = window.viewport_size().width.to_f64() as f32;
        // The next page loads in the rung for the current scale. The loaded pages keep their rung.
        self.image_size = self.image_size.max(Rung::fit(Kind::Poster, POSTER_WIDTH, window.scale_factor()));
        self.columns = columns(viewport - crate::pages::sidebar_width(viewport, false) - 80.);
        let len = self.paging.items.len();
        let status = match (&self.error, &self.loading) {
            (Some(error), _) => Some(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .text_color(t.danger)
                    .child(error.clone())
                    .child(
                        button("grid-retry", "Retry", ButtonVariant::Secondary, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
                    )
                    .into_any_element(),
            ),
            (None, Some(_)) => Some(div().w_full().text_color(t.secondary).child("Loading…").into_any_element()),
            _ => None,
        };
        // The list draws only the visible rows. Each row holds `columns` cards and the gap below it.
        let rows = uniform_list(
            "grid",
            len.div_ceil(self.columns),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let cards = row_cards(row, this.columns, this.paging.items.len())
                            .map(|i| this.card(i, &this.paging.items[i], cx));
                        div().flex().gap(px(GAP)).pb(px(GAP)).children(cards)
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .on_scroll_wheel(cx.listener(|this, _, _, cx| {
            if this.near_end() {
                this.load_more(cx);
            }
        }));
        // An empty list takes no height, so that "Loading…" shows at the top.
        let rows = if len > 0 { rows.flex_1() } else { rows };
        div().size_full().flex().flex_col().child(rows).children(status.map(|s| div().pb(px(80.)).child(s)))
    }
}

/// A page with a title and the grid of one catalog query: the titles of a Person, the Watchlist, the History,
/// or the full list of a row ("View all").
pub struct GridPage {
    title: String,
    grid: Entity<Grid>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for GridPage {}
impl EventEmitter<Open> for GridPage {}

impl GridPage {
    pub fn new(
        client: Arc<Client>,
        title: String,
        query: wire::CatalogQuery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let grid = cx.new(|cx| Grid::new(client, query, window, cx));
        let events = vec![
            cx.subscribe(&grid, |_, _, Play(item), cx| cx.emit(Play(item.clone()))),
            cx.subscribe(&grid, |_, _, Open(item), cx| cx.emit(Open(item.clone()))),
        ];
        Self { title, grid, _events: events }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.grid.update(cx, |grid, cx| {
            grid.active = active;
            cx.notify();
        });
    }

    /// Handles a key. Returns false when the key leaves the grid or the grid does not use it.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        matches!(self.grid.update(cx, |grid, cx| grid.key(key, cx)), Move::To(_))
    }
}

impl Render for GridPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let grid = self.grid.read(cx);
        let content = if grid.is_empty() {
            div().px(px(40.)).text_color(t.secondary).child("No titles.").into_any_element()
        } else {
            div().size_full().px(px(40.)).child(self.grid.clone()).into_any_element()
        };
        let count = grid.total().map(|total| format!("{total} titles"));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(40.))
                    .pt(px(28.))
                    .pb(px(18.))
                    .flex()
                    .items_end()
                    .gap(px(16.))
                    .child(
                        div()
                            .text_size(px(34.))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(t.bright)
                            .child(self.title.clone()),
                    )
                    .children(
                        count.map(|count| div().pb(px(6.)).text_size(px(13.)).text_color(t.secondary).child(count)),
                    ),
            )
            .child(div().flex_1().min_h_0().child(content))
    }
}

#[cfg(test)]
mod tests {
    use super::{Move, columns, move_focus, near_end, row_cards};
    use crate::keys::Key;

    #[test]
    fn the_directions_move_in_the_grid_and_leave_it_at_the_top_and_the_left() {
        // 7 cards in 3 columns: rows [0 1 2] [3 4 5] [6].
        assert_eq!(move_focus(0, Key::Right, 7, 3), Move::To(1));
        assert_eq!(move_focus(2, Key::Right, 7, 3), Move::Stay, "the end of a row stays");
        assert_eq!(move_focus(6, Key::Right, 7, 3), Move::Stay, "the last card stays");
        assert_eq!(move_focus(4, Key::Left, 7, 3), Move::To(3));
        assert_eq!(move_focus(3, Key::Left, 7, 3), Move::OutLeft);
        assert_eq!(move_focus(1, Key::Down, 7, 3), Move::To(4));
        assert_eq!(move_focus(5, Key::Down, 7, 3), Move::To(6), "a short last row takes the last card");
        assert_eq!(move_focus(6, Key::Down, 7, 3), Move::Stay);
        assert_eq!(move_focus(4, Key::Up, 7, 3), Move::To(1));
        assert_eq!(move_focus(1, Key::Up, 7, 3), Move::OutTop);
        assert_eq!(move_focus(0, Key::Other("x"), 7, 3), Move::Stay);
    }

    #[test]
    fn each_row_holds_the_cards_of_its_columns() {
        // 7 cards in 3 columns: rows [0 1 2] [3 4 5] [6].
        assert_eq!(row_cards(0, 3, 7), 0..3);
        assert_eq!(row_cards(1, 3, 7), 3..6);
        assert_eq!(row_cards(2, 3, 7), 6..7, "a short last row");
        assert_eq!(row_cards(3, 3, 7), 7..7, "past the end");
    }

    #[test]
    fn the_next_page_loads_within_three_posters_of_the_end() {
        assert!(!near_end(0., 1000.));
        assert!(!near_end(300., 1000.));
        assert!(near_end(400., 1000.));
        assert!(near_end(0., 0.), "a list that does not scroll is at its end");
    }

    #[test]
    fn the_columns_fill_the_width() {
        assert_eq!(columns(150.), 1);
        assert_eq!(columns(0.), 1);
        assert_eq!(columns(1000.), 6);
    }
}
