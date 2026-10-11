//! Sign in with a password, refresh after a 401, and read Home, against a fake HTTP client.

use std::sync::{Arc, Mutex};

mod common;

use common::{DEVICE_ID, Reply, Seen, fixture, http, json, ok, problem, reply, server};
use futures::executor::block_on;
use http_client::HttpClient;
use silo_omarchy::api::{self, Client, Device, Error, Tokens, wire};
use silo_omarchy::artwork::Rung;

fn device() -> Device {
    Device { id: DEVICE_ID.into(), name: "omarchy-box".into() }
}

fn signed_in(http: Arc<dyn HttpClient>, access: &str) -> Client {
    Client::new(http, silo(), device(), Tokens { access_token: access.into(), refresh_token: "ref".into() })
}

fn silo() -> api::Server {
    server("https://silo.test/")
}

#[test]
fn password_sign_in_sends_the_device_headers_and_returns_the_account() {
    let (http, seen) = http(|req| match req.path.as_str() {
        "auth/login" => ok("login_ok.json"),
        _ => reply(404, "text/plain", String::new()),
    });
    let (client, account) = block_on(Client::login(http, silo(), device(), "laura", "pw", None)).unwrap();
    assert_eq!(account.username, "laura");
    assert_eq!(*account.id, "1");
    assert_eq!(client.tokens().access_token, "acc");

    let seen = seen.lock().unwrap();
    let login = &seen[0];
    assert_eq!(login.method, "POST");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&login.body).unwrap(),
        serde_json::json!({"username": "laura", "password": "pw"})
    );
    assert_eq!(login.header("X-Silo-Device-Id"), Some("6c1f0d3e-0000-4000-8000-000000000001"));
    assert_eq!(login.header("X-Silo-Device-Name"), Some("omarchy-box"));
    assert_eq!(login.header("X-Silo-Device-Platform"), Some("linux"));
    assert_eq!(login.header("X-Silo-Client-Family"), Some("desktop"));
    assert_eq!(login.header("Authorization"), None);
}

#[test]
fn wrong_credentials_show_the_server_detail() {
    let (http, _) = http(|_| problem(401, "login_invalid_credentials.json"));
    let err = block_on(Client::login(http, silo(), device(), "laura", "nope", None)).err().unwrap();
    let Error::Problem(problem) = &err else { panic!("{err:?}") };
    assert_eq!(err.to_string(), problem.detail);
}

#[test]
fn requests_carry_the_token_and_the_profile() {
    let (http, seen) = http(|req| match req.path.as_str() {
        "profiles" => ok("list_profiles_ok.json"),
        "home/layout" => ok("get_home_layout_ok.json"),
        "home/sections?image_size=small" => ok("list_home_sections_ok.json"),
        _ => reply(404, "text/plain", String::new()),
    });
    let client = signed_in(http, "acc");
    let profiles = block_on(client.profiles()).unwrap();
    assert_eq!(profiles[0].name, "Laura");
    client.set_profile(Some(profiles[0].id.to_string()));
    block_on(client.home(Rung::Small)).unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].header("Authorization"), Some("Bearer acc"));
    assert_eq!(seen[0].header("X-Profile-Id"), None);
    assert_eq!(seen[0].header("X-Silo-Device-Id"), Some("6c1f0d3e-0000-4000-8000-000000000001"));
    assert_eq!(seen[0].header("X-Silo-Client-Family"), Some("desktop"));
    assert_eq!(seen[1].header("Authorization"), Some("Bearer acc"));
    assert_eq!(seen[1].header("X-Profile-Id"), Some("p-owner"));
}

/// A server where only the token `acc2` is valid, and the refresh token `ref` gives `acc2`.
fn expired_token_server(req: &Seen) -> Reply {
    match req.path.as_str() {
        "auth/refresh" if req.body.contains("\"ref\"") => ok("refresh_session_ok.json"),
        "auth/refresh" => problem(401, "refresh_session_revoked.json"),
        _ if req.header("Authorization") != Some("Bearer acc2") => problem(401, "token_refresh_required.json"),
        "profiles" => ok("list_profiles_ok.json"),
        _ => reply(404, "text/plain", String::new()),
    }
}

