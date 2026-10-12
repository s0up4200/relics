//! The Library page: the video Libraries, the Recommended rows, the grid query and its paging, against a
//! fake HTTP client.

mod common;

use common::{Seen, fake, fixture, json, not_found, ok};
use futures::executor::block_on;
use serde_json::json;
use silo_omarchy::api::wire;
use silo_omarchy::artwork::Rung;
use silo_omarchy::browse::{self, Browse, Choice, Facet, Paging, SortField, Watch};
use silo_omarchy::filter_panel::{Entry, Facets, MenuKind, Outcome, Panel, Text};
use silo_omarchy::keys::Key;

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
        "library/1/sections/continue_watching/items?image_size=medium" => ok("get_library_section_items_ok.json"),
        "library/1/sections/recently_added/items?image_size=small" => {
            let mut section: serde_json::Value =
                serde_json::from_str(&fixture("get_library_section_items_ok.json")).unwrap();
            section["id"] = "recently_added".into();
            section["total_count"] = 40.into();
            json(200, section.to_string())
        }
        _ => not_found(),
    });
    // Each section loads with its own rung.
    let size = |entry: &wire::SectionLayoutEntry| {
        if entry.section_type == "continue_watching" { Rung::Medium } else { Rung::Small }
    };
    let rows = block_on(client.library_rows("1", size)).unwrap();
    let titles: Vec<_> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, ["Continue Watching", "Recently Added"]);
    assert_eq!(rows[0].section_type, "continue_watching");
    assert_eq!(rows[0].items.len(), 1);
    assert!(!rows[0].view_all);
    assert!(rows[1].view_all, "the server has more items than the row holds");
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn the_filter_panel_reads_the_filter_values_of_the_library_or_of_the_whole_catalog() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/filters?library_id=1&skip_technical=true" | "catalog/filters?skip_technical=true" => {
            ok("get_catalog_filters_ok.json")
        }
        _ => not_found(),
    });
    assert_eq!(block_on(client.filters(Some("1"))).unwrap().genres, ["Crime"]);
    assert_eq!(block_on(client.filters(None)).unwrap().genres, ["Crime"]);
    assert_eq!(seen.lock().unwrap().len(), 2);
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
    assert_eq!(
        fields,
        [
            "title",
            "added_at",
            "release_date",
            "year",
            "content_rating",
            "runtime",
            "rating_imdb",
            "rating_tmdb",
            "resolution",
            "bitrate",
            "progress",
            "date_viewed",
            "plays",
        ]
    );
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
fn the_new_sort_fields_and_filters_go_to_the_server_in_the_catalog_query() {
    let (client, seen) = fake(|req| match req.path.as_str() {
        "catalog/query?image_size=small" => ok("query_catalog_items_ok.json"),
        _ => not_found(),
    });
    let mut browse = Browse { sort: SortField::Plays, descending: true, ..Browse::default() };
    for choice in [
        Choice::YearFrom(1990),
        Choice::YearTo(2005),
        Choice::MinRating(7),
        Choice::Facet(Facet::Studio, "A24".into()),
        Choice::Facet(Facet::Language, "en".into()),
        Choice::Facet(Facet::Actor, "Tom Hanks".into()),
        Choice::Facet(Facet::Director, "Ridley Scott".into()),
        Choice::Facet(Facet::Writer, "Aaron Sorkin".into()),
        Choice::Facet(Facet::Producer, "Kevin Feige".into()),
        Choice::Facet(Facet::Network, "HBO".into()),
        Choice::Facet(Facet::Country, "US".into()),
        Choice::DolbyVision,
        Choice::AddedIn("30d"),
        Choice::ReleasedIn("1y"),
    ] {
        browse.choose(&choice);
    }
    block_on(client.query_catalog(&browse.query("1", false), Rung::Small)).unwrap();
    let rule = |field: &str, op: &str, value: serde_json::Value| json!({"field": field, "op": op, "value": value});
    assert_eq!(
        body(&seen.lock().unwrap()[0]),
        json!({
            "source": "query",
            "library_id": "1",
            "sort": "plays",
            "order": "desc",
            "limit": 60,
            "match": "all",
            "groups": [{"match": "all", "rules": [
                rule("year", "gte", 1990.into()),
                rule("year", "lte", 2005.into()),
                rule("rating_imdb", "gte", 7.into()),
                rule("original_language", "is", "en".into()),
                rule("actor", "is", "Tom Hanks".into()),
                rule("director", "is", "Ridley Scott".into()),
                rule("writer", "is", "Aaron Sorkin".into()),
                rule("producer", "is", "Kevin Feige".into()),
                rule("studio", "is", "A24".into()),
                rule("network", "is", "HBO".into()),
                rule("country", "is", "US".into()),
                rule("added_at", "in_last", "30d".into()),
                rule("release_date", "in_last", "1y".into()),
                rule("dolby_vision", "is", true.into()),
            ]}],
        })
    );
}

