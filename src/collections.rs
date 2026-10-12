//! The collections of a Library (its Collections tab) or of the Server (the Collections page), as rows of
//! poster cards that scroll sideways: one row for each collection group of a Library, or for each Library of the
//! Server. Enter or a click opens the Items of a collection in a grid.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Render,
    ScrollHandle, StatefulInteractiveElement as _, Styled, Task, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::click::Clickable as _;
use crate::grid::{POSTER_HEIGHT, POSTER_WIDTH};
use crate::home::{Focus, art, move_focus};
use crate::keys::Key;

/// Open the grid of the Items of a collection.
#[derive(Clone)]
pub struct OpenCollection {
    pub title: String,
    pub library_id: String,
    pub id: String,
}

/// Where the collections come from.
#[derive(Debug, Clone)]
pub enum Scope {
    /// The Collections tab of the Library with this id.
    Library(String),
    /// The collections of the Server, for each Library.
    Server,
}

/// One row: a collection group of a Library, or the collections of one Library of the Server.
#[derive(Debug)]
pub struct Group {
    pub title: String,
    /// The Library of the collections, for the call that lists their Items.
    pub library_id: String,
    pub cards: Vec<wire::LibraryCollectionCard>,
}

/// The rows of the Collections tab of a Library: the groups and the ungrouped collections, in their sort order.
pub fn library_groups(tab: wire::LibraryCollectionTab) -> Vec<Group> {
    let library_id = tab.library_id.to_string();
    let mut rows: Vec<_> = tab.groups.into_iter().map(|g| (g.sort_order, g.name, g.collections)).collect();
    rows.extend(tab.ungrouped.map(|u| (u.sort_order, "Collections".to_string(), u.collections)));
    rows.sort_by_key(|row| row.0);
    rows.into_iter()
        .filter(|row| !row.2.is_empty())
        .map(|(_, title, cards)| Group { title, library_id: library_id.clone(), cards })
        .collect()
}

/// The rows of the Collections page: one for each Library of the Server that has collections.
pub fn server_groups(libraries: Vec<wire::ServerCollectionLibrary>) -> Vec<Group> {
    libraries
        .into_iter()
        .filter(|l| !l.collections.is_empty())
        .map(|l| Group { title: l.library_name, library_id: l.library_id.to_string(), cards: l.collections })
        .collect()
}

pub struct Collections {
    client: Arc<Client>,
    scope: Scope,
    groups: Vec<Group>,
    /// "Loading…", or the error of the last load (shown with Retry).
    status: Option<String>,
    failed: bool,
    focus: Focus,
    /// The view has the keyboard focus, so the focused card shows the focus ring.
    pub active: bool,
    /// The page scroll, with one child for each row.
    scroll: ScrollHandle,
    row_scrolls: Vec<ScrollHandle>,
    _loading: Task<()>,
}

impl EventEmitter<OpenCollection> for Collections {}

