//! The poster grid of a catalog query. It loads the next page with the cursor when the focus or the scroll
//! comes near the end. The Library page uses it, and Search and the Person grid can use it with their own query.

use std::sync::Arc;

use gpui::{
    Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Render,
    RenderImage, ScrollHandle, StatefulInteractiveElement as _, Styled, Task, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::artwork::{self, ArtworkCache};
use crate::browse::Paging;
use crate::home::{Open, Play, artwork_image, short_line};

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

pub fn move_focus(index: usize, key: &str, len: usize, columns: usize) -> Move {
    match key {
        "h" | "left" if index.is_multiple_of(columns) => Move::OutLeft,
        "h" | "left" => Move::To(index - 1),
        "l" | "right" if index % columns + 1 < columns && index + 1 < len => Move::To(index + 1),
        "k" | "up" if index < columns => Move::OutTop,
        "k" | "up" => Move::To(index - columns),
        // Down from a column that the last row does not reach goes to the last card.
        "j" | "down" if index / columns < (len.max(1) - 1) / columns => Move::To((index + columns).min(len - 1)),
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
    image_size: &'static str,
    scroll: ScrollHandle,
    artwork: Entity<ArtworkCache>,
}

impl EventEmitter<Play> for Grid {}
impl EventEmitter<Open> for Grid {}

impl Grid {
    pub fn new(client: Arc<Client>, query: wire::CatalogQuery, window: &Window, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.drop_thumbs(cx)).detach();
        // ponytail: the rung is read once; a move to a larger scale keeps it until the query changes.
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
            image_size: artwork::image_size(POSTER_WIDTH, window.scale_factor()),
            scroll: ScrollHandle::new(),
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
        self.scroll.set_offset(gpui::Point::default());
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
        -self.scroll.offset().y > self.scroll.max_offset().y - px(POSTER_HEIGHT * 3.)
    }

    /// Handles a key. Returns `Move::OutTop` or `Move::OutLeft` when the key leaves the grid, and `Move::Stay`
    /// when the grid does not use it.
    pub fn key(&mut self, key: &str, cx: &mut Context<Self>) -> Move {
        let items = &self.paging.items;
        let Some(item) = items.get(self.focus) else {
            return match key {
                "k" | "up" => Move::OutTop,
                "h" | "left" => Move::OutLeft,
                _ => Move::Stay,
            };
        };
        match key {
            "p" => cx.emit(Play(item.clone())),
            "enter" => cx.emit(Open(item.clone())),
            _ => match move_focus(self.focus, key, items.len(), self.columns) {
                Move::To(index) => {
                    self.focus = index;
                    self.scroll.scroll_to_item(index);
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
                    .child(div().absolute().inset_0().child(artwork_image(url.map(String::from), thumb))),
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
        self.columns = columns(viewport - crate::pages::sidebar_width(viewport) - 80.);
        let cards: Vec<_> = self.paging.items.iter().enumerate().map(|(i, item)| self.card(i, item, cx)).collect();
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
        div()
            .image_cache(self.artwork.clone())
            .id("grid")
            .size_full()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(GAP))
            .pb(px(80.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                if this.near_end() {
                    this.load_more(cx);
                }
            }))
            .children(cards)
            .children(status)
    }
}

#[cfg(test)]
mod tests {
    use super::{Move, columns, move_focus};

    #[test]
    fn hjkl_and_arrows_move_in_the_grid_and_leave_it_at_the_top_and_the_left() {
        // 7 cards in 3 columns: rows [0 1 2] [3 4 5] [6].
        assert_eq!(move_focus(0, "l", 7, 3), Move::To(1));
        assert_eq!(move_focus(2, "right", 7, 3), Move::Stay, "the end of a row stays");
        assert_eq!(move_focus(6, "l", 7, 3), Move::Stay, "the last card stays");
        assert_eq!(move_focus(4, "h", 7, 3), Move::To(3));
        assert_eq!(move_focus(3, "left", 7, 3), Move::OutLeft);
        assert_eq!(move_focus(1, "j", 7, 3), Move::To(4));
        assert_eq!(move_focus(5, "down", 7, 3), Move::To(6), "a short last row takes the last card");
        assert_eq!(move_focus(6, "j", 7, 3), Move::Stay);
        assert_eq!(move_focus(4, "k", 7, 3), Move::To(1));
        assert_eq!(move_focus(1, "up", 7, 3), Move::OutTop);
        assert_eq!(move_focus(0, "x", 7, 3), Move::Stay);
    }

    #[test]
    fn the_columns_fill_the_width() {
        assert_eq!(columns(150.), 1);
        assert_eq!(columns(0.), 1);
        assert_eq!(columns(1000.), 6);
    }
}
