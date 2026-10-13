//! The subtitle choice at the start of playback. These are the cases of the web player
//! tests for `resolveSubtitleAutoSelect` and `findPreferredSubtitleIndex`
//! (silo-server web/src/player/utils/subtitleSort.test.ts). The web
//! `findPreferredSubtitleIndex` cases go through `always` mode, which uses the same match.

use relics::api::wire::SubtitleInventoryItemV3;
use relics::playback::subtitles::choose;
use relics::settings::{ServerSettings, SubtitleMode};
use serde_json::{Value, json};

/// A track of the plan subtitle inventory. `fields` replace the defaults.
fn sub(fields: Value) -> SubtitleInventoryItemV3 {
    let mut track = json!({
        "track_id": "t", "combined_index": 0, "default": false, "delivery": "sidecar", "source": "embedded",
        "forced": false, "hearing_impaired": false, "language": "en", "label": "English", "url": "/test",
    });
    track.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
    serde_json::from_value(track).unwrap()
}

fn tracks(fields: &[Value]) -> Vec<SubtitleInventoryItemV3> {
    fields.iter().cloned().map(sub).collect()
}

/// The settings of the web `opts` helper: no languages, and forced subtitles on.
fn settings(mode: SubtitleMode, subtitle: Option<&str>, profile: Option<&str>) -> ServerSettings {
    ServerSettings {
        subtitle_mode: mode,
        subtitle_language: subtitle.map(String::from),
        audio_language: profile.map(String::from),
        ..ServerSettings::default()
    }
}

fn no_forced(settings: ServerSettings) -> ServerSettings {
    ServerSettings { show_forced_subtitles: false, ..settings }
}

/// The web `findPreferredSubtitleIndex(tracks, language)`, with -1 as `None`.
fn preferred(fields: &[Value], language: &str) -> Option<i64> {
    choose(&tracks(fields), &settings(SubtitleMode::Always, Some(language), None), None)
}

use SubtitleMode::{Always, Auto, Off};

#[test]
fn an_exact_regional_tag_wins_over_the_generic_and_other_regions() {
    let t = [
        json!({"combined_index": 0, "language": "en-GB"}),
        json!({"combined_index": 1, "language": "en"}),
        json!({"combined_index": 2, "language": "en-US"}),
    ];
    assert_eq!(preferred(&t, "en-US"), Some(2));
}

#[test]
fn the_generic_language_comes_before_another_region() {
    let t = [json!({"combined_index": 0, "language": "en-GB"}), json!({"combined_index": 1, "language": "en"})];
    assert_eq!(preferred(&t, "en-US"), Some(1));
}

#[test]
fn embedded_beats_external_beats_downloaded_for_the_same_language() {
    for (better, worse) in [("embedded", "external"), ("external", "downloaded"), ("embedded", "downloaded")] {
        let t = [json!({"combined_index": 0, "source": worse}), json!({"combined_index": 1, "source": better})];
        assert_eq!(preferred(&t, "en"), Some(1), "{better} over {worse}");
    }
}

#[test]
fn the_choice_is_the_server_index_not_the_position_in_the_list() {
    let t = [json!({"combined_index": 2, "language": "en"}), json!({"combined_index": 4, "language": "es"})];
    assert_eq!(preferred(&t, "en"), Some(2));
    assert_eq!(preferred(&t, "es"), Some(4));
}

#[test]
fn iso_639_2_codes_of_the_inventory_match_the_setting() {
    let t = [json!({"combined_index": 0, "language": "fre"}), json!({"combined_index": 1, "language": "eng"})];
    assert_eq!(preferred(&t, "en"), Some(1));
    assert_eq!(preferred(&t, "fr"), Some(0));
}

// Off mode.

#[test]
fn off_shows_nothing_when_forced_subtitles_are_off() {
    let t = tracks(&[json!({"source": "external"})]);
    assert_eq!(choose(&t, &no_forced(settings(Off, Some("en"), None)), None), None);
}

#[test]
fn off_shows_a_forced_track_in_the_audio_language() {
    let t = tracks(&[
        json!({"combined_index": 0, "language": "en", "forced": true}),
        json!({"combined_index": 1, "language": "ja", "forced": true, "source": "external"}),
    ]);
    assert_eq!(choose(&t, &settings(Off, None, Some("en")), Some("ja")), Some(1));
}

