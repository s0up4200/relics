//! The sort and the filters of a Library grid, the catalog query that they make, and the paging of its
//! results. The rule fields and ops are the same as in the web client.

use crate::api::wire;

/// The number of Items in one page of the grid.
pub const PAGE_SIZE: u64 = 60;

/// The grid of a personal list of the Profile, in the order of the list: `watchlist` or `history`.
pub fn source_query(source: &str) -> wire::CatalogQuery {
    wire::CatalogQuery {
        source: Some(source.into()),
        limit: std::num::NonZeroU64::new(PAGE_SIZE),
        ..Default::default()
    }
}

/// The grid of "View all" on a Home row, or on a row of the Recommended tab of the Library `library_id`.
pub fn section_query(section_id: &str, library_id: Option<&str>) -> wire::CatalogQuery {
    wire::CatalogQuery {
        scope: Some(if library_id.is_some() { "library" } else { "home" }.into()),
        library_id: library_id.and_then(|id| id.try_into().ok()),
        section_id: Some(section_id.into()),
        ..source_query("section")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortField {
    #[default]
    Title,
    DateAdded,
    ReleaseDate,
    Year,
    ContentRating,
    Duration,
    ImdbRating,
    TmdbRating,
    Resolution,
    Bitrate,
    Progress,
    DateViewed,
    Plays,
}

impl SortField {
    /// The sort fields in the order of the web client.
    pub const ALL: [SortField; 13] = [
        SortField::Title,
        SortField::DateAdded,
        SortField::ReleaseDate,
        SortField::Year,
        SortField::ContentRating,
        SortField::Duration,
        SortField::ImdbRating,
        SortField::TmdbRating,
        SortField::Resolution,
        SortField::Bitrate,
        SortField::Progress,
        SortField::DateViewed,
        SortField::Plays,
    ];

    /// The name of the field in the catalog query.
    pub fn wire(self) -> &'static str {
        match self {
            SortField::Title => "title",
            SortField::DateAdded => "added_at",
            SortField::ReleaseDate => "release_date",
            SortField::Year => "year",
            SortField::ContentRating => "content_rating",
            SortField::Duration => "runtime",
            SortField::ImdbRating => "rating_imdb",
            SortField::TmdbRating => "rating_tmdb",
            SortField::Resolution => "resolution",
            SortField::Bitrate => "bitrate",
            SortField::Progress => "progress",
            SortField::DateViewed => "date_viewed",
            SortField::Plays => "plays",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortField::Title => "Title",
            SortField::DateAdded => "Date added",
            SortField::ReleaseDate => "Release date",
            SortField::Year => "Year",
            SortField::ContentRating => "Content rating",
            SortField::Duration => "Duration",
            SortField::ImdbRating => "IMDb rating",
            SortField::TmdbRating => "TMDB rating",
            SortField::Resolution => "Resolution",
            SortField::Bitrate => "Bitrate",
            SortField::Progress => "Progress",
            SortField::DateViewed => "Date viewed",
            SortField::Plays => "Plays",
        }
    }
}

/// A filter that takes one text value: a language, a Person in one role, a studio, a network or a country.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facet {
    Language,
    Actor,
    Director,
    Writer,
    Producer,
    Studio,
    Network,
    Country,
}

impl Facet {
    /// The facets in the order of the rules of the web client.
    pub const ALL: [Facet; 8] = [
        Facet::Language,
        Facet::Actor,
        Facet::Director,
        Facet::Writer,
        Facet::Producer,
        Facet::Studio,
        Facet::Network,
        Facet::Country,
    ];

    /// The rule field in the catalog query.
    pub fn wire(self) -> &'static str {
        match self {
            Facet::Language => "original_language",
            Facet::Actor => "actor",
            Facet::Director => "director",
            Facet::Writer => "writer",
            Facet::Producer => "producer",
            Facet::Studio => "studio",
            Facet::Network => "network",
            Facet::Country => "country",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Facet::Language => "Language",
            Facet::Actor => "Actor",
            Facet::Director => "Director",
            Facet::Writer => "Writer",
            Facet::Producer => "Producer",
            Facet::Studio => "Studio",
            Facet::Network => "Network",
            Facet::Country => "Country",
        }
    }
}

