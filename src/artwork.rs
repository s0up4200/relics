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
    AnyElement, App, AppContext as _, Context, Entity, EntityId, ImageCacheError, ImageCacheItem, ImageSource,
    InteractiveElement as _, IntoElement as _, ObjectFit, RenderImage, Resource, SharedString, Styled as _,
    StyledImage as _, WeakEntity, Window, div, img,
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
/// image from a `Resource::Path`. The app has one Artwork cache, see [`global`]. Views show artwork with [`image`].
pub struct ArtworkCache {
    this: WeakEntity<Self>,
    http: Arc<dyn HttpClient>,
    dir: PathBuf,
    entries: HashMap<String, Entry>,
    /// The downloads that wait for a free slot, in render order.
    queue: VecDeque<Job>,
    running: usize,
    /// The decoded bytes of the images in memory.
    bytes: usize,
    /// Counts the drawn frames. Each entry keeps the frame that last drew it.
    frame: u64,
    /// The current frame has an eviction queued for its end.
    frame_open: bool,
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
    /// The image, and the cache file that it decodes from.
    Ready(ImageCacheItem, PathBuf),
}

/// A download: the artwork at `url` for `target` pixels wide, see [`download_sized`].
struct Job {
    key: String,
    url: String,
    target: u32,
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
        frame: 0,
        frame_open: false,
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
            && let Some(Job { key, url, target }) = self.queue.pop_front()
        {
            self.running += 1;
            let (http, dir, this) = (self.http.clone(), self.dir.clone(), self.this.clone());
            let download =
                cx.background_executor().spawn(async move { download_sized(&http, &dir, &url, target).await });
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
                Ok((file, _)) => State::Ready(ImageCacheItem::new(&Resource::Path(file.clone().into()), cx), file),
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

    /// Runs after a frame is drawn. It drops the images that this frame did not draw, see [`evictions`], and
    /// starts the next frame.
    fn end_frame(&mut self, window: &mut Window, cx: &mut App) {
        let images = self.entries.iter().map(|(key, entry)| (key.clone(), entry.last_use, entry.bytes));
        for key in evictions(images, self.frame, self.bytes, MEMORY_LIMIT) {
            let entry = self.entries.remove(&key).expect("the key comes from the map");
            self.bytes -= entry.bytes;
            if let State::Ready(item, _) = entry.state
                && let Some(Ok(image)) = item.get()
            {
                cx.drop_image(image, Some(window));
            }
        }
        self.frame += 1;
        self.frame_open = false;
    }
}

/// The keys of the decoded images to drop, least recently drawn first, so that `bytes` falls to `limit` or less.
/// Each image is (key, last frame that drew it, decoded bytes). An image that `frame` drew always stays, also
/// when the images stay over the limit: the next frame draws it again, and a dropped image loads again.
pub fn evictions<K>(
    images: impl IntoIterator<Item = (K, u64, usize)>,
    frame: u64,
    mut bytes: usize,
    limit: usize,
) -> Vec<K> {
    let mut old: Vec<_> = images.into_iter().filter(|&(_, last, size)| size > 0 && last < frame).collect();
    old.sort_by_key(|&(_, last, _)| last);
    old.into_iter()
        .take_while(|&(_, _, size)| {
            let over = bytes > limit;
            bytes = bytes.saturating_sub(size);
            over
        })
        .map(|(key, ..)| key)
        .collect()
}

impl ArtworkCache {
    /// The image at `url` for `target` pixels wide. `None` while it loads.
    fn load(
        &mut self,
        url: &str,
        target: u32,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let key = format!("{}-w{target}", file_name(url).unwrap_or_else(|| url.to_string()));
        if !self.frame_open {
            self.frame_open = true;
            let this = self.this.clone();
            // A deferred call runs when the window has drawn the frame.
            window.defer(cx, move |window, cx| {
                this.update(cx, |this, cx| this.end_frame(window, cx)).ok();
            });
        }
        let entry = self.entries.entry(key.clone()).or_insert_with(|| {
            self.queue.push_back(Job { key: key.clone(), url: url.to_string(), target });
            Entry { state: State::Fetching(HashSet::new()), last_use: 0, bytes: 0, refetched: false }
        });
        entry.last_use = self.frame;
        let result = match &mut entry.state {
            State::Fetching(views) => {
                views.insert(window.current_view());
                None
            }
            State::Failed(err) => Some(Err(err.clone())),
            State::Ready(item, _) => item.use_image(window),
        };
        // A cache file that does not decode goes, so that a revisioned file does not stay broken. The cache
        // downloads it again once.
        if let (Some(Err(_)), State::Ready(_, file)) = (&result, &entry.state) {
            discard(file);
            if !entry.refetched {
                entry.refetched = true;
                entry.state = State::Fetching(HashSet::from([window.current_view()]));
                self.queue.push_back(Job { key, url: url.to_string(), target });
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
        }
        self.start_downloads(cx);
        result
    }
}

/// The artwork at `url`, shown `width` logical pixels wide. It loads through the Artwork cache, in the size
/// for `width` at the window scale. The Thumbhash `thumb` shows while the image loads and after it fails.
pub fn image(url: Option<String>, thumb: Option<Arc<RenderImage>>, width: f32) -> AnyElement {
    let preview = move || match thumb.clone() {
        Some(thumb) => img(ImageSource::Render(thumb)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
        None => div().into_any_element(),
    };
    let Some(url) = url else {
        return preview();
    };
    let id = SharedString::from(url.clone());
    let source = move |window: &mut Window, cx: &mut App| {
        let target = (width * window.scale_factor()).ceil() as u32;
        global(cx).update(cx, |cache, cx| cache.load(&url, target, window, cx))
    };
    // GPUI keeps the loading state, and thus shows the Thumbhash, only for an image with an id.
    img(ImageSource::Custom(Arc::new(source)))
        .id(id)
        .size_full()
        .object_fit(ObjectFit::Cover)
        .with_loading(preview.clone())
        .with_fallback(preview)
        .into_any_element()
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

/// [`download`] for an image that shows `target` pixels wide. When the image is much wider, the cache also keeps
/// a copy that is `target` pixels wide, and returns that copy. Later calls use the copy while the image on disk
/// stays the same. The number is the bytes that the call wrote to `dir`.
pub async fn download_sized(
    http: &Arc<dyn HttpClient>,
    dir: &Path,
    url: &str,
    target: u32,
) -> anyhow::Result<(PathBuf, u64)> {
    let (file, written) = download(http, dir, url).await?;
    let mut name = file.file_name().context("a cache file has a name")?.to_os_string();
    name.push(format!("-w{target}"));
    let copy = file.with_file_name(name);
    if written == 0 && copy.exists() {
        std::fs::File::options().write(true).open(&copy)?.set_modified(SystemTime::now())?;
        return Ok((copy, 0));
    }
    let Some(small) = shrink(&std::fs::read(&file)?, target) else {
        _ = std::fs::remove_file(&copy);
        return Ok((file, written));
    };
    let temp = copy.with_extension("tmp");
    let copied = small.len() as u64;
    std::fs::write(&temp, small)?;
    std::fs::rename(&temp, &copy)?;
    Ok((copy, written + copied))
}

/// An image that is at least this many times wider than its target gets a shrunk copy.
const SHRINK_RATIO: f32 = 1.5;

/// The image in `bytes` shrunk to `target` pixels wide, as a JPEG, when it is at least [`SHRINK_RATIO`] times
/// wider. Else `None`, also when the bytes are not an image.
pub fn shrink(bytes: &[u8], target: u32) -> Option<Vec<u8>> {
    let reader = || image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok();
    let (width, _) = reader()?.into_dimensions().ok()?;
    if target == 0 || (width as f32) < target as f32 * SHRINK_RATIO {
        return None;
    }
    let small = reader()?.decode().ok()?.resize(target, u32::MAX, image::imageops::FilterType::CatmullRom);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90).encode_image(&small.to_rgb8()).ok()?;
    Some(jpeg)
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

/// A kind of artwork. Each kind has its own widths on the Artwork ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Poster,
    /// The 16:9 image of an episode.
    Still,
    Backdrop,
    /// The photo of a person.
    Profile,
}

impl Kind {
    /// The widths in pixels of `small`, `medium` and `large`, from the server docs (`docs/images-api.md`).
    // ponytail: hard-coded; read `/images/capabilities` when the server changes its ladder.
    fn widths(self) -> [u32; 3] {
        match self {
            Kind::Poster | Kind::Still => [300, 500, 780],
            Kind::Backdrop => [300, 1920, 1920],
            Kind::Profile => [300, 500, 500],
        }
    }
}

/// A rung of the Artwork ladder, from the smallest. It is the `image_size` of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rung {
    Small,
    Medium,
    Large,
}

impl Rung {
    /// The smallest rung whose `kind` image is at least as wide as `width` logical pixels at the window `scale`.
    /// When no rung is wide enough, `Large`. A request with more kinds takes the largest of their rungs.
    pub fn fit(kind: Kind, width: f32, scale: f32) -> Self {
        let pixels = width * scale;
        let rungs = [Rung::Small, Rung::Medium, Rung::Large];
        rungs.into_iter().zip(kind.widths()).find(|&(_, w)| w as f32 >= pixels).map_or(Rung::Large, |(rung, _)| rung)
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
