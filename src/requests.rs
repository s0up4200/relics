//! Requests: titles that are not on the Server. Search shows them under Request to add, and Enter sends a
//! request. The My requests page lists the requests of the user with their state, and Enter or a click on
//! Cancel withdraws a request that nothing was sent for yet.

use std::sync::Arc;

use gpui::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_omarchy::{ActiveTheme, ButtonVariant, button};

use crate::api::{Client, wire};
use crate::filter_panel::bar_pill;
use crate::grid::kept_focus;
use crate::keys::Key;
use crate::pages::freshness::{Deps, Fresh, Reload};
use crate::pages::loading::{Failure, Loader};

/// The most Request to add results that Search shows, as in the web client.
const LIMIT: usize = 20;

/// The Request to add results of a search page: the titles that are not in the catalog.
pub fn to_add(page: wire::RequestMediaPage) -> Vec<wire::RequestMediaResult> {
    page.results.into_iter().filter(|r| r.availability != "available").take(LIMIT).collect()
}

/// The request for a Request to add result. `None` when the result has no TMDB id or title to send.
pub fn create(result: &wire::RequestMediaResult) -> Option<wire::MediaRequestCreate> {
    Some(wire::MediaRequestCreate {
        backdrop_path: result.backdrop_path.clone(),
        imdb_id: None,
        media_type: result.media_type.clone(),
        overview: result.overview.clone(),
        poster_path: result.poster_path.clone(),
        seasons: Vec::new(),
        title: result.title.as_str().try_into().ok()?,
        tmdb_id: std::num::NonZeroU64::new(result.tmdb_id.try_into().ok()?)?,
        tvdb_id: None,
        year: result.year,
    })
}

