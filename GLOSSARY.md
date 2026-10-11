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

**Item**:
An entry in a Server's catalog that the user can open: a movie, a series, a season or an episode. Only movies and episodes play.
_Avoid_: media, title, video

**Home section**:
One row on the home page. The server defines it, with a type, a title and an order.
_Avoid_: Shelf, carousel, rail

**Featured section**:
A Home section that the server marks as featured. The Library window shows it as a hero above the other rows.
_Avoid_: Banner

**Player window**:
The window that libmpv draws video in. Its app id is `silo-player`. The app has at most one Player window. It can show several Playback sessions one after another. It closes when the user quits it or the last item ends.

**Playback session**:
One viewing of one Item, from the start request to the stop report. The server knows it by its session id. A reload at the current position and a replan stay in the same Playback session.
_Avoid_: stream, play

**Normal end**:
An end of file that mpv reports within 8 s of the Item's duration or at 98.5% of it or more. Only a Normal end lets the stop report carry the full duration.
_Avoid_: finished, complete
