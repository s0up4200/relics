# Glossary

**Build identity**:
The version and source commit recorded when Relics was built, with a marker for local changes.

**Server**:
A Silo server that the user added. The app knows a Server by the stable id that the server reports, not by its URL.
_Avoid_: host, instance

**Active server**:
The one Server and Account pair that the Library window shows now. The app shows content from one Active server only.
_Avoid_: current server, selected server

**Library window**:
The app window where the user browses the Silo library and starts playback. Its app id is `relics`.

**Account**:
A user's sign-in on one Server. An Account holds the tokens and owns zero or more Profiles. One Server can have several Accounts in the app.
_Avoid_: user, login

**Browser sign-in**:
Sign-in through the page of a provider in the system browser. The browser gives the app the result, and the app gets the tokens of the Account.
_Avoid_: OAuth login, SSO

**Network sign-in**:
Sign-in with no password, where the Server knows the person from the network that the request comes from, for example Tailscale.
_Avoid_: Tailscale login, auto-login

**Profile**:
A household member inside an Account. Progress, watched state, favorites and the Home layout belong to the Profile. A Profile can have a PIN and content limits.
_Avoid_: user, household member

**Profile picker**:
The full-screen view where the user chooses the Profile that watches. The PIN entry is inline, below the selected Profile. At launch the app skips the picker when it can open the last Profile of the Account, or the only Profile when that Profile has no PIN.
_Avoid_: profile selector, who's watching screen

**PIN token**:
The token that the server gives when the user types the right PIN of a Profile. The app sends it as `X-Profile-Token` and keeps it in the Secret Service. It stays valid until the sign-in ends or the PIN changes. Five wrong PINs lock the Profile for five minutes.
_Avoid_: profile token, unlock token

**Item**:
An entry in a Server's catalog that the user can open: a movie, a series, a season or an episode. Only movies and episodes play.
_Avoid_: media, title, video

**Library**:
A collection of Items of one media type on a Silo server, for example Movies or Shows. The user opens each one from the sidebar.
_Avoid_: Collection (Silo uses Collection for a curated group of Items)

**Collection**:
A curated group of Items on a Silo server. An administrator makes it, or a Profile makes a personal one. A Library lists its collections in its Collections tab, in collection groups. The Collections page lists the collections of the Server for each Library. The app only browses collections: it does not make, change or order them.
_Avoid_: Library, list, playlist

**Home section**:
One row on the home page. The server defines it, with a type, a title and an order.
_Avoid_: Shelf, carousel, rail

**Featured section**:
A Home section that the server marks as featured. The Library window shows it as a hero above the other rows.
_Avoid_: Banner

**Hero**:
The top block of a page, with the Backdrop, the title and the actions. On the detail page, the Hero clips the Backdrop and scrolls away with the page.
_Avoid_: Banner, header

**Backdrop**:
The wide artwork of an Item that fills the Hero. On an episode, it can be the episode still or the artwork of the series.
_Avoid_: Background, fanart

**User rating**:
The 1 to 5 stars that a Profile gives an Item. The Profile can clear it. It is not the TMDB or IMDb score, and it is not the content rating.
_Avoid_: rating, score, vote

**Person**:
A cast or crew member that the Silo server knows, with the titles that they took part in.
_Avoid_: Actor, as the general name. Actor is one role that a Person can have.

**Filter panel**:
The panel below the bar of a Library grid or of the Search titles where the user picks the filters, in sections: watch status, video quality, rating, year, "added or released in the last", genres, and the facets. The app does not have the Advanced rule mode of the web client.
_Avoid_: Filter menu, Guided mode

**Facet**:
A filter that takes one text value: a language, a studio, a network, a country, or a Person in one role (actor, director, writer, producer). The server lists the values of each facet except the Person facets, where the user types the name.
_Avoid_: Facet filter, people filter

**Request**:
A request from the user that the Server adds a movie or a series that it does not have. Search shows such titles under Request to add, and Enter sends a request. My requests lists the requests of the user with their state (pending, approved, processing, available, and others). The user can cancel a request until the Server sent something for it. The app does not have the discover pages or follows of the web client.
_Avoid_: order, wish

**Artwork ladder**:
The fixed set of widths at which the Silo server caches each artwork type. `image_size` picks one rung for a whole response.
_Avoid_: Variants, resolutions

