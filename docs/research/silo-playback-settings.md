# Research: playback and language settings of the Silo web client

Date: 2026-10-16. Status: research only. No production code changed. Issue: [#127](https://github.com/s0up4200/relics/issues/127).

Relics shows eight playback Server settings. The Silo web client shows more of them, and it lets a library, a series or a device override some of them. This research lists each playback and language setting of the web client: its scopes, default and values, how the web reads and applies it, and what the server does with it. It ends with what Relics gets wrong today.

Terms in this file:

- Scope: the level where a stored value lives: `profile`, `profile_device`, `profile_library`, `profile_series`, `profile_client` or `account`.
- Effective read: `GET settings/values/effective`. The server resolves each key through the scopes of its `resolution_order`. It answers with the value and the scope that the value came from (`source`).
- Override: a stored value at a scope above `profile`.
- Household cap: a limit that a household manager or an administrator sets, for example `max_playback_quality` on a Profile. A household cap is not a Server setting.

## Sources

| Source | Version |
|---|---|
| silo-server checkout `~/github/oss/silo-server` | `b78c1ff663d10a1296a70317b98e0008de15343b`, 2026-10-11 |
| Its `origin/main` ref | 131 commits ahead (`ece16f8`, 2026-10-14). `git fetch --dry-run` shows that the remote moved again (`9cc3091`). |
| Settings contract | `contracts/settings/v1/manifest.json`, revision 17 |
| Relics | `main` at `235cd9a` |

The checkout has no Go server code. It has only `contracts/`, `docs/` and `web/`. All server behavior in this file comes from the docs, the OpenAPI spec, the settings contract and its conformance cases. The label "Not confirmed" marks a statement that no available source settles. Paths in this file are in the silo-server repo, unless they start with `relics:`.

## Summary

- The server is the only resolver of settings. A client must not apply its own precedence (`docs/architecture/settings-contract.md:83-87`).
- The server applies some settings itself. The client must apply the others:
  - Audio language: the server applies it. When the start request has no `audio_track_index`, the server picks the track from the series preference, the audio-language setting and the library override (`docs/architecture/playback-protocol-v3.md:203-211`). The web sends a track only when the user picked one.
  - Subtitles: the server resolves the language, the mode and the forced flag for the Item, with the device, library and series overrides. It returns them on the item detail and on the watch detail (`effective_subtitle_*`). The client picks the track and sends `subtitle_track_index`. The server does not pick a subtitle track at start.
  - Quality and bitrate: the client reads both settings and sends them as `quality_preference` and `bandwidth_cap_kbps`. No source says that the server reads them at start.
  - Intro, recap and credits skips, auto-play, seek intervals and subtitle appearance: only the client applies them.
  - Metadata language: only the server applies it. It changes the titles and descriptions in the catalog answers.
- Library overrides exist for four keys: `playback.audio_language`, `playback.subtitle_language`, `playback.subtitle_mode` and `playback.show_forced_subtitles`. Series overrides exist for the same four keys. The web edits library overrides on Settings, Libraries. It writes series overrides only when the user picks a subtitle in the player.
- Relics can read a value that the web Devices page sets for the Relics device. Relics sends its device id in `X-Silo-Device-Id`, and the effective read uses that id.
- Household cap: the effective read caps `playback.preferred_quality` at the `max_playback_quality` of the Profile, and reports `constrained: true`. Relics sends `original` and never reads the setting, so its request skips that cap. The server still keeps versions above the cap out of playback (`docs/playback-api.md:126`). Not confirmed: whether the planner also caps a transcode of an allowed version.

## How the web client reads and writes

- Every request of the web carries `X-Silo-Device-Id` (`web/src/api/client.ts:328`, `web/src/player/player-fetch.ts:71`). An effective read without that header drops the device scope (`contracts/settings/v1/conformance.json:339`). The server requires the header when a requested key allows `profile_device` (`docs/settings-api.md:933-939`).
- The effective read takes `keys`, `library_ids`, `series_ids`, `device_id` and `profile_id`, one parameter for each value (`docs/settings-api.md:109-123`, `contracts/api/v2/openapi.json:189458`). The web sends `library_ids` only on the Libraries page, and `device_id` only on the Devices page (`web/src/hooks/queries/settingValues.ts:180-206`).
- Each answer item has `value`, `source`, `scope`, the identity of the winning row and `suggested_values`. When a cap changed the value, it also has `constrained`, `constrained_by`, `constraint_kind`, `permitted_values` and `stored_value` (`contracts/api/v2/openapi.json:28621`).
- `POST settings/values/effective` resolves up to 100 content contexts (`{context_id, library_id | series_id}`) in one call. It takes no `device_id` or `profile_id` query (`docs/settings-api.md:173-193`). The web uses it for grids, not for playback.
- The item detail (`GET catalog/items/{id}`) and the watch detail (`GET watch/{id}`) carry the resolved playback answers for one Item. These are `effective_subtitle_language`, `effective_subtitle_mode`, `effective_show_forced_subtitles`, `effective_subtitle_track_signature`, and for each version `effective_audio_track_index` and `effective_audio_language`. Both reads use `X-Silo-Device-Id` (`docs/catalog-api.md:335-344`, `contracts/api/v2/openapi.json:22161`, `:47738`, `:206115`).
- The Settings pages write at `profile` scope. When the resolved value came from the device row of this browser, they also delete that row, so that the control does not snap back (`web/src/hooks/queries/profileDefaults.ts:41-79`, `web/src/hooks/queries/autoPlayNext.ts:39-68`).
- A write is `PUT settings/values/{key}?scope=...` with `{"value": ...}`. `library_id`, `series_id`, `device_id` and `profile_id` are query parameters (`web/src/hooks/queries/settingValues.ts:129-137`, `docs/settings-api.md:782-797`). The server answers 422 for a value outside the contract. It stores a value above a household cap, and applies the cap when it resolves the value (`docs/architecture/settings-contract.md:105-108`).

## Quality

| Key | Scopes, in resolution order | Default | Values |
|---|---|---|---|
| `playback.preferred_quality` | `profile_device`, `profile` | `auto` | `auto` "Auto", `480p`, `720p`, `1080p`, `2160p` "2160p / 4K", `original` "Original quality". The list is ordered. `auto` ranks lowest and `original` highest. The cap `max_playback_quality` constrains it as a ceiling. |
| `playback.max_bitrate_kbps` | `profile_device`, `profile` | `null` (no cap) | An integer from 100 to 200000, or null. |

Sources: `contracts/settings/v1/manifest.json:465-512`.

How the web reads them: the Playback settings page reads both with an effective read (`web/src/pages/settings/PlaybackSettings.tsx:80-98`). The player reads both in one effective read with the skip settings (`web/src/playback/WatchPlaybackChrome.tsx:602-616`, `:1084-1097`). The web offers 1.5, 2, 4, 6, 10, 15, 20, 40, 60, 100 and 200 Mbps. "No limit" deletes the rows (`web/src/lib/bitrateOptions.ts:20-22`, `web/src/pages/settings/PlaybackSettings.tsx:134-144`).

How the web applies them: the start and replan bodies carry `quality_preference` (the resolved value, or `auto`). They carry `bandwidth_cap_kbps` only when the value is not null (`web/src/player/playback-session-wire-v3.ts:93-123`, `:151-186`, `web/src/player/hooks/usePlaybackSession.ts:396`, `:690-703`). `original` and `auto` are different on the wire. For `original` the planner keeps the source. For `auto` it adapts to the device limits, the bandwidth and metered networks (`web/src/hooks/queries/qualityPreference.ts:1-33`).

What the server does:

- `quality_preference` is a required field of the v2 start body (`contracts/api/v2/openapi.json:38830`). The server changes an unknown value to `auto` and adds the warning `quality_preference_normalized` (`docs/architecture/playback-protocol-v3.md:1563-1567`). No doc says that the server reads `playback.preferred_quality` when it plans. So the client must send it.
- A replan without `quality_preference` keeps the quality of the session (`docs/architecture/playback-protocol-v3.md:976-981`).
- `bandwidth_cap_kbps` limits the selection, with the warning `bandwidth_cap_applied` (`docs/architecture/playback-protocol-v3.md:1240`). A compound rung keeps its resolution under a cap. `auto` and the plain labels use 80% of the cap as their budget (`docs/architecture/playback-protocol-v3.md:1534-1554`).
- The administrator bitrate limit for each stream location is a separate limit. The server applies it at start, and "a client's lower bandwidth preference still wins" (`docs/playback-api.md:151-177`).

## Intro, recap and credits skips, and auto-play

| Key | Scopes, in resolution order | Default | Values |
|---|---|---|---|
| `playback.intro_skip_mode` (revision 7) | `profile_device`, `profile` | `ask` | `never` "Never", `ask` "Ask to skip", `always` "Skip automatically" |
| `playback.auto_skip_intro` (deprecated) | `profile_device`, `profile` | `false` | Boolean. `true` is `always`, `false` is `ask`. |
| `playback.auto_skip_recap` | `profile_device`, `profile` | `false` | Boolean |
| `playback.auto_skip_credits` | `profile_device`, `profile` | `false` | Boolean |
| `playback.auto_play_next` | `profile_device`, `profile` | `true` | Boolean |
| `playback.auto_play_next_preview` | `profile_device`, `profile` | `false` | Boolean |

Sources: `contracts/settings/v1/manifest.json:514-600`.

- For one release, the server keeps `auto_skip_intro` and `intro_skip_mode` in step at the same scope (`docs/settings-api.md:825-868`). A client that shows the enum must not also show the boolean. The web shows the boolean only after the capability read proves that the server has no enum (`web/src/pages/settings/PlaybackSettings.tsx:443-447`).
- The player reads `intro_skip_mode`, `auto_skip_recap` and `auto_play_next_preview` with an effective read (`web/src/playback/WatchPlaybackChrome.tsx:602-616`, `:1113-1139`). Until the answer comes, the player does not prompt and does not skip.
- `never` shows no prompt. `ask` shows "Skip Intro". `always` skips and shows "Watch Intro" as an undo. The prompt shows for 5 s (`web/src/player/hooks/useIntroSkipPrompt.ts:5`, `web/src/player/components/VideoPlayer.tsx:1860-1875`).
- `auto_skip_recap` seeks to the end of the recap, one time for each marker (`web/src/player/components/VideoPlayer.tsx:1877-1898`).
- `auto_skip_credits`: the web shows it on the Playback page and on the Devices page, but the web player does not read it (`web/src/pages/settings/PlaybackSettings.tsx:681-694`). The player shows a skip button in the credits (`web/src/player/components/VideoPlayer.tsx:1842-1847`).
- `auto_play_next_preview`: the web label is "Start next at preview". When it is on, the 10 s countdown to the next episode starts at the preview marker, not only at the credits (`web/src/pages/settings/PlaybackSettings.tsx:711-724`, `web/src/player/components/VideoPlayer.tsx:1784-1789`, `web/src/player/utils/watchPageMarkers.ts:177-191`). The contract text says "Show a preview of the next episode while credits play" (`contracts/settings/v1/manifest.json:589-600`). The two texts do not agree. The web behavior is the one that users see.
- `auto_play_next` controls the countdown of the post-roll screen after the end of the file (`web/src/player/components/PlayingNextScreen.tsx:54`, `:80`). The countdown in the player during the credits does not read it (`web/src/player/hooks/useNextEpisode.ts:13-78`). The switch on the post-roll screen writes the profile row and deletes the device row (`web/src/hooks/queries/autoPlayNext.ts:39-68`).
- The server does nothing with these keys during playback. It supplies the Markers (`docs/playback-api.md:43-51`).

## Seek intervals

| Key | Scopes | Default | Values |
|---|---|---|---|
| `player.video_skip_back_seconds` (revision 9) | `profile` only | 10 | 5, 10, 15, 30, 45, 60, 90, with the labels "N seconds" |
| `player.video_skip_forward_seconds` (revision 9) | `profile` only | 30 | The same |

Sources: `contracts/settings/v1/manifest.json:160-253`, `docs/settings-api.md:1067-1097`.

- The client applies them to buttons, keys, gestures and media controls. Timestamp seeks, chapters, marker skips and the resume rewind do not use them (`docs/settings-api.md:1092-1097`).
- The web reads them with an effective read, after the capability read lists the keys (`web/src/hooks/queries/seekPreferences.ts:34-40`). The player uses one value for each direction (`web/src/player/components/VideoPlayer.tsx:1212`).
- They are not on the Devices page, because they have no device scope.

## Audio and subtitle language

| Key | Scopes, in resolution order | Default | Values |
|---|---|---|---|
| `playback.audio_language` | `profile_series`, `profile_library`, `profile_device`, `profile` | `null` "No preference" | A language tag. Suggested list: the option set `playback_audio_languages` (46 tags). `x-silo-original` means the original language of each Item. |
| `playback.subtitle_language` | The same | `null` "None" | A language tag. Suggested list: `playback_subtitle_languages`. |
| `playback.subtitle_mode` | The same | `auto` | `auto` "Auto", `always` "Always on", `off` "Off" |
| `playback.show_forced_subtitles` | The same | `true` | Boolean |

Sources: `contracts/settings/v1/manifest.json:349-438`, option sets at `:4-157`.

### Audio

- The server resolves `x-silo-original` against the `original_language` of the Item. When that is unknown, it uses the profile choice (`contracts/settings/v1/manifest.json:367`).
- The item detail and the watch detail carry `effective_audio_track_index` for each version (`docs/catalog-api.md:335-344`).
- The web start body has `audio_track_index` only when the user picked a track before play (`web/src/player/playback-session-wire-v3.ts:112-114`, `web/src/pages/watchRouteHelpers.ts:413`). In all other cases the server picks the track, "resolved from the series preference, then the profile's audio-language setting, then the library override" (`docs/architecture/playback-protocol-v3.md:203-211`).
- Not confirmed: the precedence that the server uses at start. The protocol doc lists series, then profile, then library. The contract resolves series, library, device, then profile (`contracts/settings/v1/manifest.json:353-359`). The api-contract doc says that playback resolves the canonical `profile_library` and `profile_series` rows (`docs/architecture/api-contract.md:1446-1480`). The "series preference" in the protocol doc can be the remembered track of `audio-prefs`, and not the setting. Only the Go code can settle this.
- The web does not call `audio-prefs`. An audio pick in the player sends a `track_change` replan (`web/src/player/hooks/usePlaybackSession.ts:1375`). Not confirmed: whether the server stores that pick as the series preference.

### Subtitles

- The server resolves the subtitle language, mode and forced flag for the Item. It uses the device scope (`X-Silo-Device-Id`) and the library and series overrides, and it returns the result as `effective_subtitle_*` (`contracts/api/v2/openapi.json:22161-22168`, `:47738-47745`, `docs/catalog-api.md:335-344`). Not confirmed from Go: that these fields include the library and series layers. The web depends on it. The player never sends `library_ids` or `series_ids` for subtitles, and the library editor says that its rows apply.
- The client picks the track. The web takes `effective_subtitle_*` from the watch detail, and falls back to the legacy profile fields (`web/src/pages/watchRouteHelpers.ts:287-296`). Then it runs `resolveSubtitleAutoSelect` for each version and sends the result as `subtitle_track_index` (`web/src/pages/watchRouteHelpers.ts:238-260`, `web/src/player/playback-session-wire-v3.ts:115-117`).
- `resolveSubtitleAutoSelect` works as follows (`web/src/player/utils/subtitleSort.ts:212-264`):
  - `off`: a forced track in the audio language when `show_forced_subtitles` is on. Otherwise no track.
  - `always`: the track that matches the remembered signature. Otherwise the best track in the preferred language. Otherwise no track.
  - `auto`: no track when the preferred language is the empty string. Only a forced track when the audio language is the profile language. Otherwise the best track in the preferred subtitle language, or in the profile language.
  - The profile language is the legacy `Profile.language` field. An empty value counts as `en`.
  - Inside one language, a track that needs no burn-in wins. Then a full track wins over a forced track, a plain track over SDH, and embedded over external over downloaded (`web/src/player/utils/subtitleSort.ts:43-52`).
- The subtitle inventory of the plan is the authoritative list. A client sends a `track_id` or a `combined_index` from it (`docs/architecture/playback-protocol-v3.md:1331-1366`). A start without a subtitle track plays with subtitles off. No doc says that the server picks a subtitle at start.
- A subtitle pick in the player writes the series override. The section "Library and series overrides" gives the details.

## Subtitle appearance

| Key | Scopes, in resolution order | Default |
|---|---|---|
| `playback.subtitle_appearance` | `profile_device`, `profile` | `{fontSize: "large", fontFamily: "sans-serif", fontColor: "#ffffff", backgroundColor: "#000000", backgroundStyle: "box", backgroundOpacity: 75, textOpacity: 100, textOutline: false, textOutlineColor: "#000000", position: "bottom"}` |

Sources: `contracts/settings/v1/manifest.json:440-463`, schema `contracts/settings/v1/schemas/subtitle-appearance.json`.

| Field | Contract range | Web labels | Web rendering |
|---|---|---|---|
| `fontSize` | `small`, `medium`, `large`, `xlarge`, `xxlarge` | Small to XX-Large | 20, 26, 32, 40 and 48 px at a reference height of 720 px. The size scales with the video height, with a minimum of 12 px. |
| `fontFamily` | Any family name, 1 to 64 characters, with no quotes, braces or separators | Sans-serif, Serif, Monospace | CSS `font-family`. The web parser keeps only its three generic names. It changes all other names to `sans-serif`. |
| `fontColor` | `#rrggbb` | 9 swatches | Text color |
| `textOpacity` (revision 14) | An integer from 1 to 100 | Slider | Alpha of the text color |
| `backgroundColor` | `#rrggbb` | 5 swatches | Box color |
| `backgroundStyle` | `box`, `shadow`, `outline`, `none` | Box, Drop Shadow, Outline, None | `box` draws a background with `backgroundOpacity`. `shadow` draws a black `2px 2px 4px` shadow. `outline` draws 8 hard shadows in `textOutlineColor`. |
| `backgroundOpacity` | An integer from 0 to 100 | Slider | Alpha of the box, only for `box` |
| `textOutline` | Boolean | Switch | Adds a 3 px glow in `textOutlineColor`, on top of the style |
| `textOutlineColor` | `#rrggbb` | Swatches | Color of the outline and the glow |
| `position` | `bottom`, `lower-third`, `top` | Bottom, Lower Third, Top | `bottom` is 7% of the player height above the bottom. `lower-third` is 12% and `top` is 7% of a 16:9 frame on the video. |

Sources: `contracts/settings/v1/schemas/subtitle-appearance.json:9-49`, `web/src/lib/subtitleAppearance.ts:6-62`, `:108-163`, `:170-283`.

- A stored value is sparse. A client merges the fields that are present over the default. A device value replaces the profile value, and the two values do not merge (`contracts/settings/v1/schemas/subtitle-appearance.json:5`, `contracts/settings/v1/conformance.json:737`).
- The web writes it only at `profile_device`, for its own browser. It writes from the Subtitles settings page and from the panel in the player (`web/src/hooks/queries/subtitleAppearance.ts:30-59`). So on the web the value belongs to one device. A profile value comes only from other clients or from the migration.
- The server does not use it. The old endpoints `settings/subtitle-appearance/effective` and `settings/device/subtitle-appearance` still exist (`docs/settings-api.md:330-339`). The canonical key is the current path.

## Metadata language

| Key | Scopes | Default | Values |
|---|---|---|---|
| `catalog.metadata_language` | `profile` only | `null` "Library default" | A language tag. Suggested list: `catalog_metadata_languages`. `x-silo-original` (revision 3) means the original language of each Item. |
| `catalog.metadata_language_overrides` (revision 3) | `profile` only | `{}` | An object. Each key is the canonical ISO 639 code of the original language of an Item (`^[a-z]{2,3}$`). Each value is a BCP 47 tag, or `x-silo-original`. The object has 256 entries at most. |

Sources: `contracts/settings/v1/manifest.json:617-649`, `contracts/settings/v1/schemas/metadata-language-overrides.json:1-20`.

- An example value is `{"ja": "en", "no": "x-silo-original"}`. The server resolves it as one object (`contracts/settings/v1/conformance.json:642-661`). The web calls the entries "exceptions", for example "for items in Japanese, show English" (`web/src/components/settings/MetadataLanguageSetting.tsx:99-186`). An empty map deletes the row (`web/src/pages/settings/PlaybackSettings.tsx:502-511`).
- The server applies both keys. They change the localized titles, descriptions and artwork in the catalog answers. On the catalog reads, `library_id` picks the metadata language of that library as the fallback (`docs/catalog-api.md:320-326`). Not confirmed from Go: the exact rule that joins the profile value, the exceptions and the library value.
- The answers that change are item details, Home sections, library rows, people, collections, episodes, calendar, shuffle cards and search. This list comes from the web cache refresh in commit `7f5c607` on `origin/main`, which is not in the checkout. After a change, the web cancels and fetches these reads again.
- The client does nothing with the value, except to fetch the catalog again.

## Library and series overrides

| Override | Keys | Where the web edits it | How the web writes it |
|---|---|---|---|
| Library (`profile_library`) | `playback.audio_language`, `playback.subtitle_language`, `playback.subtitle_mode`, `playback.show_forced_subtitles` | Settings, Libraries. Each library row has "Edit playback overrides", which opens a panel in the row with Spoken language, Subtitle language, Subtitle behavior and Forced subtitles. Each field has "Profile default" (inherit). Each change saves at once. | `PUT` or `DELETE settings/values/{key}?scope=profile_library&library_id=N`. Inherit is a delete. "None" stores `null`. |
| Series (`profile_series`) | The contract allows the same four keys. The web writes only `playback.subtitle_language` and `playback.subtitle_mode`. | No settings page. A subtitle pick in the player writes it. "Auto" in the Subs picker on the item page deletes it. | `PUT settings/values/{key}?scope=profile_series&series_id=<series id>`, and `PUT subtitle-prefs/{series_id}` for the track index and signature. A movie uses its own content id as `series_id`. |

Sources: `web/src/pages/settings/libraryPlaybackPreferences.ts:17-27`, `:127-158`, `web/src/pages/settings/LibrarySettings.tsx:267-270`, `:391`, `:405-471`, `web/src/player/utils/subtitleChoicePersistence.ts:48-153`, `web/src/lib/seriesSubtitleSettings.ts:3-23`, `web/src/player/components/WatchPage.tsx:395-399`, `web/src/hooks/queries/subtitles.ts:176-266`.

- The web player does not write `show_forced_subtitles` at series scope. A series row would keep the default above the profile switch (`web/src/lib/seriesSubtitleSettings.ts:12-18`).
- The pre-play Subs picker on the item page writes only `subtitle-prefs/{series_id}` (`web/src/hooks/queries/subtitles.ts:237-266`). The v2 handlers of these routes keep the canonical rows in sync, but the docs do not list which rows (`docs/architecture/api-contract.md:1446-1453`). Not confirmed.
- An administrator can see and edit every stored row at every scope, also series rows, on the preferences page of an admin user (`web/src/pages/admin-users/detail/preferences/levels.ts:140-279`).
- When all four layers have a value, series wins over library, library over device, and device over profile (`contracts/settings/v1/conformance.json:207-300`).
- Not confirmed: which library override applies to an Item that is in two libraries. The web sends the `library_id` of the route with the watch detail.

## Device overrides and the Devices page

- Settings, Devices lists the devices of the Profile (`GET devices`). The server registers a device when the device sends `X-Silo-Device-Id` on an effective read, a playback start, a device write or a download (`docs/settings-api.md:941-963`). Relics sends the header on every call, so Relics is in the list after its first effective read.
- For a selected device, the page reads `GET settings/values/effective?keys=...&device_id=<id>`. It writes `PUT settings/values/{key}?scope=profile_device&device_id=<id>` (`web/src/pages/settings/DeviceSettings.tsx:274-293`, `:476-505`). The page says "These apply to this device, for your profile only", and "This device picks up your changes the next time it's used" (`:425-435`).
- So a value that the web sets for the Relics device is a row at `profile_device` with the Relics device id. The Relics effective read sends the same id in `X-Silo-Device-Id`, and the server includes that row (`contracts/api/v2/openapi.json:189458`). Relics can read it.
- The page shows each key that allows `profile_device` (`web/src/lib/settingsDisplay.ts:141-150`), in these groups (`web/src/lib/deviceSettingGroups.ts:44-73`):
  - Picture: `playback.preferred_quality`, `playback.max_bitrate_kbps`, HDR, Dolby Vision, frame rate and other `player.*` keys.
  - Sound: `playback.audio_language`, theme music, audio sync and playback speed.
  - Subtitles: `playback.subtitle_language`, `playback.subtitle_mode`, `playback.show_forced_subtitles`, `playback.subtitle_appearance` (in a panel) and subtitle sync.
  - Episodes: `playback.intro_skip_mode`, `playback.auto_skip_credits`, `playback.auto_skip_recap`, `playback.auto_play_next`, `playback.auto_play_next_preview`, `playback.next_up_prompt_seconds` and the sleep timer.
- The page hides keys that the manifest tags for other platforms. Relics sends `X-Silo-Device-Platform: linux`. The web has no mapping for `linux`, so the page shows every key for Relics (`web/src/lib/deviceSettingGroups.ts:136-164`).
- "Use profile settings" deletes all rows of the device (`DELETE devices/{device_id}/settings`). "Forget" deletes the device (`DELETE devices/{device_id}`) (`web/src/hooks/queries/devices.ts:48-49`).

## Household caps

- `max_playback_quality` is a field of the Profile (`1080p` or `2160p`, empty for no cap), of the account and of access groups. A household manager or an administrator sets it. It is not a Server setting (`contracts/api/v2/openapi.json:39921`, `contracts/settings/v1/manifest.json:491`).
- `playback.preferred_quality` declares `constrained_by: {policy_input: "max_playback_quality", constraint: "ceiling"}` (`contracts/settings/v1/manifest.json:482-486`). The effective read then answers the capped value, with `constrained: true`, `constraint_kind: "ceiling"` and `stored_value`. A stored `2160p` under a `1080p` cap reads as `1080p`. A stored `original` under a `2160p` cap reads as `2160p`. The cap never changes `auto` (`contracts/settings/v1/conformance.json:422-503`). `permitted_values` lists the values that the cap allows.
- A client uses the resolved value. It can show the stored value and the cap from `stored_value` and `permitted_values` (`docs/architecture/settings-contract.md:96-108`).
- At playback, the server does not play a version above the cap, and does not use one as a fallback version (`docs/playback-api.md:120-127`, `docs/playback-api.md:330-333`).
- Not confirmed: whether the planner also caps the transcode or the original of an allowed version, when a start sends `original` above the cap. The manifest note says that the cap code did not rank `original` when the contract was written (`contracts/settings/v1/manifest.json:491`).
- `playback.max_bitrate_kbps` has no `constrained_by` in the manifest. The conformance cases test a bitrate ceiling only with a binding in the fixture (`contracts/settings/v1/conformance.json:504-572`). The server applies the administrator bitrate limit for each stream location at start (`docs/playback-api.md:151-177`).
- `catalog.metadata_language` has no constraint (`contracts/settings/v1/manifest.json:631`).

## What Relics gets wrong today

All five points of the issue are true.

| Relics behavior | Effect for the user | Relics source |
|---|---|---|
| The effective read sends only `keys=` for 8 keys, with `X-Silo-Device-Id`. | Device overrides apply. Library and series overrides never apply to the subtitle choice. For example, a user sets "Japanese subtitles always on" for an anime library, or picks a track in a series on the web. Relics then uses the profile choice. | `relics:src/api/mod.rs:376-378`, `relics:src/settings.rs:13-23` |
| The read asks for `playback.intro_skip_mode` with no capability read first. | A server before revision 7 answers 422 for the whole read. Then all eight settings use their defaults. | `relics:src/api/mod.rs:376` |
| Writes use only `?scope=profile`. | When a device row exists for the Relics device, for example from the web Devices page, the write succeeds but the effective value does not change. Relics shows the new value until the next read, and then shows the device value again. The web deletes the device row in this case. | `relics:src/api/mod.rs:388-404` |
| The start and replan bodies send `quality_preference: "original"`. A Quality ladder replan sends the picked step. | `playback.preferred_quality` and its device override have no effect. The household cap on the setting has no effect on the request. A Profile set to `auto` or `1080p` on a remote network always asks for the source. | `relics:src/api/mod.rs:1324`, `:538` |
| The start body has no `bandwidth_cap_kbps`. | `playback.max_bitrate_kbps` has no effect. | `relics:src/api/mod.rs:1273-1290` |
| Relics picks subtitles after the start, with a port of `resolveSubtitleAutoSelect` on the profile values, and selects the track in mpv. It sends no `subtitle_track_index`. | Library and series overrides are lost, as above. Relics decodes `effective_subtitle_*` on the item detail, but does not use it. A bitmap track that needs burn-in shows no subtitles. Relics uses `playback.audio_language` as the profile language, and the web uses the legacy `Profile.language`. These two usually have the same value. | `relics:src/playback/subtitles.rs:14-47`, `relics:src/playback.rs:720-745`, `relics:src/api/wire.rs:690-700` |
| Left and Right seek 10 s in both directions. | `player.video_skip_back_seconds` (default 10) and `player.video_skip_forward_seconds` (default 30) have no effect. Forward is 10 s, and the web default is 30 s. | `relics:src/player_view.rs:35-36`, `:313-314` |
| The start body has no `audio_track_index`. | This is correct. The server picks the track with its own rules. | `relics:src/api/mod.rs:1273-1290` |
| `auto_skip_credits` skips credits that do not run to the end, and advances at the end. | The web player ignores this key. Relics follows the contract text, so the two clients differ. | `relics:src/playback.rs:909-918` |
| Relics does not read `playback.preferred_quality`, `playback.max_bitrate_kbps`, `playback.auto_play_next_preview`, the two seek intervals, `playback.subtitle_appearance`, `catalog.metadata_language` or `catalog.metadata_language_overrides`. | These settings do nothing in Relics. The exception is the metadata language, because the server applies it to every answer. | `relics:src/settings.rs:13-23` |

Design notes from the sources, for the implementation issue:

- For subtitles, read `effective_subtitle_language`, `effective_subtitle_mode` and `effective_show_forced_subtitles` from `GET catalog/items/{id}` with `X-Silo-Device-Id`. `GET watch/{id}` has the same fields, but it is not in `relics:api/operations.txt` yet. Give these values to the existing port. Then the server resolves the library and series layers, and the client keeps only the track choice.
- Send the resolved `playback.preferred_quality` as `quality_preference`. Send `playback.max_bitrate_kbps` as `bandwidth_cap_kbps` when it is not null. Do this on the start and on each replan.
- A write from the Relics Settings page needs a scope. To match the web, write `profile`, and delete the `profile_device` row of this device when the resolved `source` is `profile_device`. Write `playback.subtitle_appearance` at `profile_device`, as the web does.
- Before you read or write a key, make sure that the server has it: `GET settings/contract/capabilities` must report `manifest_revision >= introduced_in`. The web does the same (`docs/settings-api.md:101-107`).
- Read `source`, `constrained` and `stored_value`, so that the Settings page can show "Set on this device" or "Limited by the household".

## Changes on origin/main

The checkout is 131 commits behind its own `origin/main` ref. I compared the files that this research cites:

- `contracts/settings/v1/manifest.json` did not change.
- `docs/architecture/playback-protocol-v3.md`: when the server picks the default audio track, it skips tracks that FFmpeg cannot decode. A selected track that FFmpeg cannot decode ends with `audio_conversion_unsupported`.
- `web/src/player/utils/subtitleSort.ts`: a new rule puts text tracks before PGS tracks, which the web now draws itself. The burn-in rank now uses `subtitleNeedsBurnIn`, and not every bitmap codec.
- `web/src/pages/settings/PlaybackSettings.tsx` and the other Settings pages have a new layout. The subtitles page is now "Language & Subtitles". I did not read the new layout.
- The remote moved again (`9cc3091`). I did not fetch it.

## Limits

- No Go server code was available. Each server behavior comes from the docs, the OpenAPI spec and the contract. The items marked "Not confirmed" need the server source.
- I sent no request to a server, and I did not run the web client.
- I read the web client at the checkout commit. A given server can run another version.
