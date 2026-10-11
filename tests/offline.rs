//! Offline and an expired sign-in: what counts as Offline, the retry delays, and the progress that the app
//! keeps while the Server does not answer. Against a fake HTTP client.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::AsyncReadExt as _;
use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use serde_json::{Value, json};
use silo_omarchy::api::{Client, Decision, Device, Error, Server, Tokens, Trouble};
use silo_omarchy::library::retry_delay;
use silo_omarchy::playback::{Replan, Sample};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: String,
}

type Reply = Option<(u16, &'static str, String)>;

/// A fake Silo server at `https://silo.test/`. `reply` gets each request and returns the reply, or `None`
/// for no reply (a network failure).
fn fake(reply: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (Client, Arc<Mutex<Vec<Seen>>>, Troubles) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    let http: Arc<dyn HttpClient> = FakeHttpClient::create(move |req| {
        let (log, reply) = (log.clone(), reply.clone());
        async move {
            let method = req.method().to_string();
            let path = req.uri().to_string().strip_prefix("https://silo.test/api/v2/").unwrap().to_string();
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            let request = Seen { method, path, body };
            log.lock().unwrap().push(request.clone());
            let (status, content_type, body) = reply(&request).ok_or_else(|| anyhow::anyhow!("no route to host"))?;
            Ok(Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .body(AsyncBody::from(body))
                .unwrap())
        }
    });
    let server =
        Server { id: "server-1".into(), name: "silo.test".into(), url: Url::parse("https://silo.test/").unwrap() };
    let device = Device { id: "6c1f0d3e-0000-4000-8000-000000000001".into(), name: "omarchy-box".into() };
    let client = Client::new(http, server, device, Tokens { access_token: "acc".into(), refresh_token: "ref".into() });
    client.set_profile(Some("p-owner".into()));
    let troubles = Troubles::default();
    let log = troubles.clone();
    client.on_trouble(move |trouble| log.0.lock().unwrap().push(trouble));
    (client, seen, troubles)
}

#[derive(Clone, Default)]
struct Troubles(Arc<Mutex<Vec<Trouble>>>);

impl Troubles {
    fn get(&self) -> Vec<Trouble> {
        self.0.lock().unwrap().clone()
    }
}

fn ok(path: &str) -> Reply {
    Some((200, "application/json", fixture(path)))
}

fn problem(status: u16, code: &str) -> Reply {
    let body = json!({ "type": format!("https://silo.test/problems/{code}"), "title": code, "status": status });
    Some((status, "application/problem+json", body.to_string()))
}

/// The playback start requests of the server fixtures.
fn playback(req: &Seen) -> Reply {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "catalog/items/movie:heat-1995/versions") => ok("silo/list_catalog_item_versions_ok.json"),
        ("GET", "playback/capabilities") => ok("silo/playback_capability_available.json"),
        ("GET", p) if p.starts_with("markers/") => problem(404, "not_found"),
        ("POST", "playback/start") => Some((201, "application/json", fixture("silo/playback_start_opaque_ids.json"))),
        _ => None,
    }
}

#[test]
fn the_app_retries_after_2_4_8_and_16_seconds_then_every_30_seconds() {
    let delays: Vec<u64> = (0..7).map(|attempt| retry_delay(attempt).as_secs()).collect();
    assert_eq!(delays, [2, 4, 8, 16, 30, 30, 30]);
    assert_eq!(retry_delay(u32::MAX), Duration::from_secs(30));
}

#[test]
fn a_refused_refresh_with_400_401_or_403_signs_the_user_out() {
    for status in [400, 401, 403] {
        let (client, _, troubles) = fake(move |req| match req.path.as_str() {
            "auth/refresh" => problem(status, "refresh_session_revoked"),
            _ => problem(401, "token_refresh_required"),
        });
        let err = block_on(client.profiles()).err().unwrap();
        assert!(matches!(err, Error::SignedOut), "{status}: {err:?}");
        assert_eq!(troubles.get(), [Trouble::SignedOut], "{status}");
    }
}

