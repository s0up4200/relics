//! Live updates against a local WebSocket server on 127.0.0.1. The ticket request goes to the fake HTTP client, so
//! no test reaches a real host.
// The test plays the Server, so it uses the socket library directly (clippy.toml).
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
// tungstenite fixes the error type of the handshake callback.
#![allow(clippy::result_large_err)]

use std::io::{Read as _, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_tungstenite::tungstenite::handshake::server::{Request, Response};
use async_tungstenite::tungstenite::protocol::CloseFrame;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::{self, Message};
use futures::StreamExt as _;
use futures::executor::block_on;
use futures::future::BoxFuture;
use http_client::{AsyncBody, HttpClient, Url, http};
use relics::api::live::LiveChange;
use relics::api::{Client, Tokens};
use serde_json::json;

mod common;

/// What the server saw on one connection: the path, the offered subprotocols, and the first message.
type Seen = (String, String, String);

fn favorite(item: &str) -> LiveChange {
    LiveChange::UserState { item: Some(item.into()), series: None, progress: false }
}

/// The fake HTTP client with the proxy that `main` reads from the environment.
struct Proxied(Arc<dyn HttpClient>, Url);

impl HttpClient for Proxied {
    fn user_agent(&self) -> Option<&http::HeaderValue> {
        self.0.user_agent()
    }

    fn proxy(&self) -> Option<&Url> {
        Some(&self.1)
    }

    fn send(&self, req: http::Request<AsyncBody>) -> BoxFuture<'static, anyhow::Result<http::Response<AsyncBody>>> {
        self.0.send(req)
    }
}

/// A client of the Server at `root` with the Profile `p1`, and the HTTP proxy `proxy`. The fake HTTP client gives the
/// tickets `ticket1`, `ticket2` and so on, each for a connection of `seconds`.
fn client(root: &str, seconds: u64, proxy: Option<Url>) -> Arc<Client> {
    let minted = AtomicUsize::new(0);
    let (http, _) = common::http(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("POST", "events/ws-ticket") => {
            let n = minted.fetch_add(1, Ordering::SeqCst) + 1;
            let ticket = json!({"ticket": format!("ticket{n}"), "expires_in": 30, "max_connection_seconds": seconds, "protocol": "silo.events.v2"});
            common::json(201, ticket.to_string())
        }
        _ => None,
    });
    let http = match proxy {
        Some(proxy) => Arc::new(Proxied(http, proxy)),
        None => http,
    };
    let client = Arc::new(common::client(http, root));
    client.set_profile(Some("p1".into()));
    client
}

/// Plays the Server for one connection for each close code in `closes`. Each connection sends `hello`, a favorite
/// event of another Profile, a favorite event of `movie:m<n>` for `p1`, and then a close frame with its code.
/// `None` drops the connection with no close frame, as the Server does at the end of the connection time.
fn serve(listener: TcpListener, closes: Vec<Option<CloseCode>>) -> (JoinHandle<()>, mpsc::Receiver<Seen>) {
    let (seen, saw) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for (n, code) in (1..).zip(closes) {
            let (stream, _) = listener.accept().unwrap();
            let (mut path, mut protocols) = (String::new(), String::new());
            let mut socket = tungstenite::accept_hdr(stream, |req: &Request, mut reply: Response| {
                path = req.uri().to_string();
                protocols = req.headers()["Sec-WebSocket-Protocol"].to_str().unwrap().to_string();
                reply.headers_mut().insert("Sec-WebSocket-Protocol", "silo.events.v2".parse().unwrap());
                Ok(reply)
            })
            .unwrap();
            socket.send(Message::text(r#"{"type":"hello"}"#)).unwrap();
            let first = socket.read().unwrap().into_text().unwrap().to_string();
            seen.send((path, protocols, first)).unwrap();
            let other = json!({"profile_id": "p2", "content_id": "movie:other", "change": "favorite"});
            let own = json!({"profile_id": "p1", "content_id": format!("movie:m{n}"), "change": "favorite"});
            for data in [other, own] {
                let event =
                    json!({"type": "event", "channel": "user_state", "event": "user_state.changed", "data": data});
                socket.send(Message::text(event.to_string())).unwrap();
            }
            let Some(code) = code else {
                continue;
            };
            socket.close(Some(CloseFrame { code, reason: "".into() })).unwrap();
            while socket.read().is_ok() {}
        }
    });
    (server, saw)
}

