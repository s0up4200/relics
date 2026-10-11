//! The Silo API client. It sends each request through the GPUI `HttpClient`, so the
//! app adds no second async runtime, and tests can give it a fake client.

use std::sync::{Arc, Mutex};

use futures::AsyncReadExt as _;
use gpui::http_client::{AsyncBody, HttpClient, Method, Request};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub mod types;
pub mod wire;

pub use gpui::http_client::Url;
pub use types::{Decision, Problem};

use crate::playback::Sample;

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
    /// The server refused the refresh token. The user must sign in again.
    SignedOut,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidUrl => f.write_str("Type the address of a Silo server, for example https://silo.example.com"),
            Error::TooOld => f.write_str("This Silo server is too old for silo-omarchy"),
            Error::Network(name) => write!(f, "Can't reach {name}"),
            Error::Problem(problem) => {
                f.write_str(if problem.detail.is_empty() { &problem.title } else { &problem.detail })
            }
            Error::Status(status) => write!(f, "The server answered with HTTP {status}"),
            Error::Decode(err) => write!(f, "The server sent a reply that silo-omarchy cannot read: {err}"),
            Error::SignedOut => f.write_str("Your sign-in has expired. Sign in again."),
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
    let server = Server { id: String::new(), name: url.host_str().unwrap_or_default().to_string(), url };
    let _: wire::SystemInfo =
        send(http, &server, Method::GET, "system/info", &[], None).await.map_err(|err| match err {
            Error::Status(404) => Error::TooOld,
            err => err,
        })?;
    let identity: wire::ServerIdentity = send(http, &server, Method::GET, "system/identity", &[], None).await?;
    Ok(Server { id: identity.server_id, ..server })
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

/// The Profile that the app opens with no picker: the only Profile of the Account,
/// when it has no PIN.
pub fn auto_profile(profiles: &[wire::Profile]) -> Option<&wire::Profile> {
    match profiles {
        [only] if !only.has_pin => Some(only),
        _ => None,
    }
}

/// The values that identify this install to each Server.
#[derive(Debug, Clone)]
pub struct Device {
    /// One random UUID for each install. The app sends the same id to each Server.
    pub id: String,
    /// The host name.
    pub name: String,
}

/// The tokens of an Account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
}

/// One row of Home, in the order of the server Home layout.
#[derive(Debug, Clone)]
pub struct HomeSection {
    pub id: String,
    pub title: String,
    /// The Library window shows a Featured section as a hero above the rows.
    pub featured: bool,
    pub items: Vec<wire::CatalogItem>,
    /// The server has more items for the section than the row holds.
    pub view_all: bool,
}

/// A signed-in Account on one Server. The client adds the auth, profile and device
/// headers to each request. After a 401 it refreshes the tokens once and retries once.
pub struct Client {
    http: Arc<dyn HttpClient>,
    server: Server,
    device: Device,
    auth: Mutex<Auth>,
    /// Lets only one refresh run at a time. The server replaces the refresh token at
    /// each refresh, so a second refresh with the old token can fail.
    refreshing: futures::lock::Mutex<()>,
    on_tokens: Mutex<SaveTokens>,
    /// The id of the server installation from the playback capabilities. Progress and stop send it.
    installation_id: Mutex<String>,
}

type SaveTokens = Box<dyn Fn(&Tokens) + Send + Sync>;

struct Auth {
    tokens: Tokens,
    profile_id: Option<String>,
}

impl Client {
    pub fn new(http: Arc<dyn HttpClient>, server: Server, device: Device, tokens: Tokens) -> Self {
        Self {
            http,
            server,
            device,
            auth: Mutex::new(Auth { tokens, profile_id: None }),
            refreshing: futures::lock::Mutex::new(()),
            on_tokens: Mutex::new(Box::new(|_| {})),
            installation_id: Mutex::default(),
        }
    }

