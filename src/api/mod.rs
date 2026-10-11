//! The Silo API client. It sends each request through the GPUI `HttpClient`, so the
//! app adds no second async runtime, and tests can give it a fake client.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::AsyncReadExt as _;
use futures::future::BoxFuture;
use gpui::http_client::{AsyncBody, HttpClient, Method, Request};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub mod types;
pub mod wire;

pub use gpui::http_client::Url;
use types::Marker;
pub use types::{Decision, Problem};

use crate::artwork::Rung;
use crate::playback::{Access, Replan, Sample};
use crate::settings::{self, ServerSettings};

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

impl Error {
    /// The Server did not answer, or answered 503. The app keeps the tokens and retries.
    pub fn is_offline(&self) -> bool {
        matches!(self, Error::Network(_)) || self.status() == Some(503)
    }

    fn status(&self) -> Option<u16> {
        match self {
            Error::Status(status) => Some(*status),
            Error::Problem(problem) => Some(problem.status),
            _ => None,
        }
    }
}

/// A failure that concerns the whole Account, not only the request that got it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    /// The Server does not answer, or answers 503.
    Offline,
    /// The server refused the refresh token.
    SignedOut,
}

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

/// The Profile that the app opens at launch with no picker: the `last` Profile of the Account
/// when it still exists, else the `auto_profile`.
pub fn launch_profile<'a>(profiles: &'a [wire::Profile], last: Option<&str>) -> Option<&'a wire::Profile> {
    last.and_then(|last| profiles.iter().find(|p| *p.id == last)).or_else(|| auto_profile(profiles))
}

/// The result of a PIN check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinCheck {
    /// The PIN matched. The value is the PIN token.
    Unlocked(String),
    Wrong,
    /// Too many wrong PINs locked the Profile. The value is the seconds left on the lock.
    Locked(Option<u64>),
}

/// The text that tells the user how long the PIN lock stays.
pub fn lock_message(seconds: Option<u64>) -> String {
    match seconds {
        None => "Too many wrong PINs. Try again later.".into(),
        Some(seconds) => {
            let minutes = seconds.div_ceil(60).max(1);
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            format!("Too many wrong PINs. Try again in {minutes} {unit}.")
        }
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
    /// The server type of the section, for example `continue_watching`.
    pub section_type: String,
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
    /// Lets only one PIN entry run at a time.
    unlocking: futures::lock::Mutex<()>,
    on_pin_required: Mutex<AskPin>,
    /// The id of the server installation from the playback capabilities. Progress and stop send it.
    installation_id: Mutex<String>,
    /// The Server settings of the active Profile.
    server_settings: Mutex<ServerSettings>,
    /// The `client_features` of the last start request. A replan sends the same list.
    features: Mutex<Vec<&'static str>>,
    on_trouble: Mutex<Box<dyn Fn(Trouble) + Send + Sync>>,
    /// The Item of each Playback session that this client started: its content id and duration in seconds.
    // ponytail: never pruned; one small entry for each Item played in this run.
    sessions: Mutex<HashMap<String, (String, i64)>>,
    /// The last position of each Item whose report failed, keyed by Profile id and content id.
    pending: Mutex<HashMap<(String, String), Pending>>,
}

/// A position that the server did not get. `sync_progress` sends it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    position_ms: i64,
    duration_ms: i64,
    at: chrono::DateTime<chrono::Utc>,
}

type SaveTokens = Box<dyn Fn(&Tokens) + Send + Sync>;

/// Asks the user for the PIN of a Profile. It gets the Profile id and returns the PIN token, or `None` when
/// the user cancels.
type AskPin = Box<dyn Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync>;

struct Auth {
    tokens: Tokens,
    profile_id: Option<String>,
    /// The PIN token of the Profile, sent as `X-Profile-Token`.
    profile_token: Option<String>,
}

impl Client {
    pub fn new(http: Arc<dyn HttpClient>, server: Server, device: Device, tokens: Tokens) -> Self {
        Self {
            http,
            server,
            device,
            auth: Mutex::new(Auth { tokens, profile_id: None, profile_token: None }),
            refreshing: futures::lock::Mutex::new(()),
            on_tokens: Mutex::new(Box::new(|_| {})),
            unlocking: futures::lock::Mutex::new(()),
            on_pin_required: Mutex::new(Box::new(|_| Box::pin(async { None }))),
            installation_id: Mutex::default(),
            server_settings: Mutex::default(),
            features: Mutex::default(),
            on_trouble: Mutex::new(Box::new(|_| {})),
            sessions: Mutex::default(),
            pending: Mutex::default(),
        }
    }

