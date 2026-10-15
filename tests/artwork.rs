//! The Artwork cache: the rung of the Artwork ladder, the file names, and the disk
//! cache against a fake HTTP client.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response};
use image::GenericImageView as _;
use relics::artwork::{self, Rung};

fn empty_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relics-test-artwork-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// One reply of the fake artwork host: status, ETag, body.
type Reply<B> = (u16, Option<&'static str>, B);

/// The `If-None-Match` header of each request.
type Seen = Arc<Mutex<Vec<Option<String>>>>;

/// A fake artwork host. It answers with `replies` in order.
fn host<B: Into<Vec<u8>> + Send + 'static>(replies: Vec<Reply<B>>) -> (Arc<dyn HttpClient>, Seen) {
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
            Ok(response.body(AsyncBody::from(body.into())).unwrap())
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
    let file = block_on(artwork::download(&http, &dir, REVISIONED, start())).unwrap().0;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "poster");
    let resigned = REVISIONED.replace("exp=1&sig=a", "exp=2&sig=b");
    assert_eq!(block_on(artwork::download(&http, &dir, &resigned, start())).unwrap().0, file);
    assert_eq!(seen.lock().unwrap().len(), 1, "a revisioned file is never revalidated");
    let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(names.iter().all(|n| !n.to_string_lossy().ends_with(".tmp")), "{names:?}");
}

/// The time of the first download in a test. Later calls add to it, so no test waits.
fn start() -> SystemTime {
    SystemTime::now()
}

const MINUTE: Duration = Duration::from_secs(60);

#[test]
fn other_artwork_revalidates_with_the_etag_at_most_once_per_hour() {
    let dir = empty_dir("mutable");
    let (http, seen) =
        host(vec![(200, Some("\"e1\""), "old"), (304, Some("\"e1\""), ""), (200, Some("\"e2\""), "new")]);
    let t0 = start();
    let (file, _) = block_on(artwork::download(&http, &dir, MUTABLE, t0)).unwrap();
    assert!(!artwork::revalidation_due(&dir, MUTABLE, t0 + 59 * MINUTE));
    assert_eq!(block_on(artwork::download(&http, &dir, MUTABLE, t0 + 59 * MINUTE)).unwrap(), (file.clone(), 0));
    assert_eq!(seen.lock().unwrap().len(), 1, "a file checked less than an hour ago comes from disk");
    let t1 = t0 + 61 * MINUTE;
    assert!(artwork::revalidation_due(&dir, MUTABLE, t1));
    block_on(artwork::download(&http, &dir, MUTABLE, t1)).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old", "304 keeps the file");
    assert!(!artwork::revalidation_due(&dir, MUTABLE, t1 + 30 * MINUTE), "the 304 starts a new hour");
    let t2 = t1 + 61 * MINUTE;
    block_on(artwork::download(&http, &dir, MUTABLE, t2)).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new", "200 replaces the file");
    assert_eq!(*seen.lock().unwrap(), [None, Some("\"e1\"".into()), Some("\"e1\"".into())]);
}

#[test]
fn revisioned_artwork_never_revalidates_and_a_missing_file_is_not_due() {
    let dir = empty_dir("never-due");
    let (http, _) = host(vec![(200, Some("\"e1\""), "poster")]);
    let t0 = start();
    assert!(!artwork::revalidation_due(&dir, MUTABLE, t0), "there is no file to check");
    block_on(artwork::download(&http, &dir, REVISIONED, t0)).unwrap();
    assert!(!artwork::revalidation_due(&dir, REVISIONED, t0 + Duration::from_secs(400 * 24 * 3600)));
}

#[test]
fn a_failed_download_writes_no_file_and_a_failed_revalidation_keeps_the_old_one() {
    let dir = empty_dir("failed");
    let (http, seen) = host(vec![(404, None, "expired"), (200, Some("\"e1\""), "old"), (503, None, "down")]);
    let t0 = start();
    assert!(block_on(artwork::download(&http, &dir, MUTABLE, t0)).is_err());
    assert_eq!(std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0), 0);
    let (file, _) = block_on(artwork::download(&http, &dir, MUTABLE, t0)).unwrap();
    let t1 = t0 + 61 * MINUTE;
    assert_eq!(block_on(artwork::download(&http, &dir, MUTABLE, t1)).unwrap(), (file.clone(), 0));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
    assert!(!artwork::revalidation_due(&dir, MUTABLE, t1 + 30 * MINUTE), "a failed check also waits an hour");
    assert_eq!(seen.lock().unwrap().len(), 3);
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
    let crashed = std::fs::File::options().write(true).open(dir.join("crashed.tmp")).unwrap();
    crashed.set_modified(now - std::time::Duration::from_secs(7200)).unwrap();
    std::fs::write(dir.join("running.tmp"), [0u8; 10]).unwrap();
    artwork::prune(&dir, 250);
    let mut left: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    assert_eq!(left, ["mid", "new", "running.tmp"]);
}

