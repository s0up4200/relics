# Detail page backdrop in the official clients

This file tells how the official Silo clients draw the backdrop on an Item detail page
(movie, series, season, episode). The survey was on 2026-10-13. The survey was read-only.

Method:

- Web: the source in `Silo-Server/silo-server` (local clone `~/github/oss/silo-server`, commit `b78c1ff`).
  The live bundle on s.brr.tf has the same class names and the same gradient values as this source.
  Bundle files: `assets/index-QzZEJBi9.css`, `assets/ItemDetailSkeleton-D2gOQLCn.js`,
  `assets/ItemDetailSkeleton-96hIlgX5.css`, `assets/index-pK0btP1p.js`. The survey did not use a browser.
- Apple (iOS, iPadOS, macOS, tvOS): `Silo-Server/silo-apple`, commit `b02d55a`. Paths below are under `iosApp/iosApp/`.
- Android (phone, tablet, Android TV): `Silo-Server/silo-android`, commit `a36f4af`.

Terms in this file:

- Hero: the top block of the detail page. It holds the backdrop, the poster, the title and the actions.
- Scrim: a gradient layer on top of the artwork. It makes the text easy to read.
- Mask: a gradient that changes the alpha of the artwork itself. A masked image fades to transparent.
- Page surface: what the page shows behind all content.
- `--background`: the theme background color of the web UI. In the default theme (midnight-cinema) it is `#141417`.

## Official clients

The server README names two native client repos, `silo-apple` and `silo-android`
(`README.md` lines 133-136 in silo-server). The web UI is part of the server repo (`web/`). The GitHub
organization `Silo-Server` has no other client repos.

| Client | Repo | Platforms |
|---|---|---|
| Web UI | silo-server `web/` | Browser |
| Silo for Apple | silo-apple | iOS, iPadOS, macOS, tvOS |
| Silo for Android | silo-android | Android phone and tablet, Android TV |

The README also names third-party clients that use the Jellyfin API (VidHub, Findroid, Infuse). They are not official.

## Summary table

| Client | Backdrop position | Clipped to hero | Bottom of artwork | Page surface behind content |
|---|---|---|---|---|
| Web | In the hero, scrolls with the page | Yes | Scrim fades to opaque `--background` | `--background`, with a faint ambient tint at the top |
| macOS | In the header, scrolls with the page | Yes, 520 pt | Mask fades the artwork to clear | Black with 42% sampled tint (flat) |
| iPhone | In the hero, scrolls with the page (parallax) | Yes, 430-540 pt | Mask fades the artwork to clear | Fixed blurred copy of the backdrop |
| iPad | In the hero, scrolls with the page | Yes | Gradients | Black with 42% sampled tint |
| tvOS | In the hero, top-right corner, scrolls with the page | Movie yes, series no | Mask reaches clear at 90% of the height | Black with 42% sampled tint (flat) |
| Android phone | Same as iPhone | Yes | Mask fades the artwork to transparent | Fixed blurred copy of the backdrop |
| Android tablet | In the hero | Yes | Gradient to the page surface color | Black with 42% sampled tint |
| Android TV | Same as tvOS | Movie yes, series no | Same mask as tvOS | Sampled tint |

No client draws a sharp, full-window backdrop that stays fixed while the content scrolls.
No client lets the artwork end with a visible edge.

## Web UI

### Structure

Every video detail page uses one component, `DetailHero` (`web/src/pages/ItemDetail/DetailHero.tsx`).
The hero is a `<section>` with `relative isolate overflow-hidden` (line 127). So the hero clips the backdrop.
The supporting sections (cast, extras, similar titles) come after the hero, on the page shell
(`DetailLayout.tsx` lines 17-25). They have no backdrop behind them.

Layers in the hero, from bottom to top (`DetailHero.tsx` lines 132-165):

1. `hero-backdrop-artwork`: `absolute inset-0 h-full w-full`. It fills the hero.
2. `detail-hero-scrim-under`: a left fade and a flat tint.
3. `ambient-glow detail-hero-ambient`: a radial glow in the average color of the artwork.
4. `detail-hero-scrim-over`: a bottom fade and a vignette.
5. The content (poster, title, facts, actions).

### Image size and fit

- The image is an `<img>` with `object-cover object-[center_20%]` (line 151). It covers the hero. It is centered
  horizontally and anchored 20% from the top.
- Before the image loads, the backdrop thumbhash shows as a CSS background image with `background-size: cover`
  and `background-position: center 20%` (lines 136-141). Then the image fades in over 300 ms.
- The detail page has no `filter` and no blur on the backdrop. A code comment says that the flat tint in
  `scrim-under` replaces `brightness()` (`app.css` lines 134-143). The `brightness(var(--hero-backdrop-brightness))`
  filter is only on the Home featured hero (bundle `index-CvUwv1Ul.js`).