#[test]
fn a_facet_takes_one_value_and_each_new_filter_is_a_chip() {
    let mut browse = Browse::default();
    browse.choose(&Choice::Facet(Facet::Studio, "A24".into()));
    browse.choose(&Choice::Facet(Facet::Studio, "Neon".into()));
    browse.choose(&Choice::YearFrom(1990));
    browse.choose(&Choice::MinRating(8));
    browse.choose(&Choice::AddedIn("7d"));
    browse.choose(&Choice::DolbyVision);
    let labels: Vec<_> = browse.chips().iter().map(Choice::chip_label).collect();
    assert_eq!(labels, ["From 1990", "IMDb 8+", "Studio: Neon", "Added in the last 7 days", "Dolby Vision"]);
    for chip in browse.chips() {
        browse.remove(&chip);
    }
    assert_eq!(browse.filters, browse::Filters::default());
}

fn facets() -> Facets {
    Facets { studios: vec!["A24".into(), "Neon".into(), "Annapurna".into()], ..Facets::default() }
}

fn keys(panel: &mut Panel, browse: &mut Browse, keys: &[(&str, Option<&str>)]) -> Outcome {
    let mut last = Outcome::Unused;
    for (key, typed) in keys {
        last = panel.key(Key::parse(key), *typed, browse);
    }
    last
}

#[test]
fn down_and_up_in_the_filter_panel_move_from_section_to_section() {
    let (mut panel, mut browse) = (Panel::new(MenuKind::Filter, facets(), 2026), Browse::default());
    let titles: Vec<_> = panel.sections().iter().map(|s| s.title).collect();
    assert_eq!(
        titles,
        [
            "Watch status",
            "Video quality",
            "Minimum IMDb rating",
            "Year",
            "Added in the last",
            "Released in the last",
            "Studio",
            "Actor",
            "Director",
            "Writer",
            "Producer",
        ],
        "a facet with no values on the server has no section"
    );
    keys(&mut panel, &mut browse, &[("down", None), ("down", None)]);
    assert_eq!(panel.focused(), Entry::Pick(Choice::MinRating(5)));
    keys(&mut panel, &mut browse, &[("right", None), ("up", None)]);
    assert_eq!(panel.focused(), Entry::Pick(Choice::FourK));
    assert_eq!(keys(&mut panel, &mut browse, &[("enter", None)]), Outcome::Changed);
    assert!(browse.filters.four_k);
}

#[test]
fn a_year_field_takes_digits_and_enter_sets_the_year() {
    let (mut panel, mut browse) = (Panel::new(MenuKind::Filter, facets(), 2026), Browse::default());
    keys(&mut panel, &mut browse, &[("down", None), ("down", None), ("down", None)]);
    assert_eq!(panel.focused(), Entry::Field(Text::YearFrom));
    let typed = [("1", Some("1")), ("9", Some("9")), ("x", Some("x")), ("9", Some("9")), ("5", Some("5"))];
    keys(&mut panel, &mut browse, &typed);
    assert_eq!(panel.text(Text::YearFrom), "1995", "a letter is not a digit");
    assert_eq!(keys(&mut panel, &mut browse, &[("enter", None)]), Outcome::Changed);
    assert_eq!(browse.filters.year_from, Some(1995));
    assert_eq!(panel.text(Text::YearFrom), "", "Enter empties the field");
}

