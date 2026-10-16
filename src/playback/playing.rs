//! The Item that plays in the Player view. `Playing` holds its Playback session, the Items before and after it,
//! and the state that carries from one Item to the next. The Player view reads the state of the Item that plays
//! from here, through the controller (`player.rs`).

use std::time::SystemTime;

use crate::api::wire;
use crate::playback::{Command, Input, Session};
use crate::settings::ServerSettings;

/// What the controller needs to play an Item.
#[derive(Debug, Clone)]
pub struct PlayItem {
    pub(crate) content_id: String,
    /// The runtime in seconds.
    pub(crate) runtime: Option<f64>,
    /// The series id and the season number of an episode. The next episode lookup needs them.
    pub(crate) season: Option<(String, i64)>,
    /// The Item is an episode of a series card with no season, so the player reads the season of the episode.
    pub(crate) find_season: bool,
    /// Start at the beginning instead of the saved position.
    pub start_over: bool,
    /// The file version from the Version picker. `None` plays the first version.
    pub file_id: Option<String>,
    /// Start at this source position, for a version switch. It wins over `start_over`.
    start: Option<f64>,
    /// The id of the server shuffle that picked the Item, and the title of its next pick when it has one.
    pub(crate) shuffle: Option<(String, Option<String>)>,
}

impl From<&wire::CatalogItem> for PlayItem {
    /// A series or season card plays its `play_content_id`, an episode.
    fn from(item: &wire::CatalogItem) -> Self {
        let content_id = item.play_content_id.clone().unwrap_or_else(|| item.content_id.clone());
        let season = item.series_id.clone().zip(item.season_number);
        Self {
            find_season: season.is_none() && content_id != item.content_id,
            content_id,
            runtime: item.duration_seconds.or(item.runtime.map(|minutes| minutes as f64 * 60.0)),
            season,
            start_over: false,
            file_id: None,
            start: None,
            shuffle: None,
        }
    }
}

impl From<&wire::CatalogItemDetail> for PlayItem {
    /// A series plays its `play_content_id`, the Next up episode, in season `play_season_number`.
    fn from(item: &wire::CatalogItemDetail) -> Self {
        match &item.play_content_id {
            Some(episode) if item.type_ == "series" => Self {
                content_id: episode.clone(),
                runtime: None,
                season: item.play_season_number.map(|season| (item.content_id.clone(), season)),
                find_season: item.play_season_number.is_none(),
                start_over: false,
                file_id: None,
                start: None,
                shuffle: None,
            },
            _ => Self {
                content_id: item.content_id.clone(),
                runtime: item.duration_seconds.or(item.runtime.map(|minutes| minutes as f64 * 60.0)),
                season: item.series_id.clone().zip(item.season_number),
                find_season: false,
                start_over: false,
                file_id: None,
                start: None,
                shuffle: None,
            },
        }
    }
}

impl PlayItem {
    pub fn episode(series_id: String, episode: wire::Episode) -> Self {
        Self {
            content_id: episode.content_id,
            runtime: Some(episode.runtime as f64 * 60.0),
            season: Some((series_id, episode.season_number)),
            find_season: false,
            start_over: false,
            file_id: None,
            start: None,
            shuffle: None,
        }
    }

    /// The current pick of a shuffle. A pick plays from the beginning, as in the web app.
    pub fn shuffled(shuffle: &wire::Shuffle) -> Self {
        let next = &shuffle.next;
        let next = (next.content_id != shuffle.current.content_id)
            .then(|| next_label(next.season_number, next.episode_number, &next.title));
        Self { start_over: true, shuffle: Some((shuffle.id.to_string(), next)), ..Self::from(&shuffle.current) }
    }

    /// Where playback starts: `None` resumes at the saved position.
    pub fn start_position(&self) -> Option<f64> {
        self.start.or(self.start_over.then_some(0.))
    }
}

/// The title of the next Item on the Up Next card: `S1:E2 · Title` for an episode.
pub fn next_label(season: Option<i64>, episode: Option<i64>, title: &str) -> String {
    match season.zip(episode) {
        Some((season, episode)) => format!("S{season}:E{episode} · {title}"),
        None => title.to_string(),
    }
}

