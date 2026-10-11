# Glossary

**Server**:
A Silo server that the user added. The app knows a Server by the stable id that the server reports, not by its URL.
_Avoid_: host, instance

**Library window**:
The app window where the user browses the Silo library and starts playback. Its app id is `silo-omarchy`.

**Account**:
A user's sign-in on one Server. An Account holds the tokens and owns zero or more Profiles. One Server can have several Accounts in the app.
_Avoid_: user, login

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

**Home section**:
One row on the home page. The server defines it, with a type, a title and an order.
_Avoid_: Shelf, carousel, rail

**Featured section**:
A Home section that the server marks as featured. The Library window shows it as a hero above the other rows.
_Avoid_: Banner

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

**Player window**:
The window that libmpv draws video in. Its app id is `silo-player`. The app has at most one Player window. It can show several Playback sessions one after another. It closes when the user quits it or the last item ends.

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

**Marker**:
A time range in a file version that Silo labels `intro`, `credits`, `recap` or `preview`. A file version can have more than one marker of the same kind.

**Skip prompt**:
The button that the player shows while playback is inside a marker. It offers "Skip intro", "Watch intro" (after an automatic skip), or "Next episode" (when the credits run to the end of the file). The app decides when the prompt shows. The controller only draws it.
