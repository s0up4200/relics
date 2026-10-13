//! Sign in with a password, refresh after a 401, and read Home, against a fake HTTP client.

use std::sync::{Arc, Mutex};

mod common;

use common::{DEVICE_ID, Reply, Seen, fixture, http, json, not_found, ok, problem, reply, server};
use futures::executor::block_on;
use http_client::HttpClient;
use relics::api::{self, Client, Device, Error, Tokens, wire};
use relics::artwork::Rung;

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
    block_on(client.home(|_| Rung::Small)).unwrap();

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
    let sections = serde_json::json!({"sections": [recent, picks, watching.clone()]});
    // The 16:9 cards need a larger rung, so their section loads again with its own rung.
    watching["items"] = serde_json::json!([item("Heat wide"), item("Ronin wide")]);
    let (http, _) = http(move |req| match req.path.as_str() {
        "home/layout" => json(200, layout.to_string()),
        "home/sections?image_size=small" => json(200, sections.to_string()),
        "home/sections/continue_watching/items?image_size=medium" => json(200, watching.to_string()),
        _ => reply(404, "text/plain", String::new()),
    });
    let client = signed_in(http, "acc");
    client.set_profile(Some("p-owner".into()));
    let size = |entry: &wire::SectionLayoutEntry| {
        if entry.section_type == "continue_watching" { Rung::Medium } else { Rung::Small }
    };
    let home = block_on(client.home(size)).unwrap();
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
            ("continue_watching", false, false, vec!["Heat wide", "Ronin wide"]),
        ]
    );
}

