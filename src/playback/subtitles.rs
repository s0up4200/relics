//! The subtitle choice at the start of playback. It is a port of the web player rule
//! `resolveSubtitleAutoSelect` (silo-server web/src/player/utils/subtitleSort.ts), so a
//! Profile gets the same subtitles on each client. The Item detail names the track that the
//! web player remembers for the series (`effective_subtitle_track_signature`), and the choice
//! prefers it. The app does not save the choice of the Track picker. That choice carries only
//! to the next episode in the same run (`carried`).

use std::cmp::Reverse;

use super::tracks::Choice;
use crate::api::wire::{self, SubtitleInventoryItemV3 as Track, WatchSubtitleSignature as Signature};
use crate::settings::{ServerSettings, SubtitleMode};

/// The `playback.audio_language` values that mean "the original language of the Item".
const ORIGINAL: [&str; 2] = ["original", "x-silo-original"];

/// Returns the `combined_index` of the subtitle track to show, or `None` for no subtitles.
/// `audio_language` is the language of the audio track that plays.
pub fn choose(
    tracks: &[Track],
    settings: &ServerSettings,
    audio_language: Option<&str>,
    remembered: Option<&Signature>,
) -> Option<i64> {
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
        SubtitleMode::Always => remembered
            .and_then(|remembered| tracks.iter().find(|track| is_remembered(track, remembered)))
            .map(|track| track.combined_index)
            .or_else(|| best_match(tracks, preferred.filter(|p| !p.is_empty())?, remembered)),
        SubtitleMode::Auto => {
            if preferred == Some("") {
                return None;
            }
            if profile_lang.is_some() && same_language(audio_lang.as_deref(), profile_lang.as_deref()) {
                return forced();
            }
            match subtitle_lang.or(profile_lang) {
                Some(lang) => best_match(tracks, &lang, remembered),
                None => forced(),
            }
        }
    }
}

/// Returns the `combined_index` of the best track for a choice of the Track picker that carried over from the
/// last episode: a track with the same language and forced state.
pub fn carried(tracks: &[Track], choice: &Choice) -> Option<i64> {
    let matching = tracks.iter().filter(|track| choice.matches(track.language.as_deref(), track.forced));
    matching.min_by_key(|track| priority(track)).map(|track| track.combined_index)
}

/// The Server must draw `track` into the video, because it has no file that mpv can show.
pub fn needs_burn_in(track: &Track) -> bool {
    track.delivery == "burn_in_only" || track.url.is_none()
}

/// The `combined_index` of the subtitle track that the Server draws into the video of `plan`.
pub fn burned_in(plan: &wire::PlaybackPlan) -> Option<i64> {
    (plan.subtitle.mode == "burn_in").then(|| plan.selected_tracks.subtitle.as_ref()?.index).flatten()
}

fn normalize(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_lowercase()
}

/// The language in lower case, or `None` when it is empty or means the original language.
fn concrete(value: Option<&str>) -> Option<String> {
    let value = normalize(value);
    (!value.is_empty() && !ORIGINAL.contains(&value.as_str())).then_some(value)
}

pub fn same_language(a: Option<&str>, b: Option<&str>) -> bool {
    match (concrete(a), concrete(b)) {
        (Some(a), Some(b)) => language_code(&a) == language_code(&b),
        _ => false,
    }
}

/// The best forced track in `language`.
fn forced_track(tracks: &[Track], language: Option<&str>) -> Option<i64> {
    best_match(tracks.iter().filter(|track| track.forced), language?, None)
}

/// The best track for `language`: the exact tag, then the bare language, then another
/// variant of the language. Inside one of these ranks, the `score` against the remembered
/// track decides, then `priority`.
fn best_match<'a>(
    tracks: impl IntoIterator<Item = &'a Track>,
    language: &str,
    remembered: Option<&Signature>,
) -> Option<i64> {
    tracks
        .into_iter()
        .filter_map(|track| {
            let rank = match_rank(track.language.as_deref()?, language)?;
            Some(((rank, Reverse(score(track, remembered)), priority(track)), track))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, track)| track.combined_index)
}

/// The track is the remembered track: each field is the same, and the label too when the
/// remembered track has one.
fn is_remembered(track: &Track, remembered: &Signature) -> bool {
    let label = normalize(remembered.label.as_deref());
    same(Some(&track.source), remembered.source.as_deref())
        && same(track.language.as_deref(), remembered.language.as_deref())
        && same(track.codec.as_deref(), remembered.codec.as_deref())
        && (label.is_empty() || label == normalize(Some(&label_of(track))))
        && track.forced == remembered.forced
        && track.hearing_impaired == remembered.hearing_impaired
}

/// Higher is better: a sum of weights for the fields of the track that are the same as in
/// the remembered track. The source has the largest weight, then the forced and the hearing
/// impaired state, then the codec and the label. These are the weights of the web player.
fn score(track: &Track, remembered: Option<&Signature>) -> u8 {
    let Some(remembered) = remembered else {
        return 0;
    };
    u8::from(same(Some(&track.source), remembered.source.as_deref())) * 4
        + u8::from(track.forced == remembered.forced) * 2
        + u8::from(track.hearing_impaired == remembered.hearing_impaired) * 2
        + u8::from(same(track.codec.as_deref(), remembered.codec.as_deref()))
        + u8::from(same(Some(&label_of(track)), remembered.label.as_deref()))
}

/// The label that the web player shows and remembers for a track.
pub fn label_of(track: &Track) -> String {
    track
        .label
        .clone()
        .or_else(|| track.language.clone())
        .unwrap_or_else(|| format!("Track {}", track.combined_index + 1))
}

fn same(a: Option<&str>, b: Option<&str>) -> bool {
    normalize(a) == normalize(b)
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
