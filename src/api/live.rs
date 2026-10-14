//! Live updates (ADR 0006): the Silo v2 events socket of the active Profile. This module is the only one that may
//! use the socket library (clippy.toml blocks it everywhere else). It mints the ticket, connects, answers the pings,
//! connects again after each close, and turns each event into a `LiveChange`.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_io::Timer;
use async_tungstenite::WebSocketStream;
use async_tungstenite::smol::{ConnectStream, connect_async_with_tls_connector};
use async_tungstenite::tungstenite::client::IntoClientRequest as _;
use async_tungstenite::tungstenite::http::HeaderValue;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::{Message, Utf8Bytes};
use futures::future::{Either, select};
use futures::stream::BoxStream;
use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::http_client::Method;
use serde::Deserialize;
use serde_json::Value;

use super::Client;
use crate::library::retry_delay;

/// A change that the Server tells about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveChange {
    /// The socket closed without a plan, so the app can have missed changes.
    Resync,
    /// The state of the Profile changed for an Item or a series: a favorite, the watchlist, the watched state, the
    /// history, a Home dismissal, or the progress.
    UserState { item: Option<String>, series: Option<String>, progress: bool },
    /// An Item or a Library changed. `None` is no Library, or no Item for a change of the whole Library.
    Catalog { library: Option<String>, item: Option<String> },
}

/// One text frame of the socket.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Change(LiveChange),
    /// The access of the ticket changed. The Server closes the socket next.
    AccessChanged,
    /// A frame that changes no page, for example `hello` or an event of another Profile.
    Other,
}

/// The Server pings each 20 seconds. After this long with no frame, the connection is dead.
const SILENCE: Duration = Duration::from_secs(60);

/// A socket that closes sooner than this after the ticket request counts as a failure, so that the next connect
/// waits the retry delay.
const STEADY: Duration = Duration::from_secs(10);

/// How a socket ended.
#[derive(Debug, PartialEq, Eq)]
enum Close {
    /// The Server closed the socket at the end of its connection time. The app missed no change.
    Planned,
    /// The connection failed or dropped, or the access changed. The app can have missed changes.
    Lost,
}

/// The close code that follows `access_changed`.
const ACCESS_CHANGED: u16 = 4001;

const SUBSCRIBE: &str = r#"{"type":"subscribe","channels":["catalog","user_state"]}"#;

#[derive(Deserialize)]
struct Ticket {
    ticket: String,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    data: Value,
}

/// Reads one text frame. `profile_id` is the Profile of the socket: a `user_state` event of another Profile is
/// `Other`.
pub fn decode(text: &str, profile_id: &str) -> Frame {
    let Ok(frame) = serde_json::from_str::<Envelope>(text) else {
        return Frame::Other;
    };
    let id = |key: &str| id(&frame.data[key]);
    let change = match (frame.kind.as_str(), frame.channel.as_str(), frame.event.as_str()) {
        ("access_changed", ..) => return Frame::AccessChanged,
        ("event", "user_state", "user_state.changed") if id("profile_id").as_deref() == Some(profile_id) => {
            let progress = match frame.data["change"].as_str() {
                Some("progress") => true,
                Some("favorite" | "watchlist" | "watched" | "history" | "home_dismissal") => false,
                _ => return Frame::Other,
            };
            LiveChange::UserState { item: id("content_id"), series: id("series_id"), progress }
        }
        ("event", "catalog", "catalog.item.changed" | "metadata.updated" | "library.item_added") => {
            LiveChange::Catalog { library: id("library_id"), item: id("content_id") }
        }
        ("event", "catalog", "catalog.library.changed" | "library.changed") => {
            LiveChange::Catalog { library: id("library_id"), item: None }
        }
        _ => return Frame::Other,
    };
    Frame::Change(change)
}

/// An id of an event: a string, or a number. An empty string and 0 are no id.
fn id(value: &Value) -> Option<String> {
    let id = match value {
        Value::String(id) => id.clone(),
        Value::Number(id) => id.to_string(),
        _ => return None,
    };
    (!id.is_empty() && id != "0").then_some(id)
}

impl Client {
    /// The Live updates of the active Profile. The stream holds the socket: dropping the stream closes it. The
    /// stream ends when the client acts for another Profile.
    pub fn live_updates(self: &Arc<Self>) -> BoxStream<'static, LiveChange> {
        let (changes, stream) = mpsc::unbounded();
        let run = run(self.clone(), changes).into_stream().filter_map(|()| async { None });
        futures::stream::select(stream, run).boxed()
    }
}

/// Connects again and again while the Profile stays active and the stream lives.
async fn run(client: Arc<Client>, changes: mpsc::UnboundedSender<LiveChange>) {
    let profile = client.auth.lock().unwrap().profile_id.clone();
    let Some(profile) = profile else {
        return;
    };
    // The app can have missed changes since the last socket: send Resync after the next connect.
    let mut missed = false;
    let mut failures = 0;
    while client.auth.lock().unwrap().profile_id.as_deref() == Some(profile.as_str()) && !changes.is_closed() {
        let started = Instant::now();
        let close = match connect(&client).await {
            Ok(socket) => {
                if std::mem::take(&mut missed) {
                    changes.unbounded_send(LiveChange::Resync).ok();
                }
                listen(socket, &profile, &changes).await
            }
            Err(err) => {
                eprintln!("relics: the events socket cannot connect: {err}");
                Close::Lost
            }
        };
        missed |= close == Close::Lost;
        if started.elapsed() < STEADY {
            Timer::after(retry_delay(failures)).await;
            failures = failures.saturating_add(1);
        } else {
            failures = 0;
        }
    }
}

