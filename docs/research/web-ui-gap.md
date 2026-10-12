# Web UI compared with the native app

This file lists what the Silo web UI shows, and what the native app (silo-omarchy) does not show yet.
The survey used the web UI at s.brr.tf on 2026-10-12, with an administrator account. The survey was read-only.

Terms in this file:

- Wire field: a field in `src/api/wire.rs`. The generator made the types in that file from the OpenAPI spec.
- Op list: `api/operations.txt`. It lists the API operations that the generator makes types for.
- Not in wire: the web calls the endpoint, but `wire.rs` has no type for it. To use the endpoint, add the
  operation to the op list and run the generator again.

The player test is not complete. The 4K Dolby Vision stream did not load in Chrome. The server sent
503 to `playback/route-events` and to `playback/{id}/progress`. The player section comes from the
menus that opened while the video loaded, and from the strings in the web JavaScript bundle.

## Sidebar and navigation

The web sidebar has these entries:

- Home.
- Libraries: one entry for each library (Movies, Movies (4K), TV, TV (4K), Anime, and more).
- Discover: Search (Ctrl K), For You (`/recommendations`), Requests, Calendar, and Notifications with an unread count.
- Your Stuff: Favorites, Watchlist, Watch Party (`/rooms`), Collections, History.
- The user menu (profile name) and Settings.

The native app has no For You, Requests, Calendar, Notifications, Favorites, Watch Party,
Collections or History pages.

## Home page

The web shows these parts:

- A Featured hero carousel. It has a "FEATURED" label with a rank ("NO. 01"), the logo or the title, the year,
  the TMDB rating, the genres, the content rating, the overview, Play, and More Info. A pager shows "01" to "20"
  and has a pause button.
- Rows of cards. A poster card shows a quality badge (1080P, 4K, 4K DV, 4K HDR), the airing status of a
  series (RETURNING, ENDED), the network (HBO, NETFLIX), the title, and "2022 SERIES" or the year.
- Continue Watching cards with "Season 1 Episode 1 • Freedom Day" and "53 min left".
- On hover, a card shows a play button and a menu (three dots).

Endpoints: `GET home/sections/{id}/items`, `GET settings/overlay-config`.

| Web shows | Native app | API |
|---|---|---|
| Quality badge on each card | No | Wire field `CatalogItem.overlay_summary` (resolution, hdr, video_codec, audio, edition) |
| Airing status and network on a series card | No | Wire fields `show_status`, `networks` |
| TMDB rating and genres in the hero | Partly | Wire field `rating_tmdb` |
| "53 min left" on Continue Watching | No, a progress bar only | Wire fields `position_seconds`, `duration_seconds` |
| Logo artwork in place of the title | No | Wire field `logo_url` |
| Hero pager and auto-advance | No | Client only |
| Play button and menu on a card | No | Client only |

## Movie detail page

The example is Young Frankenstein (`/item/movie-tmdb-3034`). The web shows these parts, from top to bottom:

- A back button, the backdrop, and the poster.
- "MOVIE · CROSSBOW PRODUCTIONS", which is the type and the first studio.
- The title logo, an image. Screen readers get the title as text.
- The tagline, in italics.
- Chips: year, content rating, runtime (1H 46M), resolution (1080P), audio codec (EAC3).
- The ratings: TMDB 7.9. An administrator can turn on more sources, for example Rotten Tomatoes.
- The overview.
- "Directed by Mel Brooks · Written by Mel Brooks, Gene Wilder · Comedy". Each name is a link to the person.
- Buttons: Play, Mark Watched, Add to favorites (a heart), a rating of 1 to 5 stars, and More actions (three dots).
- The More actions menu: Add to Watchlist, Add to Collection, Download, Add Subtitles, "Start a party with this"
  (Watch Together), and Edit Markers. The code has more entries that show for some users or items:
  Shuffle, Play from Beginning, Request Seasons, Suggest to, Play in, Media Info, View Play History,
  Refresh Metadata, Re-detect Credits, Re-detect Intro Markers, Re-detect Markers, Seek Previews,
  Edit Metadata, Match Item, Split Versions.
