//! The MPRIS service. While the Player window is open, the app owns the bus name
//! `org.mpris.MediaPlayer2.silo_omarchy`, so the media keys and the desktop media
//! controls reach the Playback session as inputs.

use std::path::Path;

use mpris_server::{Metadata, Player, Time, TrackId, zbus};

use crate::api::{Url, wire};
use crate::playback::Input;

/// Takes the bus name and gives each MPRIS control to `on_input` as a Playback session input.
/// The name stays until the `Player` and the task of `Player::run` are dropped.
pub async fn start(on_input: impl Fn(Input) + Clone + 'static) -> zbus::Result<Player> {
    let player = Player::builder("silo_omarchy")
        .identity("Silo")
        .desktop_entry("silo-omarchy")
        .can_raise(false)
        .can_quit(false)
        .can_play(true)
        .can_pause(true)
        .can_seek(true)
        .can_control(true)
        .build()
        .await?;
    let control = |input: fn() -> Input| {
        let on_input = on_input.clone();
        move |_: &Player| on_input(input())
    };
    player.connect_play(control(|| Input::Play));
    player.connect_pause(control(|| Input::Pause));
    player.connect_play_pause(control(|| Input::PlayPause));
    player.connect_stop(control(|| Input::Shutdown));
    player.connect_next(control(|| Input::Next));
    player.connect_previous(control(|| Input::Previous));
    let on = on_input.clone();
    player.connect_seek(move |_, offset| on(Input::SeekBy(seconds(offset))));
    player.connect_set_position(move |player, track, position| {
        // MPRIS ignores a SetPosition for a track that does not play now.
        if player.metadata().trackid().as_ref() == Some(track) {
            on_input(Input::SetPosition(seconds(position)));
        }
    });
    Ok(player)
}

/// The MPRIS metadata of `track`. `length` is the duration in seconds, `art` the poster file.
pub fn metadata(track: Option<&Track>, length: Option<f64>, art: Option<&Path>) -> Metadata {
    let mut metadata = Metadata::new();
    let Some(track) = track else {
        return metadata;
    };
    metadata.set_trackid(TrackId::try_from(track.id.as_str()).ok());
    metadata.set_title(Some(&track.title));
    metadata.set_artist(track.artist.as_ref().map(|artist| [artist]));
    metadata.set_album(track.album.as_ref());
    metadata.set_length(length.map(time));
    metadata.set_art_url(art.and_then(|art| Url::from_file_path(art).ok()).map(String::from));
    metadata
}

pub fn time(seconds: f64) -> Time {
    Time::from_micros((seconds * 1e6) as i64)
}

fn seconds(time: Time) -> f64 {
    time.as_micros() as f64 / 1e6
}

/// What MPRIS shows about the Item that plays.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// `mpris:trackid`, a D-Bus object path made from the Item id.
    pub id: String,
    pub title: String,
    /// The series of an episode, the year of a movie.
    pub artist: Option<String>,
    /// The season and episode of an episode, for example `S1 E3`.
    pub album: Option<String>,
}

impl From<&wire::CatalogItemDetail> for Track {
    fn from(item: &wire::CatalogItemDetail) -> Self {
        // An object path allows only [A-Za-z0-9_] in an element, so the id goes in as hex.
        let hex: String = item.content_id.bytes().map(|byte| format!("{byte:02x}")).collect();
        let (artist, album) = match &item.series_title {
            Some(series) => (
                Some(series.clone()),
                item.season_number.zip(item.episode_number).map(|(season, episode)| format!("S{season} E{episode}")),
            ),
            None => (item.year.map(|year| year.to_string()), None),
        };
        Self { id: format!("/org/silo_omarchy/item/{hex}"), title: item.title.clone(), artist, album }
    }
}

#[cfg(test)]
mod tests {
    use super::Track;
    use crate::api::wire;

    fn item(fields: serde_json::Value) -> wire::CatalogItemDetail {
        let mut item: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/silo/get_catalog_item_ok.json")).unwrap();
        item.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        serde_json::from_value(item).unwrap()
    }

    #[test]
    fn an_episode_shows_its_title_its_series_and_its_season_and_episode() {
        let episode = item(serde_json::json!({
            "content_id": "episode:severance-s01e03", "type": "episode", "title": "In Perpetuity",
            "series_title": "Severance", "season_number": 1, "episode_number": 3, "year": 2022,
        }));
        let track = Track::from(&episode);
        assert_eq!(track.title, "In Perpetuity");
        assert_eq!(track.artist.as_deref(), Some("Severance"));
        assert_eq!(track.album.as_deref(), Some("S1 E3"));
    }

    #[test]
    fn a_movie_shows_its_title_and_its_year() {
        let track = Track::from(&item(serde_json::json!({})));
        assert_eq!((track.title.as_str(), track.artist.as_deref(), track.album), ("Heat", Some("1995"), None));
    }

    #[test]
    fn the_track_id_is_an_object_path_that_keeps_items_apart() {
        let id = |content_id: &str| Track::from(&item(serde_json::json!({"content_id": content_id}))).id;
        assert_eq!(id("movie:heat-1995"), "/org/silo_omarchy/item/6d6f7669653a686561742d31393935");
        assert_ne!(id("movie:a_b"), id("movie:a-b"));
    }
}
