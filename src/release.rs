//! The Release notice. The app asks GitHub for the latest Relics release at startup, and then every 24 hours
//! while it runs. When that release is newer than the build that runs, the Library window and the Settings page
//! link to its release page. The app never downloads or installs a release.

use std::sync::Arc;
use std::time::Duration;

use futures::AsyncReadExt as _;
use gpui::http_client::{AsyncBody, HttpClient, Request};
use gpui::{Context, Task};
use serde::Deserialize;

/// The latest release. GitHub skips drafts and pre-releases here.
pub const LATEST_URL: &str = "https://api.github.com/repos/s0up4200/relics/releases/latest";

const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// A release that is newer than the build that runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// The tag without the `v`, for example `0.2.0`.
    pub version: String,
    /// The release page on GitHub.
    pub url: String,
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    html_url: String,
}

/// Asks GitHub for the latest release. Gives the release when its tag is a higher semver than `current`.
pub async fn newer_release(http: &dyn HttpClient, current: &str) -> anyhow::Result<Option<Release>> {
    let request = Request::get(LATEST_URL).header("Accept", "application/vnd.github+json").body(AsyncBody::empty())?;
    let response = http.send(request).await?;
    anyhow::ensure!(response.status().is_success(), "GitHub answered {}", response.status());
    let mut body = Vec::new();
    response.into_body().read_to_end(&mut body).await?;
    let latest: Latest = serde_json::from_slice(&body)?;
    let version = latest.tag_name.strip_prefix('v').unwrap_or(&latest.tag_name);
    // A tag that is not semver is never newer.
    let newer = matches!(
        (semver::Version::parse(version), semver::Version::parse(current)),
        (Ok(latest), Ok(current)) if latest > current
    );
    Ok(newer.then(|| Release { version: version.into(), url: latest.html_url }))
}

/// The release check of the app, and the newer release that it found.
pub struct ReleaseCheck {
    /// `None` in a debug build, which sends no request to GitHub.
    http: Option<Arc<dyn HttpClient>>,
    pub release: Option<Release>,
    _timer: Task<()>,
}

impl ReleaseCheck {
    pub fn new(http: Option<Arc<dyn HttpClient>>, enabled: bool, cx: &mut Context<Self>) -> Self {
        let mut check = Self { http, release: None, _timer: Task::ready(()) };
        check.set_enabled(enabled, cx);
        check
    }

    /// The "Check for new releases" Local setting. On, the app checks at once and then every 24 hours. Off, the
    /// timer stops and the notice goes away. A failed check writes a log line and keeps the last result.
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.release = None;
        self._timer = match self.http.clone().filter(|_| enabled) {
            Some(http) => cx.spawn(async move |this, cx| {
                loop {
                    match newer_release(&*http, env!("CARGO_PKG_VERSION")).await {
                        Ok(release) => {
                            let kept = this.update(cx, |this, cx| {
                                this.release = release;
                                cx.notify();
                            });
                            if kept.is_err() {
                                return;
                            }
                        }
                        Err(err) => eprintln!("relics: cannot check for a new release: {err}"),
                    }
                    cx.background_executor().timer(INTERVAL).await;
                }
            }),
            None => Task::ready(()),
        };
        cx.notify();
    }
}