#[test]
fn home_drops_a_section_with_no_items() {
    let layout = serde_json::json!({"sections": [
        section("recommended", "Recommended for You", false),
        section("recently_added", "Recently Added", false),
        section("because_you_watched", "Because You Watched", false),
    ]});
    // The server sends Recommended for You with no items, and does not send Because You Watched.
    let mut recommended = section("recommended", "Recommended for You", false);
    recommended["items"] = serde_json::json!([]);
    recommended["total_count"] = 0.into();
    let mut recent = section("recently_added", "Recently Added", false);
    recent["items"] = serde_json::json!([item("Heat")]);
    recent["total_count"] = 1.into();
    let sections = serde_json::json!({"sections": [recommended, recent]});
    let (http, _) = http(move |req| match req.path.as_str() {
        "home/layout" => json(200, layout.to_string()),
        "home/sections?image_size=small" => json(200, sections.to_string()),
        _ => reply(404, "text/plain", String::new()),
    });
    let client = signed_in(http, "acc");
    client.set_profile(Some("p-owner".into()));
    let home = block_on(client.home(|_| Rung::Small)).unwrap();
    assert_eq!(home.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Recently Added"]);
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

fn providers(extra: serde_json::Value, password_login: bool) -> String {
    let mut list: serde_json::Value = serde_json::from_str(&fixture("list_auth_providers_ok.json")).unwrap();
    list["items"].as_array_mut().unwrap().extend(extra.as_array().unwrap().iter().cloned());
    list["password_login"] = password_login.into();
    list.to_string()
}

fn methods(body: String) -> api::SignInMethods {
    let (http, seen) = http(move |_| json(200, body.clone()));
    let methods = block_on(api::sign_in_methods(&http, &silo(), &device())).unwrap();
    assert_eq!(seen.lock().unwrap()[0].path, "auth/providers");
    methods
}

fn ids(providers: &[wire::AuthProvider]) -> Vec<&str> {
    providers.iter().map(|p| p.id.as_str()).collect()
}

const LDAP: &str =
    r#"{"id": "ldap", "display_name": "Directory", "mode": "credentials", "default": false, "installation_id": "4"}"#;

#[test]
fn only_providers_that_take_a_password_are_offered() {
    let only_local = methods(providers(serde_json::json!([]), true));
    assert_eq!(ids(&only_local.password), ["local"]);
    let ldap: serde_json::Value = serde_json::from_str(LDAP).unwrap();
    assert_eq!(ids(&methods(providers(serde_json::json!([ldap]), true)).password), ["local", "ldap"]);
}

#[test]
fn with_password_sign_in_off_only_a_directory_provider_keeps_the_form() {
    assert!(methods(providers(serde_json::json!([]), false)).password.is_empty());
    let ldap: serde_json::Value = serde_json::from_str(LDAP).unwrap();
    assert_eq!(ids(&methods(providers(serde_json::json!([ldap]), false)).password), ["ldap"]);
}

#[test]
fn the_screen_offers_network_and_each_oidc_provider_with_the_default_first() {
    let extra = serde_json::json!([
        {"id": "plugin-8", "display_name": "Authentik", "mode": "oauth", "default": true, "installation_id": "8"},
        {"id": "plugin-5", "display_name": "Tailscale", "mode": "network", "default": false, "installation_id": "5",
         "network_sign_in_path": "/api/v2/auth/network/5/sign-in",
         "network_identity": {"display_name": "Laura", "username": "laura@example.test"}},
        {"id": "plugin-9", "display_name": "Passkey", "mode": "webauthn", "default": false},
    ]);
    let methods = methods(providers(extra, true));
    assert_eq!(ids(&methods.oauth), ["plugin-8", "plugin-3"]);
    assert_eq!(methods.network.as_ref().map(|p| p.id.as_str()), Some("plugin-5"));
    assert_eq!(ids(&methods.password), ["local"]);
    // The fixture has no network provider: the request did not come through its network.
    assert!(self::methods(providers(serde_json::json!([]), true)).network.is_none());
}

fn provider(mode: &str) -> wire::AuthProvider {
    let list: wire::AuthProviderCollection = serde_json::from_str(&fixture("list_auth_providers_ok.json")).unwrap();
    let mut provider = list.items.into_iter().find(|p| p.mode == "oauth").unwrap();
    if mode == "network" {
        provider.mode = mode.into();
        provider.native_start_path = None;
        provider.network_sign_in_path = Some("/api/v2/auth/network/5/sign-in".into());
    }
    provider
}

/// Starts a Browser sign-in against a Server whose capabilities report `select_account`.
fn start(
    server: api::Server,
    provider: &wire::AuthProvider,
    has_account: bool,
    select_account: bool,
) -> (Option<(api::BrowserSignIn, api::Url)>, Vec<Seen>) {
    let (http, seen) = http(move |req| match req.path.as_str() {
        "auth/oauth/capabilities" => {
            let mut body: serde_json::Value =
                serde_json::from_str(&fixture("get_oauth_handshake_capabilities_ok.json")).unwrap();
            body["select_account"] = select_account.into();
            json(200, body.to_string())
        }
        _ => not_found(),
    });
    let started = block_on(api::BrowserSignIn::start(&http, &server, &device(), provider, has_account)).unwrap();
    let seen = seen.lock().unwrap().clone();
    (started, seen)
}

fn query(url: &api::Url) -> Vec<(String, String)> {
    url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

fn param<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

#[test]
fn browser_sign_in_opens_the_native_start_with_an_s256_challenge() {
    let (started, seen) = start(server("https://silo.test/sub/"), &provider("oauth"), false, true);
    let (sign_in, url) = started.unwrap();
    assert!(seen.is_empty(), "no Account on the Server: no capabilities request");
    assert_eq!(url.as_str().split('?').next(), Some("https://silo.test/sub/api/v2/auth/oauth/3/native/start"));
    let pairs = query(&url);
    assert_eq!(
        pairs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["code_challenge", "code_challenge_method", "app_state"]
    );
    assert_eq!(param(&pairs, "code_challenge_method"), Some("S256"));
    let state = param(&pairs, "app_state").unwrap();
    assert!(state.len() >= 43 && state.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)), "{state}");

    // The verifier that the redemption sends is the one behind the challenge.
    let (http, seen) = http(|_| ok("complete_oauth_login_native_ok.json"));
    let link = format!(
        "org.siloserver.silo:/auth/callback?code=n4tive&state={state}&server=server-1&iss=https%3A%2F%2Fsilo.test"
    );
    assert_eq!(sign_in.check(&link), api::BrowserReturn::Code("n4tive".into()));
    block_on(sign_in.redeem(http, device(), "n4tive")).unwrap();
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
    let verifier = body["code_verifier"].as_str().unwrap();
    use base64::Engine as _;
    use sha2::Digest as _;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    assert_eq!(param(&pairs, "code_challenge"), Some(challenge.as_str()));
}

#[test]
fn a_second_account_asks_for_the_account_chooser_when_the_server_supports_it() {
    let prompt = |has_account, select_account| {
        let (started, _) = start(silo(), &provider("oauth"), has_account, select_account);
        param(&query(&started.unwrap().1), "prompt").map(str::to_string)
    };
    assert_eq!(prompt(true, true).as_deref(), Some("select_account"));
    assert_eq!(prompt(true, false), None);
    assert_eq!(prompt(false, true), None);
    let (_, seen) = start(silo(), &provider("oauth"), true, true);
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("GET", "auth/oauth/capabilities"));
}

