# Research: poster loading and the image disk cache

Date: 2026-10-11. Status: research, plus one small proof program (see
section 6). Ticket: issue #11.

This file answers one question. How does the library window load artwork,
and how does it keep artwork on disk between runs?

The label "Inference" marks a conclusion that I drew from the sources. A
source does not state it.

## Answer

- `image_size` sets one size for all artwork in one response. For a 2:3
  poster that is 150px wide, `small` (300px) is sufficient up to a display
  scale of 2. A 16:9 card that is 290px wide needs `small` at scale 1,
  `medium` up to scale 1.7 and `large` above that. A full-width backdrop
  needs `medium` or `large`. Both give 1920px. The detail page uses `large`.
- The server documents the cache rule for clients. The URL path identifies
  the bytes, and the query only authorizes the request. A revisioned path
  (`w500.<revision>.webp`) never changes its bytes. So the cache key is the
  origin plus the path, without the query.
- GPUI has no disk cache. Its default image cache in memory never releases
  an image, and it uses the full URL as the key. A signed URL changes at
  least once a day, so the app downloads and decodes the same poster again.
- GPUI has a public `ImageCache` trait, and the app can implement it. The
  cache gets the URL that the view gave to `img(url)`. It downloads the
  bytes with `cx.http_client()` and writes them to `$XDG_CACHE_HOME`. Then
  it gives GPUI a file path to decode. GPUI decodes the file on a worker
  thread with its own decoder, so the app needs no second decode path.
- Show the thumbhash while the poster loads. The Silo web and Android
  clients do this. The Rust crate `thumbhash` 0.1.0 decodes one hash in
  about 23 µs to a picture of about 23x32 pixels.
- Recommendation: one `ArtworkCache` entity that implements `ImageCache`.
  It holds an LRU (least recently used) list of decoded images in memory.
  It also holds a disk cache of 512 MiB, with the SHA-256 of the origin and
  the path as the file name. Revisioned paths never expire. Other paths
  expire after one hour. `img(url).with_loading(..)` shows the thumbhash.

## Sources and versions

