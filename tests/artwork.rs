//! The Artwork cache: the rung of the Artwork ladder, the file names, and the disk
//! cache against a fake HTTP client.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response};
use silo_omarchy::artwork;

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
fn home_uses_small_at_scale_1_and_medium_above_it() {
    let card = artwork::HOME_CARD_WIDTH;
    assert_eq!(artwork::image_size(card, 1.0), "small");
    assert_eq!(artwork::image_size(card, 1.25), "medium");
    assert_eq!(artwork::image_size(card, 1.5), "medium");
    assert_eq!(artwork::image_size(card, 3.0), "large");
    assert!(artwork::is_larger("medium", "small"));
    assert!(artwork::is_larger("large", "medium"));
    assert!(!artwork::is_larger("small", "medium"), "a smaller scale keeps the loaded list");
    assert!(!artwork::is_larger("medium", "medium"));
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
