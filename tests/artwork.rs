//! The Artwork cache: the rung of the Artwork ladder, the file names, and the disk
//! cache against a fake HTTP client.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response};
use image::GenericImageView as _;
use silo_omarchy::artwork::{self, Rung};

fn empty_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("silo-omarchy-test-artwork-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// One reply of the fake artwork host: status, ETag, body.
type Reply = (u16, Option<&'static str>, &'static str);

/// The `If-None-Match` header of each request.
type Seen = Arc<Mutex<Vec<Option<String>>>>;

/// A fake artwork host. It answers with `replies` in order.
fn host(replies: Vec<Reply>) -> (Arc<dyn HttpClient>, Seen) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let replies = Arc::new(Mutex::new(replies.into_iter()));
    let http = FakeHttpClient::create(move |req| {
        let tag = req.headers().get("If-None-Match").map(|v| v.to_str().unwrap().to_string());
        log.lock().unwrap().push(tag);
        let (status, etag, body) = replies.lock().unwrap().next().expect("no more replies");
        async move {
            let mut response = Response::builder().status(status);
            if let Some(etag) = etag {
                response = response.header("ETag", etag);
            }
            Ok(response.body(AsyncBody::from(body.to_string())).unwrap())
        }
    });
    (http, seen)
}

const REVISIONED: &str = "https://silo.test/api/v2/artwork/poster/abc/w300.r1.webp?exp=1&sig=a";
const MUTABLE: &str = "https://silo.test/api/v2/artwork/poster/abc/w300.webp?exp=1&sig=a";

#[test]
fn revisioned_artwork_downloads_once_and_then_comes_from_disk() {
    let dir = empty_dir("revisioned");
    let (http, seen) = host(vec![(200, Some("\"e1\""), "poster")]);
    let file = block_on(artwork::fetch(&http, &dir, REVISIONED)).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "poster");
    let resigned = REVISIONED.replace("exp=1&sig=a", "exp=2&sig=b");
    assert_eq!(block_on(artwork::fetch(&http, &dir, &resigned)).unwrap(), file);
    assert_eq!(seen.lock().unwrap().len(), 1, "a revisioned file is never revalidated");
    let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(names.iter().all(|n| !n.to_string_lossy().ends_with(".tmp")), "{names:?}");
}

#[test]
fn other_artwork_revalidates_with_the_etag() {
    let dir = empty_dir("mutable");
    let (http, seen) =
        host(vec![(200, Some("\"e1\""), "old"), (304, Some("\"e1\""), ""), (200, Some("\"e2\""), "new")]);
    let file = block_on(artwork::fetch(&http, &dir, MUTABLE)).unwrap();
    assert_eq!(block_on(artwork::fetch(&http, &dir, MUTABLE)).unwrap(), file);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old", "304 keeps the file");
    block_on(artwork::fetch(&http, &dir, MUTABLE)).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new", "200 replaces the file");
    assert_eq!(*seen.lock().unwrap(), [None, Some("\"e1\"".into()), Some("\"e1\"".into())]);
}

#[test]
fn a_failed_download_writes_no_file_and_a_failed_revalidation_keeps_the_old_one() {
    let dir = empty_dir("failed");
    let (http, _) = host(vec![(404, None, "expired"), (200, Some("\"e1\""), "old"), (404, None, "expired")]);
    assert!(block_on(artwork::fetch(&http, &dir, MUTABLE)).is_err());
    assert_eq!(std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0), 0);
    let file = block_on(artwork::fetch(&http, &dir, MUTABLE)).unwrap();
    assert_eq!(block_on(artwork::fetch(&http, &dir, MUTABLE)).unwrap(), file);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
}

#[test]
fn a_thumbhash_decodes_to_a_small_bgra_preview() {
    use base64::Engine as _;
    let red: Vec<u8> = [255, 0, 0, 255].repeat(16 * 24);
    let hash = base64::engine::general_purpose::STANDARD.encode(thumbhash::rgba_to_thumb_hash(16, 24, &red));
    let preview = artwork::thumbhash(&hash).unwrap();
    let size = preview.size(0);
    assert!(size.width.0 > 0 && size.height.0 > size.width.0, "{size:?}");
    let first = &preview.as_bytes(0).unwrap()[..4];
    assert!(first[0] < 40 && first[2] > 200, "BGRA order, got {first:?}");
    assert!(artwork::thumbhash("not base64!").is_none());
}