/// The label of the state of a request: "Pending", "Partially available".
pub fn state_label(state: &str) -> String {
    let text = state.replace('_', " ");
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What a Request to add result shows in place of the Request action: the state of its request, or why the
/// user cannot request it.
pub fn result_status(result: &wire::RequestMediaResult) -> String {
    let request = &result.request;
    if request.requestable {
        return "Request".into();
    }
    if let Some(state) = &request.state {
        return state_label(state);
    }
    match request.reason.as_deref() {
        Some("already_requested") => "Already requested",
        Some("already_available") => "Available",
        Some("requests_disabled") => "Requests disabled",
        Some("blocked") => "Blocked",
        Some("quota_exceeded") => "Request limit reached",
        _ => "Unavailable",
    }
    .into()
}

/// The user can cancel a request until something was sent for it: while it is pending, or approved with no
/// target yet. This is the rule of the server and the web client.
pub fn can_cancel(request: &wire::MediaRequest) -> bool {
    request.outcome == "active"
        && (request.status == "pending" || request.status == "approved" && request.targets.is_empty())
}

/// "Movie · 1995".
pub fn kind_line(media_type: &str, year: Option<i64>) -> String {
    let kind = if media_type == "series" { "Series" } else { "Movie" };
    year.map_or(kind.to_string(), |year| format!("{kind} · {year}"))
}

/// The My requests page.
pub struct Requests {
    client: Arc<Client>,
    items: Vec<wire::MediaRequest>,
    /// The cursor of the next page, while the server has more.
    next: Option<String>,
    focus: usize,
    /// The page has the keyboard focus, so the focused row shows the focus ring.
    pub active: bool,
    /// "Loading…", "No requests.", or the error of the last load (shown with Retry).
    status: Option<String>,
    failed: bool,
    /// The error of the last cancel.
    notice: Option<String>,
    scroll: ScrollHandle,
    fresh: Loader,
}

impl Requests {
    pub fn new(client: Arc<Client>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            client,
            items: Vec::new(),
            next: None,
            focus: 0,
            active: false,
            status: None,
            failed: false,
            notice: None,
            scroll: ScrollHandle::new(),
            fresh: Loader::default(),
        };
        page.load(cx);
        page
    }

    /// Loads the requests from the first page, as many as the page has. The new list replaces the old one when all
    /// pages are in, so the scroll position stays, and the focus stays on its request.
    fn load(&mut self, cx: &mut Context<Self>) {
        let (client, want) = (self.client.clone(), self.items.len());
        Loader::run(
            self,
            cx,
            |this| &mut this.fresh,
            Failure::Retain,
            async move {
                let (mut items, mut next) = (Vec::new(), None);
                loop {
                    let page = client.my_requests(next.as_deref()).await?;
                    items.extend(page.items);
                    next = page.page.and_then(|p| p.next_cursor);
                    if items.len() >= want || next.is_none() {
                        return Ok::<_, crate::api::Error>((items, next));
                    }
                }
            },
            move |this, result, _cx| {
                match result {
                    Ok((items, next)) => {
                        this.focus = kept_focus(&this.items, this.focus, &items, |r| r.id.clone());
                        this.items = items;
                        this.next = next;
                        this.status = this.items.is_empty().then(|| "No requests.".into());
                        this.failed = false;
                    }
                    // The Library window shows Offline or the sign-in screen.
                    Err(err) => {
                        this.status = Some(err.to_string());
                        this.failed = true;
                    }
                }
            },
            Self::load,
        );
        cx.notify();
    }

    /// Retry after a failed load: the next page again, or the first page.
    fn retry(&mut self, cx: &mut Context<Self>) {
        self.failed = false;
        match self.next.take() {
            Some(next) if !self.items.is_empty() => self.load_more(next, cx),
            _ => self.load(cx),
        }
    }

    /// Loads the page after `cursor`.
    fn load_more(&mut self, cursor: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.next = Some(cursor.clone());
        Loader::run(
            self,
            cx,
            |this| &mut this.fresh,
            Failure::Page,
            async move { client.my_requests(Some(&cursor)).await },
            |this, result, _| match result {
                Ok(page) => {
                    this.items.extend(page.items);
                    this.next = page.page.and_then(|p| p.next_cursor);
                    this.status = this.items.is_empty().then(|| "No requests.".into());
                }
                Err(err) => {
                    this.status = Some(err.to_string());
                    this.failed = true;
                }
            },
            Self::load,
        );
        cx.notify();
    }

    /// Enter on row `index`: cancel the request when the server still lets the user do that.
    fn cancel(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(request) = self.items.get(index).filter(|r| can_cancel(r)) else {
            return;
        };
        let (client, id) = (self.client.clone(), request.id.to_string());
        self.notice = None;
        self.fresh.write();
        cx.spawn(async move |this, cx| {
            let result = client.cancel_request(&id).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(request) => {
                        if let Some(item) = this.items.iter_mut().find(|r| r.id == request.id) {
                            *item = request;
                        }
                    }
                    Err(err) => this.notice = Some(format!("Not cancelled: {err}")),
                }
                if this.fresh.written() {
                    this.load(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Handles a key. Returns false when the page does not use it, for example Left to the sidebar.
    pub fn key(&mut self, key: Key, cx: &mut Context<Self>) -> bool {
        let last = self.items.len().saturating_sub(1);
        let focus = match key {
            Key::Other("enter") => {
                self.cancel(self.focus, cx);
                return true;
            }
            Key::Down if self.focus < last => self.focus + 1,
            Key::Up if self.focus > 0 => self.focus - 1,
            _ => return false,
        };
        self.focus = focus;
        self.scroll.scroll_to_item(focus + 1);
        // A reload that runs covers the requests that the page has, so the next page waits for it.
        if focus == last
            && !self.failed
            && self.status.is_none()
            && !self.fresh.loading()
            && let Some(next) = self.next.take()
        {
            self.load_more(next, cx);
        }
        cx.notify();
        true
    }

    fn row(&self, index: usize, request: &wire::MediaRequest, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.omarchy();
        let focused = self.active && self.focus == index;
        let mut line = kind_line(&request.media_type, request.year);
        line.push_str(&format!(" · Requested {}", request.created_at.format("%b %-d, %Y")));
        let state = match &request.outcome_reason {
            Some(reason) => format!("{}: {reason}", state_label(&request.state)),
            None => state_label(&request.state),
        };
        let cancel = can_cancel(request).then(|| {
            bar_pill(("request-cancel", index), "Cancel".into(), false, focused, t).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.focus = index;
                    this.key(Key::Other("enter"), cx);
                },
            ))
        });
        div()
            .px(px(16.))
            .py(px(12.))
            .flex()
            .items_center()
            .gap(px(16.))
            .rounded(px(12.))
            .bg(t.surface)
            .border_2()
            .border_color(if focused && cancel.is_none() { t.accent } else { gpui::transparent_black() })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.bright)
                            .child(request.title.clone()),
                    )
                    .child(div().text_size(px(13.)).text_color(t.secondary).child(line)),
            )
            .child(div().text_size(px(14.)).text_color(t.foreground).child(state))
            .children(cancel)
    }
}

