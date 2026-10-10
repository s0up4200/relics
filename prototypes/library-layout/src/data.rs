//! PROTOTYPE fake catalog. In-memory only; no requests to any server.

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Movie,
    Series,
    Episode,
}

#[derive(Clone)]
pub struct Item {
    pub id: usize,
    pub kind: Kind,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub year: u16,
    pub runtime_min: u16,
    pub rating: &'static str,
    pub genres: &'static str,
    pub overview: &'static str,
    /// 0.0..1.0 watched; None when not started.
    pub progress: Option<f32>,
    /// Hue for the placeholder art (0..1).
    pub hue: f32,
}

pub struct Section {
    pub title: &'static str,
    /// true = 16:9 landscape thumbnails, false = 2:3 posters.
    pub landscape: bool,
    pub items: Vec<usize>,
}

pub struct Library {
    pub name: &'static str,
    pub items: Vec<usize>,
}

pub struct Catalog {
    pub items: Vec<Item>,
    pub sections: Vec<Section>,
    pub libraries: Vec<Library>,
}

const OVERVIEW: &str = "A placeholder overview long enough to wrap over two or three lines, so the detail view shows how real text sits next to the art and the buttons.";

pub fn catalog() -> Catalog {
    let raw: &[(Kind, &str, &str, u16, u16, Option<f32>)] = &[
        (Kind::Movie, "Arrival", "", 2016, 116, Some(0.42)),
        (Kind::Episode, "Severance", "S2 · E4 · Woe's Hollow", 2025, 52, Some(0.18)),
        (Kind::Movie, "Dune: Part Two", "", 2024, 166, Some(0.77)),
        (Kind::Episode, "The Bear", "S3 · E1 · Tomorrow", 2024, 37, Some(0.05)),
        (Kind::Movie, "Past Lives", "", 2023, 106, None),
        (Kind::Movie, "Blade Runner 2049", "", 2017, 164, None),
        (Kind::Series, "Andor", "2 seasons", 2022, 0, None),
        (Kind::Movie, "Perfect Days", "", 2023, 124, None),
        (Kind::Series, "Shōgun", "1 season", 2024, 0, None),
        (Kind::Movie, "The Zone of Interest", "", 2023, 105, None),
        (Kind::Series, "Slow Horses", "4 seasons", 2022, 0, None),
        (Kind::Movie, "Aftersun", "", 2022, 102, None),
        (Kind::Movie, "Oppenheimer", "", 2023, 180, None),
        (Kind::Series, "Pachinko", "2 seasons", 2022, 0, None),
        (Kind::Movie, "Drive My Car", "", 2021, 179, None),
        (Kind::Movie, "Decision to Leave", "", 2022, 138, None),
        (Kind::Series, "Twin Peaks", "3 seasons", 1990, 0, None),
        (Kind::Movie, "In the Mood for Love", "", 2000, 98, None),
        (Kind::Movie, "Stalker", "", 1979, 162, None),
        (Kind::Series, "The Leftovers", "3 seasons", 2014, 0, None),
        (Kind::Movie, "Mad Max: Fury Road", "", 2015, 120, None),
        (Kind::Movie, "Parasite", "", 2019, 132, None),
        (Kind::Series, "Mr. Robot", "4 seasons", 2015, 0, None),
        (Kind::Movie, "The Holdovers", "", 2023, 133, None),
    ];
    let items: Vec<Item> = raw
        .iter()
        .enumerate()
        .map(|(id, &(kind, title, subtitle, year, runtime_min, progress))| Item {
            id,
            kind,
            title,
            subtitle,
            year,
            runtime_min,
            rating: ["PG-13", "R", "TV-MA", "12"][id % 4],
            genres: ["Drama · Sci-Fi", "Thriller", "Drama · Romance", "Comedy · Drama"][id % 4],
            overview: OVERVIEW,
            progress,
            hue: (id as f32 * 0.137) % 1.0,
        })
        .collect();
    let movies = items.iter().filter(|i| i.kind == Kind::Movie).map(|i| i.id).collect();
    let shows = items.iter().filter(|i| i.kind == Kind::Series).map(|i| i.id).collect();
    Catalog {
        sections: vec![
            Section { title: "Continue watching", landscape: true, items: vec![0, 1, 2, 3] },
            Section { title: "Recently added movies", landscape: false, items: vec![4, 5, 7, 9, 11, 12, 14, 15, 17] },
            Section { title: "Recently added shows", landscape: false, items: vec![6, 8, 10, 13, 16, 19, 22] },
            Section { title: "Watchlist", landscape: false, items: vec![18, 20, 21, 23, 5, 9] },
        ],
        libraries: vec![
            Library { name: "Movies", items: movies },
            Library { name: "Shows", items: shows },
        ],
        items,
    }
}