#[test]
fn prune_deletes_the_least_recently_used_files_and_left_over_temp_files() {
    let dir = empty_dir("prune");
    std::fs::create_dir_all(&dir).unwrap();
    let now = std::time::SystemTime::now();
    for (name, age) in [("old", 30), ("mid", 20), ("new", 10)] {
        std::fs::write(dir.join(name), [0u8; 100]).unwrap();
        let file = std::fs::File::options().write(true).open(dir.join(name)).unwrap();
        file.set_modified(now - std::time::Duration::from_secs(age)).unwrap();
    }
    std::fs::write(dir.join("old.etag"), "\"e1\"").unwrap();
    std::fs::write(dir.join("crashed.tmp"), [0u8; 10]).unwrap();
    artwork::prune(&dir, 250);
    let mut left: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    assert_eq!(left, ["mid", "new"]);
}

#[test]
fn each_surface_gets_the_smallest_rung_at_least_as_wide_as_its_pixels() {
    use artwork::Kind::{Backdrop, Poster, Profile, Still};
    // The widths come from the server docs: poster and still 300/500/780, backdrop 300/1920/1920, profile 300/500/500.
    assert_eq!(Rung::fit(Poster, 150.0, 1.0), Rung::Small);
    assert_eq!(Rung::fit(Poster, 150.0, 2.0), Rung::Small, "300 px fits exactly");
    assert_eq!(Rung::fit(Poster, 150.0, 3.0), Rung::Medium);
    assert_eq!(Rung::fit(Poster, 512.0, 1.0), Rung::Large);
    assert_eq!(Rung::fit(Poster, 2000.0, 1.0), Rung::Large, "nothing is wider than large");
    assert_eq!(Rung::fit(Still, 290.0, 2.0), Rung::Large);
    assert_eq!(Rung::fit(Backdrop, 290.0, 1.0), Rung::Small);
    assert_eq!(Rung::fit(Backdrop, 290.0, 2.0), Rung::Medium, "medium is as wide as large");
    assert_eq!(Rung::fit(Profile, 200.0, 2.0), Rung::Medium);
    assert!(Rung::Medium > Rung::Small && Rung::Large > Rung::Medium, "a request takes the largest rung it needs");
    assert_eq!(Rung::Medium.to_string(), "medium");
}

#[test]
fn a_download_reports_the_bytes_that_it_wrote_and_prune_the_bytes_that_it_kept() {
    let dir = empty_dir("written");
    let (http, _) = host(vec![(200, Some("\"e1\""), "poster")]);
    let (file, written) = block_on(artwork::download(&http, &dir, REVISIONED)).unwrap();
    assert_eq!(written, 6);
    assert_eq!(
        block_on(artwork::download(&http, &dir, REVISIONED)).unwrap(),
        (file, 0),
        "a file on disk writes nothing"
    );
    assert_eq!(artwork::prune(&dir, 1000), 6);
    assert_eq!(artwork::prune(&dir, 5), 0);
}

#[test]
fn eviction_drops_the_oldest_images_but_never_one_drawn_in_this_frame() {
    // (key, last frame that drew it, decoded bytes). Frame 7 is the current frame.
    let images = [("old", 3, 40), ("older", 1, 40), ("drawn", 7, 100), ("also-drawn", 7, 100)];
    assert_eq!(artwork::evictions(images, 7, 280, 250), ["older"], "the least recently drawn goes first");
    assert_eq!(artwork::evictions(images, 7, 280, 100), ["older", "old"], "drawn images stay over the limit");
    assert!(artwork::evictions(images, 7, 250, 250).is_empty());
}

