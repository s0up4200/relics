//! The Silo API client. It sends each request through the GPUI `HttpClient`, so the
//! app adds no second async runtime, and tests can give it a fake client.

use std::sync::Arc;

use futures::AsyncReadExt as _;
use gpui::http_client::{AsyncBody, HttpClient, Request, Url};
use serde::de::DeserializeOwned;

pub mod types;
pub mod wire;

pub use types::Problem;

/// A Silo server that the user added. The app knows it by `id`, the stable id that
/// the server reports, not by its URL.
#[derive(Debug, Clone)]
pub struct Server {
    pub id: String,
    /// The host of `url`, as the app shows it.
    pub name: String,
    /// The base URL, with the path and a trailing slash. The API is at `api/v2/` below it.
    pub url: Url,
}

#[derive(Debug)]
pub enum Error {
    /// The text that the user typed is not an http or https URL with a host.
    InvalidUrl,
    /// The server has no `/api/v2`.
    TooOld,
    /// The request got no reply. The value is the server name.
    Network(String),
    /// The server refused the request with a problem reply.
    Problem(Problem),
    /// The server replied with an error status and no problem body.
    Status(u16),
    /// The reply body does not match the API.
    Decode(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidUrl => f.write_str("Type the address of a Silo server, for example https://silo.example.com"),
            Error::TooOld => f.write_str("This Silo server is too old for silo-omarchy"),
            Error::Network(name) => write!(f, "Can't reach {name}"),
            Error::Problem(problem) => f.write_str(if problem.detail.is_empty() { &problem.title } else { &problem.detail }),
            Error::Status(status) => write!(f, "The server answered with HTTP {status}"),
            Error::Decode(err) => write!(f, "The server sent a reply that silo-omarchy cannot read: {err}"),
        }
    }
}

impl std::error::Error for Error {}

/// Connects to the Silo server at the URL that the user typed. A URL without a
/// scheme tries `https://` first and `http://` when https gets no reply.
pub async fn connect(http: Arc<dyn HttpClient>, input: &str) -> Result<Server, Error> {
    let input = input.trim();
    let candidates = if input.contains("://") {
        vec![input.to_string()]
    } else {
        vec![format!("https://{input}"), format!("http://{input}")]
    };
    let mut result = Err(Error::InvalidUrl);
    for candidate in candidates {
        result = connect_to(&http, base_url(&candidate)?).await;
        if !matches!(result, Err(Error::Network(_))) {
            break;
        }
    }
    result
}

async fn connect_to(http: &Arc<dyn HttpClient>, url: Url) -> Result<Server, Error> {
    let client = Client { http: http.clone(), url };
    let _: wire::SystemInfo = client.get("system/info").await.map_err(|err| match err {
        Error::Status(404) => Error::TooOld,
        err => err,
    })?;
    let identity: wire::ServerIdentity = client.get("system/identity").await?;
    Ok(Server {
        id: identity.server_id,
        name: client.name(),
        url: client.url,
    })
}

/// Parses a base URL. It keeps the path, adds a trailing slash, and drops the query.
fn base_url(text: &str) -> Result<Url, Error> {
    let mut url = Url::parse(text).map_err(|_| Error::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(Error::InvalidUrl);
    }
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

struct Client {
    http: Arc<dyn HttpClient>,
    url: Url,
}

impl Client {
    fn name(&self) -> String {
        self.url.host_str().unwrap_or_default().to_string()
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let url = self.url.join("api/v2/").and_then(|api| api.join(path)).map_err(|_| Error::InvalidUrl)?;
        let request = Request::get(url.as_str())
            .header("Accept", "application/json")
            .body(AsyncBody::empty())
            .map_err(|_| Error::InvalidUrl)?;
        let response = self.http.send(request).await.map_err(|_| Error::Network(self.name()))?;
        let status = response.status();
        let is_problem = response
            .headers()
            .get("content-type")
            .is_some_and(|value| value.as_bytes().starts_with(b"application/problem+json"));
        let mut body = Vec::new();
        response
            .into_body()
            .read_to_end(&mut body)
            .await
            .map_err(|_| Error::Network(self.name()))?;
        if status.is_success() {
            serde_json::from_slice(&body).map_err(|err| Error::Decode(err.to_string()))
        } else if is_problem && let Ok(problem) = serde_json::from_slice(&body) {
            Err(Error::Problem(problem))
        } else {
            Err(Error::Status(status.as_u16()))
        }
    }
}