- An Audio picker. The button shows "Auto: English · EAC3 · 5.1". Each row has the language, a codec
  badge, a channels badge, a DEFAULT badge, and a second line, for example
  "E-AC-3 5.1 · 5.1(side) · 640 kbps · 48,000 Hz". The second line is the codec name, the layout, the bitrate, and the sample rate.
- A Subs picker. The button shows "Auto: English · SRT". The rows are Auto (with the language it resolves to),
  Off, and then the groups "Embedded" and "External". The player also has a "Downloaded" group. Each row has the language,
  a format badge (PGS, SRT), and the track title, for example "English (SDH) (PGS)".
- A Version picker when the item has more than one file. The episode page below shows it.
- Trailers & More: a row of video cards. Each card has a thumbnail, the title, the kind (Trailer, Teaser,
  Featurette, Extra), and an "Official" label.
- Cast: photo, name, and character. Each card is a link to the person page.
- Crew: groups by job (Directors, Writers, Producers), with links.
- Chapters: the code has a Chapters section with a count and a sort button. This movie had no chapters.
- More Like This: a carousel of similar titles.

Endpoints: `GET catalog/items/{id}`, `GET recommendations/similar/{id}?limit=12`,
`GET catalog/people/{id}?prefetch=true` (one call for each cast member), `GET subtitles/providers/status`,
`GET capabilities/metadata-ai`, `GET catalog/search/capabilities`, `GET subtitles/{id}`.

## Series, season and episode pages

The series page (`/item/series-tvdb-403245`, Silo) has these parts:

- "SERIES · APPLE TV", which is the type and the network. Then the title logo and the tagline.
- Chips: the year range (2023–2026), the content rating, "3 SEASONS", and "30 EPISODES".
- The TMDB rating, the overview, and the genres.
- Buttons: Resume, Mark Series Watched, favorite, star rating, and More actions.
- Seasons: a grid of season poster cards, with "3 total". Each card shows the title and "0 of 10 episodes"
  (the watched count) or "10 episodes". A card opens a separate season page.
- Trailers & More, Cast, Crew (Producers), and More Like This.
- The series page has no Audio, Subs or Version picker.

The season page (`/item/season-tvdb-403245-1`) has these parts:

- A breadcrumb "Silo > Season 1", the series title, chips (year, SEASON 1, 10 EPISODES), and the season overview.
- Buttons: Play First Episode, Mark Season Watched, and More (Shuffle, "Start a party with this").
- Episodes, with "10 total". Each card has the still, a quality badge (4K DV), a progress bar, "Episode 1",
  the title, the runtime, the air date ("May 5, 2023"), and two lines of the overview.

The episode page (`/item/episode-tvdb-403245-1-1`) has these parts:

- A breadcrumb "Silo > Season 1 > Episode 1" and the title.
- Chips: runtime, air date, resolution (2160P), HDR type (DV HDR10), and audio codec (EAC3).
- The TMDB rating and the overview.
- Buttons: Resume, Mark Watched, and More (Play from Beginning, Download, Add Subtitles, "Start a party with this", Edit Markers).
- A Version picker: "2160p · HEVC · DV HDR10 · EAC3". Each row has the label, an HDR badge, and the
  file size ("10.6 GB", "4.5 GB"). A check mark shows the selected version.
- Audio and Subs pickers, the same as on the movie page.
- More Episodes: a row of the episodes of the season. The current episode has a "NOW VIEWING" label.

Endpoints: `GET catalog/series/{id}/seasons`, `GET catalog/items/{season}/episodes`,
`GET catalog/series/{id}/seasons/{n}`, `GET catalog/series/{id}/seasons/{n}/episodes`.

## Detail page gaps

The native detail page (`src/detail.rs`) shows the backdrop, the poster, the type, the title, and one metadata line.
The line has the year, the runtime or season count, the content rating, and up to 3 genres.
The page also shows quality badges (resolution, HDR, video codec), the overview (3 lines), Play or Resume,
Start over, Watchlist, season pills, an episode grid (still, progress, "N. Title", runtime), and the cast of a movie.