#[test]
fn a_401_refreshes_once_and_retries_once() {
    let (http, seen) = http(expired_token_server);
    let saved = Arc::new(Mutex::new(Vec::new()));
    let log = saved.clone();
    let client = signed_in(http, "acc");
    client.on_tokens(move |tokens| log.lock().unwrap().push(tokens.clone()));

    let profiles = block_on(client.profiles()).unwrap();
    assert_eq!(profiles.len(), 1);

    let seen = seen.lock().unwrap();
    let paths: Vec<_> = seen.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, ["profiles", "auth/refresh", "profiles"]);
    assert_eq!(seen[1].method, "POST");
    assert_eq!(seen[1].header("Authorization"), None);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&seen[1].body).unwrap(),
        serde_json::json!({"refresh_token": "ref"})
    );
    assert_eq!(seen[2].header("Authorization"), Some("Bearer acc2"));
    assert_eq!(client.tokens().refresh_token, "ref2");
    assert_eq!(saved.lock().unwrap().len(), 1, "the new tokens are saved once");
    assert_eq!(saved.lock().unwrap()[0].access_token, "acc2");
}

#[test]
fn a_second_401_after_the_refresh_is_not_retried_again() {
    let (http, seen) = http(|req| match req.path.as_str() {
        "auth/refresh" => ok("refresh_session_ok.json"),
        _ => problem(401, "authentication_required.json"),
    });
    let err = block_on(signed_in(http, "acc").profiles()).err().unwrap();
    assert!(matches!(err, Error::Problem(ref p) if p.status == 401), "{err:?}");
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn a_refused_refresh_signs_the_user_out() {
    let (http, seen) = http(expired_token_server);
    let client =
        Client::new(http, silo(), device(), Tokens { access_token: "acc".into(), refresh_token: "revoked".into() });
    let err = block_on(client.profiles()).err().unwrap();
    assert!(matches!(err, Error::SignedOut), "{err:?}");
    assert_eq!(seen.lock().unwrap().len(), 2, "no retry after a refused refresh");
}

#[test]
fn a_refresh_that_gets_no_reply_does_not_sign_the_user_out() {
    let (http, _) = http(|req| match req.path.as_str() {
        "auth/refresh" => problem(503, "refresh_session_provider_unavailable.json"),
        _ => problem(401, "token_refresh_required.json"),
    });
    let err = block_on(signed_in(http, "acc").profiles()).err().unwrap();
    assert!(!matches!(err, Error::SignedOut), "{err:?}");
}

fn item(title: &str) -> serde_json::Value {
    serde_json::json!({"content_id": title, "type": "movie", "title": title, "genres": [], "keywords": [], "status": "available"})
}

fn section(id: &str, title: &str, featured: bool) -> serde_json::Value {
    serde_json::json!({"id": id, "section_type": id, "title": title, "featured": featured, "item_limit": 2, "is_custom": false, "customized": false})
}

#[test]
fn home_follows_the_layout_order_and_marks_featured_and_view_all() {
    let layout = serde_json::json!({"sections": [
        section("picks", "Picks", true),
        section("recently_added", "Recently Added", false),
        section("continue_watching", "Continue Watching", false),
    ]});
    let mut recent = section("recently_added", "Recently Added", false);
    recent["items"] = serde_json::json!([item("Heat"), item("Ronin")]);
    recent["total_count"] = 9.into();
    let mut picks = section("picks", "Picks", true);
    picks["items"] = serde_json::json!([item("Alien")]);
    picks["total_count"] = 1.into();
    // The server has no full list of Continue watching, so it has no View all.
    let mut watching = section("continue_watching", "Continue Watching", false);
    watching["items"] = serde_json::json!([item("Heat"), item("Ronin")]);
    watching["total_count"] = 9.into();
    let sections = serde_json::json!({"sections": [recent, picks, watching]});
    let (http, _) = http(move |req| match req.path.as_str() {
        "home/layout" => json(200, layout.to_string()),
        "home/sections?image_size=medium" => json(200, sections.to_string()),
        _ => reply(404, "text/plain", String::new()),
    });
    let client = signed_in(http, "acc");
    client.set_profile(Some("p-owner".into()));
    let home = block_on(client.home(Rung::Medium)).unwrap();
    let rows: Vec<_> = home
        .iter()
        .map(|s| {
            (
                s.section_type.as_str(),
                s.featured,
                s.view_all,
                s.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("picks", true, false, vec!["Alien"]),
            ("recently_added", false, true, vec!["Heat", "Ronin"]),
            ("continue_watching", false, false, vec!["Heat", "Ronin"]),
        ]
    );
}

fn profile(id: &str, has_pin: bool) -> wire::Profile {
    let mut profile: serde_json::Value =
        serde_json::from_str::<serde_json::Value>(&fixture("list_profiles_ok.json")).unwrap()["items"][0].clone();
    profile["id"] = id.into();
    profile["has_pin"] = has_pin.into();
    serde_json::from_value(profile).unwrap()
}

#[test]
fn only_a_single_profile_without_a_pin_opens_without_the_picker() {
    let id = |profiles: &[wire::Profile]| api::auto_profile(profiles).map(|p| p.id.to_string());
    assert_eq!(id(&[profile("a", false)]).as_deref(), Some("a"));
    assert_eq!(id(&[profile("a", true)]), None);
    assert_eq!(id(&[profile("a", false), profile("b", false)]), None);
    assert_eq!(id(&[]), None);
}

#[test]
fn a_chosen_provider_goes_with_the_password() {
    let (http, seen) = http(|_| ok("login_ok.json"));
    block_on(Client::login(http, silo(), device(), "laura", "pw", Some("ldap"))).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&seen.lock().unwrap()[0].body).unwrap(),
        serde_json::json!({"username": "laura", "password": "pw", "provider": "ldap"})
    );
}