/// The spans of "Added in the last".
pub const ADDED_SPANS: [&str; 5] = ["7d", "30d", "90d", "6m", "1y"];
/// The spans of "Released in the last".
pub const RELEASED_SPANS: [&str; 5] = ["30d", "90d", "6m", "1y", "5y"];

/// A span such as `30d` in words: "30 days".
pub fn span_label(span: &str) -> String {
    let (count, unit) = span.split_at(span.len().saturating_sub(1));
    let unit = match unit {
        "h" => "hour",
        "d" => "day",
        "w" => "week",
        "m" => "month",
        _ => "year",
    };
    format!("{count} {unit}{}", if count == "1" { "" } else { "s" })
}

/// The watch status filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    Watched,
    Unwatched,
    InProgress,
}

impl Watch {
    pub const ALL: [Watch; 3] = [Watch::Unwatched, Watch::InProgress, Watch::Watched];

    pub fn label(self) -> &'static str {
        match self {
            Watch::Watched => "Watched",
            Watch::Unwatched => "Unwatched",
            Watch::InProgress => "In progress",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filters {
    /// An Item must have each genre.
    pub genres: Vec<String>,
    /// The first year of the decade, for example 1990.
    pub decade: Option<i64>,
    pub year_from: Option<i64>,
    pub year_to: Option<i64>,
    /// The minimum IMDb rating.
    pub min_rating: Option<u8>,
    /// At most one value for each facet.
    pub facets: Vec<(Facet, String)>,
    pub watch: Option<Watch>,
    /// A span such as `30d`.
    pub added_in: Option<&'static str>,
    pub released_in: Option<&'static str>,
    pub four_k: bool,
    pub hdr: bool,
    pub dolby_vision: bool,
}

impl Filters {
    /// The value of a facet, if it has one.
    pub fn facet(&self, facet: Facet) -> Option<&str> {
        self.facets.iter().find(|(f, _)| *f == facet).map(|(_, value)| value.as_str())
    }

    /// The rule groups and the match of the catalog query. Search can use them with its own text.
    pub fn groups(&self) -> (Vec<wire::CatalogQueryGroup>, Option<String>) {
        let rules = self.rules();
        if rules.is_empty() {
            return (Vec::new(), None);
        }
        (vec![wire::CatalogQueryGroup { match_: "all".into(), rules }], Some("all".into()))
    }

    fn rules(&self) -> Vec<wire::CatalogQueryRule> {
        let rule = |field: &str, op: &str, value: serde_json::Value| wire::CatalogQueryRule {
            field: field.into(),
            op: op.into(),
            value,
        };
        let mut rules: Vec<_> = self.genres.iter().map(|genre| rule("genre", "is", genre.as_str().into())).collect();
        if let Some(decade) = self.decade {
            rules.push(rule("year", "gte", decade.into()));
            rules.push(rule("year", "lte", (decade + 9).into()));
        }
        rules.extend(self.year_from.map(|year| rule("year", "gte", year.into())));
        rules.extend(self.year_to.map(|year| rule("year", "lte", year.into())));
        rules.extend(self.min_rating.map(|rating| rule("rating_imdb", "gte", rating.into())));
        for facet in Facet::ALL {
            rules.extend(self.facet(facet).map(|value| rule(facet.wire(), "is", value.into())));
        }
        match self.watch {
            Some(Watch::Watched) => rules.push(rule("watched", "is", true.into())),
            Some(Watch::InProgress) => rules.push(rule("in_progress", "is", true.into())),
            Some(Watch::Unwatched) => {
                rules.push(rule("watched", "is", false.into()));
                rules.push(rule("in_progress", "is", false.into()));
            }
            None => {}
        }
        rules.extend(self.added_in.map(|span| rule("added_at", "in_last", span.into())));
        rules.extend(self.released_in.map(|span| rule("release_date", "in_last", span.into())));
        if self.four_k {
            rules.push(rule("resolution", "is", "2160p".into()));
        }
        if self.hdr {
            rules.push(rule("hdr", "is", true.into()));
        }
        if self.dolby_vision {
            rules.push(rule("dolby_vision", "is", true.into()));
        }
        rules
    }
}

/// The sort, the filters, the letter and the Series/Episodes mode of one Library grid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Browse {
    pub sort: SortField,
    pub descending: bool,
    pub filters: Filters,
    /// The A to Z filter: only titles that start with this letter.
    pub letter: Option<char>,
    /// A Shows library lists episodes instead of series.
    pub episodes: bool,
}

impl Browse {
    /// The catalog query of the first page of the grid of `library_id`. `shows` is true for a Shows library.
    pub fn query(&self, library_id: &str, shows: bool) -> wire::CatalogQuery {
        let (groups, match_) = self.filters.groups();
        wire::CatalogQuery {
            source: Some("query".into()),
            library_id: library_id.try_into().ok(),
            type_: shows.then(|| if self.episodes { "episode" } else { "series" }.into()),
            sort: Some(self.sort.wire().into()),
            order: Some(if self.descending { "desc" } else { "asc" }.into()),
            limit: std::num::NonZeroU64::new(PAGE_SIZE),
            name_prefix: self.letter.map(String::from),
            match_,
            groups,
            ..Default::default()
        }
    }

