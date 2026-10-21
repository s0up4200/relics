//! The Settings store. It holds the Server settings of the active Profile in memory. The
//! API client reads them when a Profile opens, and each new Playback session uses them.
//! The Local settings of this computer live in `$XDG_CONFIG_HOME/relics/settings.toml`.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::wire;

/// A group of the Settings page. The order is the order of the groups on the web Settings pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Quality,
    Skipping,
    NextEpisode,
    SeekControls,
    LanguageAndSubtitles,
    SubtitleAppearance,
    /// The Local settings.
    ThisComputer,
}

impl Group {
    pub const ALL: [Group; 7] = [
        Group::Quality,
        Group::Skipping,
        Group::NextEpisode,
        Group::SeekControls,
        Group::LanguageAndSubtitles,
        Group::SubtitleAppearance,
        Group::ThisComputer,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Quality => "Quality",
            Group::Skipping => "Skipping",
            Group::NextEpisode => "Next episode",
            Group::SeekControls => "Seek controls",
            Group::LanguageAndSubtitles => "Language & subtitles",
            Group::SubtitleAppearance => "Subtitle appearance",
            Group::ThisComputer => "This computer",
        }
    }
}

/// The values that a Server setting allows, and their labels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choices {
    /// Null ("No preference") or a language tag. The labels come from `LANGUAGES`.
    Language,
    /// true ("On") or false ("Off").
    OnOff,
    /// The values with their labels, in the order of the web Settings pages.
    List(&'static [(&'static str, &'static str)]),
}

/// The definition of one Server setting.
#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    pub key: &'static str,
    /// The revision of the settings contract that introduced the key.
    pub revision: u32,
    /// The label of the web Settings pages.
    pub label: &'static str,
    pub default: Value,
    pub choices: Choices,
    pub group: Group,
}

impl Definition {
    /// True when the setting can have this value.
    pub fn allows(&self, value: &Value) -> bool {
        match self.choices {
            Choices::Language => value.is_null() || value.is_string(),
            Choices::OnOff => value.is_boolean(),
            Choices::List(list) => list.iter().any(|&(v, _)| value.as_str() == Some(v)),
        }
    }

    /// The choices, with their labels. A language setting has "No preference" (null) first. Like the web page,
    /// a stored language outside the list is the second choice, so that it stays selected.
    pub fn options(&self, current: &Value) -> Vec<(Value, String)> {
        let pairs = |list: &[(&str, &str)]| list.iter().map(|&(v, l)| (Value::from(v), l.to_string())).collect();
        match self.choices {
            Choices::Language => {
                let mut options = vec![(Value::Null, "No preference".to_string())];
                if let Some(tag) = current.as_str()
                    && !LANGUAGES.iter().any(|&(code, _)| code == tag)
                {
                    let name = if tag == "x-silo-original" { "Original language" } else { tag };
                    options.push((current.clone(), name.to_string()));
                }
                options.extend(pairs(&LANGUAGES));
                options
            }
            Choices::OnOff => vec![(Value::Bool(true), "On".to_string()), (Value::Bool(false), "Off".to_string())],
            Choices::List(list) => pairs(list),
        }
    }
}

/// The Server settings that the app reads, in the order of the Settings page.
pub static DEFINITIONS: LazyLock<[Definition; 8]> = LazyLock::new(|| {
    let setting =
        |key, revision, label, default, choices, group| Definition { key, revision, label, default, choices, group };
    [
        setting(
            "playback.intro_skip_mode",
            7,
            "Skip intros",
            json!("ask"),
            Choices::List(&[("never", "Never"), ("ask", "Ask to skip"), ("always", "Skip automatically")]),
            Group::Skipping,
        ),
        setting("playback.auto_skip_recap", 1, "Auto-skip recaps", json!(false), Choices::OnOff, Group::Skipping),
        setting("playback.auto_skip_credits", 1, "Auto-skip credits", json!(false), Choices::OnOff, Group::Skipping),
        setting(
            "playback.auto_play_next",
            1,
            "Auto-play next episode",
            json!(true),
            Choices::OnOff,
            Group::NextEpisode,
        ),
        setting(
            "playback.audio_language",
            1,
            "Spoken language",
            Value::Null,
            Choices::Language,
            Group::LanguageAndSubtitles,
        ),
        setting(
            "playback.subtitle_language",
            1,
            "Subtitle language",
            Value::Null,
            Choices::Language,
            Group::LanguageAndSubtitles,
        ),
        setting(
            "playback.subtitle_mode",
            1,
            "Subtitle behavior",
            json!("auto"),
            Choices::List(&[("auto", "Auto"), ("always", "Always on"), ("off", "Off")]),
            Group::LanguageAndSubtitles,
        ),
        setting(
            "playback.show_forced_subtitles",
            1,
            "Show forced subtitles",
            json!(true),
            Choices::OnOff,
            Group::LanguageAndSubtitles,
        ),
    ]
});

/// The definition of a key, or `None` for a key that the app does not read.
pub fn definition(key: &str) -> Option<&'static Definition> {
    DEFINITIONS.iter().find(|definition| definition.key == key)
}

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
    /// The revision of the settings contract that the server gives to the app: the `manifest_revision` of its
    /// capabilities, or 1 when it has no batched effective reads. The server knows a key when this is equal to or
    /// higher than the revision of its definition. `None` until the app reads capabilities that allow the settings.
    pub manifest_revision: Option<u32>,
    /// Where the Effective value of each key came from, by key. A key that the server did not send has none.
    pub origins: HashMap<&'static str, Origin>,
}

