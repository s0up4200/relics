//! The Artwork cache, the rung of the Artwork ladder, and the Thumbhash previews.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context as _, bail};
use base64::Engine as _;
use futures::AsyncReadExt as _;
use gpui::http_client::{AsyncBody, HttpClient, Request};
use gpui::{
    App, AppContext as _, Context, Entity, EntityId, ImageCache, ImageCacheError, ImageCacheItem, RenderImage,
    Resource, WeakEntity, Window,
};
use sha2::{Digest as _, Sha256};

use crate::api::Url;

/// The most bytes that the files on disk can use.
const DISK_LIMIT: u64 = 512 * 1024 * 1024;
/// The most bytes that the decoded images in memory can use (4 bytes for each pixel).
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
/// The most artwork downloads that run at the same time.
const DOWNLOADS: usize = 6;

/// The Artwork cache. It keeps an LRU of decoded images in memory, limited by decoded
/// bytes, and the files in `$XDG_CACHE_HOME/silo-omarchy/artwork/`. GPUI decodes each
/// image from a `Resource::Path`. The app has one Artwork cache, see [`global`].
pub struct ArtworkCache {
    this: WeakEntity<Self>,
    http: Arc<dyn HttpClient>,
    dir: PathBuf,
    entries: HashMap<String, Entry>,
    /// The downloads that wait for a free slot, in render order: (key, URL).
    queue: VecDeque<(String, String)>,
    running: usize,
    /// The decoded bytes of the images in memory.
    bytes: usize,
    /// Counts the calls to `load`. Each entry keeps the count of its last use.
    clock: u64,
    /// The bytes of the files on disk, as far as the cache knows: the bytes that the last prune kept, and the
    /// bytes that downloads wrote after it started.
    disk_bytes: u64,
    pruning: bool,
}

struct Entry {
    state: State,
    last_use: u64,
    bytes: usize,
    /// The file did not decode once, so the cache downloaded it again.
    refetched: bool,
}

enum State {
    /// The file is queued or downloading. The views wait for it.
    Fetching(HashSet<EntityId>),
    Failed(ImageCacheError),
    Ready(ImageCacheItem),
}

struct GlobalArtworkCache(Entity<ArtworkCache>);

impl gpui::Global for GlobalArtworkCache {}

/// The Artwork cache of the app. The first call makes it and prunes the disk cache.
pub fn global(cx: &mut App) -> Entity<ArtworkCache> {
    if let Some(cache) = cx.try_global::<GlobalArtworkCache>() {
        return cache.0.clone();
    }
    let dir = cache_dir();
    let http = cx.http_client();
    let cache = cx.new(|cx: &mut Context<ArtworkCache>| ArtworkCache {
        this: cx.weak_entity(),
        http,
        dir,
        entries: HashMap::new(),
        queue: VecDeque::new(),
        running: 0,
        bytes: 0,
        clock: 0,
        disk_bytes: 0,
        pruning: false,
    });
    cache.update(cx, |cache, cx| cache.prune(cx));
    cx.set_global(GlobalArtworkCache(cache.clone()));
    cache
}

/// `$XDG_CACHE_HOME/silo-omarchy/artwork/`, or `~/.cache/silo-omarchy/artwork/`.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("silo-omarchy/artwork")
}

impl ArtworkCache {
    /// Forgets the failed downloads, so that the next render tries them again.
    /// Call it when a list of artwork URLs loads again.
    pub fn retry_failed(&mut self) {
        self.entries.retain(|_, entry| !matches!(entry.state, State::Failed(_)));
    }

    /// Starts the queued downloads while fewer than `DOWNLOADS` run.
    fn start_downloads(&mut self, cx: &mut App) {
        while self.running < DOWNLOADS
            && let Some((key, url)) = self.queue.pop_front()
        {
            self.running += 1;
            let (http, dir, this) = (self.http.clone(), self.dir.clone(), self.this.clone());
            let download = cx.background_executor().spawn(async move { download(&http, &dir, &url).await });
            cx.spawn(async move |cx| {
                let result = download.await;
                this.update(cx, |this, cx| this.finish(key, result, cx)).ok();
            })
            .detach();
        }
    }