| Web shows | Native app | API |
|---|---|---|
| Tagline | No | Wire field `tagline` |
| Title logo | No | Wire field `logo_url` |
| Studio or network next to the type | No | Wire fields `studios`, `networks` |
| Ratings (TMDB, IMDb, Rotten Tomatoes) | No | Wire fields `ratings` (name and display text, in display order), `rating_tmdb`, `rating_imdb`, `rating_rt_*` |
| Your star rating (1 to 5) | No | Wire field `user_rating` for reading. The op list has no write operation |
| Favorite (heart) | No | Wire field `user_state.is_favorite` for reading. The op list has no write operation |
| Mark Watched, Mark Season Watched, Mark Series Watched | No | Wire field `user_state.played`. The op list has no write operation |
| Audio codec chip in the hero | No | Wire field `overlay_summary.audio` |
| Directed by and Written by line | No | Wire field `crew` (job, name, person_id) |
| Crew section | No | Wire field `crew` |
| Cast on a series | No, movies only | Wire field `cast`, which a series also has |
| Trailers & More | No | Wire field `videos` (kind, name, site, site_key, is_official), and `extras` for local extras |
| Chapters | No | Wire field `FileVersion.chapters` (title, start, end, thumbnail_url). The app already calls `listCatalogItemVersions` |
| More Like This | No | Not in wire (`GET recommendations/similar/{id}`) |
| Audio picker before play, with codec, channels, layout, bitrate, sample rate, DEFAULT | No | Wire fields `FileVersion.audio_tracks` (codec, channels, layout, bitrate, sample_rate, default, language, title) and `effective_audio_track_index` |
| Subtitle picker before play, with Embedded and External groups, format, forced, SDH | No | Wire fields `FileVersion.subtitle_tracks` (language, codec, forced, hearing_impaired, external, file_name, default, title) and `effective_subtitle_*` |
| Version picker with file size and HDR type | No, the app plays the first version | Wire field `versions` with `FileVersion` (resolution, codec_video, codec_audio, hdr, file_size, bitrate, edition) |
| Dolby Vision label ("DV HDR10") | No, the app shows "HDR" | Wire fields `VideoTrack.dolby_vision`, `video_range_type`, `hdr10_plus` |
| Air date on an episode card | No | Wire field `Episode.air_date` |
| Episode overview on the card | No | Wire field `Episode.overview` |
| Quality badge on an episode card | No | Wire field `Episode.overlay_summary` |
| Season poster cards with the watched count | No, text pills | Wire fields `Season.poster_url`, `episode_count`, `user_data.watched_count` |
| Season overview and season page | No | Wire field `Season.overview` |
| Year range and episode count of a series | No | Wire fields `first_air_date`, `last_air_date`, `episode_count` |
| Breadcrumb (series, season, episode) | No | Wire fields `series_title`, `season_number` |
| Episode detail page with More Episodes | No, Enter plays the episode | `getCatalogItem` accepts an episode id |
| Media Info (see below) | No | Wire types `FileVersion`, `VideoTrack`, `AudioTrack`, `VersionSubtitleTrack` |
| Add to Collection, Download, Add Subtitles, Watch Party, Edit Markers, admin actions | No | Not in the op list |

Media Info is an entry in the More actions menu. It did not show for this account. The bundle code lists these fields:

- File: container, file size, duration, overall bitrate, edition, date added, file path. The API sends `file_path` only to viewers who can see paths.
- Video: codec, profile, level, resolution, aspect ratio, frame rate, bitrate, bit depth, pixel format,
  chroma subsampling, dynamic range, color primaries, color transfer, color space, reference frames, scan type.
- Audio: title, language, codec, layout, channels, sample rate, default.
- Subtitle: title, language, format, source (Embedded or External), file, resolution, forced, default, hearing impaired.

## Person page

The example is Gene Wilder (`/person/128410352714055685`).

The web shows the photo, the name, Refresh metadata (for an administrator), "BORN JUN 11, 1933",
"DIED AUG 29, 2016 (AGE 83)", the birthplace, the full biography, and a Filmography grid with the tabs
All, Movies and Series. Each card has a quality badge, the title, and the year.

Endpoint: `GET catalog/people/{id}`.

The native app opens a grid of the titles of the person (`listPeople` and `queryCatalogItems`).
The grid has no photo, dates, birthplace or biography, and no Movies or Series tabs.

