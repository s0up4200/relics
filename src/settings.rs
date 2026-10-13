//! The Settings store. It holds the Server settings of the active Profile in memory. The
//! API client reads them when a Profile opens, and each new Playback session uses them.
//! The Local settings of this computer live in `$XDG_CONFIG_HOME/relics/settings.toml`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
            settings.set(&item.key, &item.value);
        }
        settings
    }

    /// Sets one value. A value of the wrong type, or an unknown enum value, gives the default of the key.
    pub fn set(&mut self, key: &str, value: &Value) {
        let text = value.as_str();
        let flag = value.as_bool();
        match key {
            "playback.audio_language" if value.is_null() || text.is_some() => {
                self.audio_language = text.map(String::from)
            }
            "playback.subtitle_language" if value.is_null() || text.is_some() => {
                self.subtitle_language = text.map(String::from)
            }
            "playback.subtitle_mode" => {
                self.subtitle_mode = match text {
                    Some("always") => SubtitleMode::Always,
                    Some("off") => SubtitleMode::Off,
                    _ => SubtitleMode::Auto,
                }
            }
            "playback.intro_skip_mode" => {
                self.intro_skip_mode = match text {
                    Some("never") => IntroSkipMode::Never,
                    Some("always") => IntroSkipMode::Always,
                    _ => IntroSkipMode::Ask,
                }
            }
            "playback.show_forced_subtitles" => self.show_forced_subtitles = flag.unwrap_or(true),
            "playback.auto_skip_credits" => self.auto_skip_credits = flag.unwrap_or(false),
            "playback.auto_skip_recap" => self.auto_skip_recap = flag.unwrap_or(false),
            "playback.auto_play_next" => self.auto_play_next = flag.unwrap_or(true),
            _ => {}
        }
    }

    /// The value of one key, as the server stores it. An unknown key gives null.
    pub fn value(&self, key: &str) -> Value {
        match key {
            "playback.audio_language" => self.audio_language.clone().into(),
            "playback.subtitle_language" => self.subtitle_language.clone().into(),
            "playback.subtitle_mode" => match self.subtitle_mode {
                SubtitleMode::Auto => "auto",
                SubtitleMode::Always => "always",
                SubtitleMode::Off => "off",
            }
            .into(),
            "playback.intro_skip_mode" => match self.intro_skip_mode {
                IntroSkipMode::Never => "never",
                IntroSkipMode::Ask => "ask",
                IntroSkipMode::Always => "always",
            }
            .into(),
            "playback.show_forced_subtitles" => self.show_forced_subtitles.into(),
            "playback.auto_skip_credits" => self.auto_skip_credits.into(),
            "playback.auto_skip_recap" => self.auto_skip_recap.into(),
            "playback.auto_play_next" => self.auto_play_next.into(),
            _ => Value::Null,
        }
    }
}

/// The label of a Server setting, from the settings contract.
pub fn label(key: &str) -> &'static str {
    match key {
        "playback.audio_language" => "Preferred audio language",
        "playback.subtitle_language" => "Preferred subtitle language",
        "playback.subtitle_mode" => "Subtitles",
        "playback.show_forced_subtitles" => "Show forced subtitles",
        "playback.intro_skip_mode" => "Skip intros",
        "playback.auto_skip_credits" => "Auto-skip credits",
        "playback.auto_skip_recap" => "Auto-skip recaps",
        "playback.auto_play_next" => "Auto-play next episode",
        _ => "",
    }
}

/// The language list of the web Settings page: the `playback_audio_languages` option set of the settings
/// contract (revision 17), in its order, with the English names that the web page shows.
pub const LANGUAGES: [(&str, &str); 46] = [
    ("ar", "Arabic"),
    ("bn", "Bangla"),
    ("bg", "Bulgarian"),
    ("zh", "Chinese"),
    ("zh-Hans", "Simplified Chinese"),
    ("zh-Hant", "Traditional Chinese"),
    ("hr", "Croatian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("nl", "Dutch"),
    ("en", "English"),
    ("en-US", "American English"),
    ("en-GB", "British English"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("fr-CA", "Canadian French"),
    ("de", "German"),
    ("el", "Greek"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("ms", "Malay"),
    ("no", "Norwegian"),
    ("fa", "Persian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("pt-BR", "Brazilian Portuguese"),
    ("pt-PT", "European Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("es", "Spanish"),
    ("es-419", "Latin American Spanish"),
    ("es-ES", "European Spanish"),
    ("sv", "Swedish"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("vi", "Vietnamese"),
];

/// The choices of a Server setting, with their labels. A language setting has "No preference" (null) first.
/// Like the web page, a stored language outside the list is the second choice, so that it stays selected.
pub fn options(key: &str, current: &Value) -> Vec<(Value, String)> {
    let pairs = |list: &[(&str, &str)]| list.iter().map(|&(v, l)| (Value::from(v), l.to_string())).collect();
    match key {
        "playback.audio_language" | "playback.subtitle_language" => {
            let mut options = vec![(Value::Null, "No preference".to_string())];
            if let Some(tag) = current.as_str()
                && !LANGUAGES.iter().any(|&(code, _)| code == tag)
            {
                let name = if tag == "x-silo-original" { "Original language" } else { tag };
                options.push((current.clone(), name.to_string()));
            }
            options.extend(LANGUAGES.iter().map(|&(v, l)| (Value::from(v), l.to_string())));
            options
        }
        "playback.subtitle_mode" => pairs(&[("auto", "Auto"), ("always", "Always on"), ("off", "Off")]),
        "playback.intro_skip_mode" => {
            pairs(&[("never", "Never"), ("ask", "Ask to skip"), ("always", "Skip automatically")])
        }
        _ => vec![(Value::Bool(true), "On".to_string()), (Value::Bool(false), "Off".to_string())],
    }
}

/// The Local settings: the preferences of this app on this computer only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSettings {
    /// mpv decodes on the GPU (`hwdec=auto-safe`), or on the CPU only (`hwdec=no`).
    pub hardware_decoding: bool,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self { hardware_decoding: true }
    }
}

const LOCAL_FILE: &str = "settings.toml";

impl LocalSettings {
    /// `$XDG_CONFIG_HOME/relics`, or `~/.config/relics`.
    pub fn dir() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
        base.join("relics")
    }

    /// Reads the file in `dir`. A missing or broken file gives the defaults.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(LOCAL_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            eprintln!("relics: cannot read {}: {err}", path.display());
            Self::default()
        })
    }

    /// Writes the file in `dir` through a temp file, so that a crash never leaves half a file.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let temp = dir.join(format!("{LOCAL_FILE}.tmp"));
        std::fs::write(&temp, toml::to_string(self).map_err(io::Error::other)?)?;
        std::fs::rename(temp, dir.join(LOCAL_FILE))
    }

    /// The mpv `hwdec` option.
    pub fn hwdec(&self) -> &'static str {
        if self.hardware_decoding { "auto-safe" } else { "no" }
    }
}