    /// The active filters, in the order of their rules, then the letter. The chip row shows them.
    pub fn chips(&self) -> Vec<Choice> {
        let f = &self.filters;
        let mut chips: Vec<_> = f.genres.iter().cloned().map(Choice::Genre).collect();
        chips.extend(f.decade.map(Choice::Decade));
        chips.extend(f.year_from.map(Choice::YearFrom));
        chips.extend(f.year_to.map(Choice::YearTo));
        chips.extend(f.min_rating.map(Choice::MinRating));
        for facet in Facet::ALL {
            chips.extend(f.facet(facet).map(|value| Choice::Facet(facet, value.into())));
        }
        chips.extend(f.watch.map(Choice::Watch));
        chips.extend(f.added_in.map(Choice::AddedIn));
        chips.extend(f.released_in.map(Choice::ReleasedIn));
        chips.extend(f.four_k.then_some(Choice::FourK));
        chips.extend(f.hdr.then_some(Choice::Hdr));
        chips.extend(f.dolby_vision.then_some(Choice::DolbyVision));
        chips.extend(self.letter.map(|letter| Choice::Letter(Some(letter))));
        chips
    }

    /// Turns off the active filter of a chip.
    pub fn remove(&mut self, chip: &Choice) {
        match chip {
            Choice::Letter(_) => self.letter = None,
            chip if self.is_chosen(chip) => self.choose(chip),
            _ => {}
        }
    }

