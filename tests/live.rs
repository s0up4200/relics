//! Live updates against a local WebSocket server on 127.0.0.1. The ticket request goes to the fake HTTP client, so
//! no test reaches a real host.
// The test plays the Server, so it uses the socket library directly (clippy.toml).
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
// tungstenite fixes the error type of the handshake callback.
#![allow(clippy::result_large_err)]

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use async_tungstenite::tungstenite::handshake::server::{Request, Response};
use async_tungstenite::tungstenite::protocol::CloseFrame;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::{self, Message};
use futures::StreamExt as _;
use futures::executor::block_on;
use relics::api::Client;
use relics::api::live::LiveChange;
use serde_json::json;

mod common;

/// What the server saw on one connection: the path, the offered subprotocols, and the first message.
type Seen = (String, String, String);

fn favorite(item: &str) -> LiveChange {
    LiveChange::UserState { item: Some(item.into()), series: None, progress: false }
}

/// A client of the Server at `root` with the Profile `p1`. The fake HTTP client gives the tickets `ticket1`,
/// `ticket2` and so on.
fn client(root: &str) -> Arc<Client> {
    let minted = AtomicUsize::new(0);
    let (http, _) = common::http(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("POST", "events/ws-ticket") => {
            let n = minted.fetch_add(1, Ordering::SeqCst) + 1;
            let ticket = json!({"ticket": format!("ticket{n}"), "expires_in": 30, "max_connection_seconds": 300, "protocol": "silo.events.v2"});
            common::json(201, ticket.to_string())
        }
        _ => None,
    });
    let client = Arc::new(common::client(http, root));
    client.set_profile(Some("p1".into()));
    client
}

/// Plays the Server for one connection for each close code in `closes`. Each connection sends `hello`, a favorite
/// event of another Profile, a favorite event of `movie:m<n>` for `p1`, and then a close frame with its code.
fn serve(listener: TcpListener, closes: Vec<CloseCode>) -> (JoinHandle<()>, mpsc::Receiver<Seen>) {
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
            socket.close(Some(CloseFrame { code, reason: "".into() })).unwrap();
            while socket.read().is_ok() {}
        }
    });
    (server, saw)
}

#[test]
fn the_client_subscribes_with_the_ticket_and_connects_again_after_a_planned_close_without_a_resync() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = client(&format!("http://{}/", listener.local_addr().unwrap()));
    // The planned close at the end of the connection time.
    let (server, saw) = serve(listener, vec![CloseCode::Normal, CloseCode::Normal]);

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
    let client = client(&format!("http://{}/", listener.local_addr().unwrap()));
    let (server, saw) = serve(listener, vec![CloseCode::from(4001), CloseCode::Normal]);

    let changes: Vec<_> = block_on(client.live_updates().take(3).collect());
    assert_eq!(changes, [favorite("movie:m1"), LiveChange::Resync, favorite("movie:m2")]);
    server.join().unwrap();
    let last = saw.iter().map(|(_, protocols, _)| protocols).last().unwrap();
    assert_eq!(last, "silo.events.v2, silo.ticket.ticket2", "a new ticket after access_changed");
}
