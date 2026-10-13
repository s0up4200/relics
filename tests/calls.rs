//! The server calls of spec #40 (person, favorites, ratings, watched, similar, collections, shuffles,
//! requests and trickplay), against a fake HTTP client.

mod common;

use common::{Seen, fake, fixture, json, not_found, ok};
use futures::executor::block_on;
use relics::artwork::Rung;
use relics::requests;
use serde_json::{Value, json as value};

/// The method and the path of each request.
fn calls(seen: &common::Log) -> Vec<(String, String)> {
    seen.lock().unwrap().iter().map(|s| (s.method.clone(), s.path.clone())).collect()
}

fn body(seen: &Seen) -> Value {
    serde_json::from_str(&seen.body).unwrap()
}

fn no_content() -> common::Reply {
    json(204, String::new())
}

#[test]
fn person_reads_one_person() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "catalog/people/7" => ok("get_person_ok.json"),
        _ => not_found(),
    });
    let person = block_on(client.person("7")).unwrap();
    assert_eq!(person.name, "Al Pacino");
}

#[test]
fn favorite_rating_and_watched_set_and_clear_the_state_of_an_item() {
    let (client, seen) = fake(|_| no_content());
    block_on(client.set_favorite("movie:heat-1995", true)).unwrap();
    block_on(client.set_favorite("movie:heat-1995", false)).unwrap();
    block_on(client.set_rating("movie:heat-1995", Some(4))).unwrap();
    block_on(client.set_rating("movie:heat-1995", None)).unwrap();
    block_on(client.set_watched("season:severance-1", true)).unwrap();
    block_on(client.set_watched("season:severance-1", false)).unwrap();
    let expected = [
        ("PUT", "favorites/movie:heat-1995"),
        ("DELETE", "favorites/movie:heat-1995"),
        ("PUT", "ratings/movie:heat-1995"),
        ("DELETE", "ratings/movie:heat-1995"),
        ("POST", "watched/season:severance-1"),
        ("DELETE", "watched/season:severance-1"),
    ];
    assert_eq!(calls(&seen), expected.map(|(m, p)| (m.to_string(), p.to_string())));
    assert_eq!(body(&seen.lock().unwrap()[2]), value!({ "rating": 4 }));
}

#[test]
fn similar_lists_the_items_like_an_item() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "recommendations/similar/movie:heat-1995" => ok("list_similar_ok.json"),
        _ => not_found(),
    });
    let items = block_on(client.similar("movie:heat-1995")).unwrap();
    assert_eq!(items[0].title, "Heat");
}

#[test]
fn personal_collections_and_their_items_page_with_a_cursor() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "collections" => ok("list_collections_ok.json"),
        "collections/c1/items" | "collections/c1/items?cursor=a%2Fb" => ok("get_collection_items_ok.json"),
        _ => not_found(),
    });
    let collections = block_on(client.collections()).unwrap();
    assert_eq!(collections[0].name, "Rainy days");
    let page = block_on(client.collection_items("c1", None)).unwrap();
    assert_eq!(page.items[0].media_item_id.as_str(), "movie:heat-1995");
    block_on(client.collection_items("c1", Some("a/b"))).unwrap();
    assert_eq!(calls(&seen)[2].1, "collections/c1/items?cursor=a%2Fb");
}

#[test]
fn library_and_server_collections_list_their_collections_and_items() {
    let (client, _) = fake(|req| {
        match req.path.as_str() {
        "library/1/collections" => ok("get_library_collections_ok.json"),
        "library/1/collections/c1/items?cursor=next" => ok("list_similar_ok.json"),
        "collections/server" => json(
            200,
            value!({ "libraries": [{ "library_id": "1", "library_name": "Movies", "total_count": 0, "collections": [] }] })
                .to_string(),
        ),
        _ => not_found(),
    }
    });
    let tab = block_on(client.library_collections("1")).unwrap();
    assert_eq!(tab.collections[0].title, "Oscar Winners");
    let items = block_on(client.library_collection_items("1", "c1", Some("next"))).unwrap();
    assert_eq!(items.items[0].title, "Heat");
    let server = block_on(client.server_collections()).unwrap();
    assert_eq!(server[0].library_name, "Movies");
}

