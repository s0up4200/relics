//! The Settings store. It holds the Server settings of the active Profile in memory. The
//! API client reads them when a Profile opens, and each new Playback session uses them.

use crate::api::wire;

/// The keys of the Server settings that the app reads.
pub const KEYS: [&str; 8] = [
    "playback.audio_language",
    "playback.subtitle_language",
    "playback.subtitle_mode",
    "playback.show_forced_subtitles",
    "playback.intro_skip_mode",
    "playback.auto_skip_credits",
    "playback.auto_skip_recap",
    "playback.auto_play_next",
];

/// The playback Server settings of a Profile. `Default` gives the defaults of the settings contract.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSettings {
    /// The spoken language that the Profile prefers, a language tag or `x-silo-original`. `None` is no preference.
    pub audio_language: Option<String>,
    /// The subtitle language. `None` is no preference.
    pub subtitle_language: Option<String>,
    pub subtitle_mode: SubtitleMode,
    pub show_forced_subtitles: bool,
    pub intro_skip_mode: IntroSkipMode,
    pub auto_skip_credits: bool,
    pub auto_skip_recap: bool,
    pub auto_play_next: bool,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            audio_language: None,
            subtitle_language: None,
            subtitle_mode: SubtitleMode::Auto,
            show_forced_subtitles: true,
            intro_skip_mode: IntroSkipMode::Ask,
            auto_skip_credits: false,
            auto_skip_recap: false,
            auto_play_next: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleMode {
    Auto,
    Always,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntroSkipMode {
    Never,
    Ask,
    Always,
}

impl ServerSettings {
    /// Reads the effective values. A key that is missing, or a value of the wrong type, keeps its default.
    pub fn from_effective(values: &[wire::EffectiveSettingValue]) -> Self {
        let mut settings = Self::default();
        for item in values {
            let value = &item.value;
            let text = value.as_str();
            let flag = value.as_bool();
            match item.key.as_str() {
                "playback.audio_language" if value.is_null() || text.is_some() => {
                    settings.audio_language = text.map(String::from)
                }
                "playback.subtitle_language" if value.is_null() || text.is_some() => {
                    settings.subtitle_language = text.map(String::from)
                }
                "playback.subtitle_mode" => {
                    settings.subtitle_mode = match text {
                        Some("always") => SubtitleMode::Always,
                        Some("off") => SubtitleMode::Off,
                        _ => SubtitleMode::Auto,
                    }
                }
                "playback.intro_skip_mode" => {
                    settings.intro_skip_mode = match text {
                        Some("never") => IntroSkipMode::Never,
                        Some("always") => IntroSkipMode::Always,
                        _ => IntroSkipMode::Ask,
                    }
                }
                "playback.show_forced_subtitles" => settings.show_forced_subtitles = flag.unwrap_or(true),
                "playback.auto_skip_credits" => settings.auto_skip_credits = flag.unwrap_or(false),
                "playback.auto_skip_recap" => settings.auto_skip_recap = flag.unwrap_or(false),
                "playback.auto_play_next" => settings.auto_play_next = flag.unwrap_or(true),
                _ => {}
            }
        }
        settings
    }
}
