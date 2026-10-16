//! The setup that the integration tests share: the server fixtures and a fake Silo server. The fake answers
//! in memory through `FakeHttpClient`, so no test sends a request to a real host.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use futures::AsyncReadExt as _;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use relics::api::{Client, Device, Server, Tokens};

/// The device id of each test client.
pub const DEVICE_ID: &str = "6c1f0d3e-0000-4000-8000-000000000001";

/// A fixture file below `tests/fixtures/silo/`. `../own/<name>` reads one of our own fixtures.
pub fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/silo/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// One request that the fake server got.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// The full URL.
    pub url: String,
    /// The part of the URL after `api/v2/`, for example `home/layout`, or the full URL when it has no `api/v2/`.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The reply to a request, or `None` for no reply (a network failure).
pub type Reply = Option<Response<AsyncBody>>;

pub type Log = Arc<Mutex<Vec<Seen>>>;

pub fn ok(name: &str) -> Reply {
    json(200, fixture(name))
}

pub fn json(status: u16, body: String) -> Reply {
    reply(status, "application/json", body)
}

pub fn reply(status: u16, content_type: &str, body: String) -> Reply {
    Some(Response::builder().status(status).header("Content-Type", content_type).body(AsyncBody::from(body)).unwrap())
}

/// A problem reply with the body of a fixture.
pub fn problem(status: u16, name: &str) -> Reply {
    reply(status, "application/problem+json", fixture(name))
}

pub fn not_found() -> Reply {
    json(404, String::new())
}

/// A fake HTTP host. `reply` gets each request and returns the reply. The log keeps each request.
pub fn http(reply: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (Arc<dyn HttpClient>, Log) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    let http = FakeHttpClient::create(move |req| {
        let (log, reply) = (log.clone(), reply.clone());
        async move {
            let method = req.method().to_string();
            let url = req.uri().to_string();
            let path = url.split_once("api/v2/").map_or(url.as_str(), |(_, path)| path).to_string();
            let headers = req.headers().iter().map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_string())).collect();
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            let request = Seen { method, url, path, headers, body };
            log.lock().unwrap().push(request.clone());
            reply(&request).ok_or_else(|| anyhow::anyhow!("no route to host"))
        }
    });
    (http, seen)
}

/// The Server at `root`, for example `https://silo.test/`.
pub fn server(root: &str) -> Server {
    let url = Url::parse(root).unwrap();
    Server { id: "server-1".into(), name: url.host_str().unwrap().into(), url }
}

/// A client of the Server at `root` with the tokens `acc` and `ref`, and no Profile.
pub fn client(http: Arc<dyn HttpClient>, root: &str) -> Client {
    let device = Device { id: DEVICE_ID.into(), name: "omarchy-box".into() };
    Client::new(http, server(root), device, Tokens { access_token: "acc".into(), refresh_token: "ref".into() })
}

/// A fake Silo server at `https://silo.test/` and a client of it with the Profile `p-owner`.
pub fn fake(reply: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (Client, Log) {
    let (http, seen) = http(reply);
    let client = client(http, "https://silo.test/");
    client.set_profile(Some("p-owner".into()));
    (client, seen)
}

/// A JWT whose payload has this `exp`. The signature is not checked.
pub fn jwt(exp: u64) -> String {
    use base64::Engine as _;
    let part = |json: serde_json::Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.to_string());
    let (header, payload) = (serde_json::json!({ "alg": "HS256" }), serde_json::json!({ "sub": "u1", "exp": exp }));
    format!("{}.{}.sig", part(header), part(payload))
}

/// Filesystem and artwork isolation for GPUI page and Library-window tests.
pub struct Storage(pub std::path::PathBuf);
impl gpui::Global for Storage {}
impl Drop for Storage {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub fn isolate(cx: &mut gpui::App) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-state")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    relics::settings::use_directory(dir.clone(), cx);
    let (http, _) = http(|_| not_found());
    relics::artwork::install(http, dir.join("artwork"), cx);
    cx.set_global(Storage(dir.clone()));
    dir
}
