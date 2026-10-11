//! Sign in with a password, refresh after a 401, and read Home, against a fake HTTP client.

use std::sync::{Arc, Mutex};

use futures::AsyncReadExt as _;
use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use silo_omarchy::api::{self, Client, Device, Error, Server, Tokens, wire};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/silo/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

type Reply = (u16, &'static str, String);

/// A fake Silo server at `https://silo.test/`. `reply` gets each request and returns the reply.
fn fake(reply: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (Arc<dyn HttpClient>, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    let client = FakeHttpClient::create(move |req| {
        let log = log.clone();
        let reply = reply.clone();
        async move {
            let headers = req.headers().iter().map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_string())).collect();
            let method = req.method().to_string();
            let path = req.uri().to_string().strip_prefix("https://silo.test/").unwrap().to_string();
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            let request = Seen { method, path, headers, body };
            log.lock().unwrap().push(request.clone());
            let (status, content_type, body) = reply(&request);
            Ok(Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .body(AsyncBody::from(body))
                .unwrap())
        }
    });
    (client, seen)
}

fn ok(name: &str) -> Reply {
    (200, "application/json", fixture(name))
}

fn problem(status: u16, name: &str) -> Reply {
    (status, "application/problem+json", fixture(name))
}

fn server() -> Server {
    Server { id: "server-1".into(), name: "silo.test".into(), url: Url::parse("https://silo.test/").unwrap() }
}

fn device() -> Device {
    Device { id: "6c1f0d3e-0000-4000-8000-000000000001".into(), name: "omarchy-box".into() }
}

fn signed_in(http: Arc<dyn HttpClient>, access: &str) -> Client {
    Client::new(http, server(), device(), Tokens { access_token: access.into(), refresh_token: "ref".into() })
}

#[test]
fn password_sign_in_sends_the_device_headers_and_returns_the_account() {
    let (http, seen) = fake(|req| match req.path.as_str() {
        "api/v2/auth/login" => ok("login_ok.json"),
        _ => (404, "text/plain", String::new()),
    });
    let (client, account) = block_on(Client::login(http, server(), device(), "laura", "pw")).unwrap();
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
    let (http, _) = fake(|_| problem(401, "login_invalid_credentials.json"));
    let err = block_on(Client::login(http, server(), device(), "laura", "nope")).err().unwrap();
    let Error::Problem(problem) = &err else { panic!("{err:?}") };
    assert_eq!(err.to_string(), problem.detail);
}

#[test]
fn requests_carry_the_token_and_the_profile() {
    let (http, seen) = fake(|req| match req.path.as_str() {
        "api/v2/profiles" => ok("list_profiles_ok.json"),
        "api/v2/home/layout" => ok("get_home_layout_ok.json"),
        "api/v2/home/sections?image_size=small" => ok("list_home_sections_ok.json"),
        _ => (404, "text/plain", String::new()),
    });
    let client = signed_in(http, "acc");
    let profiles = block_on(client.profiles()).unwrap();
    assert_eq!(profiles[0].name, "Laura");
    client.set_profile(Some(profiles[0].id.to_string()));
    block_on(client.home("small")).unwrap();

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
        "api/v2/auth/refresh" if req.body.contains("\"ref\"") => ok("refresh_session_ok.json"),
        "api/v2/auth/refresh" => problem(401, "refresh_session_revoked.json"),
        _ if req.header("Authorization") != Some("Bearer acc2") => problem(401, "token_refresh_required.json"),
        "api/v2/profiles" => ok("list_profiles_ok.json"),
        _ => (404, "text/plain", String::new()),
    }
}

#[test]
fn a_401_refreshes_once_and_retries_once() {
    let (http, seen) = fake(expired_token_server);
    let saved = Arc::new(Mutex::new(Vec::new()));
    let log = saved.clone();
    let client = signed_in(http, "acc");
    client.on_tokens(move |tokens| log.lock().unwrap().push(tokens.clone()));

    let profiles = block_on(client.profiles()).unwrap();
    assert_eq!(profiles.len(), 1);

    let seen = seen.lock().unwrap();
    let paths: Vec<_> = seen.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, ["api/v2/profiles", "api/v2/auth/refresh", "api/v2/profiles"]);
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
    let (http, seen) = fake(|req| match req.path.as_str() {
        "api/v2/auth/refresh" => ok("refresh_session_ok.json"),
        _ => problem(401, "authentication_required.json"),
    });
    let err = block_on(signed_in(http, "acc").profiles()).err().unwrap();
    assert!(matches!(err, Error::Problem(ref p) if p.status == 401), "{err:?}");
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn a_refused_refresh_signs_the_user_out() {
    let (http, seen) = fake(expired_token_server);
    let client =
        Client::new(http, server(), device(), Tokens { access_token: "acc".into(), refresh_token: "revoked".into() });
    let err = block_on(client.profiles()).err().unwrap();
    assert!(matches!(err, Error::SignedOut), "{err:?}");
    assert_eq!(seen.lock().unwrap().len(), 2, "no retry after a refused refresh");
}

#[test]
fn a_refresh_that_gets_no_reply_does_not_sign_the_user_out() {
    let (http, _) = fake(|req| match req.path.as_str() {
        "api/v2/auth/refresh" => problem(503, "refresh_session_provider_unavailable.json"),
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
    let layout =
        serde_json::json!({"sections": [section("picks", "Picks", true), section("recent", "Recently Added", false)]});
    let mut recent = section("recent", "Recently Added", false);
    recent["items"] = serde_json::json!([item("Heat"), item("Ronin")]);
    recent["total_count"] = 9.into();
    let mut picks = section("picks", "Picks", true);
    picks["items"] = serde_json::json!([item("Alien")]);
    picks["total_count"] = 1.into();
    let sections = serde_json::json!({"sections": [recent, picks]});
    let (http, _) = fake(move |req| match req.path.as_str() {
        "api/v2/home/layout" => (200, "application/json", layout.to_string()),
        "api/v2/home/sections?image_size=medium" => (200, "application/json", sections.to_string()),
        _ => (404, "text/plain", String::new()),
    });
    let client = signed_in(http, "acc");
    client.set_profile(Some("p-owner".into()));
    let home = block_on(client.home("medium")).unwrap();
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
    assert_eq!(rows, [("picks", true, false, vec!["Alien"]), ("recent", false, true, vec!["Heat", "Ronin"])]);
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