#[test]
fn the_client_subscribes_with_the_ticket_and_connects_again_after_a_planned_close_without_a_resync() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = client(&format!("http://{}/", listener.local_addr().unwrap()), 300, None);
    // The planned close at the end of the connection time.
    let (server, saw) = serve(listener, vec![Some(CloseCode::Normal), Some(CloseCode::Normal)]);

    let changes: Vec<_> = block_on(client.live_updates().take(2).collect());
    assert_eq!(changes, [favorite("movie:m1"), favorite("movie:m2")], "no Resync, and no event of another Profile");
    server.join().unwrap();
    let subscribe = r#"{"type":"subscribe","channels":["catalog","user_state"]}"#;
    for n in 1..=2 {
        let (path, protocols, first) = saw.recv().unwrap();
        assert_eq!(path, "/api/v2/events/ws", "no ticket in the URL");
        assert_eq!(protocols, format!("silo.events.v2, silo.ticket.ticket{n}"), "a new ticket for each connection");
        assert_eq!(first, subscribe);
    }
}

#[test]
fn after_access_changed_the_client_connects_again_and_sends_a_resync() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = client(&format!("http://{}/", listener.local_addr().unwrap()), 300, None);
    let (server, saw) = serve(listener, vec![Some(CloseCode::from(4001)), Some(CloseCode::Normal)]);

    let changes: Vec<_> = block_on(client.live_updates().take(3).collect());
    assert_eq!(changes, [favorite("movie:m1"), LiveChange::Resync, favorite("movie:m2")]);
    server.join().unwrap();
    let last = saw.iter().map(|(_, protocols, _)| protocols).last().unwrap();
    assert_eq!(last, "silo.events.v2, silo.ticket.ticket2", "a new ticket after access_changed");
}

#[test]
fn a_drop_at_the_end_of_the_connection_time_connects_again_without_a_resync() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    // The connection time of 1 s is over at once: the Server ends the socket with no close frame.
    let client = client(&format!("http://{}/", listener.local_addr().unwrap()), 1, None);
    let (server, _saw) = serve(listener, vec![None, Some(CloseCode::Normal)]);

    let changes: Vec<_> = block_on(client.live_updates().take(2).collect());
    assert_eq!(changes, [favorite("movie:m1"), favorite("movie:m2")], "no Resync");
    server.join().unwrap();
}

#[test]
fn a_drop_before_the_end_of_the_connection_time_sends_a_resync() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = client(&format!("http://{}/", listener.local_addr().unwrap()), 300, None);
    let (server, _saw) = serve(listener, vec![None, Some(CloseCode::Normal)]);

    let changes: Vec<_> = block_on(client.live_updates().take(3).collect());
    assert_eq!(changes, [favorite("movie:m1"), LiveChange::Resync, favorite("movie:m2")]);
    server.join().unwrap();
}

#[test]
fn a_drop_when_the_access_token_expires_connects_again_without_a_resync() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = client(&format!("http://{}/", listener.local_addr().unwrap()), 300, None);
    // The access token expires in 1 s: the Server ends the socket then, with no close frame.
    let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 1;
    client.set_tokens(Tokens { access_token: common::jwt(exp), refresh_token: "ref".into() });
    let (server, _saw) = serve(listener, vec![None, Some(CloseCode::Normal)]);

    let changes: Vec<_> = block_on(client.live_updates().take(2).collect());
    assert_eq!(changes, [favorite("movie:m1"), favorite("movie:m2")], "no Resync");
    server.join().unwrap();
}

/// Plays an HTTP proxy for one connection. It sends the request head, answers 200, and then copies the bytes both
/// ways between the client and `target`.
fn proxy(listener: TcpListener, target: SocketAddr) -> mpsc::Receiver<String> {
    let (heads, saw) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0];
        while !head.ends_with(b"\r\n\r\n") {
            client.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        heads.send(String::from_utf8(head).unwrap()).unwrap();
        client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
        let server = TcpStream::connect(target).unwrap();
        let (mut up_from, mut up_to) = (client.try_clone().unwrap(), server.try_clone().unwrap());
        std::thread::spawn(move || {
            std::io::copy(&mut up_from, &mut up_to).ok();
            up_to.shutdown(Shutdown::Write).ok();
        });
        let (mut down_from, mut down_to) = (server, client);
        std::io::copy(&mut down_from, &mut down_to).ok();
        down_to.shutdown(Shutdown::Write).ok();
    });
    saw
}

#[test]
fn the_socket_goes_through_the_proxy_of_the_http_client() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = listener.local_addr().unwrap();
    let proxy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = Url::parse(&format!("http://user:pass@{}/", proxy_listener.local_addr().unwrap())).unwrap();
    let client = client(&format!("http://{target}/"), 300, Some(proxy_url));
    let heads = proxy(proxy_listener, target);
    let (server, _saw) = serve(listener, vec![Some(CloseCode::Normal)]);

    let changes: Vec<_> = block_on(client.live_updates().take(1).collect());
    assert_eq!(changes, [favorite("movie:m1")]);
    let head = heads.recv_timeout(Duration::from_secs(5)).expect("the socket went through the proxy");
    assert!(head.starts_with(&format!("CONNECT {target} HTTP/1.1\r\n")), "{head}");
    assert!(head.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"), "{head}");
    server.join().unwrap();
}