#[test]
fn only_providers_that_take_a_password_are_offered() {
    let mut list: serde_json::Value = serde_json::from_str(&fixture("list_auth_providers_ok.json")).unwrap();
    let only_local = list.to_string();
    list["items"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id": "ldap", "display_name": "Directory", "mode": "credentials", "default": false}));
    let with_ldap = list.to_string();
    let ids = |body: String| {
        let (http, seen) = http(move |_| json(200, body.clone()));
        let providers = block_on(api::password_providers(&http, &silo(), &device())).unwrap();
        assert_eq!(seen.lock().unwrap()[0].path, "auth/providers");
        providers.into_iter().map(|p| p.id).collect::<Vec<_>>()
    };
    assert_eq!(ids(only_local), ["local"]);
    assert_eq!(ids(with_ldap), ["local", "ldap"]);
}

#[test]
fn device_sign_in_starts_with_the_device_name_and_shows_the_code_in_two_groups() {
    let (http, seen) = http(|_| json(201, fixture("start_device_login_ok.json")));
    let start = block_on(api::start_device_login(&http, &silo(), &device())).unwrap();
    assert_eq!(start.device_code, "dev-1");
    assert_eq!(api::user_code(&start.user_code), "4821 7730");

    let seen = seen.lock().unwrap();
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("POST", "auth/device/start"));
    assert_eq!(seen[0].header("X-Silo-Device-Id"), Some("6c1f0d3e-0000-4000-8000-000000000001"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&seen[0].body).unwrap(),
        serde_json::json!({"device_name": "omarchy-box", "device_platform": "linux"})
    );
}

fn poll(reply: Reply) -> (Result<api::DeviceLogin, Error>, Vec<Seen>) {
    let reply = Mutex::new(reply);
    let (http, seen) = http(move |_| reply.lock().unwrap().take());
    let result = block_on(api::poll_device_login(http, silo(), device(), "dev-1"));
    let seen = seen.lock().unwrap().clone();
    (result, seen)
}

fn poll_status(status: &str) -> Reply {
    let mut body: serde_json::Value = serde_json::from_str(&fixture("poll_device_login_opened.json")).unwrap();
    body["status"] = status.into();
    body["opened"] = false.into();
    json(200, body.to_string())
}

#[test]
fn device_poll_waits_for_the_approval_and_then_signs_in() {
    let (result, seen) = poll(ok("poll_device_login_opened.json"));
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("POST", "auth/device/poll"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&seen[0].body).unwrap(),
        serde_json::json!({"device_code": "dev-1"})
    );
    let Ok(api::DeviceLogin::Waiting { opened, poll_after }) = result else { panic!("not waiting") };
    assert!(opened);
    assert_eq!(poll_after, std::time::Duration::from_secs(5));

    let Ok(api::DeviceLogin::SignedIn(client, account)) = poll(ok("poll_device_login_ok.json")).0 else {
        panic!("not signed in")
    };
    assert_eq!(account.username, "laura");
    assert_eq!(client.tokens(), Tokens { access_token: "acc".into(), refresh_token: "ref".into() });
}

#[test]
fn a_denied_or_dead_code_ends_device_sign_in() {
    let ended = |status: &str| match poll(poll_status(status)).0 {
        Ok(api::DeviceLogin::Ended(message)) => message,
        _ => panic!("{status} did not end"),
    };
    assert_eq!(ended("denied"), "Approval was denied. Start a new code to try again.");
    for status in ["expired", "consumed", "canceled"] {
        assert_eq!(ended(status), "This code is no longer valid. Start a new one.");
    }
    assert!(matches!(poll(poll_status("pending")).0, Ok(api::DeviceLogin::Waiting { opened: false, .. })));
}