impl Reload for Requests {
    fn deps(&self) -> Vec<Deps> {
        vec![Deps::Recovery]
    }

    fn fresh(&mut self, _: usize) -> &mut Fresh {
        &mut self.fresh
    }

    fn reload(&mut self, _: usize, cx: &mut Context<Self>) {
        self.load(cx);
    }
}

impl Render for Requests {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.omarchy().clone();
        let title = div()
            .pt(px(28.))
            .pb(px(6.))
            .text_size(px(34.))
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_color(t.bright)
            .child("My requests");
        let rows: Vec<_> = self.items.iter().enumerate().map(|(i, request)| self.row(i, request, cx)).collect();
        let status = self.fresh.status(self.status.as_deref(), self.failed).map(|(status, failed)| {
            let retry = button("requests-retry", "Retry", ButtonVariant::Secondary, cx)
                .debug_selector(|| "requests-retry".into())
                .on_click(cx.listener(|this, _, _, cx| this.retry(cx)));
            div()
                .debug_selector(|| "requests-status".into())
                .pb(px(40.))
                .flex()
                .items_center()
                .gap(px(14.))
                .text_color(if failed { t.danger } else { t.secondary })
                .child(status.to_owned())
                .children(failed.then_some(retry))
        });
        let notice = self.notice.clone().map(|notice| div().text_color(t.danger).child(notice));
        div()
            .id("requests")
            .size_full()
            .px(px(40.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(title)
            .children(rows)
            .children(status)
            .children(notice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(status: &str, outcome: &str, targets: usize) -> wire::MediaRequest {
        let target = json!({ "id": "1", "request_id": "r", "quality": "1080p", "is_anime": false,
            "status": "queued", "created_at": "2026-01-02T03:04:05.678Z", "updated_at": "2026-01-02T03:04:05.678Z" });
        serde_json::from_value(json!({
            "id": "r", "provider": "tmdb", "media_type": "movie", "tmdb_id": 1, "title": "Heat",
            "status": status, "outcome": outcome, "state": status, "seasons": [], "season_progress": [],
            "source": "direct", "is_anime": false, "targets": vec![target; targets],
            "created_at": "2026-01-02T03:04:05.678Z", "updated_at": "2026-01-02T03:04:05.678Z",
        }))
        .unwrap()
    }

    #[test]
    fn a_request_can_be_cancelled_until_something_was_sent_for_it() {
        assert!(can_cancel(&request("pending", "active", 0)));
        assert!(can_cancel(&request("approved", "active", 0)));
        assert!(!can_cancel(&request("approved", "active", 1)), "approved and sent");
        assert!(!can_cancel(&request("downloading", "active", 0)));
        assert!(!can_cancel(&request("pending", "cancelled", 0)), "already cancelled");
        assert!(!can_cancel(&request("pending", "declined", 0)));
    }

    #[test]
    fn a_result_shows_request_its_request_state_or_why_it_cannot_be_requested() {
        let result = |request: serde_json::Value| -> wire::RequestMediaResult {
            serde_json::from_value(json!({ "availability": "missing", "media_type": "movie", "tmdb_id": 1,
                "title": "Heat", "in_watchlist": false, "request": request }))
            .unwrap()
        };
        let base = json!({ "following": false, "requestable": true, "requested_by_viewer": false });
        assert_eq!(result_status(&result(base.clone())), "Request");
        let mut requested = base.clone();
        requested["requestable"] = json!(false);
        requested["state"] = json!("partially_available");
        assert_eq!(result_status(&result(requested)), "Partially available");
        let mut over = base;
        over["requestable"] = json!(false);
        over["reason"] = json!("quota_exceeded");
        assert_eq!(result_status(&result(over)), "Request limit reached");
    }
}
