//! The Artwork cache, the rung of the Artwork ladder, and the Thumbhash previews. It is the only module that
//! may call `gpui::img` (clippy.toml blocks it everywhere else), so that each image loads through the cache.
#![allow(clippy::disallowed_methods)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, bail};
use base64::Engine as _;
use futures::AsyncReadExt as _;
use futures::channel::oneshot;
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
/// The most revalidations that run at the same time. They have their own slots, so that they do not delay
/// the downloads.
const REVALIDATIONS: usize = 2;
/// A file on disk that is not revisioned is revalidated when its last check is this old.
const REVALIDATE_AFTER: Duration = Duration::from_secs(60 * 60);
/// An image in memory touches its files on disk at most this often, see [`touch`].
const TOUCH_AFTER: Duration = Duration::from_secs(10 * 60);

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
    /// The images that show from disk and are due for a revalidation. They start when no download waits.
    stale: VecDeque<Job>,
    revalidating: usize,
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
    /// The last time that the cache touched the files of the image, see [`touch`].
    touched: SystemTime,
}

enum State {
    /// The file is queued or downloading. The views wait for it, and [`ArtworkCache::cover`] calls wait in the
    /// senders.
    Fetching(HashSet<EntityId>, Vec<oneshot::Sender<anyhow::Result<PathBuf>>>),
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
        stale: VecDeque::new(),
        revalidating: 0,
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
fn cache_dir() -> PathBuf {
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

    /// Looks for the image on disk, off the main thread. An image on disk shows at once, and is queued for a
    /// revalidation when it is due. Else the image is queued for a download.
    fn open(&mut self, job: Job, cx: &mut App) {
        let (dir, this) = (self.dir.clone(), self.this.clone());
        let found = cx.background_executor().spawn(async move {
            let found = on_disk(&dir, &job.url, job.target);
            let due = found.is_some() && revalidation_due(&dir, &job.url, SystemTime::now());
            (job, found, due)
        });
        cx.spawn(async move |cx| {
            let (job, found, due) = found.await;
            this.update(cx, |this, cx| {
                match found {
                    Some(found) => {
                        this.ready(&job.key, Ok(found), cx);
                        if due {
                            this.stale.push_back(job);
                        }
                    }
                    None => this.queue.push_back(job),
                }
                this.start_downloads(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Starts the queued downloads while fewer than `DOWNLOADS` run. When no download waits, it also starts the
    /// queued revalidations while fewer than `REVALIDATIONS` run.
    fn start_downloads(&mut self, cx: &mut App) {
        while self.running < DOWNLOADS
            && let Some(job) = self.queue.pop_front()
        {
            self.running += 1;
            self.spawn(job, false, cx);
        }
        while self.queue.is_empty()
            && self.revalidating < REVALIDATIONS
            && let Some(job) = self.stale.pop_front()
        {
            self.revalidating += 1;
            self.spawn(job, true, cx);
        }
    }

    /// Runs [`download_sized`] for `job` in the background, and then [`Self::revalidated`] or [`Self::finish`].
    fn spawn(&self, Job { key, url, target }: Job, revalidation: bool, cx: &mut App) {
        let (http, dir, this) = (self.http.clone(), self.dir.clone(), self.this.clone());
        let download = cx
            .background_executor()
            .spawn(async move { download_sized(&http, &dir, &url, SystemTime::now(), target).await });
        cx.spawn(async move |cx| {
            let result = download.await;
            this.update(cx, |this, cx| match revalidation {
                true => this.revalidated(key, result, cx),
                false => this.finish(key, result, cx),
            })
            .ok();
        })
        .detach();
    }

    fn finish(&mut self, key: String, result: anyhow::Result<(PathBuf, u64)>, cx: &mut Context<Self>) {
        self.running -= 1;
        self.ready(&key, result, cx);
        self.start_downloads(cx);
    }

    /// Gives the entry at `key` the image in the file of `result`, or the error, and redraws its views.
    fn ready(&mut self, key: &str, result: anyhow::Result<(PathBuf, u64)>, cx: &mut Context<Self>) {
        if let Ok((_, written)) = &result {
            self.wrote(*written, cx);
        }
        if let Some(entry) = self.entries.get_mut(key) {
            let state = match result {
                Ok((file, _)) => State::Ready(ImageCacheItem::new(&Resource::Path(file.clone().into()), cx), file),
                Err(err) => State::Failed(ImageCacheError::Other(Arc::new(err))),
            };
            entry.touched = SystemTime::now();
            if let State::Fetching(views, covers) = std::mem::replace(&mut entry.state, state) {
                for cover in covers {
                    _ = cover.send(entry.state.file());
                }
                let app: &mut App = cx;
                for view in views {
                    app.notify(view);
                }
            }
        }
    }

    /// Counts `written` bytes on disk, and prunes the disk when they go over the limit.
    fn wrote(&mut self, written: u64, cx: &mut Context<Self>) {
        self.disk_bytes += written;
        if self.disk_bytes > DISK_LIMIT {
            self.prune(cx);
        }
    }

    /// A revalidation of the image at `key` ended. When the server sent a new file, the cache forgets each
    /// image of that URL, so that each width loads again from the new file. A failed revalidation keeps the
    /// image from disk.
    fn revalidated(&mut self, key: String, result: anyhow::Result<(PathBuf, u64)>, cx: &mut Context<Self>) {
        self.revalidating -= 1;
        if let Ok((_, written)) = result
            && written > 0
        {
            self.wrote(written, cx);
            let url = &key[..key.rfind("-w").expect("each key ends with its width") + 2];
            let old: Vec<String> = self.entries.keys().filter(|k| k.starts_with(url)).cloned().collect();
            for old in old {
                let entry = self.entries.remove(&old).expect("the key comes from the map");
                self.bytes -= entry.bytes;
                if let State::Ready(item, _) = entry.state
                    && let Some(Ok(image)) = item.get()
                {
                    cx.drop_image(image, None);
                }
            }
            cx.refresh_windows();
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
        for entry in self.entries.values_mut().filter(|entry| entry.bytes == 0) {
            if let State::Ready(item, _) = &entry.state
                && let Some(Ok(image)) = item.get()
            {
                let size = image.size(0);
                entry.bytes = (size.width.0 * size.height.0 * 4) as usize;
                self.bytes += entry.bytes;
            }
        }
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
        let key = key(url, target);
        if !self.frame_open {
            self.frame_open = true;
            let this = self.this.clone();
            // A deferred call runs when the window has drawn the frame.
            window.defer(cx, move |window, cx| {
                this.update(cx, |this, cx| this.end_frame(window, cx)).ok();
            });
        }
        let frame = self.frame;
        let entry = self.entry(&key, url, target, cx);
        entry.last_use = frame;
        let result = match &mut entry.state {
            State::Fetching(views, _) => {
                views.insert(window.current_view());
                None
            }
            State::Failed(err) => Some(Err(err.clone())),
            State::Ready(item, _) => item.use_image(window),
        };
        // A cache file that does not decode goes, so that a revisioned file does not stay broken. The cache
        // downloads it again once. After that the image fails, so that the next render does not discard again.
        if let (Some(Err(err)), State::Ready(_, file)) = (&result, &entry.state) {
            discard(file);
            if entry.refetched {
                entry.state = State::Failed(err.clone());
            } else {
                entry.refetched = true;
                entry.state = State::Fetching(HashSet::from([window.current_view()]), Vec::new());
                self.queue.push_back(Job { key, url: url.to_string(), target });
                self.start_downloads(cx);
                return None;
            }
        }
        // A memory hit tells `prune` that the files are in use. It is not each frame, because a touch writes to
        // the disk.
        let now = SystemTime::now();
        if let (Some(Ok(_)), State::Ready(_, file)) = (&result, &entry.state)
            && now.duration_since(entry.touched).is_ok_and(|age| age >= TOUCH_AFTER)
        {
            let file = file.clone();
            entry.touched = now;
            let (dir, url) = (self.dir.clone(), url.to_string());
            cx.background_executor().spawn(async move { touch(&dir, &url, &file, now) }).detach();
        }
        self.start_downloads(cx);
        result
    }

    /// The entry at `key`. When there is none, it makes one and starts to load the image.
    fn entry(&mut self, key: &str, url: &str, target: u32, cx: &mut App) -> &mut Entry {
        if !self.entries.contains_key(key) {
            self.open(Job { key: key.to_string(), url: url.to_string(), target }, cx);
        }
        self.entries.entry(key.to_string()).or_insert_with(|| Entry {
            state: State::Fetching(HashSet::new(), Vec::new()),
            last_use: 0,
            bytes: 0,
            refetched: false,
            touched: SystemTime::UNIX_EPOCH,
        })
    }

    /// The cache file of the artwork at `url` for `target` pixels wide, for the MPRIS cover. It loads through the
    /// same entries as [`image`], so that one URL has one download.
    pub fn cover(
        &mut self,
        url: &str,
        target: u32,
        cx: &mut App,
    ) -> impl Future<Output = anyhow::Result<PathBuf>> + use<> {
        let (sender, receiver) = oneshot::channel();
        let entry = self.entry(&key(url, target), url, target, cx);
        match &mut entry.state {
            State::Fetching(_, covers) => covers.push(sender),
            state => _ = sender.send(state.file()),
        }
        async move { receiver.await? }
    }
}

impl State {
    /// The cache file of a ready image, or the error of a failed one.
    fn file(&self) -> anyhow::Result<PathBuf> {
        match self {
            State::Ready(_, file) => Ok(file.clone()),
            State::Failed(err) => Err(anyhow::anyhow!("{err}")),
            State::Fetching(..) => Err(anyhow::anyhow!("the artwork is still loading")),
        }
    }
}

/// The key of the image at `url` for `target` pixels wide.
fn key(url: &str, target: u32) -> String {
    format!("{}-w{target}", file_name(url).unwrap_or_else(|| url.to_string()))
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

/// When the files in `dir` hold more than `max_bytes`, deletes the least recently used files until they hold 90% of
/// `max_bytes` or less, so that the next writes do not start a new prune at once. It also deletes the temp files
/// that a crash left behind. Returns the bytes that the files keep.
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
    if total <= max_bytes {
        return total;
    }
    for (_, len, path) in files {
        if total <= max_bytes / 10 * 9 {
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

/// Sets the modification time of the cache file of `url` in `dir`, and of `file`, its shrunk copy, to `now`.
/// `prune` then keeps them. The ETag file keeps its time, because that time is the last revalidation.
pub fn touch(dir: &Path, url: &str, file: &Path, now: SystemTime) {
    let base = file_name(url).map(|name| dir.join(name));
    for path in base.iter().map(PathBuf::as_path).chain([file]) {
        if let Ok(file) = std::fs::File::options().write(true).open(path) {
            _ = file.set_modified(now);
        }
    }
}

/// Returns the cache file of the artwork at `url` in `dir`, and the number of bytes that the call wrote to `dir`.
/// A file on disk is used as it is, except when [`revalidation_due`] at `now`: then the call revalidates it with
/// its ETag. When the revalidation fails, the old file is used. `now` is the time of the call.
pub async fn download(
    http: &Arc<dyn HttpClient>,
    dir: &Path,
    url: &str,
    now: SystemTime,
) -> anyhow::Result<(PathBuf, u64)> {
    let name = file_name(url).context("not an artwork URL")?;
    let file = dir.join(&name);
    let etag_file = dir.join(format!("{name}.etag"));
    let cached = file.exists();
    if cached {
        // The modification time tells `prune` when the file was last used.
        std::fs::File::options().write(true).open(&file)?.set_modified(now)?;
        if !revalidation_due(dir, url, now) {
            return Ok((file, 0));
        }
    }
    let mut request = Request::get(url);
    if cached
        && let Ok(etag) = std::fs::read_to_string(&etag_file)
        && !etag.is_empty()
    {
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
        Ok(Some((etag, body))) => {
            std::fs::create_dir_all(dir)?;
            // Each write has its own temp file, because two widths of one URL can download it at the same time.
            static WRITES: AtomicU64 = AtomicU64::new(0);
            let temp = dir.join(format!("{name}.{}.tmp", WRITES.fetch_add(1, Ordering::Relaxed)));
            let written = body.len() as u64;
            std::fs::write(&temp, body)?;
            std::fs::rename(&temp, &file)?;
            if cached {
                remove_copies(dir, &name);
            }
            // The ETag file also keeps the time of the last check, see `revalidation_due`. It is empty when the
            // server sent no ETag.
            std::fs::write(&etag_file, etag.unwrap_or_default())?;
            checked(&etag_file, now)?;
            Ok((file, written))
        }
        Err(err) if !cached => Err(err),
        // A 304 and a failed revalidation both start a new hour, so that an offline cache does not ask again
        // at each load.
        _ => {
            checked(&etag_file, now)?;
            Ok((file, 0))
        }
    }
}

/// Sets the time of the last check in `etag_file` to `now`. It makes an empty ETag file when there is none.
fn checked(etag_file: &Path, now: SystemTime) -> anyhow::Result<()> {
    let file = std::fs::File::options().write(true).create(true).truncate(false).open(etag_file)?;
    file.set_modified(now)?;
    Ok(())
}

/// Deletes the shrunk copies of the cache file `name` in `dir`.
fn remove_copies(dir: &Path, name: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let prefix = format!("{name}-w");
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            _ = std::fs::remove_file(entry.path());
        }
    }
}

/// True when the artwork at `url` is on disk in `dir`, is not revisioned, and its last check is at least
/// [`REVALIDATE_AFTER`] older than `now`. A file with no record of a check is due.
pub fn revalidation_due(dir: &Path, url: &str, now: SystemTime) -> bool {
    let Some(name) = file_name(url) else { return false };
    if is_revisioned(url) || !dir.join(&name).exists() {
        return false;
    }
    let checked = std::fs::metadata(dir.join(format!("{name}.etag"))).and_then(|meta| meta.modified());
    checked.map_or(true, |at| now.duration_since(at).is_ok_and(|age| age >= REVALIDATE_AFTER))
}

/// [`download`] for an image that shows `target` pixels wide. When the image is much wider, the cache also keeps
/// a copy that is `target` pixels wide, and returns that copy. Later calls use the copy while the image on disk
/// stays the same. The number is the bytes that the call wrote to `dir`.
pub async fn download_sized(
    http: &Arc<dyn HttpClient>,
    dir: &Path,
    url: &str,
    now: SystemTime,
    target: u32,
) -> anyhow::Result<(PathBuf, u64)> {
    let (file, written) = download(http, dir, url, now).await?;
    sized(file, written, target)
}

/// The file for the artwork at `url` that shows `target` pixels wide, from disk only, as [`download_sized`]
/// gives it. `None` when the artwork is not on disk. The number is the bytes of a new shrunk copy.
pub fn on_disk(dir: &Path, url: &str, target: u32) -> Option<(PathBuf, u64)> {
    let file = dir.join(file_name(url)?);
    // The modification time tells `prune` when the file was last used.
    std::fs::File::options().write(true).open(&file).ok()?.set_modified(SystemTime::now()).ok()?;
    sized(file, 0, target).ok()
}

/// The shrunk copy of the cache `file` for `target` pixels wide, or `file` when the image needs no copy. It makes
/// the copy when `written` is more than zero (the file is new) or when there is no copy. The number is `written`
/// plus the bytes of a new copy.
fn sized(file: PathBuf, written: u64, target: u32) -> anyhow::Result<(PathBuf, u64)> {
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