    fn finish(&mut self, key: String, result: anyhow::Result<(PathBuf, u64)>, cx: &mut Context<Self>) {
        self.running -= 1;
        if let Ok((_, written)) = &result {
            self.disk_bytes += written;
            if self.disk_bytes > DISK_LIMIT {
                self.prune(cx);
            }
        }
        if let Some(entry) = self.entries.get_mut(&key) {
            let state = match result {
                Ok((file, _)) => State::Ready(ImageCacheItem::new(&Resource::Path(file.into()), cx)),
                Err(err) => State::Failed(ImageCacheError::Other(Arc::new(err))),
            };
            if let State::Fetching(views) = std::mem::replace(&mut entry.state, state) {
                let app: &mut App = cx;
                for view in views {
                    app.notify(view);
                }
            }
        }
        self.start_downloads(cx);
    }

    /// Deletes the least recently used files on disk until they hold `DISK_LIMIT` bytes or less.
    fn prune(&mut self, cx: &mut Context<Self>) {
        if self.pruning {
            return;
        }
        self.pruning = true;
        // The downloads that finish while the prune runs add their bytes to the bytes that it keeps.
        self.disk_bytes = 0;
        let dir = self.dir.clone();
        let kept = cx.background_executor().spawn(async move { prune(&dir, DISK_LIMIT) });
        cx.spawn(async move |this, cx| {
            let kept = kept.await;
            this.update(cx, |this, _| {
                this.disk_bytes += kept;
                this.pruning = false;
            })
            .ok();
        })
        .detach();
    }

    /// Drops the least recently used images until the images in memory hold
    /// `MEMORY_LIMIT` bytes or less. It keeps the image that was used last.
    fn evict(&mut self, window: &mut Window, cx: &mut App) {
        while self.bytes > MEMORY_LIMIT {
            let Some(key) = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.bytes > 0 && entry.last_use < self.clock)
                .min_by_key(|(_, entry)| entry.last_use)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            let entry = self.entries.remove(&key).expect("the key comes from the map");
            self.bytes -= entry.bytes;
            if let State::Ready(item) = entry.state
                && let Some(Ok(image)) = item.get()
            {
                cx.drop_image(image, Some(window));
            }
        }
    }
}

impl ImageCache for ArtworkCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let (key, url) = match resource {
            Resource::Uri(uri) => (file_name(uri).unwrap_or_else(|| uri.to_string()), Some(uri.to_string())),
            Resource::Path(path) => (path.display().to_string(), None),
            Resource::Embedded(name) => (name.to_string(), None),
        };
        self.clock += 1;
        let entry = self.entries.entry(key.clone()).or_insert_with(|| {
            let state = match &url {
                Some(url) => {
                    self.queue.push_back((key.clone(), url.clone()));
                    State::Fetching(HashSet::new())
                }
                None => State::Ready(ImageCacheItem::new(resource, cx)),
            };
            Entry { state, last_use: 0, bytes: 0, refetched: false }
        });
        entry.last_use = self.clock;
        let result = match &mut entry.state {
            State::Fetching(views) => {
                views.insert(window.current_view());
                None
            }
            State::Failed(err) => Some(Err(err.clone())),
            State::Ready(item) => item.use_image(window),
        };
        // A cache file that does not decode goes, so that a revisioned file does not stay broken. The cache
        // downloads it again once.
        if let (Some(Err(_)), Some(url), State::Ready(_)) = (&result, &url, &entry.state) {
            discard(&self.dir.join(&key));
            if !entry.refetched {
                entry.refetched = true;
                entry.state = State::Fetching(HashSet::from([window.current_view()]));
                self.queue.push_back((key, url.clone()));
                self.start_downloads(cx);
                return None;
            }
        }
        if entry.bytes == 0
            && let Some(Ok(image)) = &result
        {
            let size = image.size(0);
            entry.bytes = (size.width.0 * size.height.0 * 4) as usize;
            self.bytes += entry.bytes;
            self.evict(window, cx);
        }
        self.start_downloads(cx);
        result
    }
}

/// Decodes a base64 Thumbhash to a small blurred image.
pub fn thumbhash(hash: &str) -> Option<Arc<RenderImage>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(hash).ok()?;
    let (width, height, mut pixels) = thumbhash::thumb_hash_to_rgba(&bytes).ok()?;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Deletes the least recently used files until the files in `dir` hold `max_bytes` or
/// less. It also deletes the temp files that a crash left behind. Returns the bytes that the files keep.
pub fn prune(dir: &Path, max_bytes: u64) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("tmp") => _ = std::fs::remove_file(&path),
            Some("etag") => {
                if !path.with_extension("").exists() {
                    _ = std::fs::remove_file(&path);
                }
            }
            _ => {
                if let Ok(meta) = entry.metadata() {
                    files.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), path));
                }
            }
        }
    }
    files.sort();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        discard(&path);
        total -= len;
    }
    total
}