/// Mints a ticket and opens the socket with it.
async fn connect(client: &Client) -> anyhow::Result<WebSocketStream<ConnectStream>> {
    // The client refreshes the access token after a 401.
    let ticket: Ticket = client.call_with_retries(Method::POST, "events/ws-ticket", None).await?;
    let mut url = client.server.url.join("api/v2/events/ws")?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).map_err(|()| anyhow::anyhow!("no WebSocket URL for {url}"))?;
    let mut request = url.as_str().into_client_request()?;
    // The ticket goes in the subprotocols, never in the URL.
    let protocols = HeaderValue::from_str(&format!("silo.events.v2, silo.ticket.{}", ticket.ticket))?;
    request.headers_mut().insert("Sec-WebSocket-Protocol", protocols);
    let tls = futures_rustls::TlsConnector::from(Arc::new(http_client_tls::tls_config()));
    let (socket, _) = connect_async_with_tls_connector(request, Some(tls)).await?;
    Ok(socket)
}

/// Subscribes, and sends the changes until the socket closes. A close frame from the Server is a planned close,
/// unless the access changed.
async fn listen(
    mut socket: WebSocketStream<ConnectStream>,
    profile: &str,
    changes: &mpsc::UnboundedSender<LiveChange>,
) -> Close {
    if socket.send(Message::Text(Utf8Bytes::from_static(SUBSCRIBE))).await.is_err() {
        return Close::Lost;
    }
    loop {
        // tungstenite answers each ping while it reads.
        let message = match select(socket.next(), Timer::after(SILENCE)).await {
            Either::Left((Some(Ok(message)), _)) => message,
            _ => return Close::Lost,
        };
        match message {
            Message::Text(text) => match decode(&text, profile) {
                Frame::Change(change) => {
                    // The stream is gone, so nothing reads a Resync.
                    if changes.unbounded_send(change).is_err() {
                        return Close::Planned;
                    }
                }
                Frame::AccessChanged => return Close::Lost,
                Frame::Other => {}
            },
            Message::Close(frame) if frame.as_ref().is_some_and(|f| f.code == CloseCode::from(ACCESS_CHANGED)) => {
                return Close::Lost;
            }
            Message::Close(_) => return Close::Planned,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, LiveChange, decode};

    fn event(channel: &str, event: &str, data: serde_json::Value) -> String {
        serde_json::json!({"type": "event", "channel": channel, "event": event, "event_id": "1", "timestamp": "t", "data": data})
            .to_string()
    }

    fn user_state(profile: &str, change: &str) -> String {
        let data = serde_json::json!({"profile_id": profile, "content_id": "episode:e1", "series_id": "series:s", "change": change});
        event("user_state", "user_state.changed", data)
    }

    #[test]
    fn a_user_state_event_of_the_profile_names_its_item_and_series() {
        let favorite =
            LiveChange::UserState { item: Some("episode:e1".into()), series: Some("series:s".into()), progress: false };
        assert_eq!(decode(&user_state("p1", "favorite"), "p1"), Frame::Change(favorite));
        let Frame::Change(LiveChange::UserState { progress, .. }) = decode(&user_state("p1", "progress"), "p1") else {
            panic!("progress is a change");
        };
        assert!(progress);
    }

    #[test]
    fn the_app_ignores_other_profiles_downloads_and_frames_that_change_nothing() {
        assert_eq!(decode(&user_state("p2", "favorite"), "p1"), Frame::Other, "another Profile");
        assert_eq!(decode(&user_state("p1", "download"), "p1"), Frame::Other);
        let download = event("user_state", "download", serde_json::json!({"profile_id": "p1"}));
        assert_eq!(decode(&download, "p1"), Frame::Other);
        assert_eq!(decode(r#"{"type":"hello"}"#, "p1"), Frame::Other);
        assert_eq!(decode(r#"{"type":"snapshot","channel":"catalog","data":null}"#, "p1"), Frame::Other);
        assert_eq!(decode("not json", "p1"), Frame::Other);
        assert_eq!(decode(r#"{"type":"access_changed"}"#, "p1"), Frame::AccessChanged);
    }

    #[test]
    fn a_catalog_event_names_its_library_and_0_is_no_library() {
        let added = event("catalog", "library.item_added", serde_json::json!({"library_id": 7, "content_id": "x"}));
        let catalog = |library: Option<&str>, item: Option<&str>| {
            Frame::Change(LiveChange::Catalog { library: library.map(Into::into), item: item.map(Into::into) })
        };
        assert_eq!(decode(&added, "p1"), catalog(Some("7"), Some("x")));
        let zero = event("catalog", "catalog.library.changed", serde_json::json!({"library_id": 0}));
        assert_eq!(decode(&zero, "p1"), catalog(None, None));
        let none = event("catalog", "metadata.updated", serde_json::json!({}));
        assert_eq!(decode(&none, "p1"), catalog(None, None));
        let library = event("catalog", "library.changed", serde_json::json!({"library_id": 7, "content_id": "x"}));
        assert_eq!(decode(&library, "p1"), catalog(Some("7"), None), "a Library change names no Item");
        assert_eq!(decode(&event("catalog", "scan.started", serde_json::json!({})), "p1"), Frame::Other);
    }
}
