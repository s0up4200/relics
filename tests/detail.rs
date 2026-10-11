//! The detail page: the Item, the seasons and episodes of a series, and the Watchlist, against a fake HTTP
//! client.

mod common;

use common::{fake, json, not_found, ok};
use futures::executor::block_on;

#[test]
fn the_detail_page_reads_the_item_with_large_artwork() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "catalog/items/movie:heat-1995?image_size=large" => ok("get_catalog_item_ok.json"),
        _ => not_found(),
    });
    let item = block_on(client.item("movie:heat-1995", "large")).unwrap();
    assert_eq!(item.title, "Heat");
    assert_eq!(item.cast[0].name, "Al Pacino");
}

#[test]
fn a_series_lists_its_seasons_and_the_episodes_of_the_chosen_season() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "catalog/series/series:severance/seasons?include_artwork=false" => {
            ok("list_series_seasons_without_artwork.json")
        }
        "catalog/series/series:severance/seasons/1/episodes" => ok("list_season_episodes_ok.json"),
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
        "watchlist/movie:heat-1995" => json(204, String::new()),
        _ => not_found(),
    });
    block_on(client.set_watchlist("movie:heat-1995", true)).unwrap();
    block_on(client.set_watchlist("movie:heat-1995", false)).unwrap();
    let methods: Vec<_> = seen.lock().unwrap().iter().map(|s| s.method.clone()).collect();
    assert_eq!(methods, ["PUT", "DELETE"]);
}
