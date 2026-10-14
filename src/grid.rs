//! The poster grid of a catalog query. It loads the next page with the cursor when the focus or the scroll
//! comes near the end. The Library page uses it, and Search and the Person page can use it with their own query.

use std::sync::Arc;

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement, Render, RenderImage, ScrollStrategy, StatefulInteractiveElement as _, Styled, Subscription, Task,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::artwork::{self, ArtworkCache, Kind, Rung};
use crate::browse::Paging;
use crate::click::Clickable as _;
use crate::filter_panel::bar_pill;
use crate::home::{Open, Play, card};
use crate::keys::Key;
use crate::player::start_shuffle;

pub const POSTER_WIDTH: f32 = 150.;
pub const POSTER_HEIGHT: f32 = 225.;
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

/// The focus after a reload: the place in `new` of the Item at `focus` in `old`. When the Item is gone, the same
/// place, but not past the last card.
fn kept_focus(old: &[wire::CatalogItem], focus: usize, new: &[wire::CatalogItem]) -> usize {
    old.get(focus)
        .and_then(|item| new.iter().position(|i| i.content_id == item.content_id))
        .unwrap_or_else(|| focus.min(new.len().saturating_sub(1)))
}

/// The placeholder of the poster of `item`, from its thumbhash.
fn thumb(item: &wire::CatalogItem) -> Option<Arc<RenderImage>> {
    item.poster_thumbhash.as_deref().and_then(artwork::thumbhash)
}

/// One page of `source` from `cursor`. A `limit` of `None` takes the page size of the source.
async fn fetch(
    client: &Client,
    source: Source,
    cursor: Option<String>,
    limit: Option<u64>,
    image_size: Rung,
) -> Result<(Vec<wire::CatalogItem>, Option<i64>, Option<wire::PageInfo>), crate::api::Error> {
    match source {
        Source::Query(query) => {
            let limit = limit.and_then(std::num::NonZeroU64::new).or(query.limit);
            let query = wire::CatalogQuery { cursor, limit, ..*query };
            client.query_catalog(&query, image_size).await.map(|p| (p.items, Some(p.total), p.page))
        }
        Source::Collection { library_id, id } => client
            .library_collection_items(&library_id, &id, cursor.as_deref(), limit)
            .await
            .map(|p| (p.items, None, p.page)),
    }
}

/// The largest page that a reload asks for. The catalog query takes at most 100 Items.
const RELOAD_PAGE: usize = 100;

/// Where the Items of a grid come from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A catalog query, with the cursor of the first page.
    Query(Box<wire::CatalogQuery>),
    /// The Items of a collection of a Library.
    Collection { library_id: String, id: String },
}

impl From<wire::CatalogQuery> for Source {
    fn from(query: wire::CatalogQuery) -> Self {
        Source::Query(Box::new(query))
    }
}

pub struct Grid {
    client: Arc<Client>,
    source: Source,
    paging: Paging,
    thumbs: Vec<Option<Arc<RenderImage>>>,
    /// The focused card.
    focus: usize,
    /// The grid has the keyboard focus, so the focused card shows the focus ring.
    pub active: bool,
    columns: usize,
    /// The error of the last load, shown with Retry.
    error: Option<String>,
    /// The load of the next page or the reload. A reload drops the load of the next page.
    loading: Option<Task<()>>,
    /// A refresh came while the grid did not show. The grid reloads when it shows again.
    pub stale: bool,
    image_size: Rung,
    scroll: UniformListScrollHandle,
    artwork: Entity<ArtworkCache>,
}

impl EventEmitter<Play> for Grid {}
impl EventEmitter<Open> for Grid {}