#[test]
fn a_provider_without_a_native_start_falls_back_to_sign_in_with_a_code() {
    let mut provider = provider("oauth");
    provider.native_start_path = None;
    let (started, seen) = start(silo(), &provider, true, true);
    assert!(started.is_none());
    assert!(seen.is_empty());
}

#[test]
fn a_native_start_path_to_another_origin_is_refused() {
    let mut provider = provider("oauth");
    provider.native_start_path = Some("https://evil.test/start".into());
    let (http, _) = http(|_| not_found());
    let started = block_on(api::BrowserSignIn::start(&http, &silo(), &device(), &provider, false));
    assert!(matches!(started, Err(Error::InvalidUrl)));
}

/// A Browser sign-in on `https://silo.test/` and its `app_state`.
fn waiting() -> (api::BrowserSignIn, String) {
    let (started, _) = start(silo(), &provider("oauth"), false, false);
    let (sign_in, url) = started.unwrap();
    let state = param(&query(&url), "app_state").unwrap().to_string();
    (sign_in, state)
}

fn link(state: &str, server: &str, iss: &str, result: &str) -> String {
    let mut url = api::Url::parse("org.siloserver.silo:/auth/callback").unwrap();
    url.query_pairs_mut().append_pair("state", state).append_pair("server", server).append_pair("iss", iss);
    format!("{url}&{result}")
}

#[test]
fn a_return_link_of_another_request_server_or_address_is_refused() {
    let (sign_in, state) = waiting();
    let code = api::BrowserReturn::Code("c".into());
    assert_eq!(sign_in.check(&link(&state, "server-1", "https://silo.test", "code=c")), code);
    // Another case and the default port written out are the same origin.
    assert_eq!(sign_in.check(&link(&state, "server-1", "HTTPS://Silo.TEST:443", "code=c")), code);

    assert_eq!(sign_in.check(&link("other", "server-1", "https://silo.test", "code=c")), api::BrowserReturn::NotOurs);
    assert_eq!(sign_in.check("https://silo.test/auth/callback?code=c"), api::BrowserReturn::NotOurs);
    for (server, iss) in [
        ("server-2", "https://silo.test"),
        ("server-1", "https://evil.test"),
        ("server-1", "http://silo.test"),
        ("server-1", "https://silo.test:8443"),
        ("server-1", ""),
    ] {
        let checked = sign_in.check(&link(&state, server, iss, "code=c"));
        assert!(matches!(checked, api::BrowserReturn::Failed(_)), "{server} {iss}: {checked:?}");
    }
}