#[test]
fn a_503_or_no_reply_is_offline_and_never_signs_the_user_out() {
    let replies: [fn(&Seen) -> Reply; 4] = [
        |_| problem(503, "service_unavailable"),
        |_| Some((503, "text/plain", String::new())),
        |_| None,
        |req| if req.path == "auth/refresh" { None } else { problem(401, "token_refresh_required") },
    ];
    for (case, reply) in replies.into_iter().enumerate() {
        let (client, _, troubles) = fake(reply);
        let err = block_on(client.profiles()).err().unwrap();
        assert!(err.is_offline(), "case {case}: {err:?}");
        assert_eq!(troubles.get(), [Trouble::Offline], "case {case}");
        let err = block_on(client.refresh_access()).err().unwrap();
        assert!(err.is_offline(), "case {case}: a refresh: {err:?}");
    }
}

#[test]
fn the_probe_reads_system_info_with_no_sign_in() {
    let (client, seen, _) =
        fake(|req| if req.path == "system/info" { ok("silo/get_system_info_ok.json") } else { None });
    block_on(client.probe()).unwrap();
    assert_eq!(seen.lock().unwrap()[0].path, "system/info");
}

#[test]
fn a_failed_progress_report_is_sent_with_sync_progress_when_the_server_answers() {
    let down = Arc::new(AtomicBool::new(false));
    let (client, seen, _) = fake({
        let down = down.clone();
        move |req| match req.path.as_str() {
            _ if down.load(Ordering::SeqCst) => None,
            "sync/progress" => ok("silo/sync_progress_ok.json"),
            _ => playback(req),
        }
    });
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    down.store(true, Ordering::SeqCst);
    let sample = |sequence, position| Sample { sequence, position, paused: false };
    assert!(block_on(client.report_progress(SESSION, &sample(1, 60.0))).is_err());
    assert!(block_on(client.stop_playback(SESSION, Some(&sample(2, 75.5)))).is_err());
    assert!(block_on(client.sync_progress()).is_err(), "the sync fails while the server is down");

    down.store(false, Ordering::SeqCst);
    block_on(client.sync_progress()).unwrap();
    let syncs: Vec<Value> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.path == "sync/progress")
        .map(|s| serde_json::from_str(&s.body).unwrap())
        .collect();
    let last = &syncs.last().unwrap()["items"];
    assert_eq!(last.as_array().unwrap().len(), 1, "one position for each Item: {last}");
    assert_eq!(last[0]["media_item_id"], "movie:heat-1995");
    assert_eq!(last[0]["position_ms"], 75500, "the last position");
    assert!(last[0]["updated_at"].is_string(), "{last}");

    block_on(client.sync_progress()).unwrap();
    let count = seen.lock().unwrap().iter().filter(|s| s.path == "sync/progress").count();
    assert_eq!(count, syncs.len(), "a sent position is not sent again");
}

#[test]
fn kept_progress_goes_only_to_its_profile() {
    let down = Arc::new(AtomicBool::new(false));
    let (client, seen, _) = fake({
        let down = down.clone();
        move |req| match req.path.as_str() {
            _ if down.load(Ordering::SeqCst) => None,
            "sync/progress" => ok("silo/sync_progress_ok.json"),
            _ => playback(req),
        }
    });
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    down.store(true, Ordering::SeqCst);
    block_on(client.report_progress(SESSION, &Sample { sequence: 1, position: 60.0, paused: false })).ok();
    down.store(false, Ordering::SeqCst);

    client.set_profile(Some("p-kid".into()));
    block_on(client.sync_progress()).unwrap();
    assert!(!seen.lock().unwrap().iter().any(|s| s.path == "sync/progress"), "another Profile sends nothing");
    client.set_profile(Some("p-owner".into()));
    block_on(client.sync_progress()).unwrap();
    assert!(seen.lock().unwrap().iter().any(|s| s.path == "sync/progress"));
}

#[test]
fn a_report_that_the_server_refuses_is_synced_at_once() {
    let (client, seen, _) = fake(|req| match req.path.as_str() {
        p if p.ends_with("/progress") && p.starts_with("playback/") => problem(404, "playback_session_not_found"),
        "sync/progress" => ok("silo/sync_progress_ok.json"),
        _ => playback(req),
    });
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    assert!(block_on(client.report_progress(SESSION, &Sample { sequence: 1, position: 42.0, paused: false })).is_err());
    let seen = seen.lock().unwrap();
    let sync = seen.iter().find(|s| s.path == "sync/progress").expect("a sync after the refused report");
    assert_eq!(serde_json::from_str::<Value>(&sync.body).unwrap()["items"][0]["position_ms"], 42000);
}

