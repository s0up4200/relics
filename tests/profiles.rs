//! Choose a Profile, check its PIN, and unlock it after a 403, against a fake HTTP client.

use std::sync::{Arc, Mutex};

use futures::FutureExt as _;
use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use silo_omarchy::api::{self, Client, Device, Error, PinCheck, Server, Tokens, wire};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/silo/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// What the fake server saw: the path, the `X-Profile-Token` header and the body.
type Seen = (String, Option<String>, String);

fn reply(status: u16, content_type: &str, body: String) -> Response<AsyncBody> {
    Response::builder().status(status).header("Content-Type", content_type).body(AsyncBody::from(body)).unwrap()
}

fn ok(name: &str) -> Response<AsyncBody> {
    reply(200, "application/json", fixture(name))
}

fn problem(status: u16, name: &str) -> Response<AsyncBody> {
    reply(status, "application/problem+json", fixture(name))
}

/// A fake Silo server at `https://silo.test/`. `answer` gets what the server saw and returns the reply.
fn client(answer: impl Fn(&Seen) -> Response<AsyncBody> + Send + Sync + 'static) -> (Client, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let answer = Arc::new(answer);
    let http: Arc<dyn HttpClient> = FakeHttpClient::create(move |req| {
        let (log, answer) = (log.clone(), answer.clone());
        async move {
            use futures::AsyncReadExt as _;
            let path = req.uri().to_string().strip_prefix("https://silo.test/api/v2/").unwrap().to_string();
            let token = req.headers().get("X-Profile-Token").map(|v| v.to_str().unwrap().to_string());
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            let seen = (path, token, body);
            log.lock().unwrap().push(seen.clone());
            Ok(answer(&seen))
        }
    });
    let server =
        Server { id: "server-1".into(), name: "silo.test".into(), url: Url::parse("https://silo.test/").unwrap() };
    let device = Device { id: "device-1".into(), name: "omarchy-box".into() };
    let tokens = Tokens { access_token: "acc".into(), refresh_token: "ref".into() };
    (Client::new(http, server, device, tokens), seen)
}

#[test]
fn a_right_pin_unlocks_the_profile_and_a_wrong_one_does_not() {
    let (client, seen) = client(|(_, _, body)| {
        if body.contains("1234") { ok("verify_profile_pin_ok.json") } else { ok("verify_profile_pin_wrong.json") }
    });
    assert_eq!(block_on(client.verify_pin("p-kid", "1234")).unwrap(), PinCheck::Unlocked("pvt_fixture".into()));
    assert_eq!(block_on(client.verify_pin("p-kid", "0000")).unwrap(), PinCheck::Wrong);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0, "profiles/p-kid/verify-pin");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&seen[0].2).unwrap(), serde_json::json!({"pin": "1234"}));
}

#[test]
fn a_locked_profile_reports_the_seconds_left_on_the_lock() {
    let (client, _) = client(|_| {
        let mut locked = problem(429, "rate_limited.json");
        locked.headers_mut().insert("Retry-After", "241".parse().unwrap());
        locked
    });
    assert_eq!(block_on(client.verify_pin("p-kid", "1234")).unwrap(), PinCheck::Locked(Some(241)));
    assert_eq!(api::lock_message(Some(241)), "Too many wrong PINs. Try again in 5 minutes.");
    assert_eq!(api::lock_message(Some(30)), "Too many wrong PINs. Try again in 1 minute.");
    assert_eq!(api::lock_message(None), "Too many wrong PINs. Try again later.");
}

#[test]
fn a_profile_verification_required_asks_for_the_pin_and_sends_the_request_again() {
    let (client, seen) = client(|(path, token, _)| match (path.as_str(), token.as_deref()) {
        ("profiles", Some("pvt_fixture")) => ok("list_profiles_ok.json"),
        _ => problem(403, "profile_verification_required.json"),
    });
    client.set_profile(Some("p-kid".into()));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    client.on_pin_required(move |profile_id| {
        log.lock().unwrap().push(profile_id);
        async { Some("pvt_fixture".to_string()) }.boxed()
    });
    let profiles = block_on(client.profiles()).unwrap();
    assert_eq!(profiles[0].name, "Laura");
    assert_eq!(*asked.lock().unwrap(), ["p-kid"]);
    let tokens: Vec<_> = seen.lock().unwrap().iter().map(|(_, token, _)| token.clone()).collect();
    assert_eq!(tokens, [None, Some("pvt_fixture".into())]);
}

#[test]
fn a_cancelled_pin_entry_returns_the_error() {
    let (client, seen) = client(|_| problem(403, "profile_verification_required.json"));
    client.set_profile(Some("p-kid".into()));
    client.on_pin_required(|_| async { None }.boxed());
    let err = block_on(client.profiles()).err().unwrap();
    assert!(matches!(&err, Error::Problem(p) if p.code() == "profile_verification_required"), "{err:?}");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn the_saved_profile_token_goes_out_with_each_request() {
    let (client, seen) = client(|_| ok("list_profiles_ok.json"));
    client.set_profile(Some("p-kid".into()));
    client.set_profile_token(Some("pvt_saved".into()));
    block_on(client.profiles()).unwrap();
    client.set_profile(Some("p-owner".into()));
    block_on(client.profiles()).unwrap();
    let tokens: Vec<_> = seen.lock().unwrap().iter().map(|(_, token, _)| token.clone()).collect();
    assert_eq!(tokens, [Some("pvt_saved".into()), None], "a new Profile drops the token of the old one");
}

#[test]
fn launch_opens_the_last_profile_when_it_still_exists() {
    let profiles: wire::ProfileCollection = serde_json::from_str(&fixture("list_profiles_ok.json")).unwrap();
    let mut two = profiles.items.clone();
    two.push(wire::Profile { id: "p-kid".parse().unwrap(), has_pin: true, ..two[0].clone() });
    let id = |p: Option<&wire::Profile>| p.map(|p| p.id.to_string());
    assert_eq!(id(api::launch_profile(&two, Some("p-kid"))), Some("p-kid".into()));
    assert_eq!(id(api::launch_profile(&two, Some("p-gone"))), None, "a gone Profile shows the picker");
    assert_eq!(id(api::launch_profile(&two, None)), None);
    assert_eq!(id(api::launch_profile(&profiles.items, Some("p-gone"))), Some("p-owner".into()));
}