/// Where the Effective value of a key came from, and how the household cap changed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Origin {
    /// The scope of the row that gave the value, for example `profile` or `profile_device`, or `default`.
    pub source: String,
    /// The household cap changed the value.
    pub constrained: bool,
    /// The value that the Profile set, before the cap changed it.
    pub stored_value: Option<Value>,
    /// The values that the cap allows. Empty when no cap applies.
    pub permitted_values: Vec<Value>,
}

impl Origin {
    /// The value came from the device row of this device.
    pub fn is_device(&self) -> bool {
        self.source == "profile_device"
    }
}

impl Default for ServerSettings {
    /// The default of each definition.
    fn default() -> Self {
        // `set` replaces each of these values with the default of its definition.
        let mut settings = Self {
            audio_language: None,
            subtitle_language: None,
            subtitle_mode: SubtitleMode::Auto,
            show_forced_subtitles: false,
            intro_skip_mode: IntroSkipMode::Ask,
            auto_skip_credits: false,
            auto_skip_recap: false,
            auto_play_next: false,
            manifest_revision: None,
            origins: HashMap::new(),
        };
        for definition in DEFINITIONS.iter() {
            settings.set(definition.key, &definition.default);
        }
        settings
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
    /// False when the server does not know the key of the definition. Before the capability read succeeds, the
    /// server knows each key.
    pub fn knows(&self, definition: &Definition) -> bool {
        self.manifest_revision.is_none_or(|revision| revision >= definition.revision)
    }

    /// Keeps a value that the app wrote at the `profile` scope. The cap fields stay until the next read.
    pub fn set_profile_value(&mut self, key: &str, value: &Value) {
        self.set(key, value);
        if let Some(origin) = self.origins.get_mut(key) {
            origin.source = "profile".into();
        }
    }

    /// Takes the value and the origin of one key from `other`.
    pub fn copy_key(&mut self, key: &str, other: &ServerSettings) {
        self.set(key, &other.value(key));
        match other.origins.get_key_value(key) {
            Some((&key, origin)) => self.origins.insert(key, origin.clone()),
            None => self.origins.remove(key),
        };
    }

    /// Puts the subtitle values that the Server resolved for one Item over these values. The Server applies the
    /// Library, series and device Overrides to them. A field that is absent or null, or a value that the definition
    /// does not allow, keeps the value here.
    pub fn apply_item(&mut self, item: &wire::CatalogItemDetail) {
        let values = [
            ("playback.subtitle_language", item.effective_subtitle_language.clone().map(Value::from)),
            ("playback.subtitle_mode", item.effective_subtitle_mode.clone().map(Value::from)),
            ("playback.show_forced_subtitles", item.effective_show_forced_subtitles.map(Value::from)),
        ];
        for (key, value) in values {
            if let Some(value) = value.filter(|value| definition(key).is_some_and(|d| d.allows(value))) {
                self.set(key, &value);
            }
        }
    }

    /// Reads the effective values. A key that is missing, or a value of the wrong type, keeps its default.
    pub fn from_effective(values: &[wire::EffectiveSettingValue]) -> Self {
        let mut settings = Self::default();
        for item in values {
            settings.set(&item.key, &item.value);
            if let Some(definition) = definition(&item.key) {
                let origin = Origin {
                    source: item.source.clone(),
                    constrained: item.constrained == Some(true),
                    stored_value: item.stored_value.clone(),
                    permitted_values: item.permitted_values.clone(),
                };
                settings.origins.insert(definition.key, origin);
            }
        }
        settings
    }

    /// Sets one value. A value that the definition does not allow gives the default of the key. An unknown key
    /// changes nothing.
    pub fn set(&mut self, key: &str, value: &Value) {
        let Some(definition) = definition(key) else {
            return;
        };
        let value = if definition.allows(value) { value } else { &definition.default };
        let text = value.as_str();
        let on = value.as_bool() == Some(true);
        match key {
            "playback.audio_language" => self.audio_language = text.map(String::from),
            "playback.subtitle_language" => self.subtitle_language = text.map(String::from),
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
            "playback.show_forced_subtitles" => self.show_forced_subtitles = on,
            "playback.auto_skip_credits" => self.auto_skip_credits = on,
            "playback.auto_skip_recap" => self.auto_skip_recap = on,
            "playback.auto_play_next" => self.auto_play_next = on,
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

/// The choices of a Server setting, with their labels. An unknown key has none.
pub fn options(key: &str, current: &Value) -> Vec<(Value, String)> {
    definition(key).map_or_else(Vec::new, |definition| definition.options(current))
}

/// The Local settings: the preferences of this app on this computer only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSettings {
    /// mpv decodes on the GPU (`hwdec=auto-safe`), or on the CPU only (`hwdec=no`).
    pub hardware_decoding: bool,
    /// The app asks GitHub for a newer release (the Release notice).
    pub check_releases: bool,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self { hardware_decoding: true, check_releases: true }
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

struct LocalDirectory(PathBuf);
impl gpui::Global for LocalDirectory {}

/// Use isolated Local settings for windows created by a test rig.
pub fn use_directory(dir: PathBuf, cx: &mut gpui::App) {
    cx.set_global(LocalDirectory(dir));
}

pub fn directory(cx: &gpui::App) -> PathBuf {
    cx.try_global::<LocalDirectory>().map_or_else(LocalSettings::dir, |dir| dir.0.clone())
}

#[cfg(test)]
mod tests {
    use super::{DEFINITIONS, ServerSettings};

    #[test]
    fn the_store_holds_the_default_of_each_definition() {
        let settings = ServerSettings::default();
        for definition in DEFINITIONS.iter() {
            assert!(definition.allows(&definition.default), "{}", definition.key);
            assert_eq!(settings.value(definition.key), definition.default, "{}", definition.key);
        }
    }
}