    /// Calls `tell` each time that a request finds the Server Offline or the sign-in refused.
    pub fn on_trouble(&self, tell: impl Fn(Trouble) + Send + Sync + 'static) {
        *self.on_trouble.lock().unwrap() = Box::new(tell);
    }

    /// Reads `system/info`, which needs no sign-in, to find out whether the Server answers again.
    pub async fn probe(&self) -> Result<(), Error> {
        let headers = device_headers(&self.device);
        let _: wire::SystemInfo = send(&self.http, &self.server, Method::GET, "system/info", &headers, None).await?;
        Ok(())
    }

    /// Tells the `on_trouble` callback when `result` failed Offline or signed out.
    fn tell<T>(&self, result: Result<T, Error>) -> Result<T, Error> {
        let trouble = match &result {
            Err(Error::SignedOut) => Some(Trouble::SignedOut),
            Err(err) if err.is_offline() => Some(Trouble::Offline),
            _ => None,
        };
        if let Some(trouble) = trouble {
            (self.on_trouble.lock().unwrap())(trouble);
        }
        result
    }

    /// Calls `save` each time that a refresh gives new tokens, so that the caller can keep them.
    pub fn on_tokens(&self, save: impl Fn(&Tokens) + Send + Sync + 'static) {
        *self.on_tokens.lock().unwrap() = Box::new(save);
    }