    /// Removes each filter and the letter. The sort and the mode stay.
    pub fn clear(&mut self) {
        self.filters = Filters::default();
        self.letter = None;
    }
}

/// One entry of the sort, filter or A to Z menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Sort(SortField),
    Descending(bool),
    Watch(Watch),
    FourK,
    Hdr,
    DolbyVision,
    Decade(i64),
    YearFrom(i64),
    YearTo(i64),
    MinRating(u8),
    Facet(Facet, String),
    /// A span such as `30d`.
    AddedIn(&'static str),
    ReleasedIn(&'static str),
    Genre(String),
    /// `None` is "All".
    Letter(Option<char>),
}

impl Choice {
    pub fn letter_menu() -> Vec<Choice> {
        std::iter::once(Choice::Letter(None)).chain(('A'..='Z').map(|c| Choice::Letter(Some(c)))).collect()
    }

    /// The label of the entry in its menu section.
    pub fn label(&self) -> String {
        match self {
            Choice::Sort(field) => field.label().into(),
            Choice::Descending(false) => "Ascending".into(),
            Choice::Descending(true) => "Descending".into(),
            Choice::Watch(watch) => watch.label().into(),
            Choice::FourK => "4K".into(),
            Choice::Hdr => "HDR".into(),
            Choice::DolbyVision => "Dolby Vision".into(),
            Choice::Decade(decade) => format!("{decade}s"),
            Choice::YearFrom(year) => format!("From {year}"),
            Choice::YearTo(year) => format!("To {year}"),
            Choice::MinRating(rating) => format!("IMDb {rating}+"),
            Choice::Facet(_, value) => value.clone(),
            Choice::AddedIn(span) | Choice::ReleasedIn(span) => span_label(span),
            Choice::Genre(genre) => genre.clone(),
            Choice::Letter(None) => "All".into(),
            Choice::Letter(Some(letter)) => letter.to_string(),
        }
    }

    /// The label of the chip of an active filter, which also names the filter.
    pub fn chip_label(&self) -> String {
        match self {
            Choice::Facet(facet, value) => format!("{}: {value}", facet.label()),
            Choice::AddedIn(span) => format!("Added in the last {}", span_label(span)),
            Choice::ReleasedIn(span) => format!("Released in the last {}", span_label(span)),
            Choice::Letter(Some(letter)) => format!("Starts with {letter}"),
            choice => choice.label(),
        }
    }
}

impl Browse {
    /// Applies a menu entry. A filter that is on goes off. A decade, a year, a rating, a span, a watch status
    /// or the value of a facet replaces the old one.
    pub fn choose(&mut self, choice: &Choice) {
        let on = self.is_chosen(choice);
        let f = &mut self.filters;
        match choice {
            Choice::Sort(field) => self.sort = *field,
            Choice::Descending(descending) => self.descending = *descending,
            Choice::Watch(watch) => f.watch = (!on).then_some(*watch),
            Choice::FourK => f.four_k = !on,
            Choice::Hdr => f.hdr = !on,
            Choice::DolbyVision => f.dolby_vision = !on,
            Choice::Decade(decade) => f.decade = (!on).then_some(*decade),
            Choice::YearFrom(year) => f.year_from = (!on).then_some(*year),
            Choice::YearTo(year) => f.year_to = (!on).then_some(*year),
            Choice::MinRating(rating) => f.min_rating = (!on).then_some(*rating),
            Choice::Facet(facet, value) => {
                f.facets.retain(|(f, _)| f != facet);
                if !on {
                    f.facets.push((*facet, value.clone()));
                }
            }
            Choice::AddedIn(span) => f.added_in = (!on).then_some(*span),
            Choice::ReleasedIn(span) => f.released_in = (!on).then_some(*span),
            Choice::Genre(genre) if on => f.genres.retain(|g| g != genre),
            Choice::Genre(genre) => f.genres.push(genre.clone()),
            Choice::Letter(letter) => self.letter = *letter,
        }
    }

    pub fn is_chosen(&self, choice: &Choice) -> bool {
        let f = &self.filters;
        match choice {
            Choice::Sort(field) => self.sort == *field,
            Choice::Descending(descending) => self.descending == *descending,
            Choice::Watch(watch) => f.watch == Some(*watch),
            Choice::FourK => f.four_k,
            Choice::Hdr => f.hdr,
            Choice::DolbyVision => f.dolby_vision,
            Choice::Decade(decade) => f.decade == Some(*decade),
            Choice::YearFrom(year) => f.year_from == Some(*year),
            Choice::YearTo(year) => f.year_to == Some(*year),
            Choice::MinRating(rating) => f.min_rating == Some(*rating),
            Choice::Facet(facet, value) => f.facet(*facet) == Some(value.as_str()),
            Choice::AddedIn(span) => f.added_in == Some(*span),
            Choice::ReleasedIn(span) => f.released_in == Some(*span),
            Choice::Genre(genre) => f.genres.contains(genre),
            Choice::Letter(letter) => self.letter == *letter,
        }
    }
}

/// The Items of a grid that the server sent so far, and the cursor of the next page.
#[derive(Debug, Default)]
pub struct Paging {
    pub items: Vec<wire::CatalogItem>,
    /// The number of Items in the whole result, after the first page.
    pub total: Option<i64>,
    next_cursor: Option<String>,
    done: bool,
}

impl Paging {
    /// The query of the next page, or `None` after the last page.
    pub fn next_query(&self, first: &wire::CatalogQuery) -> Option<wire::CatalogQuery> {
        (!self.done).then(|| wire::CatalogQuery { cursor: self.next_cursor.clone(), ..first.clone() })
    }

    pub fn add(&mut self, page: wire::CatalogBrowseCollection) {
        self.items.extend(page.items);
        self.total.get_or_insert(page.total);
        self.next_cursor = page.page.filter(|p| p.has_more).and_then(|p| p.next_cursor);
        self.done = self.next_cursor.is_none();
    }
}