#[test]
fn typing_in_a_facet_field_narrows_its_values_and_enter_picks_the_first() {
    let (mut panel, mut browse) = (Panel::new(MenuKind::Filter, facets(), 2026), Browse::default());
    keys(&mut panel, &mut browse, &[("down", None); 6]);
    assert_eq!(panel.focused(), Entry::Field(Text::Facet(Facet::Studio)));
    keys(&mut panel, &mut browse, &[("a", Some("a")), ("n", Some("n"))]);
    let studio = &panel.sections()[6];
    let values = studio.entries[1..].to_vec();
    assert_eq!(
        values,
        [Entry::Pick(Choice::Facet(Facet::Studio, "Annapurna".into()))],
        "the match ignores case and looks anywhere in the name"
    );
    assert_eq!(keys(&mut panel, &mut browse, &[("enter", None)]), Outcome::Changed);
    assert_eq!(browse.filters.facets, [(Facet::Studio, "Annapurna".to_string())]);
}

#[test]
fn enter_in_a_person_field_filters_by_the_typed_name_and_esc_empties_then_closes() {
    let (mut panel, mut browse) = (Panel::new(MenuKind::Filter, facets(), 2026), Browse::default());
    keys(&mut panel, &mut browse, &[("down", None); 7]);
    assert_eq!(panel.focused(), Entry::Field(Text::Facet(Facet::Actor)));
    let name: Vec<_> = ["T", "o", "m", "space", "H", "a", "n", "k", "s"]
        .iter()
        .map(|k| (*k, Some(if *k == "space" { " " } else { *k })))
        .collect();
    keys(&mut panel, &mut browse, &name);
    keys(&mut panel, &mut browse, &[("enter", None)]);
    assert_eq!(browse.filters.facets, [(Facet::Actor, "Tom Hanks".to_string())]);
    keys(&mut panel, &mut browse, &[("x", Some("x"))]);
    assert_eq!(keys(&mut panel, &mut browse, &[("escape", None)]), Outcome::Moved);
    assert_eq!(panel.text(Text::Facet(Facet::Actor)), "");
    assert_eq!(keys(&mut panel, &mut browse, &[("escape", None)]), Outcome::Closed);
}

#[test]
fn the_sort_menu_has_a_field_section_and_an_order_section() {
    let (mut panel, mut browse) = (Panel::new(MenuKind::Sort, facets(), 2026), Browse::default());
    let sections = panel.sections();
    assert_eq!(sections.iter().map(|s| s.entries.len()).collect::<Vec<_>>(), [13, 2]);
    assert_eq!(keys(&mut panel, &mut browse, &[("down", None), ("right", None), ("enter", None)]), Outcome::Closed);
    assert!(browse.descending, "Enter in the sort menu applies and closes it");
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

#[test]
fn view_all_watchlist_and_history_list_their_catalog_source_in_the_grid() {
    let query = |q: silo_omarchy::api::wire::CatalogQuery| serde_json::to_value(q).unwrap();
    assert_eq!(
        query(browse::section_query("s1", None)),
        json!({"source": "section", "scope": "home", "section_id": "s1", "limit": 60})
    );
    assert_eq!(
        query(browse::section_query("s1", Some("1"))),
        json!({"source": "section", "scope": "library", "library_id": "1", "section_id": "s1", "limit": 60})
    );
    assert_eq!(query(browse::source_query("watchlist")), json!({"source": "watchlist", "limit": 60}));
    assert_eq!(query(browse::source_query("history")), json!({"source": "history", "limit": 60}));
}