    /// Signs in with a username and password. With no `provider`, the server picks the
    /// provider from the Account.
    pub async fn login(
        http: Arc<dyn HttpClient>,
        server: Server,
        device: Device,
        username: &str,
        password: &str,
        provider: Option<&str>,
    ) -> Result<(Self, wire::Account), Error> {
        let mut body = serde_json::json!({ "username": username, "password": password });
        if let Some(provider) = provider {
            body["provider"] = provider.into();
        }
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

    /// Takes the tokens of a new sign-in of the same Account.
    pub fn set_tokens(&self, tokens: Tokens) {
        self.auth.lock().unwrap().tokens = tokens;
    }

    /// Sets the Profile that the next requests act for. It drops the PIN token and the Server settings of the
    /// old Profile.
    pub fn set_profile(&self, profile_id: Option<String>) {
        let mut auth = self.auth.lock().unwrap();
        auth.profile_id = profile_id;
        auth.profile_token = None;
        *self.server_settings.lock().unwrap() = ServerSettings::default();
    }

    /// The Server settings of the active Profile, or the defaults until `load_server_settings` has read them.
    pub fn server_settings(&self) -> ServerSettings {
        self.server_settings.lock().unwrap().clone()
    }

    /// Reads the Server settings of the active Profile from the server and keeps them in memory.
    pub async fn load_server_settings(&self) -> Result<(), Error> {
        let profile_id = self.auth.lock().unwrap().profile_id.clone();
        let query = settings::KEYS.map(|key| format!("keys={key}")).join("&");
        let values: wire::EffectiveSettingCollection =
            self.call(Method::GET, &format!("settings/values/effective?{query}"), None).await?;
        // The user can switch to another Profile while the request runs.
        if self.auth.lock().unwrap().profile_id == profile_id {
            *self.server_settings.lock().unwrap() = ServerSettings::from_effective(&values.items);
        }
        Ok(())
    }

    /// Writes one Server setting of the active Profile at the `profile` scope, and keeps the new value in memory
    /// when the write succeeds. A null value is "No preference".
    pub async fn set_server_setting(&self, key: &str, value: serde_json::Value) -> Result<(), Error> {
        let profile_id = self.auth.lock().unwrap().profile_id.clone();
        let path = format!("settings/values/{key}?scope=profile");
        let body = serde_json::json!({ "value": value });
        let _: wire::SettingValue = self.call(Method::PUT, &path, Some(body)).await?;
        if self.auth.lock().unwrap().profile_id == profile_id {
            self.server_settings.lock().unwrap().set(key, &value);
        }
        Ok(())
    }

    pub async fn profiles(&self) -> Result<Vec<wire::Profile>, Error> {
        let profiles: wire::ProfileCollection = self.call(Method::GET, "profiles", None).await?;
        Ok(profiles.items)
    }

    /// The Home sections of the active Profile, in the order of the server Home layout.
    /// `image_size` is the rung of the Artwork ladder for the artwork URLs.
    pub async fn home(&self, image_size: Rung) -> Result<Vec<HomeSection>, Error> {
        let layout: wire::SectionLayout = self.call(Method::GET, "home/layout", None).await?;
        let path = format!("home/sections?image_size={image_size}");
        let sections: wire::SectionCollection = self.call(Method::GET, &path, None).await?;
        Ok(rows(layout, sections.sections))
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        let result = self.call_with_retries(method, path, body).await;
        self.tell(result)
    }

    async fn call_with_retries<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, Error> {
        let (headers, used) = self.headers();
        let mut result = send(&self.http, &self.server, method.clone(), path, &headers, body.clone()).await;
        if matches!(&result, Err(err) if is_unauthorized(err)) {
            self.refresh(&used).await?;
            result = send(&self.http, &self.server, method.clone(), path, &self.headers().0, body.clone()).await;
        }
        let stale = headers.iter().find(|(name, _)| *name == "X-Profile-Token").map(|(_, token)| token.as_str());
        if matches!(&result, Err(Error::Problem(p)) if p.code() == "profile_verification_required")
            && self.unlock(stale).await
        {
            result = send(&self.http, &self.server, method, path, &self.headers().0, body).await;
        }
        result
    }

    /// Returns the headers of a request, and the access token in them.
    fn headers(&self) -> (Vec<(&'static str, String)>, String) {
        let mut headers = device_headers(&self.device);
        let auth = self.auth.lock().unwrap();
        headers.push(("Authorization", format!("Bearer {}", auth.tokens.access_token)));
        if let Some(profile) = &auth.profile_id {
            headers.push(("X-Profile-Id", profile.clone()));
        }
        if let Some(token) = &auth.profile_token {
            headers.push(("X-Profile-Token", token.clone()));
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
                // A network failure and a 503 stay as they are: they are Offline, not a refused sign-in.
                |err| {
                    if matches!(err.status(), Some(400 | 401 | 403)) { Error::SignedOut } else { err }
                },
            )?;
        let tokens = Tokens { access_token: fresh.access_token, refresh_token: fresh.refresh_token };
        self.auth.lock().unwrap().tokens = tokens.clone();
        (self.on_tokens.lock().unwrap())(&tokens);
        Ok(())
    }

    // Playback (#25).

    /// The headers that mpv sends on each media request (the auth, profile and device headers), and when the
    /// access token in them expires.
    pub fn media_access(&self) -> Access {
        let (headers, token) = self.headers();
        let headers = headers.into_iter().map(|(name, value)| (name.to_string(), value)).collect();
        Access { headers, expires: token_expiry(&token) }
    }

    /// Refreshes the access token, unless another request refreshed it while this one waited, and returns the
    /// new media headers.
    pub async fn refresh_access(&self) -> Result<Access, Error> {
        let stale = self.tokens().access_token;
        let refreshed = self.refresh(&stale).await;
        self.tell(refreshed)?;
        Ok(self.media_access())
    }

    /// Asks the server for a new plan after the plan in `replan` failed (operation `failure_recovery`).
    pub async fn replan(&self, replan: &Replan) -> Result<Decision, Error> {
        let installation_id = self.installation_id.lock().unwrap().clone();
        let mut body = client_description(&self.features.lock().unwrap());
        let keys = &replan.attempted_plan_keys;
        // The server takes at most 16 keys and a count of at most 8.
        let fields = serde_json::json!({
            "operation": "failure_recovery",
            "installation_id": installation_id,
            "playback_attempt_id": replan.attempt_id,
            "replan_request_id": uuid::Uuid::new_v4().to_string(),
            "failed_plan_id": replan.failed_plan_id,
            "plan_attempt_id": uuid::Uuid::new_v4().to_string(),
            "plan_attempt_key": replan.plan_attempt_key,
            "attempted_plan_keys": keys[keys.len().saturating_sub(16)..],
            "attempt_count": keys.len().clamp(1, 8),
            "position_seconds": replan.position.max(0.0),
            "selected_tracks": replan.selected_tracks,
            "failure": { "classification": "network_degraded" },
        });
        body.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        let path = format!("playback/{}/replan", replan.session_id);
        let decision: wire::PlaybackDecision = match self.call(Method::POST, &path, Some(body)).await {
            // The server ended the Playback session, for example after 45 s with no report. A new start
            // request at the position gives a new session.
            Err(Error::Problem(problem))
                if matches!(problem.code(), "playback_session_ended" | "playback_session_not_found") =>
            {
                let content_id = self.sessions.lock().unwrap().get(&replan.session_id).map(|(id, _)| id.clone());
                let Some(content_id) = content_id else {
                    return Err(Error::Problem(problem));
                };
                return self.start_playback(&content_id, Some(replan.position.max(0.0))).await;
            }
            result => result?,
        };
        let mut decision = Decision::from(decision);
        if let Decision::Playable { plan, .. } = &mut decision {
            self.absolute_urls(plan)?;
        }
        Ok(decision)
    }

    /// Joins the stream and subtitle URLs of `plan`, which are relative to the server origin, below the base
    /// URL, which keeps a sub-path.
    fn absolute_urls(&self, plan: &mut wire::PlaybackPlan) -> Result<(), Error> {
        let absolute = |url: &str| self.server.url.join(url.trim_start_matches('/')).map(String::from);
        plan.stream.url = absolute(&plan.stream.url).map_err(|_| Error::InvalidUrl)?;
        for track in &mut plan.subtitle.inventory {
            track.url = track.url.as_deref().and_then(|url| absolute(url).ok());
        }
        Ok(())
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
        *self.features.lock().unwrap() = features.clone();
        let profile_id = self.auth.lock().unwrap().profile_id.clone().unwrap_or_default();
        let attempt = uuid::Uuid::new_v4().to_string();
        let mut body = start_request(&version.file_id, &installation_id, &profile_id, &attempt, &features);
        if let Some(position) = start_position {
            body["start_position"] = position.into();
        }
        let decision: wire::PlaybackDecision = self.call(Method::POST, "playback/start", Some(body)).await?;
        let mut decision = Decision::from(decision);
        if let Decision::Playable { session_id, plan, audio_language, markers, attempt_id } = &mut decision {
            self.sessions.lock().unwrap().insert(session_id.clone(), (content_id.to_string(), version.duration));
            self.absolute_urls(plan)?;
            *attempt_id = attempt;
            *audio_language = audio_language_of(version, plan);
            *markers = self.markers(&version.file_id).await;
        }
        Ok(decision)
    }

    /// The Markers of a file, in source time. Playback works without them, so a failure gives none.
    async fn markers(&self, file_id: &str) -> Vec<Marker> {
        match self.call::<wire::FileMarkers>(Method::GET, &format!("markers/files/{file_id}"), None).await {
            Ok(markers) => markers
                .marker_segments
                .into_iter()
                .map(|marker| Marker { kind: marker.kind.into(), start: marker.start_seconds, end: marker.end_seconds })
                .collect(),
            Err(err) => {
                eprintln!("silo-omarchy: cannot read the markers of file {file_id}: {err}");
                Vec::new()
            }
        }
    }

    /// The episode that plays after `content_id`, an episode in season `season` of the series: the next
    /// episode of the season, else the first episode of the next season. It skips an episode with no
    /// readable file, like the web client.
    pub async fn next_episode(
        &self,
        series_id: &str,
        season: i64,
        content_id: &str,
    ) -> Result<Option<wire::Episode>, Error> {
        let playable = |episode: &wire::Episode| episode.files.iter().any(|file| file.unreadable != Some(true));
        let episodes = self.season_episodes(series_id, season).await?;
        let Some(index) = episodes.iter().position(|episode| episode.content_id == content_id) else {
            return Ok(None);
        };
        if let Some(next) = episodes.into_iter().skip(index + 1).find(playable) {
            return Ok(Some(next));
        }
        match self.season_episodes(series_id, season + 1).await {
            Ok(episodes) => Ok(episodes.into_iter().find(playable)),
            Err(Error::Status(404)) => Ok(None),
            Err(Error::Problem(problem)) if problem.status == 404 => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The playable episode before `content_id`: the one before it in the season, else the last one of the
    /// season before. Season 0 holds the specials, so the lookup stops at season 1.
    pub async fn previous_episode(
        &self,
        series_id: &str,
        season: i64,
        content_id: &str,
    ) -> Result<Option<wire::Episode>, Error> {
        let playable = |episode: &wire::Episode| episode.files.iter().any(|file| file.unreadable != Some(true));
        let episodes = self.season_episodes(series_id, season).await?;
        let Some(index) = episodes.iter().position(|episode| episode.content_id == content_id) else {
            return Ok(None);
        };
        if let Some(previous) = episodes.into_iter().take(index).rfind(playable) {
            return Ok(Some(previous));
        }
        if season <= 1 {
            return Ok(None);
        }
        match self.season_episodes(series_id, season - 1).await {
            Ok(episodes) => Ok(episodes.into_iter().rfind(playable)),
            Err(Error::Status(404)) => Ok(None),
            Err(Error::Problem(problem)) if problem.status == 404 => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The episodes of a season. The server sends the whole season in one reply: `listSeasonEpisodes` has no
    /// cursor.
    pub async fn season_episodes(&self, series_id: &str, season: i64) -> Result<Vec<wire::Episode>, Error> {
        let path = format!("catalog/series/{series_id}/seasons/{season}/episodes");
        Ok(self.call::<wire::EpisodeCollection>(Method::GET, &path, None).await?.items)
    }

    pub async fn report_progress(&self, session_id: &str, sample: &Sample) -> Result<(), Error> {
        let body = serde_json::json!({
            "installation_id": *self.installation_id.lock().unwrap(),
            "sequence": sample.sequence,
            "position": sample.position,
            "is_paused": sample.paused,
        });
        let result = self.call(Method::POST, &format!("playback/{session_id}/progress"), Some(body)).await;
        self.keep_on_failure(session_id, Some(sample), result).await
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
        let result = self.call(Method::DELETE, &format!("playback/{session_id}"), Some(body)).await;
        self.keep_on_failure(session_id, sample, result).await
    }

    /// Keeps the position of a report that failed, so that `sync_progress` sends it later. A report that
    /// worked drops the kept position of its Item. When the server answered but refused the report, for
    /// example because the session ended, the position goes with `syncProgress` at once.
    async fn keep_on_failure(
        &self,
        session_id: &str,
        sample: Option<&Sample>,
        result: Result<wire::PlaybackMutation, Error>,
    ) -> Result<(), Error> {
        let item = self.sessions.lock().unwrap().get(session_id).cloned();
        let (Some((content_id, duration)), Some(sample)) = (item, sample) else {
            return result.map(drop);
        };
        let key = (self.auth.lock().unwrap().profile_id.clone().unwrap_or_default(), content_id);
        let Err(err) = result else {
            self.pending.lock().unwrap().remove(&key);
            return Ok(());
        };
        let pending = Pending {
            position_ms: (sample.position.max(0.0) * 1000.0).round() as i64,
            duration_ms: duration.max(0) * 1000,
            at: chrono::Utc::now(),
        };
        self.pending.lock().unwrap().insert(key, pending);
        if !err.is_offline() && !matches!(err, Error::SignedOut) {
            self.sync_progress().await.inspect_err(|err| eprintln!("silo-omarchy: progress sync failed: {err}")).ok();
        }
        Err(err)
    }

    /// Sends the kept positions of the active Profile with `syncProgress`. A position that the server got,
    /// or refused for good, is dropped. The positions of the other Profiles wait until their Profile is active.
    pub async fn sync_progress(&self) -> Result<(), Error> {
        let profile_id = self.auth.lock().unwrap().profile_id.clone().unwrap_or_default();
        // The server takes at most 100 items in one request.
        let sent: Vec<((String, String), Pending)> = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .filter(|((profile, _), _)| *profile == profile_id)
            .take(100)
            .map(|(key, pending)| (key.clone(), pending.clone()))
            .collect();
        if sent.is_empty() {
            return Ok(());
        }
        let items: Vec<_> = sent
            .iter()
            .map(|((_, content_id), pending)| {
                serde_json::json!({
                    "media_item_id": content_id,
                    "position_ms": pending.position_ms,
                    "duration_ms": pending.duration_ms,
                    "updated_at": pending.at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                })
            })
            .collect();
        let body = serde_json::json!({ "items": items });
        let result: wire::ProgressSyncBatchResult = self.call(Method::POST, "sync/progress", Some(body)).await?;
        for item in result.items {
            if let wire::ProgressSyncBatchResultItemsItem::Failure(failure) = item {
                eprintln!(
                    "silo-omarchy: the server refused the progress of {}: {:?}",
                    *failure.media_item_id, failure.failure
                );
            }
        }
        let mut pending = self.pending.lock().unwrap();
        for (key, kept) in sent {
            // A newer position that came in while the request ran stays.
            if pending.get(&key) == Some(&kept) {
                pending.remove(&key);
            }
        }
        Ok(())
    }

    /// Takes the kept positions, so that a new client of the same Account can send them.
    pub fn take_pending(&self) -> HashMap<(String, String), Pending> {
        std::mem::take(&mut self.pending.lock().unwrap())
    }

    /// Adds the kept positions of an older client of the same Account. A position that this client has is
    /// newer, so it stays.
    pub fn adopt_pending(&self, older: HashMap<(String, String), Pending>) {
        let mut pending = self.pending.lock().unwrap();
        for (key, kept) in older {
            pending.entry(key).or_insert(kept);
        }
    }

    // Sign out (#35).

    /// Sends the kept positions of the active Profile and then ends the sign-in on the server with `logout`.
    /// A failure does not call the `on_trouble` callback: the app signs out locally anyway.
    pub async fn sign_out(&self) -> Result<(), Error> {
        if let Err(err) = self.sync_progress().await {
            eprintln!("silo-omarchy: progress sync before the sign-out failed: {err}");
        }
        self.call_with_retries(Method::POST, "auth/logout", None).await
    }

    // Profile PIN (#23).

    /// Sets the PIN token of the active Profile. `set_profile` drops it.
    pub fn set_profile_token(&self, token: Option<String>) {
        self.auth.lock().unwrap().profile_token = token;
    }

    /// Calls `ask` when the server answers 403 `profile_verification_required`. When `ask` returns a PIN
    /// token, the client sends the request again with it.
    pub fn on_pin_required(&self, ask: impl Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync + 'static) {
        *self.on_pin_required.lock().unwrap() = Box::new(ask);
    }

    /// Checks the PIN of a Profile. The server counts each check, so the app never sends one again by itself.
    pub async fn verify_pin(&self, profile_id: &str, pin: &str) -> Result<PinCheck, Error> {
        let body = serde_json::json!({ "pin": pin });
        match self
            .call::<wire::ProfileVerification>(Method::POST, &format!("profiles/{profile_id}/verify-pin"), Some(body))
            .await
        {
            Ok(check) => Ok(match check.profile_token {
                Some(token) if check.valid => PinCheck::Unlocked(token),
                _ => PinCheck::Wrong,
            }),
            Err(Error::Problem(problem)) if problem.status == 429 => Ok(PinCheck::Locked(problem.retry_after)),
            Err(err) => Err(err),
        }
    }

    /// Asks the user for the PIN of the active Profile, unless another request already replaced the `stale`
    /// PIN token. Returns false when the user cancels.
    async fn unlock(&self, stale: Option<&str>) -> bool {
        let _guard = self.unlocking.lock().await;
        let (profile_id, token) = {
            let auth = self.auth.lock().unwrap();
            (auth.profile_id.clone(), auth.profile_token.clone())
        };
        let Some(profile_id) = profile_id else {
            return false;
        };
        if token.is_some() && token.as_deref() != stale {
            return true;
        }
        let asked = (self.on_pin_required.lock().unwrap())(profile_id.clone());
        let Some(token) = asked.await else {
            return false;
        };
        let mut auth = self.auth.lock().unwrap();
        // The user can switch to another Profile while the PIN entry is open.
        if auth.profile_id.as_deref() != Some(profile_id.as_str()) {
            return false;
        }
        auth.profile_token = Some(token);
        true
    }

    // Library page (#31).

    /// The video Libraries that the Profile sees, in the server order. Book, comic and podcast Libraries
    /// are out of scope.
    pub async fn libraries(&self) -> Result<Vec<wire::UserLibrary>, Error> {
        let list: wire::CollectionUserLibrary = self.call(Method::GET, "user/libraries", None).await?;
        let mut libraries: Vec<_> = list
            .items
            .into_iter()
            .filter(|l| {
                !matches!(
                    l.type_.as_str(),
                    "audiobooks" | "audiobook" | "ebooks" | "ebook" | "manga" | "podcasts" | "podcast"
                )
            })
            .collect();
        libraries.sort_by_key(|l| l.sort_order);
        Ok(libraries)
    }

    /// The rows of the Recommended tab of a Library, like the Home sections.
    pub async fn library_rows(&self, library_id: &str, image_size: Rung) -> Result<Vec<HomeSection>, Error> {
        let layout: wire::SectionLayout = self.call(Method::GET, &format!("library/{library_id}/layout"), None).await?;
        let sections = futures::future::try_join_all(layout.sections.iter().map(|entry| {
            let path = format!("library/{library_id}/sections/{}/items?image_size={image_size}", entry.id);
            async move { self.call::<wire::Section>(Method::GET, &path, None).await }
        }))
        .await?;
        Ok(rows(layout, sections))
    }

    /// One page of a catalog query. `image_size` is the rung of the Artwork ladder for the artwork URLs.
    pub async fn query_catalog(
        &self,
        query: &wire::CatalogQuery,
        image_size: Rung,
    ) -> Result<wire::CatalogBrowseCollection, Error> {
        let body = serde_json::to_value(query).map_err(|err| Error::Decode(err.to_string()))?;
        self.call(Method::POST, &format!("catalog/query?image_size={image_size}"), Some(body)).await
    }

    /// The genres of the Items in a Library, for the filter menu.
    pub async fn genres(&self, library_id: &str) -> Result<Vec<String>, Error> {
        let path = format!("catalog/filters?library_id={library_id}&skip_technical=true");
        Ok(self.call::<wire::CatalogFilters>(Method::GET, &path, None).await?.genres)
    }

    // Detail page (#30).

    /// The detail of an Item. `image_size` is the rung of the Artwork ladder for the artwork URLs.
    pub async fn item(&self, id: &str, image_size: Rung) -> Result<wire::CatalogItemDetail, Error> {
        self.call(Method::GET, &format!("catalog/items/{id}?image_size={image_size}"), None).await
    }

    /// The seasons of a series, in the server order, without artwork.
    pub async fn seasons(&self, series_id: &str) -> Result<Vec<wire::Season>, Error> {
        let path = format!("catalog/series/{series_id}/seasons?include_artwork=false");
        Ok(self.call::<wire::SeasonCollection>(Method::GET, &path, None).await?.items)
    }

    /// Adds the Item to the Watchlist of the Profile, or removes it.
    pub async fn set_watchlist(&self, item_id: &str, on: bool) -> Result<(), Error> {
        let method = if on { Method::PUT } else { Method::DELETE };
        self.call(method, &format!("watchlist/{item_id}"), None).await
    }

    // Search (#32).

    /// The first 20 people with movie or series credits whose name matches `text`.
    pub async fn people(&self, text: &str) -> Result<Vec<wire::Person>, Error> {
        let mut url = Url::parse("query:").map_err(|_| Error::InvalidUrl)?;
        url.query_pairs_mut().append_pair("q", text).append_pair("limit", "20").append_pair("media_scope", "video");
        let path = format!("catalog/people?{}", url.query().unwrap_or_default());
        Ok(self.call::<wire::PersonCollection>(Method::GET, &path, None).await?.items)
    }
}

/// The rows of a layout, in layout order, with the items of the matching section. A section that the
/// server did not send gives an empty row.
fn rows(layout: wire::SectionLayout, mut sections: Vec<wire::Section>) -> Vec<HomeSection> {
    layout
        .sections
        .into_iter()
        .map(|entry| {
            let section = sections.iter().position(|s| s.id == entry.id).map(|i| sections.swap_remove(i));
            let (items, total) = section.map_or((Vec::new(), 0), |s| (s.items, s.total_count));
            HomeSection {
                view_all: total > items.len() as i64,
                id: entry.id,
                section_type: entry.section_type,
                title: entry.title,
                featured: entry.featured,
                items,
            }
        })
        .collect()
}

/// The language of the audio track that the server picked for the plan: the language of the track, else
/// the effective audio language of the version, which the server resolves from the same Profile preference.
fn audio_language_of(version: &wire::FileVersion, plan: &wire::PlaybackPlan) -> Option<String> {
    let index = plan.selected_tracks.audio.as_ref().and_then(|audio| audio.index)?;
    let track = usize::try_from(index).ok().and_then(|i| version.audio_tracks.get(i));
    track.and_then(|track| track.language.clone()).or_else(|| version.effective_audio_language.clone())
}

/// When the JWT `token` expires: its `exp` claim. The app does not check the signature; the server does.
fn token_expiry(token: &str) -> Option<SystemTime> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let exp = serde_json::from_slice::<serde_json::Value>(&payload).ok()?.get("exp")?.as_f64()?;
    Some(UNIX_EPOCH + Duration::try_from_secs_f64(exp).ok()?)
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
    let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()?.trim().parse().ok());
    let is_problem = response
        .headers()
        .get("content-type")
        .is_some_and(|value| value.as_bytes().starts_with(b"application/problem+json"));
    let mut body = Vec::new();
    response.into_body().read_to_end(&mut body).await.map_err(|_| network())?;
    if status.is_success() {
        // A 204 has no body. It decodes as `null`, which is `()`.
        serde_json::from_slice(if body.is_empty() { b"null" } else { &body })
            .map_err(|err| Error::Decode(err.to_string()))
    } else if is_problem && let Ok(problem) = serde_json::from_slice::<Problem>(&body) {
        Err(Error::Problem(Problem { retry_after, ..problem }))
    } else {
        Err(Error::Status(status.as_u16()))
    }
}

/// The body of a playback protocol v3 start request. mpv uses FFmpeg, which decodes
/// nearly every codec, so the app declares wide flat lists with `declared` evidence
/// and lets the server choose direct play or a transcode.
fn start_request(
    file_id: &str,
    installation_id: &str,
    profile_id: &str,
    attempt_id: &str,
    features: &[&str],
) -> serde_json::Value {
    let mut body = client_description(features);
    let fields = serde_json::json!({
        "file_id": file_id,
        "installation_id": installation_id,
        "profile_id": profile_id,
        "playback_attempt_id": attempt_id,
        "subtitle_fidelity_preference": "preserve",
    });
    body.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
    body
}

/// The fields that a start request and a replan request share: what the client is and what it can play.
fn client_description(features: &[&str]) -> serde_json::Value {
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
        "quality_preference": "original",
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

// Sign-in with a provider choice and device sign-in (#34).

/// The providers that take a username and password, in the server order. The
/// sign-in screen shows a choice only when there is more than one.
pub async fn password_providers(
    http: &Arc<dyn HttpClient>,
    server: &Server,
    device: &Device,
) -> Result<Vec<wire::AuthProvider>, Error> {
    let list: wire::AuthProviderCollection =
        send(http, server, Method::GET, "auth/providers", &device_headers(device), None).await?;
    Ok(list.items.into_iter().filter(|p| p.mode == "credentials").collect())
}

/// Starts device sign-in. The user approves the code of the reply on another device.
pub async fn start_device_login(
    http: &Arc<dyn HttpClient>,
    server: &Server,
    device: &Device,
) -> Result<wire::DeviceLoginStart, Error> {
    let body = serde_json::json!({ "device_name": device.name, "device_platform": "linux" });
    send(http, server, Method::POST, "auth/device/start", &device_headers(device), Some(body)).await
}

/// The state of a device sign-in after one poll.
pub enum DeviceLogin {
    /// Poll again after `poll_after`. `opened` is true when the approver opened the code.
    Waiting {
        opened: bool,
        poll_after: std::time::Duration,
    },
    SignedIn(Box<Client>, wire::Account),
    /// The code cannot sign in any more. The value tells the user why.
    Ended(&'static str),
}

/// Asks the server once whether the user approved the device sign-in.
pub async fn poll_device_login(
    http: Arc<dyn HttpClient>,
    server: Server,
    device: Device,
    device_code: &str,
) -> Result<DeviceLogin, Error> {
    let body = serde_json::json!({ "device_code": device_code });
    let poll: wire::DeviceLoginPoll =
        send(&http, &server, Method::POST, "auth/device/poll", &device_headers(&device), Some(body)).await?;
    Ok(match (types::DeviceLoginStatus::from(poll.status), poll.tokens) {
        (types::DeviceLoginStatus::Approved, Some(pair)) => {
            let tokens = Tokens { access_token: pair.access_token, refresh_token: pair.refresh_token };
            DeviceLogin::SignedIn(Box::new(Client::new(http, server, device, tokens)), pair.user)
        }
        (types::DeviceLoginStatus::Denied, _) => {
            DeviceLogin::Ended("Approval was denied. Start a new code to try again.")
        }
        (
            types::DeviceLoginStatus::Expired | types::DeviceLoginStatus::Consumed | types::DeviceLoginStatus::Canceled,
            _,
        ) => DeviceLogin::Ended("This code is no longer valid. Start a new one."),
        // Pending, or a status that this version does not know: wait, like the web client.
        _ => DeviceLogin::Waiting {
            opened: poll.opened,
            poll_after: std::time::Duration::from_secs(poll.poll_after.max(1) as u64),
        },
    })
}

/// The device sign-in code as the user reads it: eight digits in two groups of four.
pub fn user_code(code: &str) -> String {
    let code: String = code.chars().filter(char::is_ascii_alphanumeric).collect();
    match code.split_at_checked(4) {
        Some((first, rest)) if !rest.is_empty() => format!("{first} {rest}"),
        _ => code,
    }
}