The op list has `getPerson`, and the wire type `Person` has `bio`, `birth_date`, `death_date`,
`birthplace`, `homepage` and `photo_url`. The client has no method that calls `getPerson` yet.

## Search

The web shows these parts:

- A search field, and the tabs Media, Audiobooks and All.
- A type picker (Movies & Series), a sort field (Date Added), a direction (Descending), and Filters.
- A People row with a photo or initials, and the name.
- A grid of titles. Each card has a quality badge, the title, and "2020 · SERIES" or the year.
- A "Request to add" section with results that are not in the library.

Endpoints: `POST catalog/query`, `GET catalog/people?q=…&limit=20&media_scope=video`,
`GET requests/search?q=…&media_type=all&page=1`, `GET capabilities/ratings`.

The native search has no sort or filter controls, no Media, Audiobooks or All tabs, and no
"Request to add" section. The request search endpoint is not in wire.

## Library page

The web has three tabs: Recommended, Library and Collections. Recommended shows the same rows as Home,
with a Featured hero. Library shows the grid. A Shuffle button plays a random title.

The Library grid has these controls:

- Sort field: Title, Date Added, Release Date, Year, Content Rating, Duration, IMDb Rating, TMDB Rating,
  Resolution, Bitrate, Progress, Date Viewed, Plays. Direction: Ascending or Descending.
- Filters panel in Guided mode: Genres (match all), Decade, Year From, Year To, Minimum IMDb Rating,
  Content Rating, Original Language, Actor, Director, Writer, Producer, Studio, Network, Country,
  Match Status, Watch Status, Video Quality (4K, HDR, DOVI), Added in the Last (for example 30d, 2w, 6m),
  and Released in the Last (for example 90d, 1y). Buttons: Clear All, Done.
- Filters panel in Advanced mode: rule groups with "Match ALL" or "ANY", Add Group, and a sort.

Endpoints: `GET library/{id}/layout`, `GET library/{id}/sections/{id}/items`, `POST catalog/query`,
and `PUT settings/values/ui.library_page_state?scope=profile_device`. The web uses the last call to save the open tab.

The native app has the Recommended and Library tabs, a sort (Title, Date added, Release date, IMDb rating,
Duration, Date viewed), a direction, filters (watch status, 4K, HDR, decade, genre), an A–Z jump, and a
Series or Episodes mode.

| Web has | Native app | API |
|---|---|---|
| Sort by Year, Content Rating, TMDB Rating, Resolution, Bitrate, Progress, Plays | No | `CatalogQuery` sort fields. Make sure that the server accepts each sort key |
| Year range, minimum rating, content rating, language, people, studio, network, country, Dolby Vision, "added in the last", "released in the last" | No | `getCatalogFilters` (in the op list) and `CatalogQuery` rules |
| Advanced rule groups | No | `CatalogQuery` |
| Collections tab | No | Not in the op list |
| Shuffle | No | Not in the op list |

## Video player

The web player has these parts:

- At the top: Minimize player, Exit, "Silo (2023)", and "S1:E1 · Freedom Day".
- In the center: Back 10 seconds, Play or Pause, Forward 30 seconds, and Next episode.
- A seek bar with the time ("6:06 / 59:23"), thumbnails while you seek (trickplay), and chapter marks.
- On the right: a captions button, a Quality button, More player options, and Fullscreen.
- The Quality menu has two groups. VERSION lists each file, for example "2160p HEVC HDR" (Playing) and "1080p H264".
  QUALITY lists Auto (Selected), Original 25.6 Mbps, 1080p 11.0 Mbps, 1080p High 10 Mbps,
  1080p Medium 6 Mbps, 1080p Low 3 Mbps, 720p High 4 Mbps, 720p Medium 2 Mbps,
  720p Low 1.5 Mbps, and 480p 1.5 Mbps.
- The captions menu (from the bundle) has Off, the groups External, Embedded and Downloaded, a subtitle Delay
  with Reset, "Add Subtitles…" (an online search by language), "Translate with AI…", and "Appearance…".
- More player options: Volume, Audio tracks, Playback info, Picture in Picture, Fill screen, Edit markers.
- Audio tracks: a check mark, the codec name ("ATSC A/52B (AC-3, E-AC-3)"), the badges EAC3, 5.1 and
  DEFAULT, and "English · 5.1(side) · 768 kbps · 48,000 Hz".
