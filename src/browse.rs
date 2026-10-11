//! The sort and the filters of a Library grid, the catalog query that they make, and the paging of its
//! results. The rule fields and ops are the same as in the web client.

use crate::api::wire;

/// The number of Items in one page of the grid.
pub const PAGE_SIZE: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortField {
    #[default]
    Title,
    DateAdded,
    ReleaseDate,
    ImdbRating,
    Duration,
    DateViewed,
}

impl SortField {
    pub const ALL: [SortField; 6] = [
        SortField::Title,
        SortField::DateAdded,
        SortField::ReleaseDate,
        SortField::ImdbRating,
        SortField::Duration,
        SortField::DateViewed,
    ];

    /// The name of the field in the catalog query.
    pub fn wire(self) -> &'static str {
        match self {
            SortField::Title => "title",
            SortField::DateAdded => "added_at",
            SortField::ReleaseDate => "release_date",
            SortField::ImdbRating => "rating_imdb",
            SortField::Duration => "runtime",
            SortField::DateViewed => "date_viewed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortField::Title => "Title",
            SortField::DateAdded => "Date added",
            SortField::ReleaseDate => "Release date",
            SortField::ImdbRating => "IMDb rating",
            SortField::Duration => "Duration",
            SortField::DateViewed => "Date viewed",
        }
    }
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
    pub watch: Option<Watch>,
    pub four_k: bool,
    pub hdr: bool,
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
        let rules = self.rules();
        let groups =
            if rules.is_empty() { Vec::new() } else { vec![wire::CatalogQueryGroup { match_: "all".into(), rules }] };
        wire::CatalogQuery {
            source: Some("query".into()),
            library_id: library_id.try_into().ok(),
            type_: shows.then(|| if self.episodes { "episode" } else { "series" }.into()),
            sort: Some(self.sort.wire().into()),
            order: Some(if self.descending { "desc" } else { "asc" }.into()),
            limit: std::num::NonZeroU64::new(PAGE_SIZE),
            name_prefix: self.letter.map(String::from),
            match_: (!groups.is_empty()).then(|| "all".into()),
            groups,
            ..Default::default()
        }
    }

    fn rules(&self) -> Vec<wire::CatalogQueryRule> {
        let rule = |field: &str, op: &str, value: serde_json::Value| wire::CatalogQueryRule {
            field: field.into(),
            op: op.into(),
            value,
        };
        let f = &self.filters;
        let mut rules: Vec<_> = f.genres.iter().map(|genre| rule("genre", "is", genre.as_str().into())).collect();
        if let Some(decade) = f.decade {
            rules.push(rule("year", "gte", decade.into()));
            rules.push(rule("year", "lte", (decade + 9).into()));
        }
        match f.watch {
            Some(Watch::Watched) => rules.push(rule("watched", "is", true.into())),
            Some(Watch::InProgress) => rules.push(rule("in_progress", "is", true.into())),
            Some(Watch::Unwatched) => {
                rules.push(rule("watched", "is", false.into()));
                rules.push(rule("in_progress", "is", false.into()));
            }
            None => {}
        }
        if f.four_k {
            rules.push(rule("resolution", "is", "2160p".into()));
        }
        if f.hdr {
            rules.push(rule("hdr", "is", true.into()));
        }
        rules
    }

    /// The active filters, in the order of the filter menu, then the letter. The chip row shows them.
    pub fn chips(&self) -> Vec<Choice> {
        let f = &self.filters;
        let mut chips: Vec<_> = f.genres.iter().cloned().map(Choice::Genre).collect();
        chips.extend(f.decade.map(Choice::Decade));
        chips.extend(f.watch.map(Choice::Watch));
        chips.extend(f.four_k.then_some(Choice::FourK));
        chips.extend(f.hdr.then_some(Choice::Hdr));
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
    Decade(i64),
    Genre(String),
    /// `None` is "All".
    Letter(Option<char>),
}

impl Choice {
    pub fn sort_menu() -> Vec<Choice> {
        let fields = SortField::ALL.into_iter().map(Choice::Sort);
        fields.chain([Choice::Descending(false), Choice::Descending(true)]).collect()
    }

    /// The filter menu: the watch status, 4K and HDR, the decades from the one of `this_year` back to the
    /// 1900s, and the `genres` of the Library.
    pub fn filter_menu(genres: &[String], this_year: i64) -> Vec<Choice> {
        let mut menu: Vec<_> = Watch::ALL.into_iter().map(Choice::Watch).collect();
        menu.extend([Choice::FourK, Choice::Hdr]);
        menu.extend((1900..=this_year / 10 * 10).rev().step_by(10).map(Choice::Decade));
        menu.extend(genres.iter().cloned().map(Choice::Genre));
        menu
    }

    pub fn letter_menu() -> Vec<Choice> {
        std::iter::once(Choice::Letter(None)).chain(('A'..='Z').map(|c| Choice::Letter(Some(c)))).collect()
    }

    pub fn label(&self) -> String {
        match self {
            Choice::Sort(field) => field.label().into(),
            Choice::Descending(false) => "Ascending".into(),
            Choice::Descending(true) => "Descending".into(),
            Choice::Watch(watch) => watch.label().into(),
            Choice::FourK => "4K".into(),
            Choice::Hdr => "HDR".into(),
            Choice::Decade(decade) => format!("{decade}s"),
            Choice::Genre(genre) => genre.clone(),
            Choice::Letter(None) => "All".into(),
            Choice::Letter(Some(letter)) => letter.to_string(),
        }
    }
}

impl Browse {
    /// Applies a menu entry. A filter that is on goes off. A decade or a watch status replaces the old one.
    pub fn choose(&mut self, choice: &Choice) {
        let on = self.is_chosen(choice);
        let f = &mut self.filters;
        match choice {
            Choice::Sort(field) => self.sort = *field,
            Choice::Descending(descending) => self.descending = *descending,
            Choice::Watch(watch) => f.watch = (!on).then_some(*watch),
            Choice::FourK => f.four_k = !on,
            Choice::Hdr => f.hdr = !on,
            Choice::Decade(decade) => f.decade = (!on).then_some(*decade),
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
            Choice::Decade(decade) => f.decade == Some(*decade),
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
