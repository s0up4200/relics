//! The Library page: the video Libraries, the Recommended rows, the grid query and its paging, against a
//! fake HTTP client.

mod common;

use common::{Seen, fake, fixture, json, not_found, ok};
use futures::executor::block_on;
use serde_json::json;
use silo_omarchy::artwork::Rung;
use silo_omarchy::browse::{Browse, Choice, Paging, SortField, Watch};

#[test]
fn the_sidebar_lists_only_the_video_libraries_in_the_server_order() {
    let libraries = json!({"items": [
        {"id": "3", "name": "Books", "type": "audiobooks", "sort_order": 0},
        {"id": "2", "name": "Shows", "type": "series", "sort_order": 3},
        {"id": "1", "name": "Movies", "type": "movies", "sort_order": 1},
        {"id": "4", "name": "Comics", "type": "manga", "sort_order": 2},
    ]});
    let (client, seen) = fake(move |req| match req.path.as_str() {
        "user/libraries" => json(200, libraries.to_string()),
        _ => not_found(),
    });
    let libraries = block_on(client.libraries()).unwrap();
    let names: Vec<_> = libraries.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Movies", "Shows"]);
    assert_eq!(seen.lock().unwrap()[0].method, "GET");
}

#[test]
fn the_fixture_libraries_decode() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "user/libraries" => ok("user_libraries.json"),
        _ => not_found(),
    });
    assert_eq!(block_on(client.libraries()).unwrap().len(), 1);
}

#[test]
fn the_recommended_tab_reads_the_library_layout_and_the_items_of_each_section() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "library/1/layout" => ok("get_library_layout_ok.json"),
        "library/1/sections/continue_watching/items?image_size=small" => ok("get_library_section_items_ok.json"),
        "library/1/sections/recently_added/items?image_size=small" => {
            let mut section: serde_json::Value =
                serde_json::from_str(&fixture("get_library_section_items_ok.json")).unwrap();
            section["id"] = "recently_added".into();
            section["total_count"] = 40.into();
            json(200, section.to_string())
        }
        _ => not_found(),
    });
    let rows = block_on(client.library_rows("1", Rung::Small)).unwrap();
    let titles: Vec<_> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, ["Continue Watching", "Recently Added"]);
    assert_eq!(rows[0].section_type, "continue_watching");
    assert_eq!(rows[0].items.len(), 1);
    assert!(!rows[0].view_all);
    assert!(rows[1].view_all, "the server has more items than the row holds");
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn the_genre_filter_lists_the_genres_of_the_library() {
    let (client, _) = fake(|req| match req.path.as_str() {
        "catalog/filters?library_id=1&skip_technical=true" => ok("get_catalog_filters_ok.json"),
        _ => not_found(),
    });
    assert_eq!(block_on(client.genres("1")).unwrap(), ["Crime"]);
}

fn body(seen: &Seen) -> serde_json::Value {
    serde_json::from_str(&seen.body).unwrap()
}

#[test]
fn the_default_grid_sorts_by_title_ascending_with_no_rules() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    let query = Browse::default().query("1", false);
    let page = block_on(client.query_catalog(&query, Rung::Small)).unwrap();
    assert_eq!(page.items.len(), 3);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].method, "POST");
    assert_eq!(
        body(&seen[0]),
        json!({"source": "query", "library_id": "1", "sort": "title", "order": "asc", "limit": 60})
    );
}

#[test]
fn the_filters_become_one_rule_group_that_must_match_all_rules() {
    let mut browse = Browse { sort: SortField::ImdbRating, descending: true, ..Browse::default() };
    browse.filters.genres = vec!["Crime".into(), "Drama".into()];
    browse.filters.decade = Some(1990);
    browse.filters.watch = Some(Watch::Unwatched);
    browse.filters.four_k = true;
    browse.filters.hdr = true;
    browse.letter = Some('H');
    let query = serde_json::to_value(browse.query("1", false)).unwrap();
    assert_eq!(
        query,
        json!({
            "source": "query",
            "library_id": "1",
            "sort": "rating_imdb",
            "order": "desc",
            "limit": 60,
            "name_prefix": "H",
            "match": "all",
            "groups": [{"match": "all", "rules": [
                {"field": "genre", "op": "is", "value": "Crime"},
                {"field": "genre", "op": "is", "value": "Drama"},
                {"field": "year", "op": "gte", "value": 1990},
                {"field": "year", "op": "lte", "value": 1999},
                {"field": "watched", "op": "is", "value": false},
                {"field": "in_progress", "op": "is", "value": false},
                {"field": "resolution", "op": "is", "value": "2160p"},
                {"field": "hdr", "op": "is", "value": true},
            ]}],
        })
    );
}

#[test]
fn each_sort_field_has_its_server_name() {
    let fields: Vec<_> = SortField::ALL.iter().map(|f| f.wire()).collect();
    assert_eq!(fields, ["title", "added_at", "release_date", "rating_imdb", "runtime", "date_viewed"]);
}

#[test]
fn a_shows_library_browses_series_or_episodes() {
    let mut browse = Browse::default();
    assert_eq!(browse.query("2", true).type_.as_deref(), Some("series"));
    browse.episodes = true;
    assert_eq!(browse.query("2", true).type_.as_deref(), Some("episode"));
    assert_eq!(browse.query("1", false).type_, None, "a Movies library sends no type");
}

