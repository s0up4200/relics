# Research: a native Linux client for Silo on Omarchy

Date: 2026-10-10. Status: research only. Nobody built or ran code for this file.

This file collects facts for one decision: the architecture of a native Linux
client and player for the [Silo](https://github.com/Silo-Server) media server.
The target desktop is Omarchy (Arch Linux, Hyprland, Wayland).

Each fact has a link to a primary source: source code at a pinned commit,
official docs, or a spec. The label "Inference" marks a conclusion that I drew
from those facts. A source does not state it.

## Summary

- Use the native Silo API at `/api/v2`. Do not use the Jellyfin compatibility
  API. The server freezes `/api/v1` and answers `410 Gone` on it after 1.0.
  The official Apple and Android clients call only `/api/v2`.
- The server decides how media plays: direct play, remux, or transcode. The
  client only reports what it can play, in a strict "playback protocol v3"
  request. mpv plays almost any file, so the client can declare wide support
  and get the original file in most cases.
- Recommended architecture: a GPUI library browser built with gpui-omarchy,
  plus libmpv in the same process. libmpv is only the control channel. mpv
  draws its own Wayland window with its normal `gpu-next` video output. This
  design keeps HDR passthrough, hardware decoding, the idle inhibit of mpv, and
  the fullscreen HDR switch of Hyprland. It also avoids the JSON IPC bugs that
  other clients hit. The cost is a separate player window. A tiling window
  manager handles that well.
- Do not put the video inside the GPUI window now. GPUI on Linux cannot show
  an external GPU surface, and the libmpv render API cannot do HDR passthrough.
- GTK4 with libmpv is the best fallback if the video must be inside the app
  window and tone-mapped SDR is acceptable.

## Sources and pinned commits

| Repository | Commit |
| --- | --- |
| [Silo-Server/silo-server](https://github.com/Silo-Server/silo-server) | `1c127801c358a671460bba374964a1090cce79d1` |
| [Silo-Server/silo-apple](https://github.com/Silo-Server/silo-apple) | `5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba` |
| [Silo-Server/silo-android](https://github.com/Silo-Server/silo-android) | `e535ddceb5bedd22b75d0e53abca27b3640982df` |
| [mpv-player/mpv](https://github.com/mpv-player/mpv) | `accb7b2821ecf0cb04cf3579ccd98bc8f751f61f` |
| [mpv-player/mpv-examples](https://github.com/mpv-player/mpv-examples) | `e0d1a84c99e8c469b58fde31a1541401acfb0eb2` |
| [zed-industries/zed](https://github.com/zed-industries/zed) (gpui) | `96984225abc235519e66b5c6c9ed23b87621f868` |
| [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit) | `ed27c6293b585acc40c10be7138534243bee5841` |
| [longbridge/gpui-fast](https://github.com/longbridge/gpui-fast) | `b8f0c026553711c1e049fee501932d3801f23567` |
| local `/home/soup/github/oss/gpui-omarchy` | `9114f116304516b5287b26d3a3c1e5bb531d02d2` |
| [kohsine/libmpv-rs](https://github.com/kohsine/libmpv-rs) (crate `libmpv2`) | `d7ccfafff663d7c731d33383f6ec74247c8dd1dc` |
| [codeberg avery42/delfin](https://codeberg.org/avery42/delfin) | `820b466bfd47f71c12e9b2cabb698e8f78942f41` |
| [jellyfin/jellyfin-desktop](https://github.com/jellyfin/jellyfin-desktop) | `2cb4a4456fd29b5b62d825ef0e5df93ed6913328` |
| [andrewrabert/jellium-desktop](https://github.com/andrewrabert/jellium-desktop) | `14dc0845981cb7c0aef5fa93ff539d927c8f0eb2` |
| [jellyfin/jellyfin-mpv-shim](https://github.com/jellyfin/jellyfin-mpv-shim) | `437ac000aaec27e3b43042b31ad2f3d4cad2cd47` |
| [jeffvli/feishin](https://github.com/jeffvli/feishin) | `9dc15477c86fbd48d1a98fa878769d09811895e8` |
| [basecamp/omarchy](https://github.com/basecamp/omarchy) | `77bfaca408d2993018d928e72cf84561480e5188` |
| [hyprwm/hyprland-wiki](https://github.com/hyprwm/hyprland-wiki) | `b8b7239a5e8ffe69203654a53c7aa9206b6786bd` |

The local machine at research time had mpv 0.41.0 with libplacebo 7.360.1,
Hyprland 0.56.2, gnome-keyring 50.0 and libsecret 0.21.7 (`pacman -Q`).

The links in the text use these prefixes:

- `SS` is `https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1`
- `SA` is `https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba`
- `SD` is `https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df`
- `MPV` is `https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f`
- `ZED` is `https://github.com/zed-industries/zed/blob/96984225abc235519e66b5c6c9ed23b87621f868`
- `DF` is `https://codeberg.org/avery42/delfin/src/commit/820b466bfd47f71c12e9b2cabb698e8f78942f41`
- `JD` is `https://github.com/jellyfin/jellyfin-desktop/blob/2cb4a4456fd29b5b62d825ef0e5df93ed6913328`
- `JS` is `https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47`
- `FS` is `https://github.com/jeffvli/feishin/blob/9dc15477c86fbd48d1a98fa878769d09811895e8`
- `OM` is `https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188`

Each link text shows the prefix and path. The link target is the full URL.

---

## 1. The native client API of Silo

### 1.1 Which API to use

- Silo is pre-release. "APIs, configuration, and database migrations may
  change before the first stable release"
  ([`SS/README.md#L31-L33`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/README.md#L31-L33)).
- `/api/v2` is the first stable native API. It locks at Silo 1.0. `/api/v1`
  is an alpha contract. The project freezes it, keeps it through a short
  bridge, and then removes it
  ([`SS/docs/architecture/api-contract.md#L3-L17`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/api-contract.md#L3-L17)).
  After the bridge, the server answers the full `/api/v1` namespace with
  `410 Gone` and `client_upgrade_required`
  ([`SS/docs/auth-api.md#L3-L7`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L3-L7)).
- Huma generates the contract from Go types into
  `contracts/api/v2/openapi.json`. Each server serves it at
  `/api/v2/openapi.json` and serves a Swagger UI at `/api/v2/docs`. After 1.0,
  a rename of an operation ID or a schema name is a breaking change
  ([`SS/docs/architecture/api-contract.md#L57-L113`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/api-contract.md#L57-L113),
  [`SS/docs/api-docs.md`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/api-docs.md)).
  At this commit the file has 685 paths. Inference: generate the Rust request
  and response types from this file, or test them against it in CI.
- `GET /api/v2/system/info` returns `server_version`, `api_major` and
  `contract_digest`, the SHA-256 of the served OpenAPI bytes
  ([`SS/docs/architecture/api-contract.md#L99-L103`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/api-contract.md#L99-L103)).
  Apple uses it as its v2 probe. On a server with v1 only, Apple shows "server
  update required"
  ([`SA/iosApp/iosApp/Networking/APIv2/APIv2Probe.swift#L36`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/APIv2/APIv2Probe.swift#L36)).
- Apple never replays a failed v2 call on v1. Its CI keeps an allow-list for
  v1 mentions
  ([`SA/docs/native-api-v2.md#L1-L10`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/docs/native-api-v2.md#L1-L10)).
  The server playback doc still says that Apple and Android use v1 until they
  adopt v2
  ([`SS/docs/playback-api.md#L359-L368`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L359-L368)).
  The client code shows that this line is out of date.

Use the native API, not the Jellyfin compatibility API ("jellycompat"):

- Jellycompat is "a supported subset of the Jellyfin protocol" that emulates
  Jellyfin 12.1
  ([`SS/docs/jellycompat-api.md#L1-L9`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/jellycompat-api.md#L1-L9)).
  The doc lists missing parts, for example `/Devices`, `/Playlists`,
  QuickConnect initiation, SyncPlay groups and remembered subtitle selections
  ([`SS/docs/jellycompat-api.md#L503-L514`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/jellycompat-api.md#L503-L514)).
- Some Silo features have no Jellyfin shape and exist only on the native API.
  Examples are household profiles with PINs, multiple marker ranges, the
  intro-skip mode, device sign-in, OIDC with native PKCE and the v3 replan
  steps.
- Jellyfin-compatible sign-ins refuse an account with a temporary password,
  because those clients cannot do the password change
  ([`SS/docs/auth-api.md#L176-L178`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L176-L178)).
- Inference: jellycompat is useful to test Silo with an existing Jellyfin
  client today. A new client built on it gets only the subset and never the
  Silo features.

### 1.2 Request headers

- Each request carries `Authorization: Bearer <access token>`. Requests in a
  profile scope add `X-Profile-Id`, and `X-Profile-Token` after a PIN check
  ([`SA/iosApp/iosApp/Networking/HTTPClient.swift#L1264-L1289`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/HTTPClient.swift#L1264-L1289)).
- Device identity uses `X-Silo-Device-Id`, `X-Silo-Device-Name` and
  `X-Silo-Device-Platform`. The id has at most 128 characters from letters,
  digits, `.`, `_`, `:` and `-`. On v2 the server refuses a bad id with
  `422 validation_failed`. The server uses these values for display and audit
  only
  ([`SS/docs/auth-api.md#L254-L265`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L254-L265)).
  A playable start registers the device for the profile
  ([`SS/docs/playback-api.md#L106-L113`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L106-L113)).
- `X-Silo-Client-Family` has a closed set of values: `tv`, `mobile`,
  `tablet`, `desktop`, `web`. A Linux desktop client sends `desktop`
  ([`SS/docs/settings-api.md#L700-L723`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/settings-api.md#L700-L723)).
- App identity uses `X-Silo-Client`, `X-Silo-Client-Version`,
  `X-Silo-Client-Build` and `X-Silo-Client-Channel`. The server trims these
  values and never rejects them. The admin Activity page shows them
  ([`SS/docs/settings-api.md#L725-L761`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/settings-api.md#L725-L761)).
- v2 errors are `application/problem+json`. The stable id is the last segment
  of `type`
  ([`SA/iosApp/iosApp/Networking/APIv2/APIv2Models.swift#L310-L318`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/APIv2/APIv2Models.swift#L310-L318)).

### 1.3 Authentication

Password login and tokens:

- `POST /api/v2/auth/login` with `{username, password, provider?}` returns
  `{access_token, refresh_token, expires_in, user}`.
  `POST /api/v2/auth/refresh` with `{refresh_token}` returns a new
  `{access_token, refresh_token, expires_in}`. `POST /api/v2/auth/logout`
  ends the session. These are the operations `login`, `refreshSession` and
  `logout` in `contracts/api/v2/openapi.json`.
- The default lifetime is 8 hours for the access token and 30 days for the
  refresh token, which is the login session
  ([`SS/internal/config/config.go#L584-L585`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/internal/config/config.go#L584-L585)).
  Each refresh moves the session expiry forward
  ([`SS/internal/auth/service.go#L1101-L1104`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/internal/auth/service.go#L1101-L1104)).
  Keep the new refresh token from each refresh.
- Do not replay login, setup, signup, OAuth completion, refresh, device start
  or device poll after an unclear answer
  ([`SS/docs/auth-api.md#L602-L628`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L602-L628)).
- If a request gets `401 token_refresh_required` (after a role change),
  refresh and retry once. Do not sign out. Only a refused refresh
  (`401 session_expired`) ends the session
  ([`SS/docs/auth-api.md#L285-L298`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L285-L298)).
- If a request gets `503` with `Retry-After`, the server could not check the
  session. Keep the tokens, wait, and retry. Sign out only on a 401
  ([`SS/docs/auth-api.md#L300-L325`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L300-L325)).
- Native clients "must distinguish failed credential exchanges from an expired
  bearer on an authenticated read, retain refresh concurrency protection, and
  preserve the selected account and profile"
  ([`SS/docs/auth-api.md#L649-L654`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L649-L654)).
- A temporary password limits the session to four operations until the user
  sets a new password. The `Account` document reports
  `password_change_required`
  ([`SS/docs/auth-api.md#L152-L178`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L152-L178)).

API keys exist (`docs/api-keys-api.md`). They cannot get realtime socket
tickets
([`SS/docs/realtime-api.md#L25-L31`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/realtime-api.md#L25-L31))
and cannot do several account operations. Inference: a player uses a normal
login session, not an API key.

Household profiles: call `GET /api/v2/profiles`. For a PIN profile, call
`POST /api/v2/profiles/{id}/verify-pin`. A match returns the
`X-Profile-Token`. A wrong PIN answers 200 with `valid: false`. Five wrong
PINs lock the profile for five minutes (`429` with `Retry-After`). Never
replay a PIN check by itself
([`SS/docs/auth-api.md#L327-L367`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L327-L367)).

Device sign-in is the Silo equivalent of Jellyfin Quick Connect. The device
calls `POST /auth/device/start`, shows an eight-digit `user_code` and a QR
link (`verification_uri_complete`), and polls `POST /auth/device/poll`. A
person who is already signed in approves it somewhere else. A code lives 900
seconds
([`SS/docs/auth-api.md#L369-L388`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L369-L388)).
Inference: this is the easiest sign-in for a desktop app on a server with
OIDC, because it needs no custom URL scheme.

OIDC and LDAP come from `auth_provider.v1` plugins. `GET /auth/providers`
lists them. LDAP goes through `POST /auth/login`. OAuth uses a native PKCE
flow ([`SS/docs/auth-api.md#L390-L539`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L390-L539)):

1. The app opens
   `<saved base URL><native_start_path>?code_challenge=…&code_challenge_method=S256&app_state=…`
   in the system browser.
2. The server redirects to
   `org.siloserver.silo:/auth/callback?code=…&state=…&server=…&iss=…`.
3. The app makes sure that `state`, `server` and `iss` match what it saved.
4. The app calls `POST /auth/oauth/complete` with the code and the
   `code_verifier`.

The server doc fixes the redirect scheme. Inference: a Linux app can register
`x-scheme-handler/org.siloserver.silo` in its `.desktop` file, but then it
shares the scheme with any other Silo app on the machine. Ask the Silo
maintainers before you use the official scheme. Device sign-in avoids the
problem.

Server discovery: each API process announces DNS-SD `_silo._tcp` with a TXT
`id`. The id is only a hint. Make sure that `GET /api/v2/system/identity`
matches before you use the server
([`SS/docs/architecture/server-discovery.md#L10-L35`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/server-discovery.md#L10-L35)).
A server base URL can include a path prefix from a reverse proxy
([`SA/iosApp/iosApp/Networking/HTTPClient.swift#L1795-L1806`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/HTTPClient.swift#L1795-L1806)).

### 1.4 Catalog and browse

The main v2 reads come from `contracts/api/v2/openapi.json`. Apple uses them
in
[`SA/iosApp/iosApp/Networking/APIv2/APIv2Client.swift#L491-L975`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/APIv2/APIv2Client.swift#L491-L975).

- Home: `GET /home/layout`, `GET /home/sections`,
  `GET /home/sections/{id}/items` and dismissals.
- Libraries: `GET /user/libraries`.
- Catalog: `GET /catalog`, `POST /catalog/query`, `GET /catalog/filters`,
  `GET /catalog/items/{id}`, `GET /catalog/items/{id}/versions`,
  `GET /catalog/series/{id}/seasons`, `.../seasons/{num}/episodes` and
  `GET /catalog/people/{id}`.
- Personal lists: `/favorites`, `/watchlist`, `/history` and `/progress`.
- Watch detail: `GET /watch/{id}` returns versions (files), tracks, the resume
  position and markers
  ([`SA/iosApp/iosApp/Networking/APIv2/APIv2CatalogReadModels.swift#L260-L304`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/APIv2/APIv2CatalogReadModels.swift#L260-L304)).
- Seek previews: `GET /watch/{id}/trickplay?file_id=` when the server
  announces `trickplay_v1`
  ([`SS/docs/playback-api.md#L57-L69`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L57-L69)).

Collections use cursor pages with `items` and `page` (`next_cursor`,
`has_more`)
([`SS/docs/auth-api.md#L267-L273`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L267-L273)).
Capability documents (`GET …/capabilities`) carry `state`, `allowed` and
`revision`, with ETag support. Clients read them and do not guess from a
version number
([`SS/docs/auth-api.md#L88-L92`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/auth-api.md#L88-L92)).

### 1.5 Images

- Artwork URLs come inside responses. Each one is a direct object-storage URL
  or a signed server route (`/api/v2/artwork/{key}`). The client does not
  build them
  ([`SS/docs/images-api.md#L1-L13`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/images-api.md#L1-L13)).
- Add `image_size=small|medium|large|original` to a read to set the size of
  each URL in the response. Read `GET /images/capabilities` first, and send
  the parameter only when `state` is `available`
  ([`SS/docs/images-api.md#L15-L60`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/images-api.md#L15-L60),
  [`#L95-L125`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/images-api.md#L95-L125)).
- The clients join a server-relative artwork path to the server origin and
  send no credentials with it
  ([`SA/iosApp/iosApp/Shared/ArtworkURL.swift#L1-L24`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Shared/ArtworkURL.swift#L1-L24)).

### 1.6 Playback

Playback uses protocol v3 inside the v2 envelope. The normative spec is
[`SS/docs/architecture/playback-protocol-v3.md`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md),
together with the JSON Schemas in `docs/design/schemas/playback-v3/`. The spec
says that a third-party client can implement playback from the spec and the
schemas alone
([`#L7-L16`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L7-L16)).
[`SS/docs/playback-api.md`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md)
describes the v2 envelope. Below, links that start with `#L` point into the v3
spec.

The flow:

1. Call `GET /api/v2/playback/capabilities` and keep `installation_id`. Each
   playback mutation sends it back. If the value changed, the server answers
   `409 installation_changed`. Read the capabilities again and start again
   ([`SS/docs/playback-api.md#L21-L41`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L21-L41)).
2. Call `POST /api/v2/playback/start` with the v3 start request. It returns
   `201` for every decision. "Cannot play" is also a 201, with `outcome:
   "adaptation_unavailable"` and a `terminal` reason. A 4xx means a bad
   request, never "this media cannot play". A `426` means that the client is
   too old
   ([`#L55-L66`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L55-L66),
   [`#L153-L182`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L153-L182)).
   Start is idempotent (safe to send twice) on `playback_attempt_id` plus the
   body digest. Make a new attempt id each time the user presses Play. Reuse it
   only to retry a lost answer
   ([`#L184-L200`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L184-L200)).
3. Load `playback_plan.stream.url` in the player. Seek to
   `timeline.player_start_seconds`.
4. Send progress every 10 seconds. Stop with `DELETE` when playback ends.
5. Use `POST /playback/{session_id}/replan` for track, quality, output and
   seek changes, and for recovery after a failure.

The server owns the decision. "A client reports what it can do; the server
decides what will be sent and how." The plan is the full instruction
([`#L27-L30`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L27-L30)).
The server has four deliveries: `original_http` (direct play),
`server_remux_progressive`, `server_remux_hls` and `server_transcode_hls`. The
client negotiates in three classes: `original_http`, `progressive` and `hls`
([`#L643-L667`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L643-L667)).

How the client declares what it can play. The start request
([schema](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/design/schemas/playback-v3/v3/start-request.schema.json))
requires `protocol_version: 3`, `file_id`, `profile_id`,
`playback_attempt_id`, `subtitle_fidelity_preference` (`preserve` or
`compatible`), `client_capabilities` and `client_playback_context`.

- `client_capabilities` has `video_evidence` and `audio_evidence`. Each one is
  `exact`, `platform_attested` or `declared`. It also has flat codec and
  container lists, `max_resolution`, `hdr_details` and `video_decode[]`.
- The evidence tier tells the server how strict to be. `exact` (Android,
  from `MediaCodecList`) gets a strict check of `video_decode[]`.
  `platform_attested` (Apple) skips profile and level for hardware entries.
  `declared` (web) is a flat list match only
  ([`#L446-L476`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L446-L476)).
  On a strict tier, a codec in the flat list without a `video_decode[]` entry
  is a refusal. Software decode counts only with `software_video_decode_v1`
  ([`#L467-L488`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L467-L488)).
  Audio passthrough needs `exact` audio evidence
  ([`#L539-L547`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L539-L547)).
- `client_playback_context.deliveries` has one entry per class. Each entry has
  `enabled`, `supported_on_device`, `containers`, `video_codecs`,
  `audio_decode_codecs`, `max_channels`, `hdr_details`, `subtitles`,
  `features` and `validated_claims`. The subtitle fields are `sidecar_text`,
  `ass_styling`, `embedded_bitmap`, `sidecar_bitmap`, `font_attachments` and
  `native_embedded`. The server treats a class that the client leaves out as
  unavailable
  ([`#L663-L689`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L663-L689)).
- For HDR, `output.hdr_details` (the attached display) wins over the device
  value
  ([`#L549-L582`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L549-L582)).
  The claim `client_managed_dynamic_range_v1` on `original_http` says that the
  player maps the source range to the live output by itself. The server can
  then send HDR or Dolby Vision to a display that does not announce it
  ([`#L584-L591`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L584-L591)).
- The claim `client_selected_audio_track_v1` on `original_http` says that the
  client maps the selected audio index to its own probe. A non-default audio
  track then does not force a remux
  ([`#L655-L661`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L655-L661)).
- The full Apple request with real values is a good template
  ([`SA/iosApp/iosApp/Screens/Player/ProtocolV3/ApplePlaybackV3Capabilities.swift#L33-L270`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/ProtocolV3/ApplePlaybackV3Capabilities.swift#L33-L270)).
  It lists containers in both spellings (`mkv` and `matroska`, `ts` and
  `mpegts`), because the server matches the spelling of its scanner
  ([`SA/iosApp/iosApp/Shared/AppleDecodeCapabilities.swift#L88-L156`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Shared/AppleDecodeCapabilities.swift#L88-L156)).

Inference, for an mpv client: mpv decodes nearly every codec with FFmpeg in
hardware or software. It renders ASS with libass, shows PGS, and maps HDR to
the output by itself. A first profile that is true to this has these parts:

- `declared` evidence for video and audio, with wide flat codec and container
  lists.
- `original_http` enabled, with the claims `client_managed_dynamic_range_v1`
  and `client_selected_audio_track_v1`.
- `progressive` and `hls` enabled as fallbacks.
- All subtitle kinds set to true.

This matches what the Jellyfin clients with mpv do (section 4). A later step
can move to `exact` evidence, with `video_decode[]` entries from a VA-API
probe and software entries with `hardware: false`. Ask the Silo maintainers if
`declared` from a client that is not a browser is acceptable.

Stream URLs and authentication:

- With `header_authenticated_media_v1` in `client_features`, plan URLs are
  relative and carry no credential: `/stream/{session_id}`,
  `/playback/transcode/{session_id}/master.m3u8`, and subtitle URLs under
  `/stream/{session_id}/subtitles/…`. The client sends its normal
  `Authorization` header with each media, segment, subtitle and font request
  ([`#L704-L733`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L704-L733),
  [`#L816-L823`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L816-L823)).
  Apple uses this mode
  ([`SA/iosApp/iosApp/Screens/Player/StreamRequest.swift#L80-L118`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/StreamRequest.swift#L80-L118)).
  mpv sends the header with `--http-header-fields`
  ([`MPV/DOCS/man/options.rst#L5714`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/options.rst#L5714)).
- In this mode the session lives only in the memory of the API process that
  started it. A server with several API replicas needs session affinity
  ([`#L833-L843`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L833-L843)).
  Without the opt-in, URLs carry a signed `st` token, and the server can
  rebuild a session after a restart
  ([`SS/docs/playback-api.md#L267-L274`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L267-L274)).
- Use plan URLs exactly as the server sends them. Never build a URL, drop a
  parameter, or move a parameter to a URL of a different kind
  ([`#L845-L870`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L845-L870)).
- Inference, open question: the access token lives 8 hours. mpv reads
  `http-header-fields` when it opens a stream. If the client refreshes the
  token during a long HLS session, later segment requests can carry the old
  header. Test this case. Two options are a refresh right before the start, or
  a property update and a reload after a 401.

The timeline: the zero of the player and the zero of the source are often
different. The plan gives `source_start_seconds`, `stream_origin_seconds`,
`player_start_seconds` and `timeline_offset_seconds`. The source position is
the player position plus `timeline_offset_seconds`. On an HLS copy remux,
`can_seek_anywhere` is false, and each seek goes back through the server as a
`seek_reanchor` replan. Use `source.duration_seconds` and never the duration
from the player, because an HLS remux grows while it plays
([`#L874-L928`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L874-L928)).
The spec calls the zero mix-up "the single most common client bug".

Audio and subtitle tracks:

- Track ids have the form `file:{media_file_id}:{kind}:{ordinal}`. Subtitles
  use one combined number range: external first, then embedded, then
  downloaded. A bitmap track without a sidecar keeps its number, with
  `delivery: "burn_in_only"`
  ([`#L1328-L1351`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L1328-L1351)).
- `playback_plan.subtitle.inventory` is the only source of truth. Select a
  track with its `track_id` or `combined_index`. Never count tracks to get a
  number
  ([`#L1353-L1356`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L1353-L1356)).
  The plan includes sidecar URLs even when subtitles are off, so the client
  can build the menu from the first plan.
- `subtitle.mode` is `off`, `render`, `convert` (the server makes WebVTT) or
  `burn_in` (forces a transcode). The server sends ASS as `.ass` and PGS as
  `.sup`. Embedded ASS has a `font_bundle_url`
  ([`#L1370-L1430`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L1370-L1430)).
  A font bundle is JSON `{items: [{name, data}]}`
  ([`SS/docs/playback-api.md#L257-L265`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L257-L265)).
- To select an embedded subtitle in the original file, the client sends
  `embedded_subtitles_v1` and a `native_embedded` entry with `track_identity`
  set to `ffmpeg_stream_index` or `container_track_id`. The plan then names
  the exact stream. Inference: mpv shows the FFmpeg stream index as
  `track-list/N/ff-index`
  ([`MPV/DOCS/man/input.rst#L3642-L3646`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/input.rst#L3642-L3646)).
  An mpv client can map the stream index to an mpv `sid` and keep full ASS
  styling.
- For an external subtitle, fetch the inventory `url` with the same
  authorization header and add it with the mpv command `sub-add`. For ASS
  fonts, write the font bundle to a temporary directory and set `sub-fonts-dir`
  to it (inference).
- Track changes go through `replan` with `operation: "track_change"`. In a
  track change, a missing subtitle field means "subtitles off"
  ([`#L930-L1000`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L930-L1000)).

### 1.7 Progress, resume and watched state

- `POST /api/v2/playback/{session_id}/progress` takes
  `{installation_id, sequence, position, is_paused}`. `sequence` is a positive
  integer that increases with each sample. The server keeps the highest
  sequence, so a late sample does no harm. The outcomes are `applied`,
  `stale_sample` and `replayed`. The same sequence with different data gets
  `409 progress_conflict`
  ([`SS/docs/playback-api.md#L189-L218`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L189-L218)).
- `DELETE /api/v2/playback/{session_id}` takes `{installation_id, stop_id}`
  and an optional final sample. Make `stop_id` once and keep it for retries.
  The first stop wins. Later stops answer `replayed`
  ([`SS/docs/playback-api.md#L220-L240`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L220-L240)).
- Progress is the heartbeat. The server ends a session after 45 seconds
  without activity, or after 30 minutes if the last report said "paused".
  Send progress every 10 seconds, while playing and while paused. If progress
  or replan gets `404 playback_session_not_found`, the session ended. Start a
  new attempt if the user still watches
  ([`#L306-L376`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L306-L376)).
- `position` is the source position: the player position plus
  `timeline_offset_seconds`.
- Resume: if the client leaves out `start_position`, the server uses the saved
  resume point of the profile. A value overrides it, and `0` means "start
  over"
  ([`#L202-L225`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L202-L225)).
- The server decides the watched state. It ignores samples below a minimum
  resume fraction and marks an item complete above a watched fraction
  ([`SS/internal/watchstate/service.go#L109-L136`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/internal/watchstate/service.go#L109-L136)).
- Offline positions and timelines that the client owns go up through
  `POST /api/v2/sync/progress`
  ([`SS/docs/progress-api.md#L87-L100`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/progress-api.md#L87-L100)).

### 1.8 Markers for intro and credits skip

- Watch detail has `marker_segments` on each file version:
  `{kind, start_seconds, end_seconds}`, with kind `intro`, `credits`, `recap`
  or `preview`. A kind can occur more than once. Skip only the active range
  ([`SS/docs/playback-api.md#L43-L55`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/playback-api.md#L43-L55)).
- Direct reads are `GET /api/v2/markers/files/{file_id}` and
  `/markers/items/{item_id}`
  ([`SS/docs/markers-api.md#L106-L134`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/markers-api.md#L106-L134)).
- The setting `playback.intro_skip_mode` is `never`, `ask` (the default) or
  `always`. All clients use one shared state machine for the prompt
  ([`SS/docs/design/2026-08-16-intro-skip-mode.md`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/design/2026-08-16-intro-skip-mode.md)).

### 1.9 Realtime

There is no SSE. Both realtime channels are WebSockets.

- The playback control socket is optional and belongs to one session. Call
  `POST /api/v2/playback/sessions/{id}/control/ws-ticket`, then open a
  WebSocket to `/api/v2/playback/sessions/{id}/control/ws` with the
  subprotocols `silo.playback-control.v2` and `silo.ticket.<ticket>`. A ticket
  lives 30 seconds and never goes in the URL. A connection ends after 4 hours
  ([`SS/docs/realtime-api.md#L120-L166`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/realtime-api.md#L120-L166)).
- The client sends `hello` with the commands it supports. The server can send
  `pause`, `unpause`, `seek`, `stop`, `terminate`, `display_message`,
  `server_restarting`, `plan_invalidated` and others. If a command has a
  `deadline_ms` and gets no `result` in time, the server stops the session
  ([`#L412-L442`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/architecture/playback-protocol-v3.md#L412-L442)).
- The events socket covers the account. Call `POST /api/v2/events/ws-ticket`,
  then open `/api/v2/events/ws` with `silo.events.v2` and
  `silo.ticket.<ticket>`. A connection ends after 5 minutes or at the expiry
  of the access token, so the client reconnects with a new ticket. On
  `access_changed` (close code 4001), fetch libraries, home and details again
  ([`SS/docs/realtime-api.md#L23-L111`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/realtime-api.md#L23-L111)).

### 1.10 Downloads

`docs/downloads-api.md` is a full client guide of about 2,000 lines, with
notes for Apple and Android
([`SS/docs/downloads-api.md`](https://github.com/Silo-Server/silo-server/blob/1c127801c358a671460bba374964a1090cce79d1/docs/downloads-api.md)).
Inference: a desktop on the home network has little need for offline
downloads. Leave them out of the first milestone.

---

## 2. How the official clients behave

Both official clients call only `/api/v2` and agree on most behavior. They
are the reference for correct client behavior.

| Topic | Apple (silo-apple) | Android (silo-android) |
| --- | --- | --- |
| HTTP layer | One `HTTPClient` actor on URLSession ([`HTTPClient.swift#L75-L94`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/HTTPClient.swift#L75-L94)) | Ktor on OkHttp with a custom auth plugin ([`SiloHttpClientImpl.kt#L38-L102`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/shared/src/commonMain/kotlin/org/siloserver/silo/network/SiloHttpClientImpl.kt#L38-L102)) |
| JSON | Snake case decode | Lenient: `ignoreUnknownKeys`, `coerceInputValues` ([`SiloHttpClientImpl.kt#L17-L23`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/shared/src/commonMain/kotlin/org/siloserver/silo/network/SiloHttpClientImpl.kt#L17-L23)) |
| Token storage | Keychain, one slot per server id ([`ServerRegistry.swift#L115-L125`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/ServerRegistry.swift#L115-L125)) | EncryptedSharedPreferences, keys per server ([`SecureSharedPrefs.kt#L14-L28`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/shared/src/androidMain/kotlin/org/siloserver/silo/network/SecureSharedPrefs.kt#L14-L28), [`EncryptedTokenManagerImpl.kt#L17-L33`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/shared/src/androidMain/kotlin/org/siloserver/silo/network/EncryptedTokenManagerImpl.kt#L17-L33)) |
| Refresh | Before expiry, margin 10% of lifetime, at most 60 s ([`MediaAccessTokenExpiry.swift#L4-L17`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/MediaAccessTokenExpiry.swift#L4-L17)) | Before expiry, margin 60 s, at most half the lifetime |
| Plain HTTP | Allowed | Asks the user to approve each cleartext origin before it sends credentials ([`CleartextOriginConsent.kt#L3-L25`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/shared/src/commonMain/kotlin/org/siloserver/silo/network/CleartextOriginConsent.kt#L3-L25)) |
| Player engine | AetherEngine, based on FFmpeg ([`project.yml#L60-L66`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/project.yml#L60-L66)) | Media3 ExoPlayer with libass and an FFmpeg audio extension ([`libs.versions.toml#L112-L120`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/gradle/libs.versions.toml#L112-L120)) |
| Evidence tier | `platform_attested` | `exact`, from MediaCodecList and display probes ([`PlaybackCapabilityDetector.kt#L336-L398`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/player/PlaybackCapabilityDetector.kt#L336-L398)) |
| Progress cadence | 10 s loop ([`PlayerViewModel.swift#L8837-L8860`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlayerViewModel.swift#L8837-L8860)) | 10 s loop, `PROGRESS_REPORT_INTERVAL_MS = 10_000` ([`PlaybackSessionLifecycle.kt#L666-L680`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/player/PlaybackSessionLifecycle.kt#L666-L680)) |
| Resume rule | Starts from 0 within 5 s of the end ([`PlaybackSessionBridge.swift#L1888-L1938`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackSessionBridge.swift#L1888-L1938)) | Goes back 7 s on a resume of 30 s or more |
| Events socket | Not used yet ([`realtime-updates-spec.md#L1-L30`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/docs/realtime-updates-spec.md#L1-L30)) | Used to refresh Home |
| Offline progress | Queued and sent through `POST sync/progress` ([`OfflineProgressQueue.swift#L1-L20`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Downloads/OfflineProgressQueue.swift#L1-L20)) | Kept on the device only ([`RoomUserItemStateRepository.kt#L207-L253`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/data/repository/RoomUserItemStateRepository.kt#L207-L253)) |
| Server discovery | DNS-SD and Tailscale names ([`ServerDiscovery.swift#L7-L29`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/ServerDiscovery.swift#L7-L29)) | None |

Behavior that both clients share, and that a Linux client copies:

1. Probe `GET /api/v2/system/info` on connect. If v2 answers a plain 404,
   show "server too old". Never fall back to v1.
2. Send the headers from section 1.2 with each request, and leave out empty
   values. Login and refresh carry the device headers but no bearer. Never
   send credentials or `X-Silo-*` headers to another origin.
3. Make the device id once as a random UUID
   ([`AppleDeviceIdentity.swift#L64-L84`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Shared/AppleDeviceIdentity.swift#L64-L84)).
   Keep it outside the token store of each server. On Linux, the host name is
   a good device name.
4. When several requests get a 401 at the same time, do one shared refresh
   and one retry
   ([`HTTPClient.swift#L1047-L1070`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/HTTPClient.swift#L1047-L1070)).
   Retry GET and HEAD. Retry a mutation only if the contract marks it safe
   (`x-silo-retry-safety`)
   ([`HTTPClient.swift#L1808-L1822`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/HTTPClient.swift#L1808-L1822)).
   Never retry playback start, replan, stop or route events after a refresh.
   Sign out only if refresh answers 400, 401 or 403
   ([`HTTPClient.swift#L2408-L2410`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/HTTPClient.swift#L2408-L2410)).
5. Tie each response to the server, credential generation and profile that
   sent it. Drop a late answer for an old profile or server
   ([`TokenStore.swift#L18-L50`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/TokenStore.swift#L18-L50),
   [`AuthInterceptorImpl.kt#L149-L160`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/shared/src/commonMain/kotlin/org/siloserver/silo/network/AuthInterceptorImpl.kt#L149-L160)).
6. Require a selected profile before playback
   ([`PlaybackSessionBridge.swift#L922-L931`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackSessionBridge.swift#L922-L931)).
   A `403 profile_verification_required` means: ask for the PIN again.
7. Play in this order: playback capabilities, `GET watch/{id}`, then
   `POST playback/start` with a real capability profile. Follow the plan.
   Apple makes attempt ids of the form `apple:<uuid>` and sends
   `subtitle_fidelity_preference: "preserve"`
   ([`PlaybackSessionBridge.swift#L1083-L1125`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackSessionBridge.swift#L1083-L1125)).
   If the user leaves during the start call, Apple stops the new session so
   that it does not stay open
   ([`PlaybackSessionBridge.swift#L1127-L1140`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackSessionBridge.swift#L1127-L1140)).
8. Join the relative stream URL to the server URL and add the bearer and
   profile headers. Accept only `/api/v2/stream/` and
   `/api/v2/playback/transcode/` paths, and only the `seek` query on media
   ([`StreamRequest.swift#L80-L118`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/StreamRequest.swift#L80-L118)).
9. Report progress every 10 seconds with an increasing `sequence`. Pause and
   seek go out with the next tick. On close, send `DELETE` with a fixed
   `stop_id` and a final sample
   ([`APIv2Client+Playback.swift#L51-L86`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/APIv2/APIv2Client+Playback.swift#L51-L86)).
   A 404, or a 409 `installation_changed`, means that the session is gone.
   Start a new one at the current position
   ([`PlaybackSessionBridge.swift#L2105-L2114`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackSessionBridge.swift#L2105-L2114)).
10. Intro skip: `ask` shows a "Skip Intro" button for 5 seconds. `always`
    skips at once and offers "Watch Intro" to undo
    ([`IntroSkipPrompt.swift#L1-L50`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/IntroSkipPrompt.swift#L1-L50)).
    If the credits run to the end of the file, Apple opens "Next Up" and does
    not seek
    ([`PlayerViewModel.swift#L6432-L6470`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlayerViewModel.swift#L6432-L6470)).
11. On the playback control socket, Apple handles the admin commands and the
    events `markers_updated` and `subtitle_ready`. It reconnects after 0.5,
    1, 2 and 5 seconds, and stops trying after 8 failures in a row. Local
    playback continues
    ([`PlaybackRealtimeProtocol.swift#L11-L135`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackRealtimeProtocol.swift#L11-L135),
    [`PlaybackRealtimeClient.swift#L20-L75`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Screens/Player/PlaybackRealtimeClient.swift#L20-L75)).

---

## 3. UI toolkit and player options

### 3.1 Facts that decide the comparison

gpui-omarchy (local library). It is a gpui-kit component library for Omarchy.
It uses gpui-kit 0.7.1, which resolves to `gpui-pre` 0.3.8, a weekly
crates.io snapshot of the gpui from Zed
(`/home/soup/github/oss/gpui-omarchy/Cargo.toml:L10-L11`,
`Cargo.lock:L2310-L2321`). `gpui_omarchy::init(cx)` reads
`~/.local/state/omarchy/current/theme/colors.toml` and follows theme changes
live (`README.md:L27-L42`, `src/system_theme.rs:L85-L126`,
`src/system_theme/watch.rs:L32-L49`). It has 54 components, `virtual_list`
among them, but no video, texture or GPU surface component (`src/lib.rs:L9-L80`,
`src/surface.rs:L300-L326`). The README still names gpui-kit 0.7.0, but
`Cargo.toml` needs 0.7.1 (`README.md:L50-L54`).

- gpui-kit pins the gpui snapshot exactly, because "any snapshot may change
  GPUI's API"
  ([gpui-kit `Cargo.toml#L63-L72`](https://github.com/longbridge/gpui-kit/blob/ed27c6293b585acc40c10be7138534243bee5841/Cargo.toml#L63-L72)).
  A new version comes out each week (crates.io: 0.7.0 on 2026-09-28, 0.7.1 on
  2026-10-05).
- On 2026-10-10, gpui-kit 0.7.1, gpui-pre 0.3.8 and gpui-omarchy 0.2.1 are
  less than 7 days old. The 7-day package age rule blocks them until
  2026-10-12. You can install versions 0.7.0, 0.3.7 and 0.2.0 now.

GPUI rendering on Linux:

- The Linux renderer is wgpu on Vulkan, with GL as fallback. Zed PR
  [#46758](https://github.com/zed-industries/zed/pull/46758) removed blade
  ([`ZED/crates/gpui_wgpu/src/wgpu_context.rs#L471-L479`](https://github.com/zed-industries/zed/blob/96984225abc235519e66b5c6c9ed23b87621f868/crates/gpui_wgpu/src/wgpu_context.rs#L471-L479)).
- The swapchain is 8-bit (`Bgra8Unorm` or `Rgba8Unorm`), without Wayland
  color management
  ([`ZED/crates/gpui_wgpu/src/wgpu_renderer.rs#L382-L392`](https://github.com/zed-industries/zed/blob/96984225abc235519e66b5c6c9ed23b87621f868/crates/gpui_wgpu/src/wgpu_renderer.rs#L382-L392)).
- Images are BGRA buffers in CPU memory that GPUI uploads to a sprite atlas
  ([`ZED/crates/gpui/src/assets.rs#L42-L68`](https://github.com/zed-industries/zed/blob/96984225abc235519e66b5c6c9ed23b87621f868/crates/gpui/src/assets.rs#L42-L68),
  [`ZED/crates/gpui_wgpu/src/wgpu_atlas.rs#L256-L267`](https://github.com/zed-industries/zed/blob/96984225abc235519e66b5c6c9ed23b87621f868/crates/gpui_wgpu/src/wgpu_atlas.rs#L256-L267)).
  This works for posters. For video it means one CPU copy and one upload for
  each frame.
- The video element `surface()` exists only on macOS and iOS
  ([`ZED/crates/gpui/src/elements/surface.rs#L5-L41`](https://github.com/zed-industries/zed/blob/96984225abc235519e66b5c6c9ed23b87621f868/crates/gpui/src/elements/surface.rs#L5-L41)).
  The wgpu renderer skips surface batches: "Surfaces are macOS-only for video
  playback and are not implemented by the WGPU renderer"
  ([`ZED/crates/gpui_wgpu/src/wgpu_renderer.rs#L1612-L1614`](https://github.com/zed-industries/zed/blob/96984225abc235519e66b5c6c9ed23b87621f868/crates/gpui_wgpu/src/wgpu_renderer.rs#L1612-L1614)).
- Upstream work: PR [#52669](https://github.com/zed-industries/zed/pull/52669)
  (a wgpu surface element for Linux) closed for inactivity. PR
  [#62379](https://github.com/zed-industries/zed/pull/62379) (native window
  composition with Wayland subsurfaces, tested on Hyprland) is open. The
  longbridge fork gpui-fast already has that composition model
  ([`composition.rs#L1-L26`](https://github.com/longbridge/gpui-fast/blob/b8f0c026553711c1e049fee501932d3801f23567/crates/gpui/src/fast/composition.rs#L1-L26)).
  Its first release was on 2026-10-06, so the 7-day age rule blocks each of
  its versions today.

Existing GPUI video players copy frames through the CPU
([cijiugechu/gpui-video-player `element.rs#L112-L156`](https://github.com/cijiugechu/gpui-video-player/blob/beb23b09f64d670446099ab902cc048074917993/src/element.rs#L112-L156))
or need a patched GPUI fork
([zortax/gpui-video `README.md#L5-L23`](https://github.com/zortax/gpui-video/blob/74d37459183d9817cd3197d158a76cb0a9d29bd4/README.md#L5-L23)).
The GPUI Jellyfin client jellybeam embeds libmpv on macOS only
([`gl_video.rs#L1-L30`](https://github.com/packetinspector/jellybeam-macos/blob/8a4b0ca0582d6bf18e30e2655a8c5c50c0203540/crates/app/src/gl_video.rs#L1-L30)).
KoalaTV controls mpv over IPC and embeds it with `--wid`, which needs X11
([`player.rs#L313-L318`](https://github.com/Louisdelez/KoalaTV/blob/606e5bf4d15d517218d9f55d23de73e0ca479b1a/desktop/src/views/player.rs#L313-L318)).

libmpv and mpv facts:

- The render API has two backends: OpenGL and software. It has no Vulkan
  backend
  ([`MPV/include/mpv/render.h#L49-L53`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/include/mpv/render.h#L49-L53)).
  The header calls the software renderer "extremely simple (but slow)"
  ([`render.h#L125-L147`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/include/mpv/render.h#L125-L147)).
- The render API uses the older `gl_video` renderer (the `vo=gpu` code), not
  the libplacebo renderer `gpu-next`
  ([`MPV/video/out/gpu/libmpv_gpu.c#L18`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/video/out/gpu/libmpv_gpu.c#L18)).
  HDR passthrough (`--target-colorspace-hint`) "requires a supporting driver
  and `--vo=gpu-next`"
  ([`MPV/DOCS/man/options.rst#L7346-L7384`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/options.rst#L7346-L7384)).
  Inference: each design that renders through the render API shows HDR as
  tone-mapped SDR.
- The render API has strict thread rules. A violation with
  `ADVANCED_CONTROL` "will freeze the mpv core thread forever"
  ([`render.h#L55-L107`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/include/mpv/render.h#L55-L107)).
  Hardware decoding needs EGL and `MPV_RENDER_PARAM_WL_DISPLAY` on Wayland
  ([`MPV/include/mpv/render_gl.h#L82-L100`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/include/mpv/render_gl.h#L82-L100)).
- `wid` embedding "works on X11, win32, and macOS only"
  ([`MPV/include/mpv/client.h#L190-L205`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/include/mpv/client.h#L190-L205)).
- Without the render API and without `wid`, libmpv uses a normal video output
  and opens its own window. The minimal libmpv example of mpv does this
  ([`mpv-examples/libmpv/simple/simple.c#L24-L42`](https://github.com/mpv-player/mpv-examples/blob/e0d1a84c99e8c469b58fde31a1541401acfb0eb2/libmpv/simple/simple.c#L24-L42)).
  libmpv loads no configuration files by default and starts in idle mode
  ([`client.h#L442-L459`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/include/mpv/client.h#L442-L459)).
- jellium-desktop continues the CEF rewrite of Jellyfin Desktop as an
  unofficial project. It uses this split: "mpv owns its own window + GPU;
  libmpv is used only for the control plane (properties/commands/events)"
  ([jellium `AGENTS.md#L32-L36`](https://github.com/andrewrabert/jellium-desktop/blob/14dc0845981cb7c0aef5fa93ff539d927c8f0eb2/AGENTS.md#L32-L36)).
  It adds an mpv fork and a Wayland proxy to put that window under its UI.
  One of its rules is to never call the sync mpv API from event callbacks,
  because that "causes deadlock during video init"
  ([`AGENTS.md#L37-L45`](https://github.com/andrewrabert/jellium-desktop/blob/14dc0845981cb7c0aef5fa93ff539d927c8f0eb2/AGENTS.md#L37-L45)).
- The default video output of mpv is `gpu-next`
  ([`MPV/DOCS/man/vo.rst#L15-L26`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/vo.rst#L15-L26)).
  mpv speaks the Wayland `color-management-v1` protocol
  ([`MPV/video/out/wayland_common.c#L2064-L2140`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/video/out/wayland_common.c#L2064-L2140)).
  `--hwdec` is off by default
  ([`options.rst#L1326-L1345`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/options.rst#L1326-L1345)).
- The Hyprland setting `render:cm_auto_hdr` is on by default and switches to
  HDR in fullscreen. It "requires the `--target-colorspace-hint-mode=source`
  mpv option to work with mpv versions greater than v0.40.0"
  ([hyprland-wiki `config-options.md#L710-L729`](https://github.com/hyprwm/hyprland-wiki/blob/b8b7239a5e8ffe69203654a53c7aa9206b6786bd/content/configuring/core/config-options.md#L710-L729)).
- mpv takes its Wayland app id from `--wayland-app-id`, with `mpv` as the
  default
  ([`options.rst#L6507-L6508`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/options.rst#L6507-L6508)).
  Omarchy floats each window whose class matches an unanchored `…|mpv)`
  pattern, at 875×600 (local
  `~/.local/share/omarchy/default/hypr/apps/system.lua:L1-L11`). Inference: an
  app id that contains "mpv" also floats. Use an id without "mpv".
- The Wayland video output of mpv creates a `zwp_idle_inhibitor_v1` on its
  surface
  ([`wayland_common.c#L3999-L4010`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/video/out/wayland_common.c#L3999-L4010)).
- JSON IPC uses `--input-ipc-server=<path>` or `--input-ipc-client=fd://N`.
  Each message is one JSON line with an optional `request_id`.
  `observe_property` pushes changes. The doc calls the protocol "explicitly
  insecure": it has no authentication and it exposes the `run` command
  ([`MPV/DOCS/man/ipc.rst#L1-L17`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/ipc.rst#L1-L17),
  [`#L79-L195`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/ipc.rst#L79-L195),
  [`#L267-L306`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/ipc.rst#L267-L306)).
- Rust: the crate `libmpv2` 6.0.0 (May 2026) has an active maintainer. Its
  render API supports OpenGL only
  ([`libmpv-rs/src/mpv/render.rs#L83-L101`](https://github.com/kohsine/libmpv-rs/blob/d7ccfafff663d7c731d33383f6ec74247c8dd1dc/src/mpv/render.rs#L83-L101)).
  The older crate `libmpv` had its last release in 2020. The IPC crates have
  few users.

### 3.2 Option a: GPUI browser and an external mpv process over JSON IPC

- Wayland and HDR: the best available. mpv runs `gpu-next` with Wayland color
  management and HDR passthrough. The fullscreen HDR switch of Hyprland works
  on the mpv window.
- Hardware decoding: from mpv (`hwdec=auto` with VA-API, NVDEC or Vulkan).
- Subtitles and tracks: the full mpv set (libass ASS, PGS, `sub-add` for
  sidecars). Track changes go through IPC properties.
- Fullscreen and tiling: a normal Hyprland window. It can tile, move to its
  own workspace, or go fullscreen. It needs a window rule for its app id.
- Omarchy theming: gpui-omarchy themes the library UI. The player window uses
  the mpv on-screen controller unless you replace it.
- Effort: low. The work is a small IPC client on a Unix socket and the
  progress sync.
- Risks: the IPC bugs in section 4. Number values sent to `set` fail without
  an error, teardown can hang on a reply, and orphan mpv processes start to
  play after a suspend. Two windows. The app needs a system `mpv` binary, and
  Flatpak makes an external mpv impractical.
- Maturity: high for mpv. KoalaTV is prior art for GPUI with mpv IPC.

### 3.3 Option a′: GPUI browser and in-process libmpv, mpv draws its own window

This option is option a with libmpv in place of a child process.

- The same as option a: `gpu-next`, HDR passthrough, hardware decoding,
  libass, the fullscreen and HDR switch of Hyprland, the idle inhibitor of mpv
  on its window, and a separate player window.
- Better than option a: typed properties and events through `libmpv2`. There
  is no string typing trap, no socket file and no 120-second reply wait. The
  player stops when the app stops, so no orphan process stays after a suspend
  or a crash. There is no mpv command line, so `ps` never shows a token.
- Worse than option a: a crash inside libmpv stops the app. libmpv loads no
  user `mpv.conf` unless you set `config` with a `config-dir`
  ([`client.h#L442-L459`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/include/mpv/client.h#L442-L459)).
  Follow the jellium rule: no sync mpv calls inside event callbacks.
- Effort: low, close to option a. Risk: low.
- Inference: the player module looks the same in options a and a′
  (properties, commands, events). A later move between them is cheap.

### 3.4 Option b: GPUI with libmpv video inside the GPUI window

- Wayland and HDR: no HDR passthrough, because the render API uses
  `gl_video` and GPUI is 8-bit. No fullscreen HDR switch from Hyprland and no
  direct scanout.
- There are three paths:
  1. Software render into a GPUI image for each frame. This works with stock
     GPUI but is slow. The mpv header says: "You probably don't want to use
     this".
  2. OpenGL render, read back, and upload. Each frame makes a round trip from
     GPU to CPU to GPU.
  3. OpenGL render on a Wayland subsurface from the gpui-fast composition.
     This is zero-copy, and GPUI can draw controls above the video. It needs a
     fork that is a few days old and an upstream PR that is still open.
- Subtitles and tracks: the same as mpv.
- Fullscreen and tiling: one window, which feels the most like one app.
- Omarchy theming: full, with GPUI controls over the video (path 3).
- Effort, as an estimate (inference): about one week to make path 1 work at
  1080p. Several weeks for path 3, plus work each week to follow the fork.
- Risks: high. No HDR, deadlocks from the render thread rules, an unstable
  upstream API, and no Linux prior art.
- Maturity: low.

### 3.5 Option c: GTK4 or libadwaita with libmpv in a GtkGLArea

- Reference: Delfin embeds mpv with a C widget of 588 lines. The widget
  subclasses `GtkGLArea`, sets `MPV_RENDER_PARAM_WL_DISPLAY`, and renders into
  the framebuffer of the GL area
  ([`DF/video_player_mpv/sys/video-player-mpv/video-player-mpv.c#L306-L350`](https://codeberg.org/avery42/delfin/src/commit/820b466bfd47f71c12e9b2cabb698e8f78942f41/video_player_mpv/sys/video-player-mpv/video-player-mpv.c#L306-L350)).
  Its author stopped work on Delfin in November 2024
  ([issue #232](https://codeberg.org/avery42/delfin/issues/232)).
- Wayland and HDR: the render API, so tone-mapped SDR only. No
  `GtkGraphicsOffload`, because offload needs dmabuf textures and libmpv gives
  none
  ([GtkGraphicsOffload docs](https://docs.gtk.org/gtk4/class.GraphicsOffload.html),
  [Delfin issue #69](https://codeberg.org/avery42/delfin/issues/69)).
  The author of the Delfin fork saw frame drops in fullscreen with HDR on
  (issue #232).
- Hardware decoding: works through EGL interop.
- Subtitles: the full mpv set.
- Fullscreen and tiling: one window. Inference: libadwaita header bars look
  odd in a tile.
- Idle inhibit: `gtk_application_inhibit` uses the Wayland idle-inhibit
  protocol, which Omarchy honors
  ([GTK `gtkapplication-wayland.c#L273-L310`](https://gitlab.gnome.org/GNOME/gtk/-/blob/dbd703e6a8d152b73d80c7e386049cf121fd5137/gtk/gtkapplication-wayland.c#L273-L310)).
- Omarchy theming: good. Omarchy sets GTK dark or light through gsettings
  ([`OM/bin/omarchy-theme-set-gnome`](https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188/bin/omarchy-theme-set-gnome)).
  An app can add a CSS template that Omarchy renders on each theme change
  ([`OM/bin/omarchy-theme-set-templates#L6-L7`](https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188/bin/omarchy-theme-set-templates#L6-L7)).
  This option does not reuse gpui-omarchy.
- Effort: medium. Risk: medium. Maturity: medium. Celluloid and Delfin prove
  the path, but no maintained Rust crate exists for it.

### 3.6 Option d: Qt6 or QML with libmpv

- Reference: Jellyfin Desktop 2.0 (formerly Jellyfin Media Player) moved to
  Qt 6 and the KDE library MpvQt. It added MPRIS and needs D-Bus for
  screensaver control
  ([v2.0.0 release](https://github.com/jellyfin/jellyfin-desktop/releases/tag/v2.0.0)).
  It must force Qt Quick to OpenGL
  ([`JD/src/main.cpp#L231`](https://github.com/jellyfin/jellyfin-desktop/blob/2cb4a4456fd29b5b62d825ef0e5df93ed6913328/src/main.cpp#L231)).
- Project direction: in March 2026 the Qt work "effectively stopped in favor
  of promoting a SDL+CEF rewrite", which had HDR on Linux. By August 2026 the
  Qt client was "revived", and the CEF author wrote "we lose HDR support"
  ([issue #523 comments](https://github.com/jellyfin/jellyfin-desktop/issues/523#issuecomment-4065050161)).
  The CEF rewrite continues as the unofficial jellium-desktop.
- Wayland and HDR: the render API, so tone-mapped SDR. Open Hyprland bugs:
  color banding
  ([#813](https://github.com/jellyfin/jellyfin-desktop/issues/813)),
  refresh-rate sync
  ([#1195](https://github.com/jellyfin/jellyfin-desktop/issues/1195)) and HDR
  ([#523](https://github.com/jellyfin/jellyfin-desktop/issues/523)).
- Idle inhibit: D-Bus only
  ([`JD/src/power/PowerComponentDBus.cpp#L112`](https://github.com/jellyfin/jellyfin-desktop/blob/2cb4a4456fd29b5b62d825ef0e5df93ed6913328/src/power/PowerComponentDBus.cpp#L112)).
  This does nothing on current Omarchy (section 5). Wayland idle inhibit is an
  open issue since v1.6
  ([#169](https://github.com/jellyfin/jellyfin-desktop/issues/169)).
- Omarchy theming: manual. Inference: QML follows no system theme by default.
- Effort: medium (C++, or Rust through cxx-qt). Risk: medium to high.

### 3.7 Comparison table

| | a: GPUI and mpv IPC | a′: GPUI and libmpv, own window | b: GPUI with embedded libmpv | c: GTK4 and libmpv | d: Qt6 and libmpv |
| --- | --- | --- | --- | --- | --- |
| HDR passthrough on Hyprland | Yes | Yes | No | No | No |
| Hardware decoding | Yes | Yes | Yes on path 3, costly on paths 1 and 2 | Yes | Yes |
| ASS, PGS, sidecars | Yes | Yes | Yes | Yes | Yes |
| Video inside the app window | No | No | Yes | Yes | Yes |
| Idle inhibit on Omarchy | mpv does it | mpv does it | The app must do it | GTK does it | The app must add it |
| Reuses gpui-omarchy | Yes | Yes | Yes | No | No |
| Needs a GPUI fork | No | No | Yes on path 3 | Not applicable | Not applicable |
| Effort | Low | Low | High | Medium | Medium |
| Risk | Low | Low | High | Medium | Medium to high |

---

## 4. Lessons from Linux Jellyfin clients

Jellyfin Desktop (formerly Jellyfin Media Player) shows jellyfin-web in Qt
WebEngine. Injected JavaScript replaces HTML5 playback with mpv
([`JD/README.md`](https://github.com/jellyfin/jellyfin-desktop/blob/2cb4a4456fd29b5b62d825ef0e5df93ed6913328/README.md)).

- What went well: one device profile that direct-plays all video and audio,
  with an HLS fallback and subtitles as External and Embed
  ([`JD/native/nativeshell.js#L64-L215`](https://github.com/jellyfin/jellyfin-desktop/blob/2cb4a4456fd29b5b62d825ef0e5df93ed6913328/native/nativeshell.js#L64-L215)).
  MPRIS since 2.0.
- What went badly: Wayland bugs on Hyprland (#813, #1195), HDR bugs (#523,
  #753), idle inhibit through D-Bus only (#169), and NVIDIA crashes (#842). The
  project changed direction twice in 2026. Its configuration is in the data
  directory, not in `$XDG_CONFIG_HOME`.

Delfin (GTK4, Rust, Relm4):

- What went well: a clean widget with GtkGLArea and the render API, idle
  inhibit through GTK, and MPRIS through `souvlaki`
  ([`DF/delfin/src/utils/inhibit.rs#L13-L27`](https://codeberg.org/avery42/delfin/src/commit/820b466bfd47f71c12e9b2cabb698e8f78942f41/delfin/src/utils/inhibit.rs#L13-L27)).
- What went badly: strict serde models crash when the server adds a field
  ([issue #240](https://codeberg.org/avery42/delfin/issues/240)). Each progress
  report starts its own task, so reports can arrive out of order
  ([`DF/delfin/src/video_player/session.rs#L60-L140`](https://codeberg.org/avery42/delfin/src/commit/820b466bfd47f71c12e9b2cabb698e8f78942f41/delfin/src/video_player/session.rs#L60-L140)).
  The access token is in a plain configuration file, with the comment "TODO:
  move to keyring"
  ([`DF/delfin/src/config/mod.rs#L111-L112`](https://codeberg.org/avery42/delfin/src/commit/820b466bfd47f71c12e9b2cabb698e8f78942f41/delfin/src/config/mod.rs#L111-L112)).
  It had one maintainer, who left.

jellyfin-mpv-shim (Python, libmpv or external mpv over IPC):

- IPC traps: the mpv `set` command takes a string, so a raw JSON number fails
  without an error. Because of this, the resume position worked on one backend
  only
  ([`JS/docs/mpv-backends.md#L20-L44`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/docs/mpv-backends.md#L20-L44)).
  A reply timeout of 120 seconds blocked shutdown
  ([`JS/jellyfin_mpv_shim/player.py#L223-L240`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/jellyfin_mpv_shim/player.py#L223-L240)).
- Progress: one ordered worker queue. A stop that overtakes the next start
  leaves all remote clients with a blank session
  ([`JS/jellyfin_mpv_shim/session_reporter.py#L1-L23`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/jellyfin_mpv_shim/session_reporter.py#L1-L23)).
  The timeline thread "must survive anything"
  ([`JS/jellyfin_mpv_shim/timeline.py#L41-L60`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/jellyfin_mpv_shim/timeline.py#L41-L60)).
- Device profile: empty containers on purpose, because "anything left off
  silently transcodes". `MaxAudioChannels` is 8
  ([`JS/jellyfin_mpv_shim/utils.py#L258-L330`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/jellyfin_mpv_shim/utils.py#L258-L330)).
- Hardware decoding is off by default after GPU resets and hangs with AMD
  VA-API
  ([`JS/docs/mpv-backends.md#L659-L675`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/docs/mpv-backends.md#L659-L675)).
- MPRIS works only through the mpv-mpris plugin with external mpv
  ([`JS/README.md#L387-L389`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/README.md#L387-L389)).
  The docs call external mpv "not practical" in Flatpak
  ([`JS/docs/configuration.md#L837-L844`](https://github.com/jellyfin/jellyfin-mpv-shim/blob/437ac000aaec27e3b43042b31ad2f3d4cad2cd47/docs/configuration.md#L837-L844)).
- An EGL regression gave a black screen on Hyprland with NVIDIA
  ([#573](https://github.com/jellyfin/jellyfin-mpv-shim/issues/573)).

Feishin (Electron music player, mpv over IPC through a fork of node-mpv):

- Its socket is in `/tmp`, not in `$XDG_RUNTIME_DIR`
  ([`FS/src/main/features/core/player/index.ts#L168-L173`](https://github.com/jeffvli/feishin/blob/9dc15477c86fbd48d1a98fa878769d09811895e8/src/main/features/core/player/index.ts#L168-L173)).
- Events after a restart look like a real track end, so the code needs a
  generation counter
  ([`index.ts#L33-L39`](https://github.com/jeffvli/feishin/blob/9dc15477c86fbd48d1a98fa878769d09811895e8/src/main/features/core/player/index.ts#L33-L39)).
  `quit` hangs on a blocked queue, so the code needs a timeout and SIGTERM
  ([`index.ts#L267-L295`](https://github.com/jeffvli/feishin/blob/9dc15477c86fbd48d1a98fa878769d09811895e8/src/main/features/core/player/index.ts#L267-L295)).
  A rogue mpv starts to play after resume from suspend
  ([#2497](https://github.com/jeffvli/feishin/issues/2497),
  [#2573](https://github.com/jeffvli/feishin/issues/2573)).
- MPRIS breaks when `mpris:length` is missing
  ([#2304](https://github.com/jeffvli/feishin/issues/2304)).

Takeaways (inference, from the items above):

1. Use mpv as the only source of playback state. The UI, MPRIS and the Silo
   reports all read from it.
2. Send all Silo playback calls of a session (start, progress, replan, stop)
   through one ordered queue. `sequence` and `stop_id` make retries safe, but
   the order of start and stop still matters.
3. Use libmpv, not IPC, in a native app. Keep IPC only to run the user's own
   mpv.
4. Declare wide direct play. The server handles the fallback.
5. Make hardware decoding a setting, with `auto-safe` as the default.
6. Ignore unknown JSON fields, as the Android client does.
7. Keep tokens in the Secret Service from the first version.

---

## 5. Linux desktop integration

MPRIS: the bus name is `org.mpris.MediaPlayer2.<name>`. `Metadata` needs
`mpris:trackid`. `Position` is in microseconds and does not go through
`PropertiesChanged`, so send `Seeked` on each jump
([MPRIS spec](https://specifications.freedesktop.org/mpris-spec/latest/),
[Player interface](https://specifications.freedesktop.org/mpris-spec/latest/Player_Interface.html)).
Omarchy installs `mpv-mpris`
([`OM/install/omarchy-base.packages#L87`](https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188/install/omarchy-base.packages#L87)).
Arch puts the plugin in `/etc/mpv/scripts/`, which only the mpv command line
loads. Inference: with libmpv and its configuration off, the app implements
MPRIS itself with `zbus` or `souvlaki` and fills it with Silo metadata
(title, artwork, length). This also prevents a second MPRIS player from the
plugin.

Idle and screensaver inhibit: Omarchy removed hypridle
([`OM/bin/omarchy-upgrade-to-quattro#L888`](https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188/bin/omarchy-upgrade-to-quattro#L888)).
Quickshell now handles idle with `IdleMonitor { respectInhibitors: true }`
([`OM/shell/plugins/services/idle/Service.qml#L270-L276`](https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188/shell/plugins/services/idle/Service.qml#L270-L276)),
which follows Wayland idle inhibitors. No Omarchy component provides
`org.freedesktop.ScreenSaver`, and xdg-desktop-portal-hyprland has no Inhibit
portal
([portal sources](https://github.com/hyprwm/xdg-desktop-portal-hyprland/tree/e87ae7823e7bf0601385220c69b5a3b245123fc5/src/portals)).
Inference, with high confidence: on current Omarchy, only a
`zwp_idle_inhibitor_v1` on a visible surface or a Hyprland `idle_inhibit`
window rule keeps the screen on. The mpv window does this by itself
([`MPV/DOCS/man/options.rst#L3815-L3825`](https://github.com/mpv-player/mpv/blob/accb7b2821ecf0cb04cf3579ccd98bc8f751f61f/DOCS/man/options.rst#L3815-L3825)).
Options b and d must add it.

Secret storage: use the Secret Service API
([spec](https://specifications.freedesktop.org/secret-service-spec/latest/)).
Omarchy ships gnome-keyring and libsecret
([`OM/install/omarchy-base.packages#L47`](https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188/install/omarchy-base.packages#L47)).
Omarchy creates a keyring without a password
([`OM/install/user/default-keyring.sh`](https://github.com/basecamp/omarchy/blob/77bfaca408d2993018d928e72cf84561480e5188/install/user/default-keyring.sh)).
Inference: the secrets are therefore not encrypted at rest. Still use the
Secret Service, because it is the standard and other desktops encrypt it. The
Rust options are `oo7` (pure Rust, with a Secret Service backend and a portal
file backend) and `keyring` 4.x
([oo7](https://github.com/linux-credentials/oo7),
[keyring-rs](https://github.com/open-source-cooperative/keyring-rs)). Store
the access token, refresh token and profile token for each server id, as the
official clients do.

XDG directories: configuration in `$XDG_CONFIG_HOME`, data in
`$XDG_DATA_HOME`, state such as the last server and the window size in
`$XDG_STATE_HOME`, the image cache in `$XDG_CACHE_HOME`, and sockets in
`$XDG_RUNTIME_DIR`
([Base Directory spec](https://specifications.freedesktop.org/basedir-spec/latest/)).

Desktop entry and app id: on Wayland the compositor matches a window by
`xdg_toplevel.set_app_id`, which matches the base name of the `.desktop` file
([xdg-shell](https://wayland.app/protocols/xdg-shell)). `StartupWMClass` is
for X11
([Desktop Entry keys](https://specifications.freedesktop.org/desktop-entry-spec/latest/recognized-keys.html)).
Use one reverse-DNS id for the `.desktop` file, the GPUI window, the MPRIS name
and the icon. Give the mpv window its own id without "mpv", and ship a
Hyprland rule for it.

Packaging: on the AUR, a VCS package has a `-git` suffix and a prebuilt
binary has a `-bin` suffix
([AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines),
[Rust package guidelines](https://wiki.archlinux.org/title/Rust_package_guidelines)).
The Arch package `mpv` provides `libmpv.so`
([package](https://archlinux.org/packages/extra/x86_64/mpv/)), so the package
needs only `depends=(mpv)` plus the GPU and font libraries. No Flatpak runtime
ships libmpv, so each Flatpak app builds it as a module. Delfin and Jellyfin
Desktop do this
([Delfin manifest](https://github.com/flathub/cafe.avery.Delfin/blob/a6e2f9940bbe75e5400bda529dde50e548b6f192/cafe.avery.Delfin.yaml),
[Jellyfin Desktop manifest](https://github.com/flathub/org.jellyfin.JellyfinDesktop/blob/23eebe530cc61066ce99f29eeb2a53885d1146fb/org.jellyfin.JellyfinDesktop.yml)).
Inference: start with an AUR package. Flatpak can come later, and it fits
option a′ (bundled libmpv) better than option a (external binary).

---

## 6. Recommendation

### The pick: option a′

Use a GPUI library UI with gpui-omarchy, plus libmpv in the same process as
the control channel. mpv draws its own Wayland window with `gpu-next`.

The reasons:

1. Picture quality. Options a and a′ are the only designs that get HDR
   passthrough, the fullscreen HDR switch of Hyprland, zero-copy hardware
   decoding and full libass subtitles on Wayland today. Each embedded design
   loses HDR, because the libmpv render API uses the old renderer.
2. No fork. The design uses stock gpui-omarchy. The video path never touches
   the GPUI renderer, so the weekly gpui-kit changes cannot break playback.
3. Fewer failure modes than IPC. libmpv calls are typed. There is no socket,
   no orphan mpv after a suspend, and no token on a command line.
   jellium-desktop uses the same control split.
4. A good fit for Omarchy. A separate player window is normal under a tiling
   window manager. mpv inhibits idle on its own surface, which is the only
   inhibit path that Omarchy honors.
5. The Silo protocol suits mpv. The server decides the route, and mpv plays
   almost any file. Most plays are direct, and the client stays simple.

What this pick gives up: the video is not inside the library window. The
in-player controls come from the mpv on-screen controller (or a Lua
controller later), not from GPUI widgets. Inference: if one window becomes a
requirement later, look at option b path 3 again when Zed merges PR #62379 or
gpui-fast is stable, and accept SDR for that path.

### Key risks

1. Silo API changes. Silo is pre-release. The server removes `/api/v1`, and
   v2 still changes (some server docs are behind the code). Generate types from
   `openapi.json`, read `system/info`, and pin a tested server version.
2. The capability declaration. `declared` evidence from a client that is not
   a browser is my reading of the spec, not a documented pattern. Ask the Silo
   maintainers.
3. Token expiry during long HLS sessions with header authentication
   (section 1.6).
4. Weekly gpui-kit releases and the 7-day age rule. Pin exact versions and
   expect upgrade work every few weeks.
5. A libmpv crash stops the app. Accept it, or move to option a. The player
   module stays the same, in a child process.
6. Two windows split focus and key input. The app needs a Hyprland rule and
   good defaults (`--fs`, a clear app id).
7. Hardware decoding fails on some AMD and NVIDIA setups (mpv-shim history).
   Make it a setting.

### First milestone, in order

1. Server and sign-in. Add a server by URL (DNS-SD discovery comes later).
   Read `GET /api/v2/system/info`. Support password login and device sign-in.
   Keep tokens in the Secret Service for each server id. Make a stable device
   id. Send all `X-Silo-*` headers. Use one HTTP client with one shared
   refresh and the 401 and 503 rules from section 1.3.
2. Profiles. List the profiles, support PIN entry, and send `X-Profile-Id` and
   `X-Profile-Token` with each request.
3. Browse. Home sections, libraries, a catalog grid with `virtual_list`, item
   detail, seasons and episodes. Load posters with `image_size` and keep a
   disk cache in `$XDG_CACHE_HOME`.
4. Play. Read the capabilities, then send a v3 start with the wide `declared`
   profile and `header_authenticated_media_v1`. Load the plan URL in libmpv
   with `http-header-fields`. Use `player_start_seconds` and
   `timeline_offset_seconds`. Show the message of a terminal decision.
5. Report. Use one ordered queue for each session. Send progress every 10
   seconds with an increasing `sequence`. Stop with a fixed `stop_id` and a
   final sample. Start a new attempt on `404 playback_session_not_found`.
6. Tracks. Build the audio and subtitle menus from the plan inventory. Send
   changes through `replan` with `track_change`. Add sidecars with `sub-add`.
7. Markers. Skip intro and credits from `marker_segments`, and follow
   `playback.intro_skip_mode`.
8. Desktop. Add the `.desktop` file and the app id, a Hyprland rule for the
   player window, MPRIS with Silo metadata, and an AUR `-git` package.

Leave these out of milestone 1: downloads, OIDC browser sign-in, the events
socket, the playback control socket, watch together, trickplay and Flatpak.
