//! The tracks that mpv reports in its `track-list` property, and the track choice that carries to the next
//! episode. The mpv adapter observes `track-list` as a JSON string: observing it as a node panics the event
//! thread in the pinned libmpv binding.

use serde::Deserialize;

/// The kind of a track, from the mpv `type` field. The Track picker lists no video tracks, so they are `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Audio,
    Sub,
    #[serde(other)]
    Other,
}

/// One track of the file that plays. `id` counts from 1 in each kind, as the mpv `aid` and `sid` do.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Track {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub codec: Option<String>,
    /// The audio channels.
    #[serde(rename = "demux-channel-count")]
    pub channels: Option<i64>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub forced: bool,
    #[serde(default)]
    pub external: bool,
    #[serde(default, rename = "hearing-impaired")]
    pub hearing_impaired: bool,
    #[serde(default)]
    pub selected: bool,
}

/// Reads the `track-list` JSON of mpv. A list that does not parse gives no tracks.
pub fn parse(json: &str) -> Vec<Track> {
    serde_json::from_str(json).unwrap_or_else(|err| {
        eprintln!("silo-omarchy: cannot read the mpv track list: {err}");
        Vec::new()
    })
}

impl Track {
    /// The language and the title, for example `eng · Commentary`, or `Unknown` with neither.
    // ponytail: the language shows as the code of the file (eng, ja). Add a name table if users ask.
    pub fn label(&self) -> String {
        let parts: Vec<&str> =
            [&self.lang, &self.title].into_iter().flatten().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() { "Unknown".into() } else { parts.join(" · ") }
    }

    /// The codec, and the channels of an audio track, for example `AAC · 5.1`.
    pub fn detail(&self) -> String {
        let channels = self.channels.filter(|_| self.kind == Kind::Audio).map(|count| match count {
            1 => "Mono".to_string(),
            2 => "Stereo".to_string(),
            6 => "5.1".to_string(),
            8 => "7.1".to_string(),
            n => format!("{n} ch"),
        });
        let codec = self.codec.as_ref().map(|codec| codec.to_uppercase());
        [codec, channels].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    /// The tags Forced, Default, External and SDH that apply. A title with "SDH" in it counts as SDH.
    pub fn tags(&self) -> Vec<&'static str> {
        let sdh = self.hearing_impaired
            || self
                .title
                .as_deref()
                .is_some_and(|title| title.split(|c: char| !c.is_alphanumeric()).any(|w| w == "SDH"));
        [(self.forced, "Forced"), (self.default, "Default"), (self.external, "External"), (sdh, "SDH")]
            .into_iter()
            .filter_map(|(on, tag)| on.then_some(tag))
            .collect()
    }
}

/// What the user picked in the Track picker, for one kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// No subtitles.
    Off,
    /// A track in this language with this forced state.
    Track { language: Option<String>, forced: bool },
}

impl Choice {
    /// The choice that `track` stands for.
    pub fn of(track: &Track) -> Self {
        Choice::Track { language: track.lang.clone(), forced: track.forced }
    }

    /// `track` has the language and the forced state of this choice.
    pub fn matches(&self, language: Option<&str>, forced: bool) -> bool {
        match self {
            Choice::Off => false,
            Choice::Track { language: wanted, forced: wanted_forced } => {
                *wanted_forced == forced && super::subtitles::same_language(wanted.as_deref(), language)
            }
        }
    }
}

/// The track choice that carries to the next episode of the same series. `None` keeps the default of the server.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Carry {
    pub audio: Option<Choice>,
    pub subtitle: Option<Choice>,
}

#[cfg(test)]
mod tests {
    use super::{Kind, parse};

    #[test]
    fn a_row_shows_the_language_title_codec_channels_and_tags() {
        let tracks = parse(
            r#"[{"id":1,"type":"audio","lang":"eng","title":"Commentary","codec":"eac3","demux-channel-count":6,
                 "default":true},
                {"id":3,"type":"sub","title":"English SDH","codec":"subrip","forced":true,"external":true,
                 "demux-channel-count":2},
                {"id":4,"type":"sub","lang":"jpn","hearing-impaired":true},
                {"id":1,"type":"attachment"}]"#,
        );
        assert_eq!(tracks.len(), 4);
        assert_eq!(tracks[3].kind, Kind::Other);
        assert_eq!(
            (tracks[0].label(), tracks[0].detail(), tracks[0].tags()),
            ("eng · Commentary".into(), "EAC3 · 5.1".into(), vec!["Default"])
        );
        assert_eq!((tracks[1].label(), tracks[1].detail()), ("English SDH".into(), "SUBRIP".into()), "no channels");
        assert_eq!(tracks[1].tags(), ["Forced", "External", "SDH"]);
        assert_eq!((tracks[2].label(), tracks[2].tags()), ("jpn".into(), vec!["SDH"]));
        assert_eq!(parse(r#"[{"id":1,"type":"audio"}]"#)[0].label(), "Unknown");
        assert_eq!(parse("not json"), vec![]);
    }
}