#[test]
fn a_good_return_link_redeems_the_code_with_the_device_headers() {
    let (sign_in, state) = waiting();
    let api::BrowserReturn::Code(code) = sign_in.check(&link(&state, "server-1", "https://silo.test", "code=n4tive"))
    else {
        panic!("no code")
    };
    let (http, seen) = http(|_| ok("complete_oauth_login_native_ok.json"));
    let (client, account) = block_on(sign_in.redeem(http, device(), &code)).unwrap();
    assert_eq!(account.username, "laura");
    assert_eq!(client.tokens(), Tokens { access_token: "acc".into(), refresh_token: "ref".into() });
    assert_eq!(client.server().id, "server-1");

    let seen = seen.lock().unwrap();
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("POST", "auth/oauth/complete"));
    assert_eq!(seen[0].header("X-Silo-Device-Id"), Some(DEVICE_ID));
    assert_eq!(seen[0].header("X-Silo-Client-Family"), Some("desktop"));
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["code"], "n4tive");
}

#[test]
fn each_error_reason_gives_its_message() {
    let (sign_in, state) = waiting();
    let message = |reason: &str| match sign_in.check(&link(
        &state,
        "server-1",
        "https://silo.test",
        &format!("error={reason}"),
    )) {
        api::BrowserReturn::Failed(text) => text,
        other => panic!("{reason}: {other:?}"),
    };
    assert_eq!(message("account_disabled"), "This account is disabled.");
    assert_eq!(message("account_required"), "No account exists for this sign-in.");
    assert_eq!(message("provider_unavailable"), "The provider is not available. Try again later.");
    assert_eq!(message("session_expired"), api::BROWSER_TIMED_OUT);
    let reasons = ["not_permitted", "email_in_use", "identity_linked_elsewhere", "state_invalid", "login_failed"];
    let texts: std::collections::HashSet<_> = reasons.iter().map(|r| message(r)).collect();
    assert_eq!(texts.len(), reasons.len(), "each reason has its own text");
    assert_eq!(message("something_new"), message("login_failed"));
}

#[test]
fn a_refused_code_gives_a_clear_error() {
    let (sign_in, _) = waiting();
    let redeem = |reply: Reply| {
        let reply = Mutex::new(reply);
        let (http, _) = http(move |_| reply.lock().unwrap().take());
        block_on(sign_in.redeem(http, device(), "n4tive")).err().unwrap()
    };
    assert_eq!(
        redeem(problem(400, "complete_oauth_login_invalid_grant.json")),
        "The sign-in code does not fit this request. Try again."
    );
    let used_code = serde_json::json!({
        "type": "https://siloserver.org/docs/api/v2/problems/invalid_token",
        "title": "Invalid token", "status": 401, "detail": "The code is unknown, used or expired."
    });
    assert_eq!(
        redeem(reply(401, "application/problem+json", used_code.to_string())),
        "The sign-in code is no longer valid. Try again."
    );
    // Another refusal shows the detail of the Server.
    assert_eq!(
        redeem(problem(422, "complete_oauth_login_code_required.json")),
        "The request did not pass validation; see errors."
    );
}

#[test]
fn network_sign_in_posts_an_empty_object_with_the_device_headers() {
    let (http, seen) = http(|_| ok("sign_in_with_network_identity_ok.json"));
    let (client, account) =
        block_on(Client::network_sign_in(http, server("https://silo.test/sub/"), device(), &provider("network")))
            .unwrap();
    assert_eq!(account.username, "laura");
    assert_eq!(client.tokens().access_token, "acc");

    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].url, "https://silo.test/sub/api/v2/auth/network/5/sign-in");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&seen[0].body).unwrap(), serde_json::json!({}));
    assert_eq!(seen[0].header("Content-Type"), Some("application/json"));
    assert_eq!(seen[0].header("X-Silo-Device-Id"), Some(DEVICE_ID));
    assert_eq!(seen[0].header("X-Silo-Device-Platform"), Some("linux"));
    assert_eq!(seen[0].header("Authorization"), None);
}

#[test]
fn a_refused_network_sign_in_shows_the_server_detail() {
    let (http, _) = http(|_| problem(403, "sign_in_with_network_identity_off_overlay.json"));
    let err = block_on(Client::network_sign_in(http, silo(), device(), &provider("network"))).err().unwrap();
    assert_eq!(err.to_string(), "Open this server through the provider's network address to sign in this way");
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
