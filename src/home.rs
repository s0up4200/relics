//! Home: the sidebar, and the Home sections of the active Profile as rows that scroll
//! sideways. A Featured section shows as a hero above the other rows.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, FontWeight, ImageSource, InteractiveElement as _,
    IntoElement, KeyDownEvent, ObjectFit, ParentElement, Render, RenderImage, Resource, ScrollHandle,
    StatefulInteractiveElement as _, Styled, StyledImage as _, Subscription, Task, Window, div, img, px,
};
use gpui_omarchy::ActiveTheme;

use crate::api::{Client, Error, HomeSection, wire};
use crate::artwork::{self, ArtworkCache};

/// A request failed because the server refused the refresh token.
pub struct SignedOut;

/// The user pressed p on a card.
pub struct Play(pub wire::CatalogItem);

pub struct Home {
    client: Arc<Client>,
    profile_name: String,
    /// The sections in display order: the Featured sections first, then the others in layout order.
    sections: Vec<HomeSection>,
    status: Option<String>,
    focus: Focus,
    focus_handle: FocusHandle,
    page_scroll: ScrollHandle,
    row_scrolls: Vec<ScrollHandle>,
    /// The decoded Thumbhash of each card, in the order of `sections`.
    thumbs: Vec<Vec<Option<Arc<RenderImage>>>>,
    artwork: Entity<ArtworkCache>,
    /// The rung of the Artwork ladder of the loaded list.
    image_size: &'static str,
    _loading: Task<()>,
    _scale: Subscription,
}

impl EventEmitter<SignedOut> for Home {}
impl EventEmitter<Play> for Home {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Card { row: usize, col: usize },
}

/// Moves the focus for an arrow or hjkl key. `rows` holds the number of cards in each row.
pub fn move_focus(focus: Focus, key: &str, rows: &[usize]) -> Focus {
    let first_card_in = |mut range: Box<dyn Iterator<Item = usize>>, col: usize| {
        range.find(|&row| rows[row] > 0).map(|row| Focus::Card { row, col: col.min(rows[row] - 1) })
    };
    let moved = match (focus, key) {
        (Focus::Sidebar, "l" | "right") => first_card_in(Box::new(0..rows.len()), 0),
        (Focus::Card { col: 0, .. }, "h" | "left") => Some(Focus::Sidebar),
        (Focus::Card { row, col }, "h" | "left") => Some(Focus::Card { row, col: col - 1 }),
        (Focus::Card { row, col }, "l" | "right") if col + 1 < rows[row] => Some(Focus::Card { row, col: col + 1 }),
        (Focus::Card { row, col }, "j" | "down") => first_card_in(Box::new(row + 1..rows.len()), col),
        (Focus::Card { row, col }, "k" | "up") => first_card_in(Box::new((0..row).rev()), col),
        _ => None,
    };
    moved.unwrap_or(focus)
}

