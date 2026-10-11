# Research: open artwork questions

Date: 2026-10-11. Status: research only. Ticket: issue #19. This file
follows `docs/research/poster-image-cache.md` (issue #11, branch
`research/poster-image-cache`).

The label "Inference" marks a conclusion that I drew from the sources. A
source does not state it.

## Answer

1. Scale change. GPUI tells the app. On Wayland the compositor sends the
   preferred scale, GPUI updates the window scale factor, and a view can
   watch it with `cx.observe_window_bounds`. That callback also runs on
   every resize and move, so the view must compare the new scale with the
   old one. Recommendation: load the list again only when the chosen
   `image_size` goes up to a larger rung.
2. Freshness. The server does not say "one hour". Mutable artwork gets
   `Cache-Control: private, no-cache` and an `ETag`. A matching
   `If-None-Match` gets `304`. Revisioned artwork gets `max-age` until the
   URL expires, plus `immutable`. Recommendation: revalidate mutable files
   with the ETag. Drop the one-hour rule.
3. Download limit. GPUI sets none. Each image load is one background task,
   and the HTTP client sets no cap on connections. The server documents no
   rate limit on the artwork route. Recommendation: `ArtworkCache` caps
   downloads itself, at 6 at a time.
4. 16:9 Home rows. `getHomeSectionItems` takes `image_size`, but it cannot
   give a smaller backdrop. The backdrop ladder has no rung between 300 and
   1920. Recommendation: do not load those rows again. Accept the 1920px
   backdrop for a movie card, because the disk cache keeps it.

## Sources and versions

