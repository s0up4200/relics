//! Live updates (ADR 0006): the Silo v2 events socket of the active Profile. This module is the only one that may
//! use the socket library (clippy.toml blocks it everywhere else). It mints the ticket, connects, answers the pings,
//! connects again after each close, and turns each event into a `LiveChange`.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context as _;
use async_io::Timer;
use async_net::TcpStream;
use async_tungstenite::WebSocketStream;
use async_tungstenite::smol::{ConnectStream, client_async_tls_with_connector};
use async_tungstenite::tungstenite::client::IntoClientRequest as _;
use async_tungstenite::tungstenite::http::HeaderValue;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::{Message, Utf8Bytes};
use futures::future::{Either, select};
use futures::stream::BoxStream;
use futures::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::http_client::{Method, Url, read_no_proxy_from_env};
use hyper_util::client::proxy::matcher::Matcher;
use serde::Deserialize;
use serde_json::Value;

use super::{Client, token_expiry};
use crate::library::retry_delay;

/// A change that the Server tells about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveChange {
    /// The socket closed without a plan, so the app can have missed changes.
    Resync,
    /// The state of the Profile changed for an Item or a series: a favorite, the watchlist, the watched state, the
    /// history, a Home dismissal, or the progress.
    UserState { item: Option<String>, series: Option<String>, progress: bool },
    /// An Item changed in a Library. `None` is no Library or no Item.
    Catalog { library: Option<String>, item: Option<String> },
    /// A Library was added, removed or changed. `None` is no Library.
    Libraries { library: Option<String> },
    /// A Server setting of the Profile or of the account changed on a client. The event has the key, never the
    /// value. `None` is no key.
    Settings { key: Option<String> },
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

/// The ticket lives 30 seconds, so a connect that takes longer cannot open the socket.
const HANDSHAKE: Duration = Duration::from_secs(30);

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

/// A drop this long before the end of the connection time is still planned. The margin covers the time of the
/// upgrade answer and a clock that is a few seconds behind the Server.
const MARGIN: Duration = Duration::from_secs(5);

/// The connection time of a socket: the time of the ticket, or less when the access token expires sooner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lifetime(Duration);

impl Lifetime {
    /// `expiry` is the expiry of the access token, `None` when the app cannot read it. `now` is the time after the
    /// handshake. A token that expired during the ticket call or the handshake leaves no time.
    fn new(ticket: Duration, expiry: Option<SystemTime>, now: SystemTime) -> Self {
        let left = expiry.map(|exp| exp.duration_since(now).unwrap_or_default());
        Self(left.map_or(ticket, |left| left.min(ticket)))
    }

    /// How a stream end or a socket error counts after `elapsed` of listening: planned at the end of the
    /// connection time, else lost.
    fn dropped(self, elapsed: Duration) -> Close {
        if elapsed + MARGIN >= self.0 { Close::Planned } else { Close::Lost }
    }
}

/// The close code that follows `access_changed`.
const ACCESS_CHANGED: u16 = 4001;

const SUBSCRIBE: &str = r#"{"type":"subscribe","channels":["catalog","user_state","user_settings"]}"#;