/// What plays after the Item that plays.
pub enum Next {
    /// The next pick of the shuffle `id`. The Server picks it when the shuffle moves past the Item `from`.
    Shuffle {
        id: String,
        from: String,
    },
    Episode(PlayItem),
}

#[derive(Default)]
pub struct Playing {
    session: Option<Session>,
    /// The Item that plays. A version switch plays it again with another file.
    item: Option<PlayItem>,
    /// The series of the episode that plays. The Track picker choice carries to the next Item of the same series.
    series: Option<String>,
    /// The next episode of the Item that plays.
    next: Option<PlayItem>,
    /// The previous episode of the Item that plays.
    previous: Option<PlayItem>,
    /// The title of the next episode or shuffle pick, for the Up Next card.
    next_title: Option<String>,
    /// How many Items in a row played by themselves up to the next Item that plays.
    auto_plays: u32,
}

impl Playing {
    /// Starts the Playback session of `item` with the Server settings `settings`. Call it after the session that
    /// played ended. It returns false when the Track picker choice of the last Item does not carry, so the
    /// Subtitle delay must not carry either.
    pub fn start(&mut self, item: PlayItem, settings: ServerSettings) -> bool {
        let series = item.season.as_ref().map(|(id, _)| id.clone());
        // A version switch plays the same Item again, so the choice carries too.
        let same = self.item.as_ref().is_some_and(|old| old.content_id == item.content_id);
        let carry = self.session.as_ref().filter(|_| same || series.is_some() && series == self.series);
        let carry = carry.map(Session::carry);
        let carries = carry.is_some();
        // Only the next Item of an automatic advance keeps the count; any other start is the user's.
        let auto_plays = std::mem::take(&mut self.auto_plays);
        let session = Session::new(item.runtime, settings).with_carry(carry.unwrap_or_default());
        self.session = Some(session.with_auto_plays(auto_plays));
        // A shuffle replaces the series order: Next plays the next pick, and Previous does nothing.
        self.next_title = item.shuffle.as_ref().and_then(|(_, next)| next.clone());
        (self.next, self.previous) = (None, None);
        (self.series, self.item) = (series, Some(item));
        carries
    }

    /// Gives `input` to the Playback session and returns the commands for the caller to carry out.
    pub fn step(&mut self, input: Input, now: SystemTime) -> Vec<Command> {
        let Some(session) = &mut self.session else {
            return Vec::new();
        };
        let commands = session.step(input, now);
        if commands.contains(&Command::PlayNext) {
            self.auto_plays = session.auto_plays();
        }
        commands
    }

    /// The Playback session of the Item that plays, or of the last Item that played.
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn item(&self) -> Option<&PlayItem> {
        self.item.as_ref()
    }

    /// Keeps the next and the previous episode that the lookup found in `series`, and the Up Next title of the
    /// next one. Returns the inputs that tell the session about them.
    pub fn found(
        &mut self,
        series: String,
        next: Option<(PlayItem, String)>,
        previous: Option<PlayItem>,
    ) -> Vec<Input> {
        self.series = Some(series);
        let mut inputs = Vec::new();
        if let Some((next, title)) = next {
            (self.next, self.next_title) = (Some(next), Some(title));
            inputs.push(Input::NextEpisode);
        }
        if let Some(previous) = previous {
            self.previous = Some(previous);
            inputs.push(Input::PreviousEpisode);
        }
        inputs
    }

    /// Takes what plays after the Item that plays: the next pick of its shuffle, else its next episode.
    pub fn take_next(&mut self) -> Option<Next> {
        match self.item.as_ref().and_then(|item| Some((item.shuffle.clone()?.0, item.content_id.clone()))) {
            Some((id, from)) => Some(Next::Shuffle { id, from }),
            None => self.next.take().map(Next::Episode),
        }
    }

    pub fn take_previous(&mut self) -> Option<PlayItem> {
        self.previous.take()
    }

    /// The Item that plays, with the file version `file_id` from the source position `position`.
    pub fn version(&self, file_id: String, position: Option<f64>) -> Option<PlayItem> {
        let item = self.item.clone()?;
        Some(PlayItem { file_id: Some(file_id), start: position, ..item })
    }

