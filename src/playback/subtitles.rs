//! The subtitle choice at the start of playback. It is a port of the web player rule
//! `resolveSubtitleAutoSelect` (silo-server web/src/player/utils/subtitleSort.ts), so a
//! Profile gets the same subtitles on each client. The app keeps no remembered track per
//! series, so the track signature of the web rule is left out.

use crate::api::wire::SubtitleInventoryItemV3 as Track;
use crate::settings::{ServerSettings, SubtitleMode};

/// The `playback.audio_language` values that mean "the original language of the Item".
const ORIGINAL: [&str; 2] = ["original", "x-silo-original"];

/// Returns the `combined_index` of the subtitle track to show, or `None` for no subtitles.
/// `audio_language` is the language of the audio track that plays.
pub fn choose(tracks: &[Track], settings: &ServerSettings, audio_language: Option<&str>) -> Option<i64> {
    if tracks.is_empty() {
        return None;
    }
    let preferred = settings.subtitle_language.as_deref();
    let subtitle_lang = concrete(preferred);
    let profile = normalize(settings.audio_language.as_deref());
    let profile_lang = concrete(Some(&profile)).or_else(|| match profile.as_str() {
        p if ORIGINAL.contains(&p) => subtitle_lang.clone(),
        "" => Some("en".into()),
        _ => None,
    });
    let audio_lang = concrete(audio_language).or_else(|| profile_lang.clone());
    let forced = || if settings.show_forced_subtitles { forced_track(tracks, audio_lang.as_deref()) } else { None };

    match settings.subtitle_mode {
        SubtitleMode::Off => forced(),
        SubtitleMode::Always => best_match(tracks, preferred.filter(|p| !p.is_empty())?),
        SubtitleMode::Auto => {
            if preferred == Some("") {
                return None;
            }
            if profile_lang.is_some() && same_language(audio_lang.as_deref(), profile_lang.as_deref()) {
                return forced();
            }
            match subtitle_lang.or(profile_lang) {
                Some(lang) => best_match(tracks, &lang),
                None => forced(),
            }
        }
    }
}

fn normalize(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_lowercase()
}

/// The language in lower case, or `None` when it is empty or means the original language.
fn concrete(value: Option<&str>) -> Option<String> {
    let value = normalize(value);
    (!value.is_empty() && !ORIGINAL.contains(&value.as_str())).then_some(value)
}

fn same_language(a: Option<&str>, b: Option<&str>) -> bool {
    match (concrete(a), concrete(b)) {
        (Some(a), Some(b)) => language_code(&a) == language_code(&b),
        _ => false,
    }
}

/// The best forced track in `language`.
fn forced_track(tracks: &[Track], language: Option<&str>) -> Option<i64> {
    best_match(tracks.iter().filter(|track| track.forced), language?)
}

/// The best track for `language`: the exact tag, then the bare language, then another
/// variant of the language. Inside one of these ranks, `priority` decides.
fn best_match<'a>(tracks: impl IntoIterator<Item = &'a Track>, language: &str) -> Option<i64> {
    tracks
        .into_iter()
        .filter_map(|track| Some(((match_rank(track.language.as_deref()?, language)?, priority(track)), track)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, track)| track.combined_index)
}

fn match_rank(candidate: &str, preferred: &str) -> Option<u8> {
    let (candidate, preferred) = (canonical_tag(candidate)?, canonical_tag(preferred)?);
    if candidate == preferred {
        Some(0)
    } else if language_code(&candidate) != language_code(&preferred) {
        None
    } else if candidate.contains('-') {
        Some(2)
    } else {
        Some(1)
    }
}

/// Lower is better. Each weight is larger than all lower weights together: a track that
/// mpv shows itself beats a burn-in, a full track beats forced, plain beats SDH, and
/// embedded beats external beats downloaded.
fn priority(track: &Track) -> u8 {
    let burn_in = is_bitmap(track.codec.as_deref()) || track.delivery == "burn_in_only";
    let source = match track.source.as_str() {
        "external" => 1,
        "downloaded" => 2,
        _ => 0,
    };
    u8::from(burn_in) * 16 + u8::from(track.forced) * 8 + u8::from(track.hearing_impaired) * 4 + source
}