#[derive(Deserialize)]
struct Ticket {
    ticket: String,
    /// The Server ends the socket after this long, or when the access token expires, with no close frame.
    max_connection_seconds: u64,
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

/// Reads one text frame. `profile_id` is the Profile of the socket: a `user_state` or `user_settings` event of
/// another Profile is `Other`.
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
        // A change with no Profile is a setting of the account, which also applies to this Profile.
        ("event", "user_settings", "user_settings.changed")
            if id("profile_id").is_none_or(|changed| changed == profile_id) =>
        {
            LiveChange::Settings { key: id("key") }
        }
        ("event", "catalog", "catalog.item.changed" | "metadata.updated" | "library.item_added") => {
            LiveChange::Catalog { library: id("library_id"), item: id("content_id") }
        }
        ("event", "catalog", "catalog.library.changed" | "library.changed") => {
            LiveChange::Libraries { library: id("library_id") }
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
            Ok((socket, lifetime)) => {
                if std::mem::take(&mut missed) {
                    changes.unbounded_send(LiveChange::Resync).ok();
                }
                listen(socket, lifetime, &profile, &changes).await
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

/// Mints a ticket and opens the socket with it. Also gives the connection time of the socket: the time of the ticket,
/// or less when the access token expires sooner.
async fn connect(client: &Client) -> anyhow::Result<(WebSocketStream<ConnectStream>, Lifetime)> {
    // The client refreshes the access token after a 401. After a 403 it waits for the user to enter the PIN of the
    // Profile, for as long as the user takes, so the deadline starts after the ticket.
    let ticket: Ticket = client.call_with_retries(Method::POST, "events/ws-ticket", None).await?;
    // A Server or a proxy can accept the TCP connection and then never answer.
    let socket = match select(handshake(client, &ticket.ticket).boxed(), Timer::after(HANDSHAKE)).await {
        Either::Left((socket, _)) => socket?,
        Either::Right(_) => anyhow::bail!("no answer in {} s", HANDSHAKE.as_secs()),
    };
    let expiry = token_expiry(&client.tokens().access_token);
    Ok((socket, Lifetime::new(Duration::from_secs(ticket.max_connection_seconds), expiry, SystemTime::now())))
}

/// Opens the socket with `ticket`.
async fn handshake(client: &Client, ticket: &str) -> anyhow::Result<WebSocketStream<ConnectStream>> {
    let mut url = client.server.url.join("api/v2/events/ws")?;
    let stream = open(client, &url).await?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).map_err(|()| anyhow::anyhow!("no WebSocket URL for {url}"))?;
    let mut request = url.as_str().into_client_request()?;
    // The ticket goes in the subprotocols, never in the URL.
    let protocols = HeaderValue::from_str(&format!("silo.events.v2, silo.ticket.{ticket}"))?;
    request.headers_mut().insert("Sec-WebSocket-Protocol", protocols);
    let tls = futures_rustls::TlsConnector::from(Arc::new(http_client_tls::tls_config()));
    Ok(client_async_tls_with_connector(request, stream, Some(tls)).await?.0)
}

/// Opens the TCP connection to the host of `url`. The connection goes through the proxy of the HTTP client, with the
/// same NO_PROXY rules, so that the socket reaches the Server where the API requests do.
async fn open(client: &Client, url: &Url) -> anyhow::Result<TcpStream> {
    let host = url.host_str().context("no host in the Server URL")?;
    let port = url.port_or_known_default().context("no port in the Server URL")?;
    let proxy = client.http.proxy().and_then(|proxy| {
        let no = read_no_proxy_from_env().unwrap_or_default();
        Matcher::builder().all(proxy.to_string()).no(no).build().intercept(&url.as_str().parse().ok()?)
    });
    let Some(proxy) = proxy else {
        return Ok(TcpStream::connect((host, port)).await?);
    };
    // A tunnel through an HTTP proxy: CONNECT, then the socket talks to the Server as if no proxy were there.
    let uri = proxy.uri();
    anyhow::ensure!(uri.scheme_str() == Some("http"), "the events socket supports only an http:// proxy, not {uri}");
    let mut stream =
        TcpStream::connect((uri.host().context("no host in the proxy URL")?, uri.port_u16().unwrap_or(80))).await?;
    // The Display of a host puts an IPv6 address in brackets.
    let target = format!("{}:{port}", url.host().context("no host in the Server URL")?);
    let mut head = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(auth) = proxy.basic_auth() {
        head += &format!("Proxy-Authorization: {}\r\n", auth.to_str()?);
    }
    stream.write_all(format!("{head}\r\n").as_bytes()).await?;
    // One byte at a time, so that no byte of the Server is read past the end of the proxy answer.
    let mut answer = Vec::new();
    let mut byte = [0];
    while !answer.ends_with(b"\r\n\r\n") {
        anyhow::ensure!(answer.len() < 8192, "the proxy answer is too long");
        stream.read_exact(&mut byte).await?;
        answer.push(byte[0]);
    }
    let answer = String::from_utf8_lossy(&answer);
    let status = answer.lines().next().unwrap_or_default();
    anyhow::ensure!(status.split_whitespace().nth(1) == Some("200"), "the proxy refused the tunnel: {status}");
    Ok(stream)
}

/// Subscribes, and sends the changes until the socket closes. A close frame from the Server is a planned close,
/// unless the access changed. A drop at the end of the connection time `lifetime` is also a planned close.
async fn listen(
    mut socket: WebSocketStream<ConnectStream>,
    lifetime: Lifetime,
    profile: &str,
    changes: &mpsc::UnboundedSender<LiveChange>,
) -> Close {
    if socket.send(Message::Text(Utf8Bytes::from_static(SUBSCRIBE))).await.is_err() {
        return Close::Lost;
    }
    let opened = Instant::now();
    loop {
        // tungstenite answers each ping while it reads.
        let message = match select(socket.next(), Timer::after(SILENCE)).await {
            Either::Left((Some(Ok(message)), _)) => message,
            Either::Left(_) => return lifetime.dropped(opened.elapsed()),
            Either::Right(_) => return Close::Lost,
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
    use std::time::{Duration, SystemTime};

    use super::{Close, Frame, Lifetime, LiveChange, decode};

    const NOW: SystemTime = SystemTime::UNIX_EPOCH;

    fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn the_connection_time_is_the_ticket_or_the_token_time_when_that_is_shorter() {
        assert_eq!(Lifetime::new(secs(300), None, NOW), Lifetime(secs(300)), "no readable expiry");
        assert_eq!(Lifetime::new(secs(300), Some(NOW + secs(60)), NOW), Lifetime(secs(60)));
        assert_eq!(Lifetime::new(secs(300), Some(NOW + secs(900)), NOW), Lifetime(secs(300)));
        assert_eq!(Lifetime::new(secs(300), Some(NOW), NOW), Lifetime(secs(0)), "the token expires now");
        let after = NOW + secs(10);
        assert_eq!(Lifetime::new(secs(300), Some(NOW), after), Lifetime(secs(0)), "an expired token");
        assert_eq!(Lifetime::new(secs(0), None, NOW), Lifetime(secs(0)), "a ticket with no time");
    }

    #[test]
    fn a_drop_is_planned_from_5_seconds_before_the_end_of_the_connection_time() {
        let lifetime = Lifetime(secs(300));
        assert_eq!(lifetime.dropped(secs(295) - Duration::from_millis(1)), Close::Lost);
        assert_eq!(lifetime.dropped(secs(295)), Close::Planned);
        assert_eq!(lifetime.dropped(secs(295) + Duration::from_millis(1)), Close::Planned);
        assert_eq!(lifetime.dropped(secs(400)), Close::Planned);
        assert_eq!(lifetime.dropped(secs(0)), Close::Lost);
    }

    #[test]
    fn with_no_time_past_the_margin_each_drop_is_planned() {
        let expired = Lifetime::new(secs(300), Some(NOW), NOW + secs(10));
        assert_eq!(expired.dropped(secs(0)), Close::Planned);
        assert_eq!(Lifetime(secs(5)).dropped(secs(0)), Close::Planned, "the margin covers all 5 seconds");
        assert_eq!(Lifetime(secs(6)).dropped(secs(0)), Close::Lost);
    }

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
    fn a_settings_event_of_the_profile_or_of_the_account_names_its_key() {
        let changed = |data| decode(&event("user_settings", "user_settings.changed", data), "p1");
        let settings = |key: Option<&str>| Frame::Change(LiveChange::Settings { key: key.map(Into::into) });
        let quality = "playback.preferred_quality";
        let profile = serde_json::json!({"key": quality, "scope": "profile", "profile_id": "p1"});
        assert_eq!(changed(profile), settings(Some(quality)));
        let account = serde_json::json!({"key": "catalog.metadata_language", "scope": "account"});
        assert_eq!(changed(account), settings(Some("catalog.metadata_language")), "no Profile is the account");
        assert_eq!(changed(serde_json::json!({})), settings(None));
        let other = serde_json::json!({"key": quality, "scope": "profile", "profile_id": "p2"});
        assert_eq!(changed(other), Frame::Other, "another Profile");
    }

    #[test]
    fn a_catalog_event_names_its_library_and_0_is_no_library() {
        let added = event("catalog", "library.item_added", serde_json::json!({"library_id": 7, "content_id": "x"}));
        let catalog = |library: Option<&str>, item: Option<&str>| {
            Frame::Change(LiveChange::Catalog { library: library.map(Into::into), item: item.map(Into::into) })
        };
        assert_eq!(decode(&added, "p1"), catalog(Some("7"), Some("x")));
        let libraries =
            |library: Option<&str>| Frame::Change(LiveChange::Libraries { library: library.map(Into::into) });
        let zero = event("catalog", "catalog.library.changed", serde_json::json!({"library_id": 0}));
        assert_eq!(decode(&zero, "p1"), libraries(None));
        let none = event("catalog", "metadata.updated", serde_json::json!({}));
        assert_eq!(decode(&none, "p1"), catalog(None, None));
        let library = event("catalog", "library.changed", serde_json::json!({"library_id": 7, "content_id": "x"}));
        assert_eq!(decode(&library, "p1"), libraries(Some("7")), "a Library change names no Item");
        assert_eq!(decode(&event("catalog", "scan.started", serde_json::json!({})), "p1"), Frame::Other);
    }
}