impl Grid {
    pub fn new(client: Arc<Client>, source: impl Into<Source>, window: &Window, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.drop_thumbs(cx)).detach();
        let mut grid = Self {
            client,
            source: source.into(),
            paging: Paging::default(),
            thumbs: Vec::new(),
            focus: 0,
            active: false,
            columns: 1,
            error: None,
            loading: None,
            stale: false,
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
        self.source = query.into();
        self.paging = Paging::default();
        self.focus = 0;
        self.loading = None;
        self.stale = false;
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
        self.paging.items.is_empty() && self.paging.next_cursor().is_none()
    }

    /// Loads the next page, unless a load runs or the last page is in.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.paging.next_cursor() else {
            return;
        };
        if self.loading.is_some() {
            return;
        }
        self.error = None;
        let (client, image_size, source) = (self.client.clone(), self.image_size, self.source.clone());
        self.loading = Some(cx.spawn(async move |this, cx| {
            let result = fetch(&client, source, cursor, None, image_size).await;
            this.update(cx, |this, cx| {
                this.loading = None;
                match result {
                    Ok((items, total, page)) => {
                        this.thumbs.extend(items.iter().map(thumb));
                        this.paging.add(items, total, page);
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

    /// The grid shows again. A stale grid reloads.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.stale) {
            self.reload(cx);
        }
    }

    /// Loads the Items again from the first page, as many as the grid has, in pages of up to 100 Items. The new
    /// Items replace the old ones when all pages are in, so the scroll position stays. The focus stays on its Item.
    fn reload(&mut self, cx: &mut Context<Self>) {
        // A grid with less than one page has all its Items, or its first load failed. It reloads one full page.
        let want = self.paging.items.len().max(crate::browse::PAGE_SIZE as usize);
        let (client, image_size, source) = (self.client.clone(), self.image_size, self.source.clone());
        self.loading = Some(cx.spawn(async move |this, cx| {
            let mut paging = Paging::default();
            let result = async {
                while paging.items.len() < want
                    && let Some(cursor) = paging.next_cursor()
                {
                    let limit = (want - paging.items.len()).min(RELOAD_PAGE) as u64;
                    let (items, total, page) = fetch(&client, source.clone(), cursor, Some(limit), image_size).await?;
                    paging.add(items, total, page);
                }
                Ok::<_, crate::api::Error>(())
            }
            .await;
            this.update(cx, |this, cx| {
                this.loading = None;
                match result {
                    Ok(()) => {
                        this.drop_thumbs(cx);
                        this.thumbs = paging.items.iter().map(thumb).collect();
                        this.focus = kept_focus(&this.paging.items, this.focus, &paging.items);
                        this.paging = paging;
                        this.error = None;
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                    }
                    // The grid keeps what it shows, with no error. The Library window shows Offline or the sign-in
                    // screen.
                    Err(err) if !this.paging.items.is_empty() => eprintln!("relics: cannot reload the grid: {err}"),
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
        let focused = self.active && index == self.focus;
        let url = item
            .poster_url
            .as_deref()
            .filter(|url| !url.is_empty())
            .and_then(|url| self.client.server().url.join(url).ok());
        let thumb = self.thumbs.get(index).cloned().flatten();
        div()
            .id(("grid-card", index))
            .clickable()
            .flex_none()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focus = index;
                if let Some(item) = this.paging.items.get(index) {
                    cx.emit(Open(item.clone()));
                }
                cx.notify();
            }))
            .child(card(item, url.map(String::from), thumb, (POSTER_WIDTH, POSTER_HEIGHT), focused, cx.omarchy()))
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

/// A page with a title and the grid of one source: the Watchlist, the History, the full list of a row
/// ("View all"), or the Items of a collection. A collection page has a Shuffle pill above the grid.
pub struct GridPage {
    client: Arc<Client>,
    title: String,
    pub grid: Entity<Grid>,
    /// The id of the collection, which a Shuffle shuffles. `None` on a page with no Shuffle.
    collection: Option<String>,
    /// The Shuffle pill has the keyboard focus.
    in_bar: bool,
    /// The page has the keyboard focus.
    active: bool,
    /// Why the last Shuffle failed.
    shuffle_error: Option<String>,
    _shuffle: Task<()>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Play> for GridPage {}
impl EventEmitter<Open> for GridPage {}

impl GridPage {
    pub fn new(
        client: Arc<Client>,
        title: String,
        source: impl Into<Source>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = source.into();
        let collection = match &source {
            Source::Collection { id, .. } => Some(id.clone()),
            Source::Query(_) => None,
        };
        let grid = cx.new(|cx| Grid::new(client.clone(), source, window, cx));
        let events = vec![
            cx.subscribe(&grid, |_, _, Play(item), cx| cx.emit(Play(item.clone()))),
            cx.subscribe(&grid, |_, _, Open(item), cx| cx.emit(Open(item.clone()))),
        ];
        Self {
            client,
            title,
            grid,
            collection,
            in_bar: false,
            active: false,
            shuffle_error: None,
            _shuffle: Task::ready(()),
            _events: events,
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        self.sync(cx);
    }

    /// Gives the focus ring to the Shuffle pill or to the grid.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let grid = self.active && !self.in_bar;
        self.grid.update(cx, |g, cx| {
            g.active = grid;
            cx.notify();
        });
        cx.notify();
    }

    fn shuffle(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.collection.clone() else {
            return;
        };
        self.shuffle_error = None;
        self._shuffle = start_shuffle(self.client.clone(), "library_collection", id, cx, |this, err| {
            this.shuffle_error = Some(err)
        });
    }

    /// Handles a key. Returns false when the key leaves the page or the page does not use it. Up from the top
    /// row of the grid goes to the Shuffle pill, and Down from it goes back.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        if self.in_bar {
            match key {
                Key::Down => {
                    self.in_bar = false;
                    self.sync(cx);
                }
                Key::Other("enter") => self.shuffle(cx),
                _ => return false,
            }
            return true;
        }
        match self.grid.update(cx, |grid, cx| grid.key(key, cx)) {
            Move::To(_) => true,
            Move::OutTop if self.collection.is_some() => {
                self.in_bar = true;
                self.sync(cx);
                true
            }
            _ => false,
        }
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
        let shuffle = self.collection.as_ref().map(|_| {
            div()
                .px(px(40.))
                .pb(px(18.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(bar_pill("grid-shuffle", "Shuffle".into(), false, self.active && self.in_bar, &t).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.in_bar = true;
                        this.sync(cx);
                        this.shuffle(cx);
                    }),
                ))
                .children(
                    self.shuffle_error.clone().map(|err| div().text_size(px(13.)).text_color(t.danger).child(err)),
                )
        });
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
            .children(shuffle)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // A click in the grid moves the focus from the Shuffle pill to the grid.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.in_bar = false;
                            this.sync(cx);
                        }),
                    )
                    .child(content),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Move, columns, kept_focus, move_focus, near_end, row_cards};
    use crate::api::wire;
    use crate::keys::Key;

    fn items(ids: &[&str]) -> Vec<wire::CatalogItem> {
        ids.iter()
            .map(|id| {
                let item = serde_json::json!({"content_id": id, "type": "movie", "title": id, "genres": [], "keywords": [], "status": "available"});
                serde_json::from_value(item).unwrap()
            })
            .collect()
    }

    #[test]
    fn the_focus_stays_on_its_item_after_a_reload() {
        let old = items(&["a", "b", "c", "d"]);
        assert_eq!(kept_focus(&old, 2, &items(&["new", "a", "b", "c", "d"])), 3, "the Item moved one place on");
        assert_eq!(kept_focus(&old, 2, &items(&["a", "b", "d", "e"])), 2, "a gone Item keeps the place");
        assert_eq!(kept_focus(&old, 3, &items(&["a", "b"])), 1, "a gone Item stops at the last card");
        assert_eq!(kept_focus(&old, 1, &[]), 0, "an empty grid");
    }

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
