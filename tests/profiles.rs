//! Choose a Profile, check its PIN, and unlock it after a 403, against a fake HTTP client.

use std::sync::{Arc, Mutex};

mod common;

use common::{fixture, http, ok, problem};
use futures::FutureExt as _;
use futures::executor::block_on;
use relics::api::{self, Client, Error, PinCheck, wire};

/// A fake Silo server at `https://silo.test/` and a client of it with no Profile.
fn client(reply: impl Fn(&common::Seen) -> common::Reply + Send + Sync + 'static) -> (Client, common::Log) {
    let (http, seen) = http(reply);
    (common::client(http, "https://silo.test/"), seen)
}

#[test]
fn a_right_pin_unlocks_the_profile_and_a_wrong_one_does_not() {
    let (client, seen) = client(|req| {
        if req.body.contains("1234") { ok("verify_profile_pin_ok.json") } else { ok("verify_profile_pin_wrong.json") }
    });
    assert_eq!(block_on(client.verify_pin("p-kid", "1234")).unwrap(), PinCheck::Unlocked("pvt_fixture".into()));
    assert_eq!(block_on(client.verify_pin("p-kid", "0000")).unwrap(), PinCheck::Wrong);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].path, "profiles/p-kid/verify-pin");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&seen[0].body).unwrap(), serde_json::json!({"pin": "1234"}));
}

#[test]
fn a_locked_profile_reports_the_seconds_left_on_the_lock() {
    let (client, _) = client(|_| {
        let mut locked = problem(429, "rate_limited.json").unwrap();
        locked.headers_mut().insert("Retry-After", "241".parse().unwrap());
        Some(locked)
    });
    assert_eq!(block_on(client.verify_pin("p-kid", "1234")).unwrap(), PinCheck::Locked(Some(241)));
    assert_eq!(api::lock_message(Some(241)), "Too many wrong PINs. Try again in 5 minutes.");
    assert_eq!(api::lock_message(Some(30)), "Too many wrong PINs. Try again in 1 minute.");
    assert_eq!(api::lock_message(None), "Too many wrong PINs. Try again later.");
}

#[test]
fn a_profile_verification_required_asks_for_the_pin_and_sends_the_request_again() {
    let (client, seen) = client(|req| match (req.path.as_str(), req.header("X-Profile-Token")) {
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
    let tokens: Vec<_> =
        seen.lock().unwrap().iter().map(|req| req.header("X-Profile-Token").map(String::from)).collect();
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
    let tokens: Vec<_> =
        seen.lock().unwrap().iter().map(|req| req.header("X-Profile-Token").map(String::from)).collect();
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