impl Home {
    pub fn new(client: Arc<Client>, profile_name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        // A larger scale loads the list again only when the rung goes up.
        let scale = cx.observe_window_bounds(window, |this, window, cx| {
            let size = artwork::image_size(artwork::HOME_CARD_WIDTH, window.scale_factor());
            if artwork::is_larger(size, this.image_size) {
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
            profile_name,
            sections: Vec::new(),
            thumbs: Vec::new(),
            artwork: artwork::global(cx),
            image_size: artwork::image_size(artwork::HOME_CARD_WIDTH, window.scale_factor()),
            status: Some("Loading…".into()),
            focus: Focus::Sidebar,
            focus_handle,
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
        let image_size = self.image_size;
        self._loading = cx.spawn(async move |this, cx| {
            let result = client.home(image_size).await;
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
                        let rows = this.row_lengths();
                        let lost = match this.focus {
                            Focus::Card { row, col } => rows.get(row).is_none_or(|&cards| col >= cards),
                            Focus::Sidebar => first_load,
                        };
                        if lost {
                            this.focus = move_focus(Focus::Sidebar, "l", &rows);
                        }
                        this.artwork.update(cx, |artwork, _| artwork.retry_failed());
                    }
                    Err(Error::SignedOut) => cx.emit(SignedOut),
                    Err(err) => this.status = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn row_lengths(&self) -> Vec<usize> {
        self.sections.iter().map(|s| s.items.len()).collect()
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let (Focus::Card { row, col }, "p") = (self.focus, event.keystroke.key.as_str()) {
            cx.emit(Play(self.sections[row].items[col].clone()));
            cx.stop_propagation();
            return;
        }
        let focus = move_focus(self.focus, event.keystroke.key.as_str(), &self.row_lengths());
        if focus == self.focus {
            return;
        }
        if let Focus::Card { row, col } = focus {
            self.page_scroll.scroll_to_item(row);
            self.row_scrolls[row].scroll_to_item(col);
        }
        self.focus = focus;
        cx.stop_propagation();
        cx.notify();
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let selected = self.focus == Focus::Sidebar;
        div()
            .w(px(240.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .p(px(14.))
            .gap(px(4.))
            .bg(t.inset)
            .border_r_1()
            .border_color(t.divider())
            .child(
                div()
                    .px(px(10.))
                    .pb(px(14.))
                    .text_size(px(22.))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(t.bright)
                    .child("silo"),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(t.selected_fill())
                    .text_color(t.bright)
                    .border_2()
                    .border_color(if selected { t.accent } else { gpui::transparent_black() })
                    .child("Home"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .px(px(10.))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(14.)).text_color(t.bright).child(self.profile_name.clone()))
                    .child(div().text_size(px(11.)).text_color(t.secondary).child(self.client.server().name.clone())),
            )
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
            let focused = self.focus == Focus::Card { row: index, col };
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
            if wide && let Some(progress) = progress(item) {
                art = art.child(
                    div().absolute().inset_0().flex().flex_col().justify_end().p(px(8.)).child(
                        div()
                            .h(px(3.))
                            .rounded_full()
                            .bg(gpui::hsla(0., 0., 0., 0.45))
                            .child(div().h_full().w(gpui::relative(progress)).rounded_full().bg(t.accent)),
                    ),
                );
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
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .px(px(40.))
                    .flex()
                    .items_baseline()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.bright)
                            .child(section.title.clone()),
                    )
                    .children(
                        section.view_all.then(|| div().text_size(px(11.)).text_color(t.secondary).child("VIEW ALL")),
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
                    .children(cards),
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
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .bg(t.background)
            .text_color(t.foreground)
            .font_family(t.font.clone())
            .text_size(px(14.))
            .child(self.sidebar(cx))
            .child(
                div()
                    .id("home-page")
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(36.))
                    .pt(px(32.))
                    .pb(px(80.))
                    .overflow_y_scroll()
                    .track_scroll(&self.page_scroll)
                    .children(rows)
                    .children(
                        self.status.clone().map(|status| div().px(px(40.)).text_color(t.secondary).child(status)),
                    ),
            )
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
fn artwork_image(url: Option<String>, thumb: Option<Arc<RenderImage>>) -> AnyElement {
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
fn progress(item: &wire::CatalogItem) -> Option<f32> {
    let (position, duration) = (item.position_seconds?, item.duration_seconds?);
    (position > 0. && duration > 0.).then(|| (position / duration).min(1.) as f32)
}

/// The line below the title: the series with the season and episode, or the year.
fn short_line(item: &wire::CatalogItem) -> String {
    match (&item.series_title, item.season_number, item.episode_number) {
        (Some(series), Some(season), Some(episode)) => format!("{series} · S{season} E{episode}"),
        _ => item.year.map(|year| year.to_string()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Focus, move_focus};

    #[test]
    fn hjkl_and_arrows_move_between_the_sidebar_and_the_cards() {
        let rows = [3, 0, 2];
        let card = |row, col| Focus::Card { row, col };
        assert_eq!(move_focus(Focus::Sidebar, "l", &rows), card(0, 0));
        assert_eq!(move_focus(card(0, 0), "h", &rows), Focus::Sidebar);
        assert_eq!(move_focus(card(0, 1), "right", &rows), card(0, 2));
        assert_eq!(move_focus(card(0, 2), "l", &rows), card(0, 2), "the last card stays");
        assert_eq!(move_focus(card(0, 2), "j", &rows), card(2, 1), "an empty row is skipped");
        assert_eq!(move_focus(card(2, 1), "up", &rows), card(0, 1));
        assert_eq!(move_focus(card(0, 1), "k", &rows), card(0, 1));
        assert_eq!(move_focus(Focus::Sidebar, "j", &rows), Focus::Sidebar);
        assert_eq!(move_focus(Focus::Sidebar, "l", &[]), Focus::Sidebar);
    }
}
