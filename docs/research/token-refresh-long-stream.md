# Research: token refresh during a long stream

Date: 2026-10-10. Status: research only. Nobody built or ran code for this file.

Question (issue #4): when the access token renews during playback, does mpv
lose access to the stream? How does Silo authorize media requests, and how long
does each credential stay valid? How do the official clients handle a renewal?
What must a libmpv client do to keep a 3-hour film playing?

Each fact has a link to a primary source at a pinned commit. The label
"Inference" marks a conclusion that I drew from those facts. A source does not
state it.

## Answer in short

- Every media request on `/api/v2` needs the account access token, in both
  media modes. The signed `st` token in the URL does not replace it. Silo has
  no media-scoped credential that a native client can use instead.
- The access token lives 8 hours. A refresh keeps the same login session, so
  the old access token stays valid until its own expiry. A refresh in the app
  does not break the header that mpv already holds.
- The server looks at the token when each request starts. A request fails only
  when it starts after the token in mpv expired. Such requests are HLS
  segments, playlist reloads, seeks and reconnects.
- mpv copies `http-header-fields` into the demuxer when it opens the file.
  With the mpv 0.41 on Omarchy, a change of the property during playback does
  not reach later segment, playlist or seek requests. Only a new `loadfile`
  uses the new value.
- silo-omarchy must do these steps:
  1. Before each `loadfile`, refresh the access token if less than about
     4 hours remain. A fresh 8-hour token then covers a 3-hour film.
  2. Pass the header as a per-file option of `loadfile`.
  3. Before the token in mpv expires, refresh it. Then reload the same plan URL
     at the current position with the new header.
  4. After a 401, or after an end of file that comes too early, refresh once
     and reload once. If the header did not change, send a `replan`.

## Sources and pinned commits

| Repository | Commit |
| --- | --- |
| [Silo-Server/silo-server](https://github.com/Silo-Server/silo-server) | `ec1dcb67c77f59688692574516276e8f6c72dfeb` |
| [Silo-Server/silo-apple](https://github.com/Silo-Server/silo-apple) | `5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba` |
| [Silo-Server/silo-android](https://github.com/Silo-Server/silo-android) | `e535ddceb5bedd22b75d0e53abca27b3640982df` |
| [mpv-player/mpv](https://github.com/mpv-player/mpv) | `accb7b2821ecf0cb04cf3579ccd98bc8f751f61f` |
| [FFmpeg/FFmpeg](https://github.com/FFmpeg/FFmpeg) tag `n9.0.1` | `bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa` |

The silo-server commit is newer than the one in
`docs/research/silo-linux-client.md`. The local machine has mpv 0.41.0 and
FFmpeg 9.0.1 (`pacman -Q`).

Link prefixes:

- `SS` is `https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb`
- `SA` is `https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba`
- `SD` is `https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df`
- `MPV` is `https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f`
- `FF` is `https://github.com/FFmpeg/FFmpeg/blob/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa`

---

## 1. How Silo authorizes media requests

### 1.1 The media routes

The v2 media routes are these
([`SS/docs/playback-api.md#L258-L266`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/playback-api.md#L258-L266)):

- `/api/v2/stream/{session_id}` for original bytes and progressive remux.
- `/api/v2/playback/transcode/{session_id}/master.m3u8` and
  `.../segment/{name}` for HLS.
- `/api/v2/stream/{session_id}/subtitles/...` for subtitles and fonts.

The v2 router registers all of them with the class `ClassProfileScoped`
([`SS/internal/apiv2/playback_delivery.go#L152`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/playback_delivery.go#L152)).
That class "requires a valid bearer token or API key with a live session",
and also a profile
([`SS/internal/apiv2/registry.go#L27-L33`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/registry.go#L27-L33)).
The delivery doc says the same: "Account authentication and viewer
authorization are always required"
([`SS/docs/playback-api.md#L268-L274`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/playback-api.md#L268-L274)).

### 1.2 The three credentials on a media request

The access token is a JWT. The auth middleware reads it from the
`Authorization: Bearer` header. If there is no header, it reads the `token`
query parameter. That parameter exists for `<video>` elements that cannot set
headers
([`SS/internal/api/middleware/auth.go#L381-L395`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/api/middleware/auth.go#L381-L395)).
On each request, the middleware makes sure that the signature and the expiry
are valid. Then it makes sure that the login session is still active and that
the role did not change
([`SS/internal/api/middleware/auth.go#L157-L181`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/api/middleware/auth.go#L157-L181)).
An expired token gets `401` with "Invalid or expired token".

The profile headers are `X-Profile-Id`, and `X-Profile-Token` for a PIN
profile. The server mints profile tokens with a TTL of zero
([`SS/internal/api/router.go#L621`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/api/router.go#L621)),
and zero means "durable". The end of the login session, a profile delete or a
PIN change ends the token. No clock ends it
([`SS/internal/access/profile_token.go#L33-L35`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/access/profile_token.go#L33-L35),
[`SS/internal/access/resolver.go#L165-L192`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/access/resolver.go#L165-L192)).

The signed stream token `st` is in the media URLs only when the client did not
opt into `header_authenticated_media_v1`
([`SS/docs/architecture/playback-protocol-v3.md#L867-L870`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L867-L870)).
It holds a recipe that lets any API process rebuild the session after a
restart. It lives 24 hours (`MaxTokenTTL`)
([`SS/internal/playback/recipecard.go#L334-L340`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/playback/recipecard.go#L334-L340)).
On `/api/v2` it does not authorize the request. The route still requires the
account token
([`SS/internal/apiv2/playback_delivery.go#L103-L106`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/playback_delivery.go#L103-L106)).

Inference: Silo has no media-scoped credential on v2 that lives longer than
the access token. The `st` token helps after a server restart, not after a
token expiry. The `token` query parameter is the same JWT with the same
expiry, and it puts the credential into URLs and logs. API keys do not expire,
but they are long-lived account credentials. Do not give one to the player.

### 1.3 `header_authenticated_media_v1`

- The client sends the feature in `client_features` after the server
  advertises it. The plan URLs are then relative and carry no credential. The
  client "must attach its current `Authorization: Bearer ...` header" to the
  manifest, every HLS segment, every subtitle and every font request
  ([`SS/docs/architecture/playback-protocol-v3.md#L704-L733`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L704-L733),
  [`#L816-L823`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L816-L823)).
- "Header-authenticated media requires the Authorization header and the
  profile selector." The `token` query fallback does not apply in this mode
  ([`SS/internal/apiv2/playback_delivery.go#L104`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/playback_delivery.go#L104)).
- The plan field `stream.headers` never contains the bearer.
  `header_refresh: none` means that the URL stays the same. "Normal API-token
  refresh remains out of band, after which a client can retry or reload that
  URL with the new header"
  ([`#L816-L823`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L816-L823)).
- The mode is fixed for the attempt. To leave it, the client stops and starts a
  new attempt
  ([`#L825-L831`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L825-L831),
  [`#L1020`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L1020)).
- In this mode the session lives only in the memory of one API process. After
  a restart, the client must start a new attempt
  ([`#L825-L843`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L825-L843)).
- A proxy origin (`authorized_media_origins_v1`) validates "the same access
  token against the same live login session the API checks"
  ([`#L795-L803`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L795-L803)).

The spec has a `header_refresh: session` mode with a `header_refresh_url`
([`#L700-L702`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L700-L702)),
and a client flag `auth_header_refresh`
([`#L682`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L682)).
At this commit, every plan builder sets `HeaderRefreshNoneV3`
([`SS/internal/playback/plan_v3.go#L462`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/playback/plan_v3.go#L462),
[`#L700`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/playback/plan_v3.go#L700),
[`#L1196`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/playback/plan_v3.go#L1196)).
The `session` value occurs only in tests. Inference: a client cannot use
`header_refresh_url` today. Handle `none`, and treat `session` as unknown.

## 2. How long each credential stays valid

| Credential | Lifetime | What ends it |
| --- | --- | --- |
| Access token | 8 hours by default ([`SS/internal/config/config.go#L583-L586`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/config/config.go#L583-L586)) | Its own expiry, or the end of the login session |
| Refresh token and login session | 30 days, moved forward by each refresh ([`SS/internal/auth/service.go#L1100-L1104`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/auth/service.go#L1100-L1104), [`#L1141`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/auth/service.go#L1141)) | Logout, admin revoke, disabled account |
| Profile token | No clock | End of the login session, profile delete, PIN change |
| `st` stream token | 24 hours | Stop, expiry or admin stop, through a deny marker ([`SS/docs/playback-api.md#L286-L294`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/playback-api.md#L286-L294)) |
| Plan `expires_at` | 24 hours ([`SS/internal/playback/protocol_v3.go#L1481`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/playback/protocol_v3.go#L1481)) | |

An admin can change the access token lifetime with `auth.access_token_expiry`
([`SS/internal/config/config.go#L229-L230`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/config/config.go#L229-L230)).
Read `expires_in` from each login or refresh response. Do not assume 8 hours.

A refresh does not end the old access token. `Refresh` finds the login
session, moves its expiry forward, and mints a new pair with the same
`SessionID`
([`SS/internal/auth/service.go#L1064-L1160`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/auth/service.go#L1064-L1160)).
The middleware accepts any access token with a valid signature, a future
expiry and an active session
([`SS/internal/api/middleware/auth.go#L157-L181`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/api/middleware/auth.go#L157-L181)).
Inference: after the app refreshes, the old token in mpv keeps working until
its own `exp`. A logout or an admin revoke ends both tokens at the same time.

Two playback timers are not about auth, but they can also stop a long film.
The server reaps a paused session after 30 minutes, and that kills its
transcode
([`SS/internal/playback/session.go#L388-L406`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/playback/session.go#L388-L406)).
A stopped or expired session answers `410 playback_session_ended`
([`SS/docs/playback-api.md#L276-L278`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/playback-api.md#L276-L278)).

## 3. What happens when the token expires during a stream

The middleware looks at the token only when a request starts (section 1.2).
The two cases below are inference from that fact and from the FFmpeg code.

Direct play and progressive remux use one long HTTP response. The open
response keeps playing after the token expires. The next new request fails
with 401. A seek, a reconnect after a network drop or a ranged read makes such
a request. FFmpeg's HTTP protocol opens a new connection for each seek
([`FF/libavformat/http.c#L2178-L2184`](https://github.com/FFmpeg/FFmpeg/blob/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa/libavformat/http.c#L2178-L2184)).
It writes the same stored `headers` string into each new request
([`FF/libavformat/http.c#L1638-L1640`](https://github.com/FFmpeg/FFmpeg/blob/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa/libavformat/http.c#L1638-L1640)).

HLS remux and transcode work differently. mpv asks for a new segment every few
seconds, and it reloads a growing playlist. The first request after the expiry
gets 401. FFmpeg's HLS demuxer logs "Failed to open segment". With the default
`seg_max_retry` of 0, it skips that segment and goes to the next one
([`FF/libavformat/hls.c#L1785-L1800`](https://github.com/FFmpeg/FFmpeg/blob/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa/libavformat/hls.c#L1785-L1800),
[`#L3066-L3067`](https://github.com/FFmpeg/FFmpeg/blob/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa/libavformat/hls.c#L3066-L3067)).
Every later segment fails the same way. mpv can then reach the end of the
playlist and report a normal end of file, not an error. The client must not
mark the item as watched because of that end of file. It must compare the
position with `source.duration_seconds`.

## 4. How the official clients handle renewal

### 4.1 Apple (silo-apple)

Apple opts into `header_authenticated_media_v1`. Its player engine is Aether,
which uses FFmpeg. Apple has two paths.

For HLS from the server, Apple sets a header for each request. The load
options carry an `httpRequestAuthorization` callback when the delivery is
server HLS with video
([`SA/iosApp/iosApp/Screens/Player/AetherLoadSpec.swift#L475-L482`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/AetherLoadSpec.swift#L475-L482)).
The callback accepts only the plan URL and its segments
([`SA/iosApp/iosApp/Screens/Player/PlaybackMediaAuthorization.swift#L6-L66`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackMediaAuthorization.swift#L6-L66),
[`#L170-L191`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackMediaAuthorization.swift#L170-L191)).
For each request it reads the current token. It refreshes when the token
expired or when the server refused that exact token. It starts a background
refresh when the token is close to expiry
([`SA/iosApp/iosApp/Networking/HTTPClient.swift#L1992-L2026`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/HTTPClient.swift#L1992-L2026)).
"Close" is the last 10% of the lifetime, and at most 60 seconds
([`SA/iosApp/iosApp/Networking/MediaAccessTokenExpiry.swift#L6-L12`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/MediaAccessTokenExpiry.swift#L6-L12)).

For direct play and progressive remux, Apple uses a static header and a
reload. The stream request freezes the bearer at load time. A code comment
says "Never recapture a new owner during playback"
([`SA/iosApp/iosApp/Screens/Player/StreamRequest.swift#L8-L10`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/StreamRequest.swift#L8-L10),
[`#L113-L118`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/StreamRequest.swift#L113-L118)).
The reload happens in two cases:

- After each successful progress report, Apple compares the current
  `Authorization` value with the value in the engine. If they differ, it
  reloads the same plan at the current position
  ([`SA/iosApp/iosApp/Screens/Player/PlayerViewModel.swift#L1806-L1852`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlayerViewModel.swift#L1806-L1852)).
- After a 401 from the engine, Apple sends a progress report through the
  normal API client. The 401 path of that client refreshes the token. Then
  Apple builds a new stream request and reloads the plan at the current
  position
  ([`SA/iosApp/iosApp/Screens/Player/AetherLoadSpec.swift#L47-L91`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/AetherLoadSpec.swift#L47-L91),
  [`PlayerViewModel.swift#L1945-L2010`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlayerViewModel.swift#L1945-L2010)).
- Apple reloads only when the header changed. This limits recovery to one
  reload for each token. If the header did not change, Apple sends a protocol
  v3 `replan`. If that fails, it shows a terminal error
  ([`AetherLoadSpec.swift#L51-L55`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/AetherLoadSpec.swift#L51-L55),
  [`PlayerViewModel.swift#L1912-L1925`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlayerViewModel.swift#L1912-L1925)).

### 4.2 Android (silo-android)

Android does not opt into `header_authenticated_media_v1`. Its feature list
has no such constant
([`SD/shared/src/commonMain/kotlin/org/siloserver/silo/model/playback/PlaybackProtocolV3.kt#L16-L24`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/shared/src/commonMain/kotlin/org/siloserver/silo/model/playback/PlaybackProtocolV3.kt#L16-L24)).
Its URLs carry `st`, but it still sends the bearer on each request.

Media3 opens each HTTP request through `RefreshingHttpDataSource`. That class
puts the current auth headers on every open. After a 401, it does one shared
refresh for the whole process and tries the request once more
([`SD/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/player/AuthenticatedDataSourceFactory.kt#L60-L119`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/player/AuthenticatedDataSourceFactory.kt#L60-L119)).
An OkHttp interceptor does the same for other media readers. Its comment gives
the reason: "A long HDR film outlives the JWT expiry, so without this a
mid-segment 401 would surface as a playback error instead of a transparent
retry"
([`SD/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/player/MediaAuthInterceptor.kt#L12-L27`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/player/MediaAuthInterceptor.kt#L12-L27),
[`#L54-L78`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/player/MediaAuthInterceptor.kt#L54-L78)).

Both clients put a current token on each new request. Where the Apple engine
holds a static header, Apple reloads at the current position. mpv holds a
static header in every delivery, so that Apple path is the model for
silo-omarchy.

## 5. What mpv does with `http-header-fields`

`--http-header-fields` is a string list of extra HTTP headers
([`MPV/DOCS/man/options.rst#L5714-L5735`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/options.rst#L5714-L5735)).
With the FFmpeg network code, mpv turns the list into the FFmpeg `headers`
option when it opens a stream
([`MPV/stream/stream_lavf.c#L187-L199`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/stream/stream_lavf.c#L187-L199)).

For HLS, mpv marks the format as `nested_network`
([`MPV/demux/demux_lavf.c#L180`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/demux/demux_lavf.c#L180)).
It builds the network options once when it opens the demuxer, and it stores a
copy in `priv->av_opts`
([`MPV/demux/demux_lavf.c#L1453-L1456`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/demux/demux_lavf.c#L1453-L1456),
[`#L1525-L1528`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/demux/demux_lavf.c#L1525-L1528)).
Each nested open gets that stored copy. A nested open is each playlist reload
and each segment
([`MPV/demux/demux_lavf.c#L958-L993`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/demux/demux_lavf.c#L958-L993)).
FFmpeg's HLS demuxer passes its own copy of these options to every playlist
and segment open
([`FF/libavformat/hls.c#L853-L858`](https://github.com/FFmpeg/FFmpeg/blob/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa/libavformat/hls.c#L853-L858),
[`#L1524-L1533`](https://github.com/FFmpeg/FFmpeg/blob/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa/libavformat/hls.c#L1524-L1533)).

Inference: with mpv 0.41, a `set http-header-fields` during playback does not
change the header on later segment, playlist or seek requests of the open
file. The new value takes effect on the next `loadfile`. Test this once with a
local fake server that logs the `Authorization` header.

The mpv master branch adds a libcurl network backend. It is a pending
interface change and is not in 0.41
([`MPV/DOCS/interface-changes/http-backend.txt#L1`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/interface-changes/http-backend.txt#L1),
[`MPV/RELEASE_NOTES#L1`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/RELEASE_NOTES#L1)).
The local mpv 0.41 has no `--curl-*` options. With that backend, nested HLS
opens go through the stream layer of mpv
([`MPV/demux/demux_lavf.c#L982-L993`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/demux/demux_lavf.c#L982-L993)).
Each new stream reads the network options again when it opens
([`MPV/stream/stream_curl.c#L1137`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/stream/stream_curl.c#L1137),
[`#L902-L913`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/stream/stream_curl.c#L902-L913)).
Inference: on that backend, a header change at runtime can reach new HLS
segments without a reload. Do not depend on it. The reload path works on both
backends.

`loadfile` takes per-file options as its fourth argument. mpv sets them for
that file only, and restores them after the file ends
([`MPV/DOCS/man/input.rst#L542`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/input.rst#L542),
[`#L594-L599`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/input.rst#L594-L599)).
With libmpv, pass them as an `MPV_FORMAT_NODE_MAP`.

## 6. What silo-omarchy must do

All of this section is inference from sections 1 to 5.

1. Keep `header_authenticated_media_v1`. It keeps credentials out of URLs, and
   Apple uses it. `st` gives no protection against expiry on v2. Its only gain
   is a session rebuild after a server restart.
2. Before each `loadfile` (start, replan, track change, seek re-anchor), read
   the `exp` claim of the access token. If less than about 4 hours remain,
   refresh through the normal single-flight refresh of the API client. A fresh
   8-hour token covers a 3-hour film with room for pauses and seeks.
3. Pass the headers as per-file options:
   `loadfile <url> replace -1 {http-header-fields: [...], start: <pos>}`. Put
   `Authorization`, `X-Profile-Id`, `X-Profile-Token` when present, and the
   `X-Silo-*` device headers in the list. mpv also uses them for `sub-add`
   requests to subtitle URLs.
4. Store the `exp` of the token in the loaded file, not of the newest token in
   the app. A refresh in the app does not end the old token (section 2), so a
   refresh by itself needs no reload.
5. When the token in mpv has about 10 minutes left and playback continues,
   refresh. Then reload the same plan URL at the current player position with
   the new header. This is the proactive path of Apple. The user sees a short
   rebuffer once every few hours. On an HLS copy remux, `can_seek_anywhere`
   is false. There, a reload far from the start can need a `seek_reanchor`
   replan. Test this case.
6. Watch for `end-file` with an error, and for an end of file when the position
   is still far from `source.duration_seconds`. Then refresh once and reload
   once at the saved position. If the new header is the same as the old one,
   send a `replan`. If that fails too, show an error. After
   `401 session_expired`, sign out. After `410 playback_session_ended`, start a
   new attempt.
7. After a pause of more than 30 minutes, the server can reap the session. On
   resume, use the same recovery path.

For tests, run a local fake server that issues short-lived tokens (for example
60 seconds) and logs the `Authorization` header of each media request. Make
sure that:

- Direct play continues after a seek that comes after the expiry.
- HLS continues through segment requests after the expiry.
- An early end of file never marks the item as watched.

## Open points

- Ask the Silo maintainers if `header_refresh: session` and
  `header_refresh_url` will ship. Also ask if they plan a media-scoped grant
  for native players. Either one removes the need for the reload.
- When the libcurl backend reaches an mpv release, test a runtime
  `set http-header-fields` on HLS. If it works, step 5 can skip the reload for
  HLS.
