//! Search: the people that match the text, the title query and the Person grid query, against a fake HTTP
//! client.

mod common;

use common::{fake, not_found, ok};
use futures::executor::block_on;
use serde_json::json;
use silo_omarchy::artwork::Rung;
use silo_omarchy::browse::{Facet, SortField};
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
fn the_titles_come_from_a_text_query_over_video_newest_added_first_as_in_the_web() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    let page = block_on(client.query_catalog(&search::query("heat", &search::browse()), Rung::Small)).unwrap();
    assert_eq!(page.items.len(), 3);
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(
        body,
        json!({"source": "query", "q": "heat", "type": "video", "sort": "added_at", "order": "desc", "limit": 60})
    );
}

#[test]
fn the_sort_and_the_filters_of_search_go_into_the_text_query() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    let mut browse = search::browse();
    browse.sort = SortField::TmdbRating;
    browse.descending = false;
    browse.filters.genres = vec!["Crime".into()];
    browse.filters.facets = vec![(Facet::Studio, "A24".into())];
    browse.filters.dolby_vision = true;
    // Search has no A to Z jump and no Episodes mode: they stay out of the query.
    browse.letter = Some('H');
    browse.episodes = true;
    block_on(client.query_catalog(&search::query("heat", &browse), Rung::Small)).unwrap();
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(
        body,
        json!({
            "source": "query",
            "q": "heat",
            "type": "video",
            "sort": "rating_tmdb",
            "order": "asc",
            "limit": 60,
            "match": "all",
            "groups": [{"match": "all", "rules": [
                {"field": "genre", "op": "is", "value": "Crime"},
                {"field": "studio", "op": "is", "value": "A24"},
                {"field": "dolby_vision", "op": "is", "value": true},
            ]}],
        })
    );
}

#[test]
fn the_person_grid_lists_the_titles_of_the_person_newest_first() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    block_on(client.query_catalog(&search::person_query("7", None), Rung::Small)).unwrap();
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(body, json!({"source": "person", "person_id": "7", "sort": "year", "order": "desc", "limit": 60}));
}

#[test]
fn the_movies_and_series_tabs_of_the_filmography_narrow_the_titles_to_one_type() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    block_on(client.query_catalog(&search::person_query("7", Some("movie")), Rung::Small)).unwrap();
    block_on(client.query_catalog(&search::person_query("7", Some("series")), Rung::Small)).unwrap();
    let seen = seen.lock().unwrap();
    let types: Vec<serde_json::Value> =
        seen.iter().map(|req| serde_json::from_str::<serde_json::Value>(&req.body).unwrap()["type"].clone()).collect();
    assert_eq!(types, [json!("movie"), json!("series")]);
}