### Gradient values

The values come from `web/src/app.css` lines 843-930. The bundle `index-QzZEJBi9.css` has the same values.

`detail-hero-scrim-under`:

```css
linear-gradient(to right,
  color-mix(in srgb, var(--background) 92%, transparent) 0%,
  color-mix(in srgb, var(--background) 62%, transparent) 18%,
  color-mix(in srgb, var(--background) 24%, transparent) 36%,
  transparent 52%),
linear-gradient(
  color-mix(in srgb, var(--background) var(--hero-backdrop-scrim, 25%), transparent),
  color-mix(in srgb, var(--background) var(--hero-backdrop-scrim, 25%), transparent));
```

`--hero-backdrop-scrim` is `10%` in the default theme. So the whole image gets a 10% wash of the background color.

`detail-hero-ambient` (class `ambient-glow`):

```css
radial-gradient(ellipse at 50% 80%, var(--ambient, var(--primary)) 0%, transparent 70%);
opacity: var(--ambient-glow-opacity, 0.12);  /* 0.08 in midnight-cinema */
```

`--ambient` is the average color of the backdrop thumbhash. The movie, series, season and episode pages set it
with `useAmbientColor(item.backdrop_thumbhash)` (for example `MovieContent.tsx` line 63 and `SeasonContent.tsx` line 37).

`detail-hero-scrim-over`:

```css
linear-gradient(to top,
  var(--background) 0%,
  color-mix(in srgb, var(--background) 86%, transparent) 14%,
  color-mix(in srgb, var(--background) 46%, transparent) 34%,
  color-mix(in srgb, var(--background) 12%, transparent) 54%,
  transparent 70%),
radial-gradient(ellipse 120% 100% at 62% 40%, transparent 55%, rgb(0 0 0 / 18%) 100%);
```

All scrims use the theme background color, not black. Only the vignette uses black (18%).
The bottom edge of the hero is 100% `--background`, so the clip line is not visible. The hero also has a
faint bottom border (`border-b border-border/10`).

### Hero height

- Movie (variant `full`): `min-h-[60dvh]`, and `lg:min-h-[72dvh]` at 1024 px and wider (`DetailHero.tsx` lines 166-174).
  The hero grows when the content is taller.
- Series, season and episode (the variants with `data-viewport-bounded`): the hero and the season or episode rail
  share one grid that is one viewport high (`detailLayout.css` lines 9 and 121). On a desktop at least
  1024 px wide and 651 px high, the series page is at least `100dvh` minus the header (line 186 onwards).
- The season or episode rail is under the hero, outside the `<section>`. So the rail is on the plain page
  background, not on the backdrop.

### After the image, and with no backdrop

- Below the hero, the page shows the `body` background (`app.css` lines 717-733). It is `--background`, with a radial
  tint `color-mix(in srgb, var(--ambient) 10%, transparent)` at the top that ends at 32%, and a linear gradient from
  92% `--background` to `--background`. The body background has no `background-attachment: fixed`.
- If the Item has no backdrop URL and no thumbhash, the hero draws no artwork layer (line 132). The scrims still draw
  over the page background. The hero keeps its height.

### Item types

- Movie: variant `full`, with the poster to the left of the title.
- Series and season: variants `series` and `season`. The poster is in the hero. The season or episode rail is
  under the hero.
- Episode: variant `episode` with `hidePoster` (`EpisodeContent.tsx` lines 303-309). The hero uses the
  `backdrop_url` of the episode.
- The server picks the episode `backdrop_url`. It can be the episode still or series artwork. The field
  `backdrop_is_episode_still` tells which one (`docs/catalog-api.md` lines 642-649). The web does not read
  that field. The web also does not blur the backdrop of an unwatched episode. It only hides the overview.

## Apple clients

### macOS

The Mac uses the shared detail views (`MovieDetailContent`, `SeriesDetailContent`) with a Mac header
(`Screens/Detail/Phone/PhoneDetailHero.swift` lines 389-392 and 626-697).

- The backdrop is in the header, so it scrolls with the page.
- Size: full width, height `macDetailBackdropHeight` = 520 pt (`Theme/SiloTheme.swift` line 77), `contentMode: .fill`, `.clipped()`.
  The image is centered, which is the SwiftUI default.
- Overlay: a horizontal gradient from black 72% at the leading edge to black 20% at the trailing edge.
- Mask: from top to bottom, opaque from 0 to 55%, then to clear at 100%. So the artwork fades out into the page.
- Page surface: `PhoneDetailPageSurface` fills the window. On macOS it is black with the sampled artwork tint
  at 42% (lines 63-110 and 129-136). The blurred artwork layer is for iPhone only.