- The Playback info panel shows the player, the play method (Direct Play, Direct Streaming, Transcode),
  the protocol (HLS, Progressive), the stream type, "auto-switched from", the player size, the video resolution,
  dropped and corrupted frames, the video codec, and the audio codec. It also shows the source file:
  container, size, bitrate, video bitrate, range type, color range, audio bitrate, channels, sample rate.
- Markers: Skip Intro, Skip Recap, Skip Credits and Skip Preview buttons, and a marker editor
  (Set start, Set end, Save, Reset all).
- Up Next: a card with a countdown ("Up Next in" seconds), the next episode as "S1:E2" and its title, and Play Now.
- A "Still watching" prompt after a number of episodes. This is a setting for each device.
- Watch Party controls: Invite, Copy invite link, Viewers, Leave.

Endpoints: `GET watch/{id}` (and `?file_id=`), `GET playback/capabilities`, `POST playback/start`,
`POST playback/route-events`, `POST playback/{session}/progress`,
`POST playback/sessions/{session}/control/ws-ticket`, `GET subtitles/ai/status`,
`GET subtitles/providers/status`, `GET subtitles/sync/status`, `GET stream/{session}/subtitles/{n}.vtt`,
and `GET catalog/series/{id}/seasons/{n}/episodes` for Next episode.

The native player is mpv with the controller `data/silo-controls.lua`. It has a seek bar with marker ranges,
play and pause, the audio and subtitle track names, the key `a` to cycle audio, and the key `s` to cycle subtitles.
It also has a Skip intro prompt (Never, Ask, Always), auto-skip of credits and recap, and a Next episode prompt at the credits.

| Web has | Native app | API |
|---|---|---|
| Audio menu with codec, channels, layout, bitrate, sample rate, DEFAULT | No, a key cycles the tracks and the bar shows a name | Wire type `AudioTrack` |
| Subtitle menu with Embedded and External groups, Forced and SDH | No, a key cycles the tracks | Wire type `VersionSubtitleTrack` |
| Subtitle delay | No. mpv has `sub-delay`, but the app has no control for it | Client only |
| Add, search, or AI-translate subtitles, and subtitle appearance | No | Not in the op list |
| Version switch in the player | No | `replanPlayback` is in the op list, and `versions` is a wire field |
| Quality and bitrate ladder | No | Look at `PlaybackPlan` and `replanPlayback` for a bitrate limit |
| Chapters list and chapter marks on the seek bar | No | Wire field `FileVersion.chapters` |
| Seek thumbnails (trickplay) | No | Not in wire |
| Back 10 s and Forward 30 s buttons | Keys only (mpv default keys) | Client only |
| Skip Preview button | No, only intro, credits and recap | `MarkerKind::Preview` exists |
| Up Next countdown card with the next title | A prompt only, with no title and no countdown | The app already loads the next episode |
| Still watching prompt | No | The server has the setting key |
| Playback info panel | No | Wire types `PlaybackPlan` and `FileVersion` |
| Picture in Picture, Fill screen | No | Client only (mpv `panscan`) |
| Marker editor, Watch Party | No | Not in the op list |

## Suggested order

The wire types already have the data for these items. They need no new endpoint:

1. Tagline, ratings (the `ratings` list), studio or network, and the title logo on the detail page.
2. The Directed by and Written by line, and a Crew row (`crew`).
3. Cast on a series (`cast`).
4. Audio and subtitle pickers before play, with the same track details as the web (`FileVersion` tracks).
5. A Version picker with the file size and the Dolby Vision label (`versions`, `VideoTrack.dolby_vision`).
6. Episode cards with the air date, the overview and a quality badge. Season cards with the poster and the watched count.
7. A person header with the photo, the dates, the birthplace and the biography. `getPerson` is in the op list. Add a client method.
8. Chapters on the detail page and in the player (`FileVersion.chapters`).
9. Card badges: quality, airing status, network, and "N min left".

These items need a new operation in the op list: More Like This (`recommendations/similar`),
favorite, star rating, mark watched, collections, trickplay, and subtitle search.
