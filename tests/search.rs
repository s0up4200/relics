//! Search: the people that match the text, the title query and the Person grid query, against a fake HTTP
//! client.

mod common;

use common::{fake, not_found, ok};
use futures::executor::block_on;
use serde_json::json;
use silo_omarchy::artwork::Rung;
use silo_omarchy::search;

#[test]
fn the_people_row_asks_for_video_people_that_match_the_text() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/people?q=al+pa%26cino&limit=20&media_scope=video" => ok("list_people_ok.json"),
        _ => not_found(),
    });
    let people = block_on(client.people("al pa&cino")).unwrap();
    let names: Vec<_> = people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Al Pacino"]);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn the_titles_come_from_a_text_query_over_video() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    let page = block_on(client.query_catalog(&search::query("heat"), Rung::Small)).unwrap();
    assert_eq!(page.items.len(), 3);
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(body, json!({"source": "query", "q": "heat", "type": "video", "limit": 60}));
}

#[test]
fn the_person_grid_lists_the_titles_of_the_person_newest_first() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    block_on(client.query_catalog(&search::person_query("7"), Rung::Small)).unwrap();
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(body, json!({"source": "person", "person_id": "7", "sort": "year", "order": "desc", "limit": 60}));
}
