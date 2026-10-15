//! The Release notice: the check of the latest release on GitHub, against a fake GitHub host.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{fixture, json, not_found, ok};
use futures::executor::block_on;
use gpui::{AppContext as _, TestAppContext};
use relics::release::{self, Release, ReleaseCheck};

const RELEASE: &str = "../own/github_release_latest.json";

fn release_99() -> Release {
    Release {
        version: "99.0.0".into(),
        url: "https://github.example.invalid/s0up4200/relics/releases/tag/v99.0.0".into(),
    }
}

#[test]
fn a_higher_tag_is_a_newer_release() {
    let (http, seen) = common::http(|_| ok(RELEASE));
    assert_eq!(block_on(release::newer_release(&*http, "0.1.0")).unwrap(), Some(release_99()));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].url, "https://api.github.com/repos/s0up4200/relics/releases/latest");
}

#[test]
fn the_same_or_a_lower_tag_is_not_newer() {
    for (tag, current) in [("v0.2.0", "0.2.0"), ("v0.1.9", "0.2.0"), ("v0.10.0", "0.9.0"), ("nightly", "0.1.0")] {
        let body = fixture(RELEASE).replace("v99.0.0", tag);
        let (http, _) = common::http(move |_| json(200, body.clone()));
        let newer = block_on(release::newer_release(&*http, current)).unwrap();
        assert_eq!(newer.is_some(), tag == "v0.10.0", "{tag} against {current}");
    }
}

#[test]
fn a_failed_check_is_an_error() {
    let (http, _) = common::http(|_| not_found());
    assert!(block_on(release::newer_release(&*http, "0.1.0")).is_err());
    let (http, _) = common::http(|_| None);
    assert!(block_on(release::newer_release(&*http, "0.1.0")).is_err());
}

#[gpui::test]
fn the_check_runs_at_startup_and_every_24_hours_until_the_user_turns_it_off(cx: &mut TestAppContext) {
    let (http, seen) = common::http(|_| ok(RELEASE));
    let check = cx.new(|cx| ReleaseCheck::new(Some(http), true, cx));
    cx.run_until_parked();
    assert_eq!(check.read_with(cx, |check, _| check.release.clone()), Some(release_99()));
    assert_eq!(seen.lock().unwrap().len(), 1);

    cx.executor().advance_clock(Duration::from_secs(24 * 60 * 60));
    cx.run_until_parked();
    assert_eq!(seen.lock().unwrap().len(), 2);

    check.update(cx, |check, cx| check.set_enabled(false, cx));
    assert_eq!(check.read_with(cx, |check, _| check.release.clone()), None);
    cx.executor().advance_clock(Duration::from_secs(48 * 60 * 60));
    cx.run_until_parked();
    assert_eq!(seen.lock().unwrap().len(), 2);

    check.update(cx, |check, cx| check.set_enabled(true, cx));
    cx.run_until_parked();
    assert_eq!(seen.lock().unwrap().len(), 3);
    assert_eq!(check.read_with(cx, |check, _| check.release.clone()), Some(release_99()));
}

#[gpui::test]
fn a_failed_check_keeps_the_release_that_the_last_check_found(cx: &mut TestAppContext) {
    let calls = AtomicUsize::new(0);
    let (http, seen) =
        common::http(move |_| if calls.fetch_add(1, Ordering::SeqCst) == 0 { ok(RELEASE) } else { None });
    let check = cx.new(|cx| ReleaseCheck::new(Some(http), true, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(24 * 60 * 60));
    cx.run_until_parked();
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert_eq!(check.read_with(cx, |check, _| check.release.clone()), Some(release_99()));
}

#[gpui::test]
fn a_check_without_a_client_sends_nothing(cx: &mut TestAppContext) {
    let check = cx.new(|cx| ReleaseCheck::new(None, true, cx));
    cx.run_until_parked();
    assert_eq!(check.read_with(cx, |check, _| check.release.clone()), None);
}