#[test]
fn off_uses_the_source_order_among_forced_tracks() {
    let t = tracks(&[
        json!({"combined_index": 0, "forced": true, "source": "external"}),
        json!({"combined_index": 1, "forced": true, "source": "embedded"}),
    ]);
    assert_eq!(choose(&t, &settings(Off, None, Some("en")), Some("en")), Some(1));
}

// Always mode.

#[test]
fn always_picks_the_subtitle_language() {
    let t = tracks(&[
        json!({"combined_index": 0, "language": "es"}),
        json!({"combined_index": 1, "language": "en", "source": "external"}),
    ]);
    assert_eq!(choose(&t, &settings(Always, Some("en"), None), None), Some(1));
}

#[test]
fn always_shows_nothing_without_a_subtitle_language() {
    let t = tracks(&[json!({})]);
    assert_eq!(choose(&t, &settings(Always, None, None), None), None);
}

#[test]
fn always_shows_nothing_when_no_track_has_the_language() {
    let t = tracks(&[json!({"language": "es"})]);
    assert_eq!(choose(&t, &settings(Always, Some("en"), None), None), None);
}

#[test]
fn always_prefers_embedded_over_external() {
    let t = tracks(&[
        json!({"combined_index": 0, "source": "external"}),
        json!({"combined_index": 1, "source": "embedded"}),
    ]);
    assert_eq!(choose(&t, &settings(Always, Some("en"), None), None), Some(1));
}

// Auto mode.

#[test]
fn auto_shows_nothing_when_the_audio_is_the_profile_language_and_forced_is_off() {
    let t = tracks(&[json!({"source": "external"})]);
    assert_eq!(choose(&t, &no_forced(settings(Auto, Some("en"), Some("en"))), Some("en")), None);
}

#[test]
fn auto_shows_a_forced_track_when_the_audio_is_the_profile_language() {
    let t = tracks(&[
        json!({"combined_index": 0, "forced": true, "source": "external"}),
        json!({"combined_index": 1, "forced": true, "source": "embedded"}),
    ]);
    assert_eq!(choose(&t, &settings(Auto, Some("fr"), Some("en")), Some("en")), Some(1));
}

#[test]
fn auto_shows_subtitles_when_the_audio_is_another_language() {
    let t = tracks(&[json!({"combined_index": 0, "language": "en"}), json!({"combined_index": 1, "language": "ja"})]);
    assert_eq!(choose(&t, &settings(Auto, Some("en"), Some("en")), Some("ja")), Some(0));
}

#[test]
fn auto_with_original_spoken_language_shows_subtitles_for_foreign_audio_only() {
    for profile in ["original", "x-silo-original", "X-Silo-Original"] {
        let t = tracks(&[json!({"source": "external"})]);
        let settings = no_forced(settings(Auto, Some("en"), Some(profile)));
        assert_eq!(choose(&t, &settings, Some("ja")), Some(0), "{profile}");
        assert_eq!(choose(&t, &settings, Some("en")), None, "{profile}");
    }
}

#[test]
fn auto_falls_back_to_the_profile_language_without_a_subtitle_language() {
    let t = tracks(&[json!({})]);
    assert_eq!(choose(&t, &settings(Auto, None, Some("en")), Some("ja")), Some(0));
}

#[test]
fn auto_shows_nothing_when_the_subtitle_language_is_set_to_none() {
    let t = tracks(&[json!({})]);
    assert_eq!(choose(&t, &settings(Auto, Some(""), Some("en")), Some("ja")), None);
}

#[test]
fn auto_uses_the_profile_language_when_the_audio_language_is_unknown() {
    let t = tracks(&[json!({"forced": true})]);
    assert_eq!(choose(&t, &settings(Auto, Some("en"), Some("en")), None), Some(0));
}

#[test]
fn auto_takes_english_when_the_profile_has_no_language() {
    let t = tracks(&[json!({})]);
    assert_eq!(choose(&t, &settings(Auto, Some("en"), None), Some("ja")), Some(0));
}

