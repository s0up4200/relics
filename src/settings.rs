//! The Settings store. It holds the Server settings of the active Profile in memory. The
//! API client reads them when a Profile opens, and each new Playback session uses them.
//! The Local settings of this computer live in `$XDG_CONFIG_HOME/relics/settings.toml`.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::wire;
use crate::playback::Quality;

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

    /// The note below the title of the group.
    pub fn note(self) -> Option<&'static str> {
        (self == Group::SubtitleAppearance).then_some("Applies to this computer only")
    }
}

/// The values that a Server setting allows, and their labels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choices {
    /// Null ("No preference") or a language tag. The labels come from `LANGUAGES`.
    Language,
    /// Null ("Library default"), "Original language" (`x-silo-original`) or a language tag from `LANGUAGES`.
    MetadataLanguage,
    /// The Metadata exceptions: a map from the ISO 639 code of an original language to a language tag or
    /// `x-silo-original`, 256 entries at most. An entry with another key or a value that is not a text does not
    /// count. The Settings page shows one row for each exception, so the setting has no choices.
    Exceptions,
    /// true ("On") or false ("Off").
    OnOff,
    /// The values with their labels, in the order of the web Settings pages.
    List(&'static [(&'static str, &'static str)]),
    /// Like `List`, but a stored value that is not in the list is the first choice, so that it stays selected.
    Open(&'static [(&'static str, &'static str)]),
    /// A `#rrggbb` color. The swatches of the web Settings pages, with their names. A stored color that is not a
    /// swatch is the first choice, so that it stays selected.
    Color(&'static [(&'static str, &'static str)]),
    /// Null ("No limit") or a bitrate in kbps from 100 to 200000. The choices are these steps, in Mbps. A stored
    /// bitrate that is not a step is a choice between them, so that it stays selected.
    Kbps(&'static [u64]),
    /// A percent from 0 (or 1 when the first step is above 0) to 100. The choices are the steps of 10% from the
    /// first step. A stored value between two steps is a choice between them, so that it stays selected.
    Percent(u64),
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
    /// The field of the Subtitle appearance object that the row shows. `None` for a key with one value.
    pub field: Option<&'static str>,
}

impl Definition {
    /// True when the setting can have this value.
    pub fn allows(&self, value: &Value) -> bool {
        match self.choices {
            Choices::Language | Choices::MetadataLanguage => value.is_null() || value.is_string(),
            // `set` drops each entry that the contract does not allow, as the web does.
            Choices::Exceptions => value.is_object(),
            Choices::OnOff => value.is_boolean(),
            Choices::List(list) => list.iter().any(|&(v, _)| value.as_str() == Some(v)),
            Choices::Open(_) => value.as_str().is_some_and(|v| !v.is_empty()),
            Choices::Color(_) => value.as_str().is_some_and(|color| {
                color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
            }),
            Choices::Kbps(_) => value.is_null() || value.as_u64().is_some_and(|n| (100..=200_000).contains(&n)),
            Choices::Percent(first) => value.as_u64().is_some_and(|n| n >= first.min(1) && n <= 100),
        }
    }

    /// The choices, with their labels. A language setting has "No preference" (null) first. Like the web page,
    /// a stored language outside the list is the second choice, so that it stays selected.
    pub fn options(&self, current: &Value) -> Vec<(Value, String)> {
        let pairs = |list: &[(&str, &str)]| list.iter().map(|&(v, l)| (Value::from(v), l.to_string())).collect();
        match self.choices {
            Choices::Language | Choices::MetadataLanguage => {
                let mut options = match self.choices {
                    Choices::Language => vec![(Value::Null, "No preference".to_string())],
                    _ => vec![
                        (Value::Null, "Library default".to_string()),
                        (Value::from(ORIGINAL), language_name(ORIGINAL).to_string()),
                    ],
                };
                if let Some(tag) = current.as_str()
                    && !options.iter().any(|(value, _)| value == current)
                    && !LANGUAGES.iter().any(|&(code, _)| code == tag)
                {
                    options.push((current.clone(), language_name(tag).to_string()));
                }
                options.extend(pairs(&LANGUAGES));
                options
            }
            Choices::Exceptions => Vec::new(),
            Choices::OnOff => vec![(Value::Bool(true), "On".to_string()), (Value::Bool(false), "Off".to_string())],
            Choices::List(list) => pairs(list),
            Choices::Color(list) | Choices::Open(list) => {
                let mut options = pairs(list);
                if let Some(stored) = current.as_str()
                    && !list.iter().any(|&(value, _)| value == stored)
                {
                    options.insert(0, (current.clone(), stored.to_string()));
                }
                options
            }
            Choices::Kbps(list) => {
                let mut steps = list.to_vec();
                if let Some(n) = current.as_u64().filter(|n| !steps.contains(n)) {
                    steps.push(n);
                    steps.sort_unstable();
                }
                let steps = steps.into_iter().map(|n| (Value::from(n), bitrate_label(n)));
                std::iter::once((Value::Null, "No limit".to_string())).chain(steps).collect()
            }
            Choices::Percent(first) => {
                let mut steps: Vec<u64> = (first..=100).step_by(10).collect();
                if let Some(n) = current.as_u64().filter(|n| !steps.contains(n)) {
                    steps.push(n);
                    steps.sort_unstable();
                }
                steps.into_iter().map(|n| (Value::from(n), format!("{n}%"))).collect()
            }
        }
    }
}

/// The tag of "Original language": the original language of each Item.
pub const ORIGINAL: &str = "x-silo-original";

/// The English name of a language tag from `LANGUAGES`, "Original language", or else the tag itself.
pub fn language_name(tag: &str) -> &str {
    if tag == ORIGINAL {
        return "Original language";
    }
    LANGUAGES.iter().find(|&&(code, _)| code == tag).map_or(tag, |&(_, name)| name)
}

/// A bitrate as the web Settings page shows it: `1.5 Mbps`, `20 Mbps`, or `800 kbps`. The Quality list of the
/// Player view shows `20.0 Mbps`.
fn bitrate_label(kbps: u64) -> String {
    match kbps {
        0..1000 => format!("{kbps} kbps"),
        _ if kbps.is_multiple_of(1000) => format!("{} Mbps", kbps / 1000),
        _ => format!("{:.1} Mbps", kbps as f64 / 1000.0),
    }
}

/// The key of the Metadata language.
pub const METADATA_LANGUAGE: &str = "catalog.metadata_language";
/// The key of the Metadata exceptions.
pub const METADATA_EXCEPTIONS: &str = "catalog.metadata_language_overrides";

/// The Server applies the key to the titles, descriptions and artwork in its answers, so a change of it reloads
/// the open pages.
pub fn changes_the_catalog(key: &str) -> bool {
    key == METADATA_LANGUAGE || key == METADATA_EXCEPTIONS
}

/// The key of the Subtitle appearance. Its value is one object, and each of its fields is one row.
pub const APPEARANCE: &str = "playback.subtitle_appearance";

/// The font color swatches of the web. The outline color uses them too.
const FONT_COLORS: &[(&str, &str)] = &[
    ("#ffffff", "White"),
    ("#facc15", "Yellow"),
    ("#22c55e", "Green"),
    ("#06b6d4", "Cyan"),
    ("#d946ef", "Magenta"),
    ("#ef4444", "Red"),
    ("#3b82f6", "Blue"),
    ("#9ca3af", "Gray"),
    ("#000000", "Black"),
];

/// The Server settings that the app reads, in the order of the Settings page. The rows of the Subtitle
/// appearance come one after the other.
pub static DEFINITIONS: LazyLock<[Definition; 22]> = LazyLock::new(|| {
    let setting = |key, revision, label, default, choices, group| Definition {
        key,
        revision,
        label,
        default,
        choices,
        group,
        field: None,
    };
    let appearance = |field, revision, label, default, choices| Definition {
        key: APPEARANCE,
        revision,
        label,
        default,
        choices,
        group: Group::SubtitleAppearance,
        field: Some(field),
    };
    [
        setting(
            "playback.preferred_quality",
            1,
            "Preferred quality",
            json!("auto"),
            Choices::Open(&[
                ("auto", "Auto"),
                ("480p", "480p"),
                ("720p", "720p"),
                ("1080p", "1080p"),
                ("2160p", "2160p / 4K"),
                ("original", "Original quality"),
            ]),
            Group::Quality,
        ),
        // The steps of the web Settings page.
        setting(
            "playback.max_bitrate_kbps",
            1,
            "Maximum bitrate",
            Value::Null,
            Choices::Kbps(&[1500, 2000, 4000, 6000, 10_000, 15_000, 20_000, 40_000, 60_000, 100_000, 200_000]),
            Group::Quality,
        ),
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
        // The option set `catalog_metadata_languages` is the same list as `LANGUAGES`.
        setting(
            METADATA_LANGUAGE,
            1,
            "Metadata language",
            Value::Null,
            Choices::MetadataLanguage,
            Group::LanguageAndSubtitles,
        ),
        setting(
            METADATA_EXCEPTIONS,
            3,
            "Metadata exceptions",
            json!({}),
            Choices::Exceptions,
            Group::LanguageAndSubtitles,
        ),
        // The fields, labels and swatches of the web Subtitles settings page. Revision 14 added the text opacity.
        appearance(
            "fontSize",
            1,
            "Font size",
            json!("large"),
            Choices::List(&[
                ("small", "Small"),
                ("medium", "Medium"),
                ("large", "Large"),
                ("xlarge", "X-Large"),
                ("xxlarge", "XX-Large"),
            ]),
        ),
        appearance(
            "fontFamily",
            1,
            "Font family",
            json!("sans-serif"),
            Choices::List(&[("sans-serif", "Sans-serif"), ("serif", "Serif"), ("monospace", "Monospace")]),
        ),
        appearance("fontColor", 1, "Font color", json!("#ffffff"), Choices::Color(FONT_COLORS)),
        appearance("textOpacity", 14, "Text opacity", json!(100), Choices::Percent(10)),
        appearance("textOutline", 1, "Text outline", json!(false), Choices::OnOff),
        appearance("textOutlineColor", 1, "Outline color", json!("#000000"), Choices::Color(FONT_COLORS)),
        appearance(
            "backgroundStyle",
            1,
            "Background style",
            json!("box"),
            Choices::List(&[("box", "Box"), ("shadow", "Drop shadow"), ("outline", "Outline"), ("none", "None")]),
        ),
        appearance("backgroundOpacity", 1, "Background opacity", json!(75), Choices::Percent(0)),
        appearance(
            "backgroundColor",
            1,
            "Background color",
            json!("#000000"),
            Choices::Color(&[
                ("#000000", "Black"),
                ("#374151", "Dark Gray"),
                ("#1e3a5f", "Navy"),
                ("#7f1d1d", "Dark Red"),
                ("#14532d", "Dark Green"),
            ]),
        ),
        appearance(
            "position",
            1,
            "Subtitle position",
            json!("bottom"),
            Choices::List(&[("bottom", "Bottom"), ("lower-third", "Lower third"), ("top", "Top")]),
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
    /// The `quality_preference` of a start request: `auto`, `480p`, `720p`, `1080p`, `2160p` or `original`.
    pub preferred_quality: String,
    /// The `bandwidth_cap_kbps` of a start request. `None` is no limit.
    pub max_bitrate_kbps: Option<u64>,
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
    pub subtitle_appearance: SubtitleAppearance,
    /// The Metadata language, a language tag or `x-silo-original`. `None` is "Library default".
    pub metadata_language: Option<String>,
    /// The Metadata exceptions: the ISO 639 code of an original language, and the language tag to show.
    pub metadata_exceptions: BTreeMap<String, String>,
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
            preferred_quality: String::new(),
            max_bitrate_kbps: None,
            audio_language: None,
            subtitle_language: None,
            subtitle_mode: SubtitleMode::Auto,
            show_forced_subtitles: false,
            intro_skip_mode: IntroSkipMode::Ask,
            auto_skip_credits: false,
            auto_skip_recap: false,
            auto_play_next: false,
            subtitle_appearance: SubtitleAppearance::default(),
            metadata_language: None,
            metadata_exceptions: BTreeMap::new(),
            manifest_revision: None,
            origins: HashMap::new(),
        };
        for definition in DEFINITIONS.iter() {
            settings.set(definition.key, &definition.default);
        }
        settings
    }
}

/// How plain-text subtitles look on this device. Each field has a value that its definition allows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleAppearance {
    /// small, medium, large, xlarge or xxlarge.
    pub font_size: String,
    /// sans-serif, serif or monospace.
    pub font_family: String,
    /// `#rrggbb`.
    pub font_color: String,
    /// The alpha of the text in percent.
    pub text_opacity: u64,
    /// box, shadow, outline or none.
    pub background_style: String,
    /// `#rrggbb`, the color of the box.
    pub background_color: String,
    /// The alpha of the box in percent.
    pub background_opacity: u64,
    /// An outline in the outline color, on top of the background style.
    pub text_outline: bool,
    /// `#rrggbb`, the color of the outline.
    pub text_outline_color: String,
    /// bottom, lower-third or top.
    pub position: String,
}

impl SubtitleAppearance {
    /// Merges the fields of a stored value over the defaults. A field that is absent, or that its definition does
    /// not allow, keeps its default. A color is in lower case, as the swatches are.
    pub fn merged(value: &Value) -> Self {
        let fields = DEFINITIONS.iter().filter_map(|definition| {
            let field = definition.field?;
            let value = value.get(field).filter(|value| definition.allows(value)).unwrap_or(&definition.default);
            let value = match (definition.choices, value.as_str()) {
                (Choices::Color(_), Some(color)) => Value::from(color.to_lowercase()),
                _ => value.clone(),
            };
            Some((field.to_string(), value))
        });
        serde_json::from_value(Value::Object(fields.collect()))
            .expect("each field has a value that its definition allows")
    }
}

impl Default for SubtitleAppearance {
    fn default() -> Self {
        Self::merged(&Value::Null)
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

    /// Keeps a value that the app wrote at `scope`. The cap fields stay until the next read.
    pub fn set_stored(&mut self, key: &str, value: &Value, scope: &str) {
        self.set(key, value);
        if let Some(origin) = self.origins.get_mut(key) {
            origin.source = scope.into();
        }
    }

    /// The value that the row of `definition` shows: the value of its key, or one field of the Subtitle appearance.
    pub fn row_value(&self, definition: &Definition) -> Value {
        let value = self.value(definition.key);
        definition.field.map_or(value.clone(), |field| value[field].clone())
    }

    /// Sets the row of `definition` to `row`. A row of the Subtitle appearance sets one field of the object.
    pub fn set_row(&mut self, definition: &Definition, row: Value) {
        let Some(field) = definition.field else {
            return self.set(definition.key, &row);
        };
        let mut value = self.value(definition.key);
        value[field] = row;
        self.set(definition.key, &value);
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

    /// The quality that a Playback session starts with.
    pub fn quality(&self) -> Quality {
        Quality { preference: self.preferred_quality.clone(), bandwidth_cap_kbps: self.max_bitrate_kbps }
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
        if key == APPEARANCE {
            self.subtitle_appearance = SubtitleAppearance::merged(value);
            return;
        }
        let Some(definition) = definition(key) else {
            return;
        };
        let value = if definition.allows(value) { value } else { &definition.default };
        let text = value.as_str();
        let on = value.as_bool() == Some(true);
        match key {
            "playback.preferred_quality" => self.preferred_quality = text.unwrap_or_default().into(),
            "playback.max_bitrate_kbps" => self.max_bitrate_kbps = value.as_u64(),
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
            METADATA_LANGUAGE => self.metadata_language = text.map(String::from),
            METADATA_EXCEPTIONS => {
                let code = |code: &str| (2..=3).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase());
                let map = value.as_object().into_iter().flatten().filter(|(source, _)| code(source));
                self.metadata_exceptions =
                    map.filter_map(|(source, target)| Some((source.clone(), target.as_str()?.to_string()))).collect();
            }
            _ => {}
        }
    }

    /// The value of one key, as the server stores it. An unknown key gives null.
    pub fn value(&self, key: &str) -> Value {
        match key {
            "playback.preferred_quality" => self.preferred_quality.clone().into(),
            "playback.max_bitrate_kbps" => self.max_bitrate_kbps.into(),
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
            METADATA_LANGUAGE => self.metadata_language.clone().into(),
            METADATA_EXCEPTIONS => json!(self.metadata_exceptions),
            APPEARANCE => serde_json::to_value(&self.subtitle_appearance).expect("the Subtitle appearance serializes"),
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
            assert!(definition.allows(&definition.default), "{}", definition.label);
            assert_eq!(settings.row_value(definition), definition.default, "{}", definition.label);
        }
    }
}