    /// The title of the next episode or shuffle pick, once it is known.
    pub fn next_title(&self) -> Option<&str> {
        self.next_title.as_deref()
    }

    /// The Item that plays is a shuffle pick.
    pub fn in_shuffle(&self) -> bool {
        self.item.as_ref().is_some_and(|item| item.shuffle.is_some())
    }

    /// The user stopped playback, so the next Item that plays starts the count of automatic plays again.
    pub fn reset_auto_plays(&mut self) {
        self.auto_plays = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{PlayItem, next_label};
    use crate::api::wire;

    fn detail(fields: serde_json::Value) -> wire::CatalogItemDetail {
        let mut item: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/silo/get_catalog_item_ok.json")).unwrap();
        item.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(item).unwrap()
    }

    #[test]
    fn a_series_page_plays_its_next_up_episode_with_the_season_so_that_next_episode_works() {
        let series = detail(serde_json::json!({
            "content_id": "series:severance", "type": "series",
            "play_content_id": "episode:severance-s01e02", "play_season_number": 1,
        }));
        let item = PlayItem::from(&series);
        assert_eq!(item.content_id, "episode:severance-s01e02");
        assert_eq!(item.season, Some(("series:severance".into(), 1)));
        assert!(!item.start_over);
    }

    #[test]
    fn a_series_card_plays_its_episode_and_looks_up_the_season_of_it() {
        let page: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/silo/query_catalog_items_ok.json")).unwrap();
        let mut card = page["items"][0].clone();
        let series = serde_json::json!({
            "content_id": "series:severance", "type": "series", "play_content_id": "episode:severance-s01e02",
            "series_id": null, "season_number": null,
        });
        card.as_object_mut().unwrap().extend(series.as_object().unwrap().clone());
        let card: wire::CatalogItem = serde_json::from_value(card).unwrap();
        let item = PlayItem::from(&card);
        assert_eq!((item.content_id.as_str(), item.season.clone()), ("episode:severance-s01e02", None));
        assert!(item.find_season, "Next episode needs the season of the episode");
        let movie = PlayItem::from(&detail(serde_json::json!({"content_id": "movie:heat-1995"})));
        assert!(!movie.find_season);
    }

    #[test]
    fn a_movie_page_plays_the_movie_and_start_over_starts_at_zero() {
        let movie = detail(serde_json::json!({"content_id": "movie:heat-1995", "runtime": 170}));
        let item = PlayItem::from(&movie);
        assert_eq!(
            (item.content_id.as_str(), item.runtime, item.season.clone()),
            ("movie:heat-1995", Some(10200.), None)
        );
        assert_eq!(item.start_position(), None, "the server resumes at the saved position");
        assert_eq!(PlayItem { start_over: true, ..item }.start_position(), Some(0.));
    }

    fn shuffle(next: usize) -> wire::Shuffle {
        let page: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/silo/query_catalog_items_ok.json")).unwrap();
        serde_json::from_value(serde_json::json!({
            "id": "s1", "created_at": "2026-01-01T00:00:00.000Z", "updated_at": "2026-01-01T00:00:00.000Z",
            "scope": {"kind": "library", "id": "1", "title": "Movies"},
            "current": page["items"][0], "next": page["items"][next],
        }))
        .unwrap()
    }

    #[test]
    fn a_shuffle_pick_plays_from_the_beginning_and_knows_if_a_next_pick_exists() {
        let item = PlayItem::shuffled(&shuffle(1));
        assert_eq!(item.content_id, shuffle(1).current.content_id);
        assert_eq!(item.start_position(), Some(0.), "a pick does not resume");
        assert_eq!(item.shuffle, Some(("s1".into(), Some(shuffle(1).next.title))));
        let only = PlayItem::shuffled(&shuffle(0));
        assert_eq!(only.shuffle, Some(("s1".into(), None)), "a scope with one Item has no next pick");
    }

    #[test]
    fn the_up_next_title_of_an_episode_has_its_season_and_number() {
        assert_eq!(next_label(Some(1), Some(2), "Half Loop"), "S1:E2 · Half Loop");
        assert_eq!(next_label(None, None, "Heat"), "Heat");
    }
}