#[test]
fn a_shuffle_starts_advances_and_skips() {
    let item: Value = serde_json::from_str::<Value>(&fixture("list_similar_ok.json")).unwrap()["items"][0].clone();
    let shuffle = value!({
        "id": "s1",
        "scope": { "kind": "library", "id": "1", "title": "Movies" },
        "current": item,
        "next": item,
        "created_at": "2026-01-02T03:04:05.000Z",
        "updated_at": "2026-01-02T03:04:05.000Z",
    })
    .to_string();
    let (client, seen) = fake(move |req| match req.path.as_str() {
        "shuffles/capabilities" => json(
            200,
            value!({ "allowed": true, "revision": "r", "scope_kinds": ["library"], "state": "available" }).to_string(),
        ),
        "shuffles?image_size=medium"
        | "shuffles/s1/advance?image_size=medium"
        | "shuffles/s1/skip?image_size=medium" => json(200, shuffle.clone()),
        _ => not_found(),
    });
    assert!(block_on(client.shuffle_capability()).unwrap().allowed);
    let started = block_on(client.start_shuffle("library", "1", Rung::Medium)).unwrap();
    assert_eq!(started.current.title, "Heat");
    block_on(client.advance_shuffle("s1", "movie:heat-1995", Rung::Medium)).unwrap();
    block_on(client.skip_shuffle_item("s1", "movie:heat-1995", Rung::Medium)).unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(body(&seen[1]), value!({ "scope": { "kind": "library", "id": "1" } }));
    assert_eq!(body(&seen[2]), value!({ "from_content_id": "movie:heat-1995" }));
    assert_eq!(body(&seen[3]), value!({ "next_content_id": "movie:heat-1995" }));
}

#[test]
fn requests_search_create_list_and_cancel() {
    let (client, seen) = fake(|req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "requests/status") => ok("request_status_ok.json"),
        ("GET", "requests/search?q=the+heat&page=1") => {
            json(200, value!({ "page": 1, "total_pages": 1, "total_results": 0, "results": [] }).to_string())
        }
        ("POST", "requests") => json(201, fixture("create_request_ok.json")),
        ("GET", "requests/mine") => ok("list_my_requests_ok.json"),
        ("POST", "requests/r-1/cancel") => ok("cancel_request_ok.json"),
        _ => not_found(),
    });
    assert!(block_on(client.request_status()).unwrap().requests_enabled);
    assert_eq!(block_on(client.search_requests("the heat", 1)).unwrap().total_pages, 1);
    let create =
        serde_json::from_value(value!({ "media_type": "movie", "tmdb_id": 12345, "title": "Example" })).unwrap();
    assert_eq!(block_on(client.create_request(&create)).unwrap().id.as_str(), "r-new");
    assert_eq!(block_on(client.my_requests(None)).unwrap().items[0].id.as_str(), "r-2");
    assert_eq!(block_on(client.cancel_request("r-1")).unwrap().state, "approved");
    let seen = seen.lock().unwrap();
    assert_eq!(body(&seen[2]), value!({ "media_type": "movie", "tmdb_id": 12345, "title": "Example" }));
}

#[test]
fn request_to_add_lists_the_missing_titles_and_requests_one_with_its_details() {
    let page = value!({ "page": 1, "total_pages": 1, "total_results": 2, "results": [
        { "availability": "available", "library_content_id": "movie:heat-1995", "media_type": "movie",
          "tmdb_id": 949, "title": "Heat", "year": 1995, "in_watchlist": false,
          "request": { "following": false, "requestable": false, "requested_by_viewer": false } },
        { "availability": "missing", "media_type": "series", "tmdb_id": 1399, "title": "The Heat",
          "year": 2013, "overview": "Two cops.", "poster_path": "/p.jpg", "in_watchlist": false,
          "request": { "following": false, "requestable": true, "requested_by_viewer": false } },
    ]});
    let (client, seen) = fake(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "requests/search?q=heat&page=1") => json(200, page.to_string()),
        ("POST", "requests") => json(201, fixture("create_request_ok.json")),
        _ => not_found(),
    });
    let results = requests::to_add(block_on(client.search_requests("heat", 1)).unwrap());
    assert_eq!(results.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["The Heat"]);
    block_on(client.create_request(&requests::create(&results[0]).unwrap())).unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        body(&seen[1]),
        value!({ "media_type": "series", "tmdb_id": 1399, "title": "The Heat", "year": 2013,
                 "overview": "Two cops.", "poster_path": "/p.jpg" })
    );
}

#[test]
fn trickplay_gives_absolute_sheet_urls_below_the_server_path() {
    let (http, _) = common::http(|req| match req.path.as_str() {
        "watch/movie:heat-1995/trickplay?file_id=42" => ok("get_watch_trickplay_ok.json"),
        _ => not_found(),
    });
    let client = common::client(http, "https://silo.test/media/");
    let trickplay = block_on(client.trickplay("movie:heat-1995", "42")).unwrap();
    assert_eq!(trickplay.tile_columns, 10);
    assert!(trickplay.sheets[0].url.starts_with("https://silo.test/media/api/v2/artwork/trickplay/42/"));
}

#[test]
fn trickplay_fails_when_the_server_has_no_thumbnails_for_the_file() {
    let (http, _) = common::http(|_| not_found());
    let client = common::client(http, "https://silo.test/media/");
    assert!(block_on(client.trickplay("movie:heat-1995", "42")).is_err());
}