**Revisioned artwork**:
Artwork whose URL path has a content revision (`w500.<rev>.webp`). The bytes never change, so the cache keeps it with no expiry.
_Avoid_: Immutable URL (the query and signature still change)

**Thumbhash**:
A hash of about 21 bytes that the API sends next to each artwork URL. It decodes to a blurred preview that the Library window shows while the image loads or after it fails.
_Avoid_: Blurhash (a different format)

**Artwork cache**:
The image cache of the app. It keeps decoded images in memory and the files in `$XDG_CACHE_HOME`, keyed by the origin plus the path.
_Avoid_: Image cache (that is the name of the GPUI trait)

**Player view**:
The page in the Library window that plays an Item. It shows the video and the controls over it. Back leaves it and ends the Playback session.
_Avoid_: player window, player page

**Playback session**:
One viewing of one Item, from the start request to the stop report. The server knows it by its session id. A reload at the current position and a replan stay in the same Playback session.
_Avoid_: stream, play

**Normal end**:
An end of file that mpv reports within 8 s of the Item's duration or at 98.5% of it or more. Only a Normal end lets the stop report carry the full duration.
_Avoid_: finished, complete

**Early end**:
An end of file that mpv reports before the Item's runtime is reached, usually because the stream lost access. An Early end never marks an Item watched.
_Avoid_: premature EOF, false EOF

**Server setting**:
A preference that Silo stores for a Profile. Every client that uses the Profile shares it.
_Avoid_: preference, user setting

**Local setting**:
A preference that only this app on this computer has, such as hardware decoding.
_Avoid_: config, device setting

**Marker**:
A time range in a file version that Silo labels `intro`, `credits`, `recap` or `preview`. A file version can have more than one marker of the same kind.

**Skip prompt**:
The button that the Player view shows over the video while playback is inside a marker. It offers "Skip intro", "Skip recap", "Skip credits", "Skip preview", "Watch intro" (after an automatic skip), or "Next episode" (when the credits run to the end of the file, the Item has a next episode, and the next episode does not play by itself). A recap or credits marker that the Server settings skip automatically shows no prompt. The Playback session decides when the prompt shows. The Player view only draws it, and a click on it does the same as Enter.

**Up Next card**:
The card that the Player view shows near the end of an Item when the next episode or the next shuffle pick plays by itself. It starts at the end credits, or 10 seconds before the end of the file when the Item has no end credits. It shows the next title and a countdown of 10 seconds. Play now (Enter) plays the next Item at once. Cancel (Esc) hides the card, and the Player view then closes at the end of the file.
_Avoid_: autoplay prompt, next episode overlay

**Shuffle**:
A random play order that the Server keeps for one scope: a Library, a collection, a series or a season. Each pick plays from the beginning. The next pick plays by itself, with the Up Next card.
_Avoid_: random play, shuffle play

**Still watching prompt**:
The prompt that pauses playback in place of the next Item after 3 Items in a row played by themselves with no input of the user. Continue (Enter) plays the next Item. Back stops playback. Any key, click or mouse move in the Player view, and any media key, starts the count again.
_Avoid_: passout prompt, idle check

**Offline**:
The app state when the active Server does not answer, or answers 503. The app keeps its tokens and retries. It never signs the user out.
_Avoid_: disconnected, down

**Live updates**:
The Server tells the app about changes to the state of the Profile and to the Libraries, so that open pages show them without a restart. A page out of view waits until it shows again.
_Avoid_: realtime, push, sync

**Track picker**:
The list in the Player view where the user picks the audio track or the subtitle track.
_Avoid_: track menu, cycle

**Choice list**:
A list where the user picks one value. A check mark shows the current value. j/k move, Enter or a click picks, Escape closes. The Track picker and the Quality list in the Player view are Choice lists, and so is the list of a Server setting on the Settings page.
_Avoid_: selector, dropdown, menu

**Chapter**:
A titled time range of a file version, from the file. The Chapters list in the Player view seeks to the start of a chapter. A chapter is not a Marker.

**Subtitle delay**:
The time in seconds that the subtitles show later (positive) or earlier (negative) than the file says. It carries to the next episode of the same series, as the Track picker choice does. Another Item starts at zero.

**Version picker**:
The list on the detail page where the user picks which file version of an Item plays.
_Avoid_: quality picker

**Quality ladder**:
The lower-bitrate renditions of the file version that plays. The user picks a step to play with less bandwidth.
_Avoid_: version