| Source | Version or commit |
| --- | --- |
| [Silo-Server/silo-server](https://github.com/Silo-Server/silo-server) (server and web client) | `ec1dcb67c77f59688692574516276e8f6c72dfeb` (2026-10-10) |
| [Silo-Server/silo-android](https://github.com/Silo-Server/silo-android) | `e535ddceb5bedd22b75d0e53abca27b3640982df` |
| crate `gpui-pre` | 0.3.7 |
| crate `gpui-pre-wgpu` | 0.3.7 |
| crate `gpui-pre-http-client` | 0.3.7 |
| crate `thumbhash` | 0.1.0 (published 2023-03-22, MIT, no dependencies) |
| crate `sha2` | 0.10.9 (2025-04-30) or 0.11.0 (2026-03-25) |
| crate `base64` | 0.22.1 |

The silo-server commit is newer than the one in the main research report.
The file `docs/images-api.md` does not exist at the older commit.

The link text uses these prefixes:

- `SS` is `https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb`
- `SD` is `https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df`
- `GP` is `https://docs.rs/crate/gpui-pre/0.3.7/source`
- `GW` is `https://docs.rs/crate/gpui-pre-wgpu/0.3.7/source`
- `TH` is `https://docs.rs/crate/thumbhash/0.1.0/source`

## 1. How Silo serves artwork

### 1.1 The size ladder

- The server caches each artwork at a fixed set of widths (the ladder).
  `image_size` picks one rung. It applies to the whole response: "every
  artwork URL in the body is resolved at that size"
  ([`SS/docs/images-api.md#L15-L29`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/images-api.md#L15-L29)).
  The server answers an unknown value with `422 validation_failed`.
- The widths, from the same file
  ([`SS/docs/images-api.md#L56-L69`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/images-api.md#L56-L69)):

  | Image type | `small` | `medium` | `large` | `original` |
  | --- | --- | --- | --- | --- |
  | poster | 300 | 500 | 780 | up to 1920 |
  | still | 300 | 500 | 780 | up to 1920 |
  | backdrop | 300 | 1920 | 1920 | up to 1920 |
  | logo | 500 | 500 | 1280 | up to 1920 |
  | profile | 300 | 500 | 500 | up to 1920 |

- The backdrop ladder also has a 1280px rung, but no `image_size` value
  selects it. `small` is the narrowest rung, `large` is the widest, and
  `medium` is a fixed 1920 for backdrops
  ([`SS/internal/artworkkey/artworkkey.go#L124-L137`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/artworkkey/artworkkey.go#L124-L137),
  [`SS/internal/imagesize/imagesize.go#L86-L122`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/imagesize/imagesize.go#L86-L122)).
- The client must not hard-code the table. It reads
  `GET /api/v2/images/capabilities`, and it sends `image_size` only when
  `state` is `available`
  ([`SS/docs/images-api.md#L77-L124`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/images-api.md#L77-L124)).
- `large` is a preference, and the response can contain a smaller variant.
  If no variant works, the URL is empty "and the client should display its
  placeholder"
  ([`SS/docs/images-api.md#L148-L158`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/images-api.md#L148-L158)).
- The 16:9 cards of the web client use `backdrop_url`. For an episode in a
  section, `backdrop_url` "is the horizontal still"
  ([`SS/web/src/components/ContinueWatchingCard.test.tsx#L36`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/web/src/components/ContinueWatchingCard.test.tsx#L36)).
  The server reads the ladder from the image type in the stored key. So an
  episode card follows the still ladder, and a movie card follows the
  backdrop ladder
  ([`SS/internal/catalog/item_cards.go#L63-L89`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/catalog/item_cards.go#L63-L89),
  [`#L145-L152`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/catalog/item_cards.go#L145-L152)).
- 22 operations accept `image_size`. The library window uses
  `listHomeSections`, `getHomeSectionItems`, `listCatalogItems`,
  `getLibrarySectionItems`, `getCatalogItem`, `listSeriesSeasons` and
  `listSeasonEpisodes`
  ([`SS/contracts/api/v2/openapi.json`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json)).
  `listHomeSections` returns all rows in one response. So the poster rows
  and the 16:9 rows of Home get the same size.
- The web client sends `image_size` only for shuffles. For the other reads
  it uses the server default, which is `medium`
  ([`SS/web/src/api/v2/shuffles.ts#L10`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/web/src/api/v2/shuffles.ts#L10)).

### 1.2 Which size fits each card

The screen needs the card width in logical pixels times the display scale
factor. GPUI gives the scale factor of the window. The table uses the
smallest rung that is at least the needed width. For the 16:9 card at
scale 2, the table accepts a rung of 86% (500 for 580), because the next
rung is much larger.

| Surface (request) | Width | Scale 1 | Scale 1.5 to 1.7 | Scale 2 |
| --- | --- | --- | --- | --- |
| Library grid, 2:3 (`listCatalogItems`, `getLibrarySectionItems`) | 150px | `small` (300) | `small` (300) | `small` (300) |
| 16:9 episode cards (`listSeasonEpisodes`) | 290px | `small` (300) | `medium` (500) | `medium` (500) or `large` (780) |
| Home, mixed rows (`listHomeSections`) | 150px and 290px | `small` | `medium` | `medium` |
| Detail page (`getCatalogItem`) | full-width backdrop, poster, cast | `large` | `large` | `large` |

Inference: at scale 1.5 and above, `medium` on Home gives a 500px poster
and a 500px still, which fit. A movie card in a 16:9 row then gets a 1920px
backdrop, because the backdrop ladder has no 500 rung. The app downloads
that file once, and the disk cache keeps it. If the size is a problem, the
app can load the 16:9 rows again with `getHomeSectionItems` and their own
size.

Inference: on the detail page, `large` gives a 1920px backdrop, a 780px
poster, a 1280px logo and 500px cast photos. That fits a 1920px window at
scale 1 and a half-screen window at scale 2.

### 1.3 Signed URLs and how long they live

- A local artwork URL is `/api/v2/artwork/{key}?exp=<unix>&sig=<hmac>`.
  The key is the storage path. The signature covers the key and `exp`
  ([`SS/internal/artworkurl/artworkurl.go#L100-L126`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/artworkurl/artworkurl.go#L100-L126)).
- A revisioned key keeps the same URL for one UTC day. Every other key gets
  a new URL every 15 minutes or sooner. The source says that "the path is
  the identity and the query only authorizes"
  ([`SS/internal/artworkurl/artworkurl.go#L37-L58`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/artworkurl/artworkurl.go#L37-L58)).
- A key is revisioned when its file name has a revision between the
  variant and the extension, for example `w500.r1.webp`
  ([`SS/internal/artworkkey/artworkkey.go#L86-L97`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/artworkkey/artworkkey.go#L86-L97)).
- S3 presigned URLs and Cloudflare token URLs keep the object key in the
  path and put the authorization in the query
  ([`SS/internal/s3client/client.go#L368-L397`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/s3client/client.go#L368-L397),
  [`#L437-L448`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/s3client/client.go#L437-L448)).
- The server document gives the cache rule for clients. "The path
  identifies the bytes and the query only authorizes, so a client may key
  its cache of revisioned artwork by path to keep entries across the daily
  change"
  ([`SS/docs/images-api.md#L86-L93`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/images-api.md#L86-L93)).
- The artwork route sends `ETag`. For a revisioned key it sends
  `private, max-age=<seconds to exp>, immutable`. For a mutable key (an
  avatar, or a library poster that a user replaced) it sends
  `private, no-cache`, and it answers `304` to a matching `If-None-Match`.
  It answers an invalid or expired signature with `404`
  ([`SS/internal/apiv2/artwork.go#L62-L113`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/artwork.go#L62-L113)).
- Plugin artwork can be a full external URL with no fixed width
  ([`SS/docs/images-api.md#L71-L75`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/images-api.md#L71-L75)).

Inference: the same cache key works for all four URL kinds. Use the origin
(scheme, host, port) and the path, and drop the query. The origin keeps two
servers apart. A root-relative URL gets the origin of the server.

### 1.4 What the other Silo clients do

- Android uses Coil with a disk cache of 512 MiB
  ([`SD/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/images/SiloImageLoader.kt#L41-L53`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/images/SiloImageLoader.kt#L41-L53)).
  For posters it keeps the default key of Coil, which is the full URL.
- The web client uses the browser cache, which also keys by full URL.

Inference: both official clients download a revisioned poster again once a
day, when its URL changes. A path key prevents that.

## 2. How GPUI loads images

- `img("https://...")` becomes `ImageSource::Resource(Resource::Uri(..))`.
  A string that is not a URL becomes `Resource::Embedded`. A `Path` or
  `PathBuf` becomes `Resource::Path`
  ([`GP/src/elements/img.rs#L42-L104`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/img.rs#L42-L104)).
  The other sources are `ImageSource::Image(Arc<Image>)`,
  `Render(Arc<RenderImage>)` and `Custom(closure)`.
- The loader reads a file with `fs::read`, or gets a URI with
  `cx.http_client().get(..)`. Then it decodes the bytes with the `image`
  crate (PNG, JPEG, WebP, GIF, and SVG as fallback). A non-2xx status is an
  error
  ([`GP/src/elements/img.rs#L620-L742`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/img.rs#L620-L742)).
  The load runs on the background executor
  ([`GP/src/asset_cache.rs#L25-L49`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/asset_cache.rs#L25-L49)).
- The decoded image is a BGRA buffer, 4 bytes for each pixel
  ([`GP/src/assets.rs#L42-L70`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/assets.rs#L42-L70)).
  A 300x450 poster uses 540 KB. A 1920x1080 backdrop uses 8.3 MB.
- If no image cache is set, `img` uses `window.use_asset`. That stores the
  result in the app-wide map `loading_assets`, with the hash of the
  resource as the key. Only `remove_asset` removes an entry
  ([`GP/src/elements/img.rs#L536-L553`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/img.rs#L536-L553),
  [`GP/src/app.rs#L2752-L2780`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L2752-L2780)).
- `Image::from_bytes(format, bytes)` exists, and `ImageSource::Image`
  decodes it on a worker
  ([`GP/src/platform.rs#L2953-L2959`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/platform.rs#L2953-L2959)).
  It also goes into `loading_assets`, with the hash of the bytes as the key.

Inference: with plain `img(url)`, each new signed URL is a new key. The old
decoded image stays in memory until the app stops. A long session adds one
decoded copy of a mutable poster every 15 minutes.

### 2.1 The `ImageCache` trait

- A view sets a cache with `div().image_cache(..)` for all child images, or
  with `img(..).image_cache(&entity)` for one image. The trait has one
  method, `load(&mut self, resource: &Resource, window, cx)`. It returns
  `None` while the image loads
  ([`GP/src/elements/image_cache.rs#L197-L218`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/image_cache.rs#L197-L218),
  [`GP/src/elements/img.rs#L230`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/img.rs#L230)).
- `ImageCacheItem::new(&Resource, cx)` starts a load with the normal loader.
  `use_image(window)` returns the result. While the load runs, it registers
  the current view for a redraw at completion
  ([`GP/src/elements/image_cache.rs#L162-L193`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/image_cache.rs#L162-L193)).
- `RetainAllImageCache` keeps every image until the cache entity drops. Its
  doc comment says "LRU", but the code has no limit
  ([`GP/src/elements/image_cache.rs#L221-L293`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/image_cache.rs#L221-L293)).
- The gpui-pre example `image_gallery` has a `SimpleLruCache` with a fixed
  number of items. It calls `cx.drop_image` on each image that it removes,
  so the image also leaves the GPU atlas
  ([`GP/examples/image_gallery.rs#L169-L235`](https://docs.rs/crate/gpui-pre/0.3.7/source/examples/image_gallery.rs#L169-L235),
  [`GP/src/app.rs#L2845-L2850`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L2845-L2850)).
- `img(..).with_loading(..)` and `.with_fallback(..)` draw another element
  while the image loads or after it fails
  ([`GP/src/elements/img.rs#L164-L176`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/img.rs#L164-L176)).

Inference: the cache gets the `Resource::Uri` that the view gave. So the
view keeps `img(signed_url)`, and the cache alone decides the key, the disk
file and the memory limit. After the file is on disk, the cache calls
`ImageCacheItem::new(&Resource::Path(file))`. GPUI then reads and decodes
the file with its own loader on a worker. A cache read needs no second
decode path, and the app does not need `Image::from_bytes`.

## 3. The disk cache

Inference. This design follows from sections 1 and 2:

- Directory: `$XDG_CACHE_HOME/silo-omarchy/artwork/`. If `XDG_CACHE_HOME`
  is not set, use `$HOME/.cache`. This is the rule of the
  [XDG Base Directory](https://specifications.freedesktop.org/basedir-spec/latest/)
  specification. The standard library can read the two variables.
- File name: the hex SHA-256 of `origin + path`, with no query. The path
  comes from the server. A hash keeps it out of the file system path, so a
  `..` segment cannot escape the directory. Do not use `DefaultHasher` or
  the GPUI `hash` function, because neither promises the same output
  across versions.
- Write: download with `cx.http_client()`, and accept only a 2xx status.
  Write to `<name>.tmp`, then rename it to `<name>`. A rename in one
  directory is atomic, so a crash leaves no half-written file.
- Freshness: a revisioned path never expires. Any other path is fresh for
  one hour after the file modification time, and the cache downloads it
  again after that. A replaced library poster can show the old picture for
  up to one hour. ETag revalidation removes that delay if it matters. The
  cache then sends `If-None-Match` and keeps the file on `304`.
- Size: 512 MiB, the same as Android. At startup, a background task lists
  the directory and adds the sizes. It deletes the oldest files by
  modification time until the total is below the limit. On each disk hit,
  the cache sets the modification time to now with `File::set_modified`.
- Memory: an LRU of decoded images with a fixed count, as in the gpui-pre
  example. 200 small posters use about 108 MB. The LRU uses the same
  origin-plus-path key, so a new signature does not cause a second decode.
- Errors: a `404` means an expired or invalid signature. Do not write a
  file. Show the thumbhash, and try again when the view loads the list
  again. The server keeps resolved URLs for at most five minutes, so a new
  list gives a working URL
  ([`SS/docs/images-api.md#L156-L158`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/images-api.md#L156-L158)).

The steps of `ArtworkCache::load(resource)`:

1. If the resource is not a `Resource::Uri`, give it to
   `ImageCacheItem::new` without changes.
2. Make the key from the origin and the path.
3. If the memory LRU has a ready item for the key, return `use_image`.
4. If a fetch for the key is running, add `window.current_view()` to its
   waiting views and return `None`.
5. Otherwise, start a task. On a worker, the task uses a fresh file if one
   exists. Else it downloads the URL and writes the file. Back on the main
   thread, it stores `ImageCacheItem::new(&Resource::Path(file))` in the
   LRU. Then it calls `cx.notify` for each waiting view
   ([`GP/src/app.rs#L2794`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L2794)).

All disk work runs on a worker. The main thread does no file system call
during a render.

## 4. The thumbhash placeholder

- A thumbhash is a hash of about 21 bytes. It decodes to a blurred preview
  with the colors and the aspect ratio of the image. The API sends it in
  base64 in `poster_thumbhash`, `backdrop_thumbhash` and `still_thumbhash`,
  next to each URL field
  ([`SS/contracts/api/v2/openapi.json`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json)).
- The web client decodes it with the npm `thumbhash` package to a data URL,
  and keeps one result for each hash
  ([`SS/web/src/lib/thumbhash.ts#L1-L21`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/web/src/lib/thumbhash.ts#L1-L21)).
  The poster card draws the thumbhash as the background and fades the
  image in when it loads. It keeps the thumbhash when the image fails. It
  shows the default artwork only when there is no thumbhash
  ([`SS/web/src/components/MediaCardArtwork.tsx#L46-L104`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/web/src/components/MediaCardArtwork.tsx#L46-L104)).
- Android does the same. The source says "A failed load keeps the
  ThumbHash; only without one does a poster fall back to the default
  artwork"
  ([`SD/.../ui/components/ThumbhashImage.kt#L181-L183`](https://github.com/Silo-Server/silo-android/blob/e535ddceb5bedd22b75d0e53abca27b3640982df/android-shared/src/androidMain/kotlin/org/siloserver/silo/common/ui/components/ThumbhashImage.kt#L181-L183)).
- The Rust crate `thumbhash` 0.1.0 comes from Evan Wallace, who designed
  the format ([repository](https://github.com/evanw/thumbhash)). It has no
  dependencies. `thumb_hash_to_rgba(&[u8])` returns the width, the height
  and the RGBA bytes. It also has `thumb_hash_to_average_rgba` and
  `thumb_hash_to_approximate_aspect_ratio`
  ([`TH/src/lib.rs#L159`](https://docs.rs/crate/thumbhash/0.1.0/source/src/lib.rs#L159),
  [`#L298`](https://docs.rs/crate/thumbhash/0.1.0/source/src/lib.rs#L298),
  [`#L321`](https://docs.rs/crate/thumbhash/0.1.0/source/src/lib.rs#L321)).
  The encoder accepts only images up to 100x100
  ([`TH/src/lib.rs#L11`](https://docs.rs/crate/thumbhash/0.1.0/source/src/lib.rs#L11)).
  The client only decodes, so this limit does not apply.
- To show it in GPUI, swap RGBA to BGRA and put the pixels in an
  `image::Frame`. Then make an `Arc<RenderImage>` with `RenderImage::new`
  ([`GP/src/assets.rs#L61-L70`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/assets.rs#L61-L70)).
  gpui-pre already depends on `image` 0.25, so the app adds the same
  version. Draw it with `img(ImageSource::Render(..))` and
  `ObjectFit::Cover` inside `with_loading` and `with_fallback`.
- The GPU atlas sampler uses linear filtering, so a 23x32 picture scales up
  smoothly and does not show blocks
  ([`GW/src/wgpu_renderer.rs#L1238-L1243`](https://docs.rs/crate/gpui-pre-wgpu/0.3.7/source/src/wgpu_renderer.rs#L1238-L1243)).

Inference: decode the thumbhash once when the list arrives, and keep the
`Arc<RenderImage>` with the item. When the page closes, call
`cx.drop_image` on each one. Each one is about 3 KB, but the atlas keeps it
until the drop.

GPUI `img` has no fade-in. The image replaces the thumbhash in one frame.

## 5. Options that I did not choose

| Option | Works? | Why not |
| --- | --- | --- |
| Plain `img(url)` with no cache | Yes | No disk cache. Memory never shrinks, and each new signature causes a new decode (section 2). |
| `RetainAllImageCache` | Yes | No memory limit. A library grid of 500 small posters holds about 270 MB. |
| A `HttpClient` wrapper that caches image responses | Yes | It sees every API request, so it must guess which responses are artwork. It still needs the LRU in memory. The `ImageCache` sees only images. |
| Read the file and use `Image::from_bytes` | Yes | A second path for the same decode. `Resource::Path` already reads and decodes on a worker. |
| Key by full URL | Yes | The key changes once a day for revisioned artwork, and every 15 minutes for other artwork. The disk fills with copies. |
| Key by the server path as a directory tree | Yes | The path comes from the server, so `..` and unusual characters need checks. A hash needs none. |

## 6. Proof: the thumbhash decode cost

A small program in the session scratch area used `thumbhash = "=0.1.0"` and
`base64 = "=0.22.1"`. It encoded a 66x100 gradient. Then it decoded the
hash 10,000 times in a release build on this machine.

```text
hash bytes=21 base64=3/cJNRqAh4CIiId4iIh4iICACPh4
decoded 23x32 (2944 bytes RGBA), 22.716µs per decode
aspect ratio from hash: Ok(0.71428573)
average rgba: Ok((0.47883597, 0.49470899, 0.5026455, 1.0))
```

Inference: a page of 100 posters costs about 2.3 ms of decode. That work
can run on a worker with the list parse.

The proof did not build a GPUI app, and it did not contact a Silo server.
The GPUI facts come from the source of gpui-pre 0.3.7 in the local cargo
registry.

## 7. Recommendation

1. Read `GET /api/v2/images/capabilities` once for each server. Pick
   `image_size` for each request with the rule in section 1.2. Multiply the
   card width by the scale factor, then take the smallest rung that is at
   least that wide, from the `widths` in the response. Use `large` on the
   detail page.
2. Add one `ArtworkCache` entity that implements `ImageCache`, as in
   section 3. Set it on the root of the library window with
   `div().image_cache(..)`. Views keep `img(signed_url)`.
3. Key memory and disk by the SHA-256 of origin plus path. Keep 512 MiB on
   disk. Revisioned paths never expire, and other paths expire after one
   hour.
4. Show the decoded thumbhash with `with_loading` and `with_fallback`. Show
   the default artwork only when the item has no thumbhash.
5. Add these crates: `thumbhash` 0.1.0, `sha2` (0.10.9 or 0.11.0, the one
   that the lock file already has), `base64` 0.22.1 if the tree does not
   have it, and `image` 0.25 to match gpui-pre. Each version is more than 7
   days old.

## Open questions

- The scale factor changes when the window moves to another monitor. Does
  the app load the list again with a new `image_size`, or keep the old
  size until the next load?
- Is one hour the right freshness for mutable artwork, or does the app use
  ETag revalidation from the start?
- Does the app limit the number of downloads that run at the same time? A
  grid of 100 new posters starts 100 requests at once.