| Source | Version or commit |
| --- | --- |
| [Silo-Server/silo-server](https://github.com/Silo-Server/silo-server) | `ec1dcb67c77f59688692574516276e8f6c72dfeb` |
| crates `gpui-pre`, `gpui-pre-linux`, `gpui-pre-reqwest-client` | 0.3.7 (the version in `Cargo.lock`) |
| [zed-industries/zed](https://github.com/zed-industries/zed) `crates/gpui` | `93d745668805029476e6d7fbb19652116fed51de` (main on 2026-10-11) |

No request went to a Silo server. I read source files only and ran no repo
code.

## 1. Does a scale change reach the app?

- On Wayland, the compositor sends a `PreferredScale` event for the
  surface. GPUI divides it by 120 and calls `rescale`. The function
  `set_size_and_scale` returns early when the scale is the same. Otherwise
  it stores the scale, resizes the drawable, and calls the window resize
  callback
  ([`wayland/window.rs`, `handle_fractional_scale_event` and `set_size_and_scale`](https://docs.rs/crate/gpui-pre-linux/0.3.7/source/src/linux/wayland/window.rs)).
- The resize callback calls `Window::bounds_changed`. It reads the scale
  factor from the platform window again, updates the viewport and the
  display id, redraws, and runs the bounds observers. The `on_moved`
  callback calls the same function
  ([`gpui-pre window.rs`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/window.rs),
  [`zed window.rs`](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/window.rs)).
  Both versions have the same code.
- `Context::observe_window_bounds(window, callback)` is the public hook
  ([`app/context.rs#L402-L416`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app/context.rs)).
  The callback gets no event data. It calls `window.scale_factor()` to read
  the new value.
- `Window::scale_factor()` is public. A view can read it for the first load,
  with no event.

Inference: the observer runs for every resize and every move, which can be
once for each frame of a drag. The view must keep the last `image_size` and
act only when the rung changes. Use one observer on the window root, not one
for each card.

Inference: the rung depends only on the scale. A move from scale 1 to 1.5
changes Home from `small` to `medium`. A move from 1.5 to 2 changes nothing.
After a move to a larger scale, the cached small pictures look soft, so
load the list again. After a move to a smaller scale, keep the sharper
pictures and do not load again.

## 2. Freshness of mutable artwork

- The artwork route sends an `ETag`. A key with a revision gets
  `private, max-age=<seconds until exp>, immutable`. Every other key gets
  `private, no-cache`. The source comment says that a mutable key "keeps its
  URL within the issuance bucket, so clients must revalidate" and that "the
  ETag turns that into a 304"
  ([`internal/apiv2/artwork.go#L62-L113`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/artwork.go#L62-L113)).
- `no-cache` means a client can store the file but must revalidate it
  before each reuse (RFC 9111, section 5.2.2.4). The server gives the app
  no freshness time for a mutable key.
- A matching `If-None-Match` gets `304` with no body. A missing, wrong, or
  expired signature gets `404` before the ETag check. So a revalidation
  request needs a current signed URL (same function).
- The OpenAPI contract lists `200`, `206`, `304`, `404` and `503` for
  `getArtwork` and `headArtwork`
  ([`contracts/api/v2/openapi.json`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json)).

Inference: the one-hour rule in `poster-image-cache.md` is a client choice
that the server does not ask for. Its cost is that a replaced poster shows
the old file for up to one hour. ETag revalidation costs one small request
for each mutable file, and the server answers it with `304`. Store the ETag
with the file. When a list shows a mutable file that is older than 5
minutes, show the cached file at once and send `If-None-Match`. A `200`
replaces the file and the picture. Revisioned files need no request.

## 3. Download concurrency

- `img` loads through `window.use_asset` or an `ImageCache`. Each load is
  one future that `CachedLoad::new` starts with
  `cx.background_executor().spawn(..)`. There is no queue and no limit
  ([`asset_cache.rs#L25-L49`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/asset_cache.rs),
  [`zed asset_cache.rs`](https://github.com/zed-industries/zed/blob/93d745668805029476e6d7fbb19652116fed51de/crates/gpui/src/asset_cache.rs)).
- The Linux background executor starts `available_parallelism` threads
  ([`linux/dispatcher.rs#L33-L36`](https://docs.rs/crate/gpui-pre-linux/0.3.7/source/src/linux/dispatcher.rs)).
  A download is an async future and does not hold a thread while it waits.
  A decode is CPU work, so at most one decode per thread runs at a time.
- The HTTP client is reqwest. Its builder sets a 10 second connect timeout
  and a 30 second idle timeout for pooled connections. It sets no limit on
  connections for each host
  ([`reqwest_client.rs`, `builder`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs)).
- The server documents no rate limit on `getArtwork`. Its response list has
  no `429`. I did not read the middleware of the whole server.

Inference: a grid of 100 posters that scroll in together starts 100
requests together. HTTP/2 can share one connection, but HTTP/1.1 opens many.
A reverse proxy in front of the server can answer some with `429` or `503`.
`ArtworkCache` from issue #11 is the right place for a cap, because it
already tracks each fetch by key. Keep a count of running fetches and a
queue of waiting keys. The number 6 is the per-host limit that browsers use
for HTTP/1.1. Fetch the visible items first, in render order.

## 4. 16:9 rows on Home

- `getHomeSectionItems` is `GET /api/v2/home/sections/{id}/items`. It takes
  an optional `image_size` (`small`, `medium`, `large`, `original`) and
  returns one `Section` with its `items`. The parameter text says "absent
  picks each surface's default"
  ([`contracts/api/v2/openapi.json`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json),
  path `/api/v2/home/sections/{id}/items`).
- The `Section` object has `item_limit` and `total_count`. So the call can
  return more items than the layout call. Its `image_size` applies to that
  section only.
- The backdrop ladder is 300, 1920, 1920 for `small`, `medium`, `large`
  (`poster-image-cache.md`, section 1.1). A movie card in a 16:9 row uses
  the backdrop. At scale 1.5 and above the card needs more than 300 pixels,
  and the next rung is 1920. No `image_size` gives a 500px or 780px
  backdrop.

Inference: a separate call for the row cannot give a smaller file at the
scale where the problem starts. It can only give `small` (300px, soft at
scale 1.5) or `medium` (1920px, the same file as before). The cost is bytes,
once for each movie in a 16:9 row, and the disk cache absorbs it. A decoded
1920x1080 backdrop uses 8.3 MB. A row of 20 movies uses 166 MB if all copies
stay in memory. So the real risk is the memory LRU, not the download. Limit
the LRU by decoded bytes, not by item count.

## Recommendation

1. Read `window.scale_factor()` for the first load. Watch
   `observe_window_bounds` on the window root. Load the list again only when
   the `image_size` rung goes up.
2. Revalidate mutable artwork with the ETag and `If-None-Match`. Keep
   revisioned artwork with no expiry. Drop the one-hour rule.
3. Cap `ArtworkCache` downloads at 6 at a time, with a queue in render
   order.
4. Keep one `listHomeSections` call for Home. Do not call
   `getHomeSectionItems` for the 16:9 rows. Limit the memory LRU by bytes.

## Open questions

- None block the design. One check needs a running server, which this
  research did not use: whether a reverse proxy in front of the server keeps
  the `ETag` header.