    /// Calls `save` each time that a refresh gives new tokens, so that the caller can keep them.
    pub fn on_tokens(&self, save: impl Fn(&Tokens) + Send + Sync + 'static) {
        *self.on_tokens.lock().unwrap() = Box::new(save);
    }

    /// Signs in with a username and password.
    pub async fn login(
        http: Arc<dyn HttpClient>,
        server: Server,
        device: Device,
        username: &str,
        password: &str,
    ) -> Result<(Self, wire::Account), Error> {
        let body = serde_json::json!({ "username": username, "password": password });
        let headers = device_headers(&device);
        let pair: wire::TokenPair = send(&http, &server, Method::POST, "auth/login", &headers, Some(body)).await?;
        let tokens = Tokens { access_token: pair.access_token, refresh_token: pair.refresh_token };
        Ok((Self::new(http, server, device, tokens), pair.user))
    }

    pub fn server(&self) -> &Server {
        &self.server
    }

    pub fn tokens(&self) -> Tokens {
        self.auth.lock().unwrap().tokens.clone()
    }

    /// Sets the Profile that the next requests act for.
    pub fn set_profile(&self, profile_id: Option<String>) {
        self.auth.lock().unwrap().profile_id = profile_id;
    }

    pub async fn profiles(&self) -> Result<Vec<wire::Profile>, Error> {
        let profiles: wire::ProfileCollection = self.call(Method::GET, "profiles", None).await?;
        Ok(profiles.items)
    }

    /// The Home sections of the active Profile, in the order of the server Home layout.
    pub async fn home(&self) -> Result<Vec<HomeSection>, Error> {
        let layout: wire::SectionLayout = self.call(Method::GET, "home/layout", None).await?;
        let sections: wire::SectionCollection = self.call(Method::GET, "home/sections", None).await?;
        let mut sections = sections.sections;
        Ok(layout
            .sections
            .into_iter()
            .map(|entry| {
                let section = sections.iter().position(|s| s.id == entry.id).map(|i| sections.swap_remove(i));
                let (items, total) = section.map_or((Vec::new(), 0), |s| (s.items, s.total_count));
                HomeSection {
                    view_all: total > items.len() as i64,
                    id: entry.id,
                    title: entry.title,
                    featured: entry.featured,
                    items,
                }
            })
            .collect())
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        let (headers, used) = self.headers();
        match send(&self.http, &self.server, method.clone(), path, &headers, body.clone()).await {
            Err(err) if is_unauthorized(&err) => {
                self.refresh(&used).await?;
                send(&self.http, &self.server, method, path, &self.headers().0, body).await
            }
            result => result,
        }
    }