#[test]
fn auto_treats_iso_639_1_and_639_2_codes_as_the_same_language() {
    let t = tracks(&[json!({"source": "external"})]);
    assert_eq!(choose(&t, &no_forced(settings(Auto, Some("en"), Some("en"))), Some("eng")), None);
}

#[test]
fn auto_shows_nothing_when_no_track_has_the_language() {
    let t = tracks(&[json!({"language": "fr"})]);
    assert_eq!(choose(&t, &settings(Auto, Some("en"), Some("en")), Some("ja")), None);
}

#[test]
fn auto_shows_nothing_when_no_forced_track_has_the_audio_language() {
    let t = tracks(&[
        json!({"combined_index": 0, "language": "fr", "forced": true}),
        json!({"combined_index": 1, "language": "es", "forced": true}),
    ]);
    assert_eq!(choose(&t, &settings(Auto, Some("en"), Some("ja")), Some("ja")), None);
}

#[test]
fn auto_takes_forced_tracks_only_in_the_audio_language() {
    let t = tracks(&[
        json!({"combined_index": 0, "language": "en", "forced": true}),
        json!({"combined_index": 1, "language": "fr", "forced": true}),
    ]);
    assert_eq!(choose(&t, &settings(Auto, Some("fr"), Some("en")), Some("en")), Some(0));
}

#[test]
fn no_tracks_means_no_subtitles_in_each_mode() {
    for mode in [Off, Auto, Always] {
        assert_eq!(choose(&[], &settings(mode, Some("en"), Some("en")), Some("ja")), None);
    }
}

// Bitmap tracks, forced and SDH tracks rank lower in the same language.

#[test]
fn a_text_track_beats_a_bitmap_track_of_the_same_language_and_source() {
    let t =
        [json!({"combined_index": 0, "codec": "hdmv_pgs_subtitle"}), json!({"combined_index": 1, "codec": "subrip"})];
    assert_eq!(preferred(&t, "en"), Some(1));
}

#[test]
fn an_external_text_track_beats_an_embedded_bitmap_track() {
    for codec in ["hdmv_pgs_subtitle", "pgs", "dvd_subtitle"] {
        let t = [
            json!({"combined_index": 0, "codec": codec}),
            json!({"combined_index": 1, "source": "external", "codec": "srt"}),
        ];
        assert_eq!(preferred(&t, "en"), Some(1), "{codec}");
    }
}

#[test]
fn embedded_text_beats_an_external_bitmap_file() {
    let t = [
        json!({"combined_index": 0, "source": "external", "codec": "pgs"}),
        json!({"combined_index": 1, "codec": "subrip"}),
    ];
    assert_eq!(preferred(&t, "en"), Some(1));
}

#[test]
fn a_full_external_track_beats_an_embedded_forced_or_sdh_track() {
    for flag in ["forced", "hearing_impaired"] {
        let t = [
            json!({"combined_index": 0, "codec": "subrip", flag: true}),
            json!({"combined_index": 1, "source": "external", "codec": "srt"}),
        ];
        assert_eq!(preferred(&t, "en"), Some(1), "{flag}");
    }
}

#[test]
fn a_burn_in_only_text_track_ranks_lower() {
    let t = [
        json!({"combined_index": 0, "codec": "subrip", "delivery": "burn_in_only"}),
        json!({"combined_index": 1, "source": "external", "codec": "srt"}),
    ];
    assert_eq!(preferred(&t, "en"), Some(1));
}

#[test]
fn a_bitmap_track_wins_when_it_is_the_only_match() {
    let t = [
        json!({"combined_index": 0, "language": "fr", "codec": "subrip"}),
        json!({"combined_index": 1, "language": "en", "codec": "pgs"}),
    ];
    assert_eq!(preferred(&t, "en"), Some(1));
}

#[test]
fn a_forced_bitmap_track_shows_in_off_mode() {
    let t = tracks(&[
        json!({"combined_index": 0, "codec": "subrip"}),
        json!({"combined_index": 1, "codec": "hdmv_pgs_subtitle", "forced": true}),
    ]);
    assert_eq!(choose(&t, &settings(Off, None, Some("en")), Some("en")), Some(1));
}