#[test]
fn a_discarded_file_downloads_again() {
    let dir = empty_dir("discard");
    let (http, seen) = host(vec![(200, Some("\"e1\""), "broken"), (200, Some("\"e2\""), "poster")]);
    let file = block_on(artwork::fetch(&http, &dir, REVISIONED)).unwrap();
    artwork::discard(&file);
    assert!(!file.exists() && !file.with_extension("etag").exists());
    assert_eq!(std::fs::read_to_string(block_on(artwork::fetch(&http, &dir, REVISIONED)).unwrap()).unwrap(), "poster");
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn a_new_signature_keeps_the_file_name_and_another_path_or_host_changes_it() {
    let name = artwork::file_name("https://silo.test/api/v2/artwork/p/w300.r1.webp?exp=1&sig=a").unwrap();
    // From `printf '%s' https://silo.test/api/v2/artwork/p/w300.r1.webp | sha256sum`.
    assert_eq!(name, "181e1620131105b7d0870488836993b4b1e3caf43e67492540c31de1c2c5362a");
    assert_eq!(artwork::file_name("https://silo.test/api/v2/artwork/p/w300.r1.webp?exp=2&sig=b").unwrap(), name);
    assert_ne!(artwork::file_name("https://silo.test/api/v2/artwork/p/w500.r1.webp").unwrap(), name);
    assert_ne!(artwork::file_name("https://other.test/api/v2/artwork/p/w300.r1.webp").unwrap(), name);
    assert_ne!(artwork::file_name("https://silo.test:8443/api/v2/artwork/p/w300.r1.webp").unwrap(), name);
    assert_eq!(artwork::file_name("not a url"), None);
}

#[test]
fn only_a_revision_between_the_width_and_the_extension_is_revisioned() {
    assert!(artwork::is_revisioned("https://silo.test/api/v2/artwork/poster/abc/w500.r1.webp?exp=1&sig=a"));
    assert!(artwork::is_revisioned("https://silo.test/api/v2/artwork/poster/abc/original.3f9a.jpg"));
    assert!(!artwork::is_revisioned("https://silo.test/api/v2/artwork/poster/abc/w500.webp?exp=1&sig=a"));
    assert!(!artwork::is_revisioned("https://s3.example.test/poster.r1.jpg"));
    assert!(!artwork::is_revisioned("https://s3.example.test/v1.2/poster.jpg"));
}

/// A PNG of `width` x `height` pixels.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(width, height).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn an_image_much_wider_than_its_target_shrinks_to_the_target_width() {
    let small = artwork::shrink(&png(600, 900), 200).expect("600 px is 3 times 200 px");
    assert_eq!(image::load_from_memory(&small).unwrap().dimensions(), (200, 300), "the aspect ratio stays");
    assert!(artwork::shrink(&png(600, 900), 500).is_none(), "600 px is near 500 px, so it stays");
    assert!(artwork::shrink(b"not an image", 200).is_none());
}

/// The width and height of the image in a cache file. Cache files have no extension.
fn dimensions(file: &std::path::Path) -> (u32, u32) {
    image::ImageReader::open(file).unwrap().with_guessed_format().unwrap().into_dimensions().unwrap()
}

/// A fake artwork host that sends the same image for each request, and counts the requests.
fn image_host(body: Vec<u8>) -> (Arc<dyn HttpClient>, Arc<Mutex<usize>>) {
    let count = Arc::new(Mutex::new(0));
    let log = count.clone();
    let http = FakeHttpClient::create(move |_| {
        *log.lock().unwrap() += 1;
        let body = body.clone();
        async move { Ok(Response::builder().status(200).body(AsyncBody::from(body)).unwrap()) }
    });
    (http, count)
}

#[test]
fn a_shrunk_copy_stays_on_disk_for_each_target_width() {
    let dir = empty_dir("shrunk");
    let (http, count) = image_host(png(600, 900));
    let (copy, _) = block_on(artwork::download_sized(&http, &dir, REVISIONED, 200)).unwrap();
    assert_eq!(dimensions(&copy), (200, 300));
    let resigned = REVISIONED.replace("exp=1&sig=a", "exp=2&sig=b");
    assert_eq!(block_on(artwork::download_sized(&http, &dir, &resigned, 200)).unwrap(), (copy.clone(), 0));
    assert_eq!(*count.lock().unwrap(), 1, "the later load uses the copy on disk");
    let (other, _) = block_on(artwork::download_sized(&http, &dir, REVISIONED, 300)).unwrap();
    assert_ne!(other, copy, "another target width has its own copy");
    let (full, _) = block_on(artwork::download_sized(&http, &dir, REVISIONED, 500)).unwrap();
    assert_eq!(dimensions(&full), (600, 900), "an image near the target stays as it is");
}