- With no backdrop, the header draws no artwork and shows the page surface (comment at lines 666-668).

### iPhone and iPad

- iPhone: the hero is 430 to 540 pt high (`width × 1.18`, lines 471-474). The artwork scrolls with the page,
  with parallax. It moves at 0.52 of the scroll speed, and a black scrim on it grows to 30% (lines 210-250).
  The mask from top to bottom is opaque to 72%, 76% at 84%, and clear at 100% (lines 286-297). A top scrim from
  black 34% to clear at the center keeps the buttons readable (lines 455-460).
- iPhone page surface: a fixed copy of the backdrop with scale 1.18, saturation 1.28, brightness -0.10 and
  blur radius 46. Black 28% and the tint at 10% are on top (lines 84-103). This is the only fixed
  full-window backdrop in any client. The blur is so strong that it shows a field of color, not a picture.
- If the Item has no backdrop, the iPhone hero uses the poster (lines 771-774).
- Season and episode Items have no page of their own. The app resolves them to the series page
  (`Screens/Detail/ItemDetailView.swift` line 671).

### tvOS

- `TVDetailPageSurface` is black with the sampled tint at 42%. A comment says: "Artwork itself lives inside the
  scrolling hero and therefore leaves the screen naturally as the viewer moves down"
  (`tvOS/Screens/Detail/TVDetailHero.swift` lines 21-37).
- The hero is 690 pt high on a 1920 × 1080 canvas (line 11). The artwork is 64% of the width and 70% of a 16:9
  height. It is anchored at the top-right (`tvOS/Components/TVRootHeroBackdrop.swift` lines 8-22, `TVDetailHero.swift` lines 156-183).
- Horizontal mask: clear at the leading edge, opaque from 32% from the trailing edge. Vertical mask: opaque to 38%,
  88% at 52%, 58% at 68%, 24% at 81%, clear at 90% (`TVRootHeroBackdrop.swift` lines 27-55). A comment says that the
  fade ends before the hero bottom "so its clip can never reveal a straight artwork edge".
- Movie: the hero clips the artwork. Series: `extendsBackdropFadeBelowHero: true`, so the fade continues behind
  the season row (`TVDetailHero.swift` lines 119-140, `TVSeriesDetailView.swift` line 408).

## Android clients

The Android code uses the Apple values. Its comments name the Apple files.

- Phone page surface: `DetailPageSurface` is a fixed blurred backdrop with the iPhone values (scale 1.18, blur 46 dp,
  saturation 1.28, brightness -0.10, black 28%, tint 10%). On API 30 and lower it is the flat color
  `lerp(black, tint, 0.42)` (`androidApp/.../screens/detail/DetailPageSurface.kt`).
- Phone hero: 430 to 540 dp, with parallax. The mask is opaque to 72%, 76% at 84%, and transparent at 100%. The top
  scrim goes from black 34% to transparent at 30%. If the Item has no backdrop, the hero uses the poster
  (`DetailSharedComponents.kt` lines 602-606 and 661-745). A comment says that an older opaque fade
  "scrolled with the hero" and "read as a stray black gradient".
- Tablet: the hero has a horizontal gradient (black 88%, 58% at 48%, 32% at 100%) and a vertical gradient that
  ends at the opaque page surface color (lines 429-470).
- Android has episode pages. The app hides the artwork if the profile hides unwatched episode images and the
  backdrop is an episode still (`hidesEpisodeArtwork`, line 864).
- Android TV: the geometry and the masks are the same as on tvOS. The hero clips, except on a compact series page
  (`androidTvApp/.../screens/detail/TvDetailHero.kt` lines 52-62, 94-138 and 536-538).

## What the native app can copy

- The web UI and the Mac app are the desktop clients. Both put the backdrop in the hero, clip it to the hero,
  and let it scroll away with the page.
- Both hide the clip line. The web fades a scrim to the opaque theme background at the bottom. The Mac masks the
  image to transparent from 55% to 100% of its height.
- Both add a dark fade or a fade in the background color on the leading side, where the text is.
- In the web UI and the Mac app, the rows under the hero (seasons, episodes, cast) are on the page surface,
  with no artwork behind them. The phone apps show a blurred backdrop behind the rows. On tvOS and Android TV,
  the fade of a series continues behind the season row.
- Only the phone apps have a fixed full-window backdrop, and it is a strongly blurred field of color.

## Not verified

- The survey did not open s.brr.tf in a browser. The deployed bundle has the same classes and values as the source.
- The survey did not find how the server picks between the episode still and the series backdrop for the
  `backdrop_url` of an episode. The local silo-server clone has no Go source.
- The survey read the Apple and Android apps as source only. It did not run them.
