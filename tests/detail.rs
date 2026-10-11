//! The detail page: the Item, the seasons and episodes of a series, and the Watchlist, against a fake HTTP
//! client.

use std::sync::{Arc, Mutex};

use futures::AsyncReadExt as _;
use futures::executor::block_on;
use http_client::{AsyncBody, FakeHttpClient, HttpClient, Response, Url};
use silo_omarchy::api::{Client, Device, Server, Tokens};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/silo/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
}

type Reply = (u16, String);

/// A fake Silo server at `https://silo.test/`. `reply` gets each request and returns the status and the
/// JSON body.
fn fake(reply: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (Client, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    let http: Arc<dyn HttpClient> = FakeHttpClient::create(move |req| {
        let (log, reply) = (log.clone(), reply.clone());
        async move {
            let method = req.method().to_string();
            let path = req.uri().to_string().strip_prefix("https://silo.test/").unwrap().to_string();
            let mut body = String::new();
            req.into_body().read_to_string(&mut body).await?;
            let request = Seen { method, path };
            log.lock().unwrap().push(request.clone());
            let (status, body) = reply(&request);
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

fn not_found() -> Reply {
    (404, String::new())
}

#[test]
fn the_detail_page_reads_the_item_with_large_artwork() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "api/v2/catalog/items/movie:heat-1995?image_size=large" => (200, fixture("get_catalog_item_ok.json")),
        _ => not_found(),
    });
    let item = block_on(client.item("movie:heat-1995", "large")).unwrap();
    assert_eq!(item.title, "Heat");
    assert_eq!(item.cast[0].name, "Al Pacino");
}

#[test]
fn a_series_lists_its_seasons_and_the_episodes_of_the_chosen_season() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "api/v2/catalog/series/series:severance/seasons?include_artwork=false" => {
            (200, fixture("list_series_seasons_without_artwork.json"))
        }
        "api/v2/catalog/series/series:severance/seasons/1/episodes" => (200, fixture("list_season_episodes_ok.json")),
        _ => not_found(),
    });
    let seasons = block_on(client.seasons("series:severance")).unwrap();
    assert_eq!(seasons.len(), 1);
    assert_eq!((seasons[0].season_number, seasons[0].title.as_str()), (1, "Season 1"));
    let episodes = block_on(client.season_episodes("series:severance", 1)).unwrap();
    assert_eq!(episodes[0].title, "Good News About Hell");
}

#[test]
fn watchlist_adds_and_removes_the_item() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "api/v2/watchlist/movie:heat-1995" => (204, String::new()),
        _ => not_found(),
    });
    block_on(client.set_watchlist("movie:heat-1995", true)).unwrap();
    block_on(client.set_watchlist("movie:heat-1995", false)).unwrap();
    let methods: Vec<_> = seen.lock().unwrap().iter().map(|s| s.method.clone()).collect();
    assert_eq!(methods, ["PUT", "DELETE"]);
}