#[test]
fn a_replan_for_an_ended_session_starts_a_new_session_at_the_position() {
    let (client, seen, _) = fake(|req| match req.path.as_str() {
        p if p.ends_with("/replan") => problem(410, "playback_session_ended"),
        _ => playback(req),
    });
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    let replan = Replan {
        session_id: SESSION.into(),
        attempt_id: "attempt".into(),
        failed_plan_id: "plan".into(),
        plan_attempt_key: "key".into(),
        attempted_plan_keys: vec!["key".into()],
        selected_tracks: json!({}),
        position: 1234.5,
    };
    let decision = block_on(client.replan(&replan)).unwrap();
    assert!(matches!(decision, Decision::Playable { .. }), "{decision:?}");
    let seen = seen.lock().unwrap();
    let starts: Vec<Value> =
        seen.iter().filter(|s| s.path == "playback/start").map(|s| serde_json::from_str(&s.body).unwrap()).collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[1]["start_position"], 1234.5);
}

#[test]
fn sign_out_sends_the_kept_progress_and_then_ends_the_session_on_the_server() {
    let down = Arc::new(AtomicBool::new(false));
    let (client, seen, _) = fake({
        let down = down.clone();
        move |req| match req.path.as_str() {
            _ if down.load(Ordering::SeqCst) => None,
            "sync/progress" => ok("silo/sync_progress_ok.json"),
            "auth/logout" => Some((204, "application/json", String::new())),
            _ => playback(req),
        }
    });
    block_on(client.start_playback("movie:heat-1995", None)).unwrap();
    down.store(true, Ordering::SeqCst);
    block_on(client.report_progress(SESSION, &Sample { sequence: 1, position: 60.0, paused: false })).ok();
    down.store(false, Ordering::SeqCst);

    block_on(client.sign_out()).unwrap();
    let paths: Vec<String> = seen.lock().unwrap().iter().map(|s| format!("{} {}", s.method, s.path)).collect();
    assert_eq!(paths[paths.len() - 2..], ["POST sync/progress", "POST auth/logout"]);
}

#[test]
fn progress_kept_across_a_server_switch_goes_out_when_the_account_is_active_again() {
    let down = Arc::new(AtomicBool::new(true));
    let reply = {
        let down = down.clone();
        move |req: &Seen| match req.path.as_str() {
            "playback/start" | "playback/capabilities" | "catalog/items/movie:heat-1995/versions" => playback(req),
            p if p.starts_with("markers/") => playback(req),
            _ if down.load(Ordering::SeqCst) => None,
            "sync/progress" => ok("silo/sync_progress_ok.json"),
            _ => None,
        }
    };
    let (old, _, _) = fake(reply.clone());
    block_on(old.start_playback("movie:heat-1995", None)).unwrap();
    block_on(old.report_progress(SESSION, &Sample { sequence: 1, position: 60.0, paused: false })).ok();
    let kept = old.take_pending();
    drop(old);

    down.store(false, Ordering::SeqCst);
    let (new, seen, _) = fake(reply);
    new.adopt_pending(kept);
    block_on(new.sync_progress()).unwrap();
    let seen = seen.lock().unwrap();
    let sync = seen.iter().find(|s| s.path == "sync/progress").expect("the kept position goes out");
    assert_eq!(serde_json::from_str::<Value>(&sync.body).unwrap()["items"][0]["position_ms"], 60000);
}

#[test]
fn a_failed_sign_out_does_not_show_the_offline_banner_or_the_sign_in_screen() {
    let (client, _, refused) = fake(|req| match req.path.as_str() {
        "auth/refresh" => problem(401, "refresh_session_revoked"),
        _ => problem(401, "token_refresh_required"),
    });
    assert!(block_on(client.sign_out()).is_err());
    let (client, _, offline) = fake(|_| None);
    assert!(block_on(client.sign_out()).is_err());
    assert_eq!(refused.get(), []);
    assert_eq!(offline.get(), []);
}