impl Collections {
    pub fn new(client: Arc<Client>, scope: Scope, cx: &mut Context<Self>) -> Self {
        let mut collections = Self {
            client,
            scope,
            groups: Vec::new(),
            status: Some("Loading…".into()),
            failed: false,
            focus: Focus::Outside,
            active: false,
            scroll: ScrollHandle::new(),
            row_scrolls: Vec::new(),
            _loading: Task::ready(()),
        };
        collections.load(cx);
        collections
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let (client, scope) = (self.client.clone(), self.scope.clone());
        self.status = Some("Loading…".into());
        self.failed = false;
        self._loading = cx.spawn(async move |this, cx| {
            let result = match scope {
                Scope::Library(id) => client.library_collections(&id).await.map(library_groups),
                Scope::Server => client.server_collections().await.map(server_groups),
            };
            this.update(cx, |this, cx| {
                match result {
                    Ok(groups) => {
                        this.status = groups.is_empty().then(|| "No collections.".into());
                        this.row_scrolls = groups.iter().map(|_| ScrollHandle::new()).collect();
                        this.groups = groups;
                        this.focus = move_focus(Focus::Outside, Key::Right, &this.row_lengths());
                    }
                    // The view keeps what it shows. The Library window shows Offline or the sign-in screen.
                    Err(err) => {
                        this.status = Some(err.to_string());
                        this.failed = true;
                    }
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn row_lengths(&self) -> Vec<usize> {
        self.groups.iter().map(|g| g.cards.len()).collect()
    }

    /// Handles a key. Returns false when the view does not use it, for example Up on the top row.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        if let (Focus::Card { row, col }, Key::Other("enter")) = (self.focus, key) {
            let group = &self.groups[row];
            let card = &group.cards[col];
            cx.emit(OpenCollection {
                title: card.title.clone(),
                library_id: group.library_id.clone(),
                id: card.id.clone(),
            });
            return true;
        }
        let focus = move_focus(self.focus, key, &self.row_lengths());
        let Focus::Card { row, col } = focus else {
            return false;
        };
        if focus == self.focus {
            return false;
        }
        // The Collections page has its title as the first child of the scroll.
        self.scroll.scroll_to_item(row + usize::from(matches!(self.scope, Scope::Server)));
        self.row_scrolls[row].scroll_to_item(col);
        self.focus = focus;
        cx.notify();
        true
    }

    fn card(&self, row: usize, col: usize, card: &wire::LibraryCollectionCard, cx: &Context<Self>) -> AnyElement {
        let t = cx.omarchy();
        let focused = self.active && self.focus == Focus::Card { row, col };
        let url = Some(card.poster_url.as_str())
            .filter(|url| !url.is_empty())
            .and_then(|url| self.client.server().url.join(url).ok())
            .map(String::from);
        let count = match card.item_count {
            1 => "1 title".to_string(),
            n => format!("{n} titles"),
        };
        div()
            .id(("collection-card", row * 10_000 + col))
            .clickable()
            .w(px(POSTER_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focus = Focus::Card { row, col };
                this.key(Key::Other("enter"), cx);
                cx.notify();
            }))
            .child(art(url, None, (POSTER_WIDTH, POSTER_HEIGHT), focused, t))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if focused { t.bright } else { t.foreground })
                    .truncate()
                    .child(card.title.clone()),
            )
            .child(div().text_size(px(12.)).text_color(t.secondary).child(count))
            .into_any_element()
    }
}

impl Render for Collections {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let rows = self.groups.iter().enumerate().map(|(row, group)| {
            let cards = group.cards.iter().enumerate().map(|(col, card)| self.card(row, col, card, cx));
            div()
                .pb(px(28.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .px(px(40.))
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.bright)
                        .child(group.title.clone()),
                )
                .child(
                    div()
                        .id(("collection-row", row))
                        .px(px(40.))
                        .flex()
                        .gap(px(14.))
                        .overflow_x_scroll()
                        .track_scroll(&self.row_scrolls[row])
                        .children(cards),
                )
        });
        let status = self.status.clone().map(|status| {
            let retry = button("collections-retry", "Retry", ButtonVariant::Secondary, cx)
                .on_click(cx.listener(|this, _, _, cx| this.load(cx)));
            div()
                .px(px(40.))
                .pb(px(80.))
                .flex()
                .items_center()
                .gap(px(14.))
                .text_color(if self.failed { t.danger } else { t.secondary })
                .child(status)
                .children(self.failed.then_some(retry))
        });
        let title = matches!(self.scope, Scope::Server).then(|| {
            div()
                .px(px(40.))
                .pt(px(28.))
                .pb(px(18.))
                .text_size(px(34.))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(t.bright)
                .child("Collections")
        });
        div()
            .id("collections")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(title)
            .children(rows)
            .children(status)
    }
}

#[cfg(test)]
mod tests {
    use super::{library_groups, server_groups};
    use crate::api::wire;
    use serde_json::json;

    fn card(id: &str) -> serde_json::Value {
        json!({ "id": id, "title": id, "poster_url": "", "item_count": 1, "poster_is_collage": false })
    }

    #[test]
    fn a_library_lists_its_groups_and_the_ungrouped_collections_in_display_order() {
        let tab: wire::LibraryCollectionTab = serde_json::from_value(json!({
            "library_id": "lib",
            "collections": [],
            "groups": [
                { "id": "g2", "name": "Mine", "kind": "user_collections", "sort_mode": "name_asc", "sort_order": 20,
                  "collections": [card("u1")] },
                { "id": "g1", "name": "Awards", "kind": "admin", "sort_mode": "name_asc", "sort_order": 0,
                  "collections": [card("c1"), card("c2")] },
            ],
            "ungrouped": { "sort_order": 10, "collections": [card("c3")] },
        }))
        .unwrap();
        let groups = library_groups(tab);
        let names: Vec<_> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(names, ["Awards", "Collections", "Mine"]);
        assert_eq!(groups[0].cards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c1", "c2"]);
        assert!(groups.iter().all(|g| g.library_id == "lib"));
    }

    #[test]
    fn the_server_lists_a_row_for_each_library_with_collections() {
        let libraries: Vec<wire::ServerCollectionLibrary> = serde_json::from_value(json!([
            { "library_id": "1", "library_name": "Movies", "total_count": 1, "collections": [card("c1")] },
            { "library_id": "2", "library_name": "Shows", "total_count": 0, "collections": [] },
        ]))
        .unwrap();
        let groups = server_groups(libraries);
        assert_eq!(groups.len(), 1, "a Library without collections has no row");
        assert_eq!((groups[0].title.as_str(), groups[0].library_id.as_str()), ("Movies", "1"));
    }
}