#[test]
fn prune_waits_for_the_limit_and_then_trims_to_90_percent_of_it() {
    let dir = empty_dir("prune-margin");
    std::fs::create_dir_all(&dir).unwrap();
    let now = std::time::SystemTime::now();
    for (name, len, age) in [("old", 10, 30), ("mid", 100, 20), ("new", 100, 10)] {
        std::fs::write(dir.join(name), vec![0u8; len]).unwrap();
        let file = std::fs::File::options().write(true).open(dir.join(name)).unwrap();
        file.set_modified(now - std::time::Duration::from_secs(age)).unwrap();
    }
    assert_eq!(artwork::prune(&dir, 210), 210, "at the limit nothing goes");
    assert_eq!(artwork::prune(&dir, 205), 100, "over the limit the files fall to 184 bytes or less");
    assert!(dir.join("new").exists());
}

#[test]
fn a_touch_keeps_the_file_and_its_shrunk_copy_but_not_the_revalidation_clock() {
    let dir = empty_dir("touch");
    let (http, _) = image_host(vec![png(600, 900)]);
    let t0 = start();
    block_on(artwork::download_sized(&http, &dir, MUTABLE, t0, 500)).unwrap();
    let (copy, _) = artwork::on_disk(&dir, MUTABLE, 200).unwrap();
    let later = t0 + 2 * MINUTE;
    artwork::touch(&dir, MUTABLE, &copy, later);
    let modified = |path: &std::path::Path| std::fs::metadata(path).unwrap().modified().unwrap();
    let file = dir.join(artwork::file_name(MUTABLE).unwrap());
    assert_eq!(modified(&file), later, "prune keeps the file that the copy comes from");
    assert_eq!(modified(&copy), later);
    assert!(artwork::revalidation_due(&dir, MUTABLE, t0 + 61 * MINUTE), "a touch is not a check");
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
    let (file, written) = block_on(artwork::download(&http, &dir, REVISIONED, start())).unwrap();
    assert_eq!(written, 6);
    assert_eq!(
        block_on(artwork::download(&http, &dir, REVISIONED, start())).unwrap(),
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
fn a_queued_download_goes_when_this_frame_did_not_draw_its_image() {
    // (key, last frame that drew it, a cover waits for it). Frame 7 is the current frame.
    let jobs = [("left-page", 5, false), ("drawn", 7, false), ("cover", 2, true)];
    assert_eq!(artwork::unwanted(jobs, 7), ["left-page"]);
}

#[test]
fn a_discarded_file_downloads_again() {
    let dir = empty_dir("discard");
    let (http, seen) = host(vec![(200, Some("\"e1\""), "broken"), (200, Some("\"e2\""), "poster")]);
    let file = block_on(artwork::download(&http, &dir, REVISIONED, start())).unwrap().0;
    artwork::discard(&file);
    assert!(!file.exists() && !file.with_extension("etag").exists());
    assert_eq!(
        std::fs::read_to_string(block_on(artwork::download(&http, &dir, REVISIONED, start())).unwrap().0).unwrap(),
        "poster"
    );
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

/// A fake artwork host that sends `bodies` in order and then the last one again, and counts the requests.
fn image_host(bodies: Vec<Vec<u8>>) -> (Arc<dyn HttpClient>, Arc<Mutex<usize>>) {
    let count = Arc::new(Mutex::new(0));
    let log = count.clone();
    let http = FakeHttpClient::create(move |_| {
        let mut count = log.lock().unwrap();
        let body = bodies[(*count).min(bodies.len() - 1)].clone();
        *count += 1;
        async move { Ok(Response::builder().status(200).body(AsyncBody::from(body)).unwrap()) }
    });
    (http, count)
}

#[test]
fn a_shrunk_copy_stays_on_disk_for_each_target_width() {
    let dir = empty_dir("shrunk");
    let (http, count) = image_host(vec![png(600, 900)]);
    let (copy, _) = block_on(artwork::download_sized(&http, &dir, REVISIONED, start(), 200)).unwrap();
    assert_eq!(dimensions(&copy), (200, 300));
    let resigned = REVISIONED.replace("exp=1&sig=a", "exp=2&sig=b");
    assert_eq!(block_on(artwork::download_sized(&http, &dir, &resigned, start(), 200)).unwrap(), (copy.clone(), 0));
    assert_eq!(*count.lock().unwrap(), 1, "the later load uses the copy on disk");
    let (other, _) = block_on(artwork::download_sized(&http, &dir, REVISIONED, start(), 300)).unwrap();
    assert_ne!(other, copy, "another target width has its own copy");
    let (full, _) = block_on(artwork::download_sized(&http, &dir, REVISIONED, start(), 500)).unwrap();
    assert_eq!(dimensions(&full), (600, 900), "an image near the target stays as it is");
}

#[test]
fn a_file_on_disk_shows_without_a_request() {
    let dir = empty_dir("on-disk");
    let (http, count) = image_host(vec![png(600, 900)]);
    assert_eq!(artwork::on_disk(&dir, MUTABLE, 200), None, "nothing is on disk yet");
    let (full, _) = block_on(artwork::download_sized(&http, &dir, MUTABLE, start(), 500)).unwrap();
    assert_eq!(full, dir.join(artwork::file_name(MUTABLE).unwrap()), "a view near the full size uses the file");
    let (copy, written) = artwork::on_disk(&dir, MUTABLE, 200).expect("the file is on disk");
    assert_eq!(dimensions(&copy), (200, 300), "another width shrinks from the file on disk");
    assert!(written > 0, "the new copy counts as written");
    assert!(full.exists(), "a view uses the full-size file, so it stays");
    assert_eq!(artwork::on_disk(&dir, MUTABLE, 200), Some((copy, 0)));
    assert_eq!(*count.lock().unwrap(), 1);
}

#[test]
fn a_new_file_from_a_revalidation_replaces_the_shrunk_copies() {
    let dir = empty_dir("replaced");
    let (http, count) = image_host(vec![png(600, 900), png(900, 600)]);
    let t0 = start();
    block_on(artwork::download_sized(&http, &dir, MUTABLE, t0, 500)).unwrap();
    artwork::on_disk(&dir, MUTABLE, 200).unwrap();
    let (copy, _) = block_on(artwork::download_sized(&http, &dir, MUTABLE, t0 + 61 * MINUTE, 500)).unwrap();
    assert_eq!(*count.lock().unwrap(), 2);
    assert_eq!(dimensions(&copy), (500, 333), "the copy comes from the new file");
    assert_eq!(artwork::on_disk(&dir, MUTABLE, 200), None, "the old copy for another width is gone");
}

#[test]
fn a_shrunk_download_keeps_only_the_copy_and_the_etag() {
    let dir = empty_dir("only-copy");
    let (http, seen) = host(vec![
        (200, Some("\"e1\""), png(600, 900)),
        (304, None, Vec::new()),
        (200, Some("\"e2\""), png(900, 600)),
        (200, Some("\"e2\""), png(900, 600)),
    ]);
    let name = artwork::file_name(MUTABLE).unwrap();
    let t0 = start();
    let (copy, _) = block_on(artwork::download_sized(&http, &dir, MUTABLE, t0, 200)).unwrap();
    assert!(!dir.join(&name).exists(), "the copy replaces the full-size file");
    assert!(dir.join(format!("{name}.etag")).exists(), "the ETag stays for the revalidation");
    assert_eq!(artwork::on_disk(&dir, MUTABLE, 200), Some((copy.clone(), 0)));
    let t1 = t0 + 61 * MINUTE;
    assert!(artwork::revalidation_due(&dir, MUTABLE, t1));
    assert_eq!(block_on(artwork::download_sized(&http, &dir, MUTABLE, t1, 200)).unwrap(), (copy.clone(), 0));
    assert_eq!(dimensions(&copy), (200, 300), "304 keeps the copy");
    assert!(!artwork::revalidation_due(&dir, MUTABLE, t1 + 30 * MINUTE), "the 304 starts a new hour");
    let t2 = t1 + 61 * MINUTE;
    let (new, written) = block_on(artwork::download_sized(&http, &dir, MUTABLE, t2, 200)).unwrap();
    assert_eq!(new, copy);
    assert!(written > 0);
    assert_eq!(dimensions(&copy), (200, 133), "200 replaces the copy");
    assert!(!dir.join(&name).exists(), "the new full-size file also goes");
    assert_eq!(artwork::on_disk(&dir, MUTABLE, 300), None, "another width has no copy and no file to shrink");
    let (other, _) = block_on(artwork::download_sized(&http, &dir, MUTABLE, t2, 300)).unwrap();
    assert_eq!(dimensions(&other), (300, 200));
    let etag = |tag: &str| Some(tag.to_string());
    assert_eq!(*seen.lock().unwrap(), [None, etag("\"e1\""), etag("\"e1\""), None], "a 304 needs a copy to serve");
}

#[test]
fn prune_keeps_an_etag_while_the_file_or_a_shrunk_copy_stays() {
    let dir = empty_dir("prune-etag");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["a", "a.etag", "b-w300", "b.etag", "c.etag"] {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    artwork::prune(&dir, 1000);
    let mut left: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    assert_eq!(left, ["a", "a.etag", "b-w300", "b.etag"]);
}

#[gpui::test]
fn replacing_and_clearing_previews_releases_the_old_images(cx: &mut gpui::TestAppContext) {
    use base64::Engine as _;
    let pixels = vec![255; 16 * 16 * 4];
    let hash = base64::engine::general_purpose::STANDARD.encode(thumbhash::rgba_to_thumb_hash(16, 16, &pixels));
    let first = relics::artwork::thumbhash(&hash).unwrap();
    let old = std::sync::Arc::downgrade(&first);
    let second = relics::artwork::thumbhash(&hash).unwrap();
    let replaced = std::sync::Arc::downgrade(&second);
    let mut previews = vec![Some(first), None];
    cx.update(|cx| relics::artwork::replace_previews(&mut previews, vec![Some(second)], cx));
    cx.run_until_parked();
    assert!(old.upgrade().is_none());
    assert!(replaced.upgrade().is_some());
    cx.update(|cx| relics::artwork::replace_previews(&mut previews, Vec::new(), cx));
    cx.run_until_parked();
    assert!(replaced.upgrade().is_none());
}
