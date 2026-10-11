//! Connect to a Server through the Silo API client, against a fake HTTP client.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response};
use silo_omarchy::api::{self, Error};

const SERVER_ID: &str = "3f2a9d5e-6b1c-4c7e-9a0d-2f4b8c1e7a35";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/silo/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A fake Silo server. `reply` maps each requested URL to a response, or to `None` for a network failure.
/// The returned list records each requested URL.
fn fake(
    reply: impl Fn(&str) -> Option<(u16, &'static str, String)> + Send + Sync + 'static,
) -> (Arc<dyn HttpClient>, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    let client = FakeHttpClient::create(move |req| {
        let url = req.uri().to_string();
        log.lock().unwrap().push(url.clone());
        let reply = reply.clone();
        async move {
            let (status, content_type, body) =
                reply(&url).ok_or_else(|| anyhow::anyhow!("connection refused"))?;
            Ok(Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .body(AsyncBody::from(body))
                .unwrap())
        }
    });
    (client, seen)
}

/// A Silo server that answers at `root` (for example `https://silo.test/`).
fn silo_at(root: &'static str) -> impl Fn(&str) -> Option<(u16, &'static str, String)> {
    move |url| {
        let path = url.strip_prefix(root)?;
        match path {
            "api/v2/system/info" => Some((200, "application/json", fixture("get_system_info_ok.json"))),
            "api/v2/system/identity" => Some((200, "application/json", fixture("server_identity_ok.json"))),
            _ => Some((404, "text/plain", "404 page not found".into())),
        }
    }
}

#[test]
fn connects_and_names_the_server_by_its_host() {
    let (http, _) = fake(silo_at("https://silo.test/"));
    let server = block_on(api::connect(http, "https://silo.test")).unwrap();
    assert_eq!(server.id, SERVER_ID);
    assert_eq!(server.name, "silo.test");
    assert_eq!(server.url.as_str(), "https://silo.test/");
}

#[test]
fn a_url_with_a_path_keeps_the_path() {
    let (http, seen) = fake(silo_at("https://home.test/media/silo/"));
    let server = block_on(api::connect(http, " https://home.test/media/silo/ ")).unwrap();
    assert_eq!(server.url.as_str(), "https://home.test/media/silo/");
    assert_eq!(server.name, "home.test");
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "https://home.test/media/silo/api/v2/system/info",
            "https://home.test/media/silo/api/v2/system/identity",
        ]
    );
}

#[test]
fn a_url_without_a_scheme_tries_https_first() {
    let (http, seen) = fake(silo_at("https://silo.test:8443/"));
    let server = block_on(api::connect(http, "silo.test:8443")).unwrap();
    assert_eq!(server.url.as_str(), "https://silo.test:8443/");
    assert_eq!(seen.lock().unwrap()[0], "https://silo.test:8443/api/v2/system/info");
}

#[test]
fn a_url_without_a_scheme_falls_back_to_http_when_https_does_not_answer() {
    let (http, seen) = fake(silo_at("http://192.168.1.20:8080/"));
    let server = block_on(api::connect(http, "192.168.1.20:8080")).unwrap();
    assert_eq!(server.url.as_str(), "http://192.168.1.20:8080/");
    assert_eq!(server.name, "192.168.1.20");
    assert_eq!(seen.lock().unwrap()[0], "https://192.168.1.20:8080/api/v2/system/info");
}

#[test]
fn a_url_with_a_scheme_does_not_fall_back() {
    let (http, seen) = fake(|_| None);
    let err = block_on(api::connect(http, "https://silo.test")).unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err:?}");
    assert_eq!(err.to_string(), "Can't reach silo.test");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn a_plain_404_means_the_server_is_too_old_and_is_not_retried() {
    let (http, seen) = fake(|_| Some((404, "text/plain", "404 page not found".into())));
    let err = block_on(api::connect(http, "silo.test")).unwrap_err();
    assert!(matches!(err, Error::TooOld), "{err:?}");
    assert_eq!(err.to_string(), "This Silo server is too old for silo-omarchy");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn a_problem_reply_shows_its_detail() {
    let (http, _) = fake(|_| Some((422, "application/problem+json", fixture("unknown_query_parameter.json"))));
    let err = block_on(api::connect(http, "https://silo.test")).unwrap_err();
    let Error::Problem(problem) = &err else { panic!("{err:?}") };
    assert_eq!(problem.code(), "validation_failed");
    assert_eq!(err.to_string(), problem.detail);
}

#[test]
fn text_that_is_not_a_url_is_refused_before_any_request() {
    let (http, seen) = fake(|_| None);
    for input in ["", "   ", "ftp://silo.test", "https://"] {
        let err = block_on(api::connect(http.clone(), input)).unwrap_err();
        assert!(matches!(err, Error::InvalidUrl), "{input:?}: {err:?}");
    }
    assert!(seen.lock().unwrap().is_empty());
}
