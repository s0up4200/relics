//! Search: the people that match the text, the title query and the Person grid query, against a fake HTTP
//! client.

use std::sync::{Arc, Mutex};

use futures::AsyncReadExt as _;
use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use serde_json::json;
use silo_omarchy::api::{Client, Device, Server, Tokens};
use silo_omarchy::search;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/silo/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A fake Silo server at `https://silo.test/` that keeps the body of each request. `reply` gets the path of
/// each request and returns the JSON body, or `None` for a 404.
fn fake(reply: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> (Client, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    let http: Arc<dyn HttpClient> = FakeHttpClient::create(move |req| {
        let (log, reply) = (log.clone(), reply.clone());
        async move {
            let path = req.uri().to_string().strip_prefix("https://silo.test/").unwrap().to_string();
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            let answer = reply(&path);
            log.lock().unwrap().push(body);
            let (status, body) = answer.map_or((404, String::new()), |body| (200, body));
            Ok(Response::builder()
                .status(status)
                .header("Content-Type", "application/json")
                .body(AsyncBody::from(body))
                .unwrap())
        }
    });
    let server =
        Server { id: "server-1".into(), name: "silo.test".into(), url: Url::parse("https://silo.test/").unwrap() };
    let device = Device { id: "device-1".into(), name: "omarchy-box".into() };
    let client = Client::new(http, server, device, Tokens { access_token: "acc".into(), refresh_token: "ref".into() });
    client.set_profile(Some("p-owner".into()));
    (client, seen)
}

#[test]
fn the_people_row_asks_for_video_people_that_match_the_text() {
    let (client, seen) = fake(|path| {
        (path == "api/v2/catalog/people?q=al+pa%26cino&limit=20&media_scope=video")
            .then(|| fixture("list_people_ok.json"))
    });
    let people = block_on(client.people("al pa&cino")).unwrap();
    let names: Vec<_> = people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Al Pacino"]);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn the_titles_come_from_a_text_query_over_video() {
    let (client, seen) =
        fake(|path| (path == "api/v2/catalog/query?image_size=small").then(|| fixture("query_catalog_items_ok.json")));
    let page = block_on(client.query_catalog(&search::query("heat"), "small")).unwrap();
    assert_eq!(page.items.len(), 3);
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(body, json!({"source": "query", "q": "heat", "type": "video", "limit": 60}));
}

#[test]
fn the_person_grid_lists_the_titles_of_the_person_newest_first() {
    let (client, seen) =
        fake(|path| (path == "api/v2/catalog/query?image_size=small").then(|| fixture("query_catalog_items_ok.json")));
    block_on(client.query_catalog(&search::person_query("7"), "small")).unwrap();
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(body, json!({"source": "person", "person_id": "7", "sort": "year", "order": "desc", "limit": 60}));
}