    /// Returns the headers of a request, and the access token in them.
    fn headers(&self) -> (Vec<(&'static str, String)>, String) {
        let mut headers = device_headers(&self.device);
        let auth = self.auth.lock().unwrap();
        headers.push(("Authorization", format!("Bearer {}", auth.tokens.access_token)));
        if let Some(profile) = &auth.profile_id {
            headers.push(("X-Profile-Id", profile.clone()));
        }
        (headers, auth.tokens.access_token.clone())
    }

    /// Refreshes the tokens, unless another request already replaced the `stale` access token.
    async fn refresh(&self, stale: &str) -> Result<(), Error> {
        let _guard = self.refreshing.lock().await;
        let tokens = self.tokens();
        if tokens.access_token != stale {
            return Ok(());
        }
        let body = serde_json::json!({ "refresh_token": tokens.refresh_token });
        let headers = device_headers(&self.device);
        let fresh: wire::RefreshedTokens =
            send(&self.http, &self.server, Method::POST, "auth/refresh", &headers, Some(body)).await.map_err(
                |err| {
                    if is_unauthorized(&err) { Error::SignedOut } else { err }
                },
            )?;
        let tokens = Tokens { access_token: fresh.access_token, refresh_token: fresh.refresh_token };
        self.auth.lock().unwrap().tokens = tokens.clone();
        (self.on_tokens.lock().unwrap())(&tokens);
        Ok(())
    }

    // Playback (#25).

    /// The headers that mpv sends on each media request: the auth, profile and device headers.
    pub fn media_headers(&self) -> Vec<(String, String)> {
        self.headers().0.into_iter().map(|(name, value)| (name.to_string(), value)).collect()
    }

    /// Starts a Playback session for the Item `content_id`. With no `start_position`, the
    /// server starts at the saved resume point of the Profile.
    pub async fn start_playback(&self, content_id: &str, start_position: Option<f64>) -> Result<Decision, Error> {
        let versions: wire::FileVersionCollection =
            self.call(Method::GET, &format!("catalog/items/{content_id}/versions"), None).await?;
        let Some(version) = versions.items.first() else {
            return Ok(Decision::Unavailable {
                reason: "no_file".into(),
                message: "This Item has no file to play".into(),
            });
        };
        let capabilities: wire::PlaybackCapabilities = self.call(Method::GET, "playback/capabilities", None).await?;
        let installation_id = capabilities.installation_id.map(String::from).unwrap_or_default();
        *self.installation_id.lock().unwrap() = installation_id.clone();
        let mut features = vec!["playback_plan_v3"];
        if capabilities.features.iter().any(|f| f == "header_authenticated_media_v1") {
            features.push("header_authenticated_media_v1");
        }
        let profile_id = self.auth.lock().unwrap().profile_id.clone().unwrap_or_default();
        let mut body = start_request(&version.file_id, &installation_id, &profile_id, &features);
        if let Some(position) = start_position {
            body["start_position"] = position.into();
        }
        let decision: wire::PlaybackDecision = self.call(Method::POST, "playback/start", Some(body)).await?;
        let mut decision = Decision::from(decision);
        if let Decision::Playable { plan, .. } = &mut decision {
            // The plan URL is relative to the server origin. Join it below the base URL, which keeps a sub-path.
            let url = self.server.url.join(plan.stream.url.trim_start_matches('/')).map_err(|_| Error::InvalidUrl)?;
            plan.stream.url = url.to_string();
        }
        Ok(decision)
    }

    pub async fn report_progress(&self, session_id: &str, sample: &Sample) -> Result<(), Error> {
        let body = serde_json::json!({
            "installation_id": *self.installation_id.lock().unwrap(),
            "sequence": sample.sequence,
            "position": sample.position,
            "is_paused": sample.paused,
        });
        let _: wire::PlaybackMutation =
            self.call(Method::POST, &format!("playback/{session_id}/progress"), Some(body)).await?;
        Ok(())
    }

    /// Stops the Playback session. The server writes `sample` as the final position.
    pub async fn stop_playback(&self, session_id: &str, sample: Option<&Sample>) -> Result<(), Error> {
        let mut body = serde_json::json!({
            "installation_id": *self.installation_id.lock().unwrap(),
            "stop_id": uuid::Uuid::new_v4().to_string(),
        });
        if let Some(sample) = sample {
            body["sequence"] = sample.sequence.into();
            body["position"] = sample.position.into();
            body["is_paused"] = sample.paused.into();
        }
        let _: wire::PlaybackMutation =
            self.call(Method::DELETE, &format!("playback/{session_id}"), Some(body)).await?;
        Ok(())
    }
}

fn is_unauthorized(err: &Error) -> bool {
    matches!(err, Error::Status(401)) || matches!(err, Error::Problem(p) if p.status == 401)
}

fn device_headers(device: &Device) -> Vec<(&'static str, String)> {
    vec![
        ("X-Silo-Device-Id", device.id.clone()),
        ("X-Silo-Device-Name", device.name.clone()),
        ("X-Silo-Device-Platform", "linux".into()),
        ("X-Silo-Client-Family", "desktop".into()),
    ]
}

/// Sends one request to `api/v2/<path>` and decodes the JSON reply.
async fn send<T: DeserializeOwned>(
    http: &Arc<dyn HttpClient>,
    server: &Server,
    method: Method,
    path: &str,
    headers: &[(&'static str, String)],
    body: Option<serde_json::Value>,
) -> Result<T, Error> {
    let url = server.url.join("api/v2/").and_then(|api| api.join(path)).map_err(|_| Error::InvalidUrl)?;
    let mut request = Request::builder().method(method).uri(url.as_str()).header("Accept", "application/json");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let body = match body {
        Some(body) => {
            request = request.header("Content-Type", "application/json");
            AsyncBody::from(body.to_string())
        }
        None => AsyncBody::empty(),
    };
    let request = request.body(body).map_err(|_| Error::InvalidUrl)?;
    let network = || Error::Network(server.name.clone());
    let response = http.send(request).await.map_err(|_| network())?;
    let status = response.status();
    let is_problem = response
        .headers()
        .get("content-type")
        .is_some_and(|value| value.as_bytes().starts_with(b"application/problem+json"));
    let mut body = Vec::new();
    response.into_body().read_to_end(&mut body).await.map_err(|_| network())?;
    if status.is_success() {
        serde_json::from_slice(&body).map_err(|err| Error::Decode(err.to_string()))
    } else if is_problem && let Ok(problem) = serde_json::from_slice(&body) {
        Err(Error::Problem(problem))
    } else {
        Err(Error::Status(status.as_u16()))
    }
}

/// The body of a playback protocol v3 start request. mpv uses FFmpeg, which decodes
/// nearly every codec, so the app declares wide flat lists with `declared` evidence
/// and lets the server choose direct play or a transcode.
fn start_request(file_id: &str, installation_id: &str, profile_id: &str, features: &[&str]) -> serde_json::Value {
    let video = ["h264", "hevc", "av1", "vp9", "vp8", "mpeg2video", "mpeg4", "vc1"];
    let audio = ["aac", "mp3", "opus", "vorbis", "flac", "alac", "ac3", "eac3", "dts", "truehd", "pcm"];
    let containers = ["mp4", "mkv", "webm", "mov", "avi", "mpegts", "m2ts"];
    // mpv draws ASS with its styles and fonts, and draws bitmap subtitles.
    let subtitles = serde_json::json!({
        "embedded_text": true,
        "sidecar_text": true,
        "ass_styling": true,
        "embedded_bitmap": true,
        "sidecar_bitmap": true,
        "font_attachments": true,
    });
    let delivery = |containers: &[&str], claims: &[&str]| {
        serde_json::json!({
            "enabled": true,
            "supported_on_device": true,
            "containers": containers,
            "video_codecs": video,
            "audio_decode_codecs": audio,
            "audio_passthrough_codecs": [],
            "subtitles": subtitles,
            "features": [],
            "auth_header_refresh": false,
            "validated_claims": claims,
            "transformations": [],
        })
    };
    serde_json::json!({
        "protocol_version": 3,
        "client_features": features,
        "file_id": file_id,
        "installation_id": installation_id,
        "profile_id": profile_id,
        "playback_attempt_id": uuid::Uuid::new_v4().to_string(),
        "quality_preference": "original",
        "subtitle_fidelity_preference": "preserve",
        "metered": false,
        "client_capabilities": {
            "video_evidence": "declared",
            "audio_evidence": "declared",
            "codecs_video": video,
            // On the declared tier the server treats both lists the same.
            "codecs_video_hardware": video,
            "codecs_audio": audio,
            "containers": containers,
            "hdr": false,
        },
        "client_playback_context": {
            "protocol_version": 3,
            "app_version": env!("CARGO_PKG_VERSION"),
            "form_factor": "desktop",
            "device": { "platform": "linux" },
            "output": {},
            "deliveries": {
                "original_http": delivery(
                    &containers,
                    &["client_managed_dynamic_range_v1", "client_selected_audio_track_v1"],
                ),
                "progressive": delivery(&containers, &[]),
                "hls": delivery(&["hls"], &[]),
            },
        },
    })
}