#[test]
fn each_active_filter_is_a_chip_that_removes_it() {
    let mut browse = Browse::default();
    browse.filters.genres = vec!["Crime".into()];
    browse.filters.watch = Some(Watch::InProgress);
    browse.filters.hdr = true;
    browse.letter = Some('B');
    let chips = browse.chips();
    let labels: Vec<_> = chips.iter().map(Choice::label).collect();
    assert_eq!(labels, ["Crime", "In progress", "HDR", "B"]);

    browse.remove(&chips[0]);
    assert!(browse.filters.genres.is_empty());
    browse.remove(&chips[3]);
    assert_eq!(browse.letter, None);
    assert_eq!(browse.chips().len(), 2);

    browse.sort = SortField::Duration;
    browse.clear();
    assert!(browse.chips().is_empty());
    assert_eq!(browse.sort, SortField::Duration, "Clear all keeps the sort");
}

#[test]
fn in_progress_and_watched_each_send_one_rule() {
    let mut browse = Browse::default();
    browse.filters.watch = Some(Watch::InProgress);
    let rules = serde_json::to_value(&browse.query("1", false).groups[0].rules).unwrap();
    assert_eq!(rules, json!([{"field": "in_progress", "op": "is", "value": true}]));
    browse.filters.watch = Some(Watch::Watched);
    let rules = serde_json::to_value(&browse.query("1", false).groups[0].rules).unwrap();
    assert_eq!(rules, json!([{"field": "watched", "op": "is", "value": true}]));
}

#[test]
fn the_filter_menu_takes_several_genres_but_one_decade_and_one_watch_status() {
    let mut browse = Browse::default();
    let crime = Choice::Genre("Crime".into());
    browse.choose(&crime);
    browse.choose(&Choice::Genre("Drama".into()));
    assert!(browse.is_chosen(&crime));
    assert_eq!(browse.filters.genres, ["Crime", "Drama"]);
    browse.choose(&crime);
    assert_eq!(browse.filters.genres, ["Drama"], "a second Enter removes the genre");

    browse.choose(&Choice::Decade(1980));
    browse.choose(&Choice::Decade(1990));
    assert_eq!(browse.filters.decade, Some(1990));
    browse.choose(&Choice::Decade(1990));
    assert_eq!(browse.filters.decade, None);

    browse.choose(&Choice::Watch(Watch::Watched));
    browse.choose(&Choice::Watch(Watch::Unwatched));
    assert_eq!(browse.filters.watch, Some(Watch::Unwatched));

    browse.choose(&Choice::FourK);
    browse.choose(&Choice::Hdr);
    assert!(browse.filters.four_k && browse.filters.hdr);
}

#[test]
fn the_sort_menu_sets_the_field_and_the_order() {
    let mut browse = Browse::default();
    browse.choose(&Choice::Sort(SortField::DateAdded));
    browse.choose(&Choice::Descending(true));
    assert_eq!((browse.sort, browse.descending), (SortField::DateAdded, true));
    assert!(browse.is_chosen(&Choice::Sort(SortField::DateAdded)));
    assert!(!browse.is_chosen(&Choice::Descending(false)));
    let menu = Choice::sort_menu();
    assert_eq!(menu.len(), 8, "six fields, ascending and descending");
}

#[test]
fn the_letter_menu_has_all_and_a_to_z() {
    let menu = Choice::letter_menu();
    assert_eq!(menu.len(), 27);
    assert_eq!(menu[0], Choice::Letter(None));
    assert_eq!(menu[26], Choice::Letter(Some('Z')));
    let mut browse = Browse::default();
    browse.choose(&Choice::Letter(Some('M')));
    assert_eq!(browse.letter, Some('M'));
    browse.choose(&Choice::Letter(None));
    assert_eq!(browse.letter, None);
}

#[test]
fn the_filter_menu_lists_watch_status_quality_decades_and_genres() {
    let menu = Choice::filter_menu(&["Crime".into()], 2026);
    let labels: Vec<_> = menu.iter().map(Choice::label).collect();
    assert_eq!(&labels[..5], ["Unwatched", "In progress", "Watched", "4K", "HDR"]);
    assert_eq!(labels[5], "2020s");
    assert_eq!(labels[17], "1900s");
    assert_eq!(labels.last().unwrap(), "Crime");
}

fn page(titles: &[&str], next: Option<&str>) -> silo_omarchy::api::wire::CatalogBrowseCollection {
    let items: Vec<_> = titles
        .iter()
        .map(|t| json!({"content_id": t, "type": "movie", "title": t, "genres": [], "keywords": [], "status": "matched"}))
        .collect();
    let page = match next {
        Some(next) => json!({"has_more": true, "next_cursor": next}),
        None => json!({"has_more": false}),
    };
    serde_json::from_value(json!({"items": items, "page": page, "total": 9, "total_exact": true, "window_cursor": "w"}))
        .unwrap()
}

#[test]
fn the_grid_pages_with_the_cursor_until_the_last_page() {
    let query = Browse::default().query("1", false);
    let mut paging = Paging::default();
    assert_eq!(paging.next_query(&query).unwrap().cursor, None, "the first page has no cursor");
    paging.add(page(&["a", "b"], Some("c2")));
    assert_eq!(paging.next_query(&query).unwrap().cursor.as_deref(), Some("c2"));
    paging.add(page(&["c"], None));
    assert!(paging.next_query(&query).is_none(), "the last page ends the paging");
    let titles: Vec<_> = paging.items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["a", "b", "c"]);
    assert_eq!(paging.total, Some(9));
}