/// The web player shows no bitmap subtitles, so it ranks them as burn-in. The app keeps the
/// same rank, so that it picks the same track as the web player.
fn is_bitmap(codec: Option<&str>) -> bool {
    const BITMAP: [&str; 9] = [
        "pgs",
        "pgssub",
        "hdmv_pgs_subtitle",
        "dvd_subtitle",
        "dvdsub",
        "vobsub",
        "dvb_subtitle",
        "dvbsub",
        "dvb_teletext",
    ];
    codec.is_some_and(|codec| BITMAP.contains(&codec.to_lowercase().as_str()))
}

/// The language subtag of a tag, for example `en` for `en-US` or `eng`.
fn language_code(tag: &str) -> String {
    canonical_tag(tag).map(|tag| tag.split('-').next().unwrap_or_default().to_string()).unwrap_or_default()
}

/// The tag in the canonical form of `Intl.Locale`, which the web player compares: a
/// two-letter language when ISO 639-1 has one, `Script` and `REGION` case, and `-` as the
/// separator. Returns `None` when the text is not a language tag.
// ponytail: an alias table for the languages of the settings list, not full CLDR data. Use a
// BCP 47 crate if a track language outside the list must match.
fn canonical_tag(value: &str) -> Option<String> {
    let value = value.trim().to_lowercase();
    let value = match value.as_str() {
        "arabic" => "ar",
        "english" => "en",
        "spanish" => "es",
        "french" => "fr",
        "german" => "de",
        "portuguese" => "pt",
        "brazilian portuguese" | "portuguese (brazil)" => "pt-br",
        "chinese (traditional)" | "traditional chinese" => "zh-hant",
        other => other,
    };
    let mut parts = value.split(['-', '_']);
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let mut tag = iso_639_1(language).unwrap_or(language).to_string();
    for part in parts {
        tag.push('-');
        match part.len() {
            2 if part.bytes().all(|b| b.is_ascii_alphabetic()) => tag.push_str(&part.to_uppercase()),
            4 if part.bytes().all(|b| b.is_ascii_alphabetic()) => {
                tag.push_str(&part[..1].to_uppercase());
                tag.push_str(&part[1..]);
            }
            _ => tag.push_str(part),
        }
    }
    Some(tag)
}

/// The ISO 639-1 code of an ISO 639-2 code (both the T and the B form) or an old code, for
/// the languages of the settings list.
fn iso_639_1(code: &str) -> Option<&'static str> {
    Some(match code {
        "ara" => "ar",
        "ben" => "bn",
        "bul" => "bg",
        "zho" | "chi" => "zh",
        "hrv" => "hr",
        "ces" | "cze" => "cs",
        "dan" => "da",
        "nld" | "dut" => "nl",
        "eng" => "en",
        "fin" => "fi",
        "fra" | "fre" => "fr",
        "deu" | "ger" => "de",
        "ell" | "gre" => "el",
        "heb" | "iw" => "he",
        "hin" => "hi",
        "hun" => "hu",
        "ind" | "in" => "id",
        "ita" => "it",
        "jpn" => "ja",
        "kor" => "ko",
        "msa" | "may" => "ms",
        "nor" => "no",
        "nob" => "nb",
        "nno" => "nn",
        "fas" | "per" => "fa",
        "pol" => "pl",
        "por" => "pt",
        "ron" | "rum" | "mo" => "ro",
        "rus" => "ru",
        "slk" | "slo" => "sk",
        "slv" => "sl",
        "spa" => "es",
        "swe" => "sv",
        "tam" => "ta",
        "tel" => "te",
        "tha" => "th",
        "tur" => "tr",
        "ukr" => "uk",
        "vie" => "vi",
        _ => return None,
    })
}