/// Deletes a cache file and its ETag, so that the next fetch downloads it again.
pub fn discard(file: &Path) {
    _ = std::fs::remove_file(file);
    _ = std::fs::remove_file(file.with_extension("etag"));
}

/// Returns the cache file of the artwork at `url` in `dir`, and downloads it first when
/// necessary. A revisioned file on disk is used as it is. Other files are revalidated
/// with their ETag. When the revalidation fails, the old file is used.
pub async fn fetch(http: &Arc<dyn HttpClient>, dir: &Path, url: &str) -> anyhow::Result<PathBuf> {
    Ok(download(http, dir, url).await?.0)
}

/// [`fetch`], and the number of bytes that it wrote to `dir`.
pub async fn download(http: &Arc<dyn HttpClient>, dir: &Path, url: &str) -> anyhow::Result<(PathBuf, u64)> {
    let name = file_name(url).context("not an artwork URL")?;
    let file = dir.join(&name);
    let etag_file = dir.join(format!("{name}.etag"));
    let cached = file.exists();
    if cached {
        // The modification time tells `prune` when the file was last used.
        std::fs::File::options().write(true).open(&file)?.set_modified(SystemTime::now())?;
        if is_revisioned(url) {
            return Ok((file, 0));
        }
    }
    let mut request = Request::get(url);
    if cached && let Ok(etag) = std::fs::read_to_string(&etag_file) {
        request = request.header("If-None-Match", etag);
    }
    let reply = async {
        let response = http.send(request.body(AsyncBody::empty())?).await?;
        if response.status() == 304 && cached {
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!("HTTP {}", response.status());
        }
        let etag = response.headers().get("ETag").and_then(|v| v.to_str().ok()).map(str::to_string);
        let mut body = Vec::new();
        response.into_body().read_to_end(&mut body).await?;
        Ok(Some((etag, body)))
    };
    match reply.await {
        Ok(None) => Ok((file, 0)),
        Ok(Some((etag, body))) => {
            std::fs::create_dir_all(dir)?;
            let temp = dir.join(format!("{name}.tmp"));
            let written = body.len() as u64;
            std::fs::write(&temp, body)?;
            std::fs::rename(&temp, &file)?;
            match etag {
                Some(etag) => std::fs::write(&etag_file, etag)?,
                None => _ = std::fs::remove_file(&etag_file),
            }
            Ok((file, written))
        }
        Err(_) if cached => Ok((file, 0)),
        Err(err) => Err(err),
    }
}

/// The name of the cache file for an artwork URL: the hex SHA-256 of the origin plus
/// the path. The query holds only the signature, which changes, so the name ignores it.
pub fn file_name(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    let digest = Sha256::digest(format!("{}{}", url.origin().ascii_serialization(), url.path()));
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Revisioned artwork has a content revision between the variant and the extension
/// (`w500.<rev>.webp`). Its bytes never change.
pub fn is_revisioned(url: &str) -> bool {
    let Ok(url) = Url::parse(url) else { return false };
    let name = url.path().rsplit('/').next().unwrap_or_default();
    match name.split('.').collect::<Vec<_>>()[..] {
        [variant, _revision, _extension] => {
            variant == "original"
                || variant.strip_prefix('w').is_some_and(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()))
        }
        _ => false,
    }
}

/// The widest card on Home: the 16:9 card of Continue watching and Next up.
pub const HOME_CARD_WIDTH: f32 = 290.;

/// A rung of the Artwork ladder of posters and stills, from the smallest. It is the `image_size` of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    /// 300 px wide.
    Small,
    /// 500 px wide.
    Medium,
    Large,
}

impl Rung {
    /// The rung for a card of `width` logical pixels at the window `scale`.
    pub fn card(width: f32, scale: f32) -> Self {
        match width * scale {
            w if w <= 300. => Rung::Small,
            w if w <= 500. => Rung::Medium,
            _ => Rung::Large,
        }
    }

    /// The rung of Home: `small` at scale 1 and `medium` above it.
    pub fn home(scale: f32) -> Self {
        if scale > 1. { Rung::Medium } else { Rung::Small }
    }
}

impl std::fmt::Display for Rung {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Rung::Small => "small",
            Rung::Medium => "medium",
            Rung::Large => "large",
        })
    }
}
