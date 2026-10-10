# Research: networking and async work inside GPUI

Date: 2026-10-10. Status: research, plus one small proof program (see
section 7). Ticket: issue #6.

This file answers one question. Which HTTP client and which way of running
background work fit a gpui-kit 0.7 app, and how do we stream and cancel a
request?

The label "Inference" marks a conclusion that I drew from the sources. A
source does not state it.

## Answer

- GPUI has its own executor. It has a main-thread (foreground) executor and a
  pool of worker threads (background executor). Both return a `Task`. If you
  drop a `Task`, the work stops.
- GPUI does not ship a real HTTP client. It ships an `HttpClient` trait. The
  default client fails every request with "No HttpClient available". The app
  must install a client at startup.
- Zed installs `ReqwestClient`, a wrapper around reqwest. It runs a small
  tokio runtime beside the GPUI executor. Only the request runs on tokio. The
  app code never touches tokio.
- The wrapper is on crates.io as `gpui-pre-reqwest-client`. Version 0.3.7
  matches the gpui-pre 0.3.7 that gpui-kit 0.7.0 uses, and it is older than 7
  days. The gpui-kit demo app installs the same client.
- Recommendation: use `gpui-pre-reqwest-client`, pinned to the same version
  as `gpui-pre`. Install it with `cx.set_http_client`. Send every Silo call
  through `cx.http_client()`. Then `img(url)` loads posters through the same
  client, and tests use the `FakeHttpClient` of the same crate family.

## Sources and versions

| Source | Version or commit |
| --- | --- |
| crate `gpui-pre` | 0.3.7 (snapshot of Zed `gpui` at `1a28cff4b409169bac058bca40dfbfeb7621d19b`) |
| crate `gpui-pre-scheduler` | 0.3.7 |
| crate `gpui-pre-linux` | 0.3.7 |
| crate `gpui-pre-http-client` | 0.3.7 |
| crate `gpui-pre-reqwest-client` | 0.3.7 (published 2026-09-28) |
| crate `gpui-base` (part of gpui-kit) | 0.7.0 |
| [zed-industries/zed](https://github.com/zed-industries/zed) | `1a28cff4b409169bac058bca40dfbfeb7621d19b` |
| [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit) | `ed27c6293b585acc40c10be7138534243bee5841` |

The gpui-pre manifest names its Zed commit in the table
`[package.metadata.gpui-pre]`
([`gpui-pre Cargo.toml`](https://docs.rs/crate/gpui-pre/0.3.7/source/Cargo.toml)).

The link text uses these prefixes:

- `GP` is `https://docs.rs/crate/gpui-pre/0.3.7/source`
- `GS` is `https://docs.rs/crate/gpui-pre-scheduler/0.3.7/source`
- `GL` is `https://docs.rs/crate/gpui-pre-linux/0.3.7/source`
- `HC` is `https://docs.rs/crate/gpui-pre-http-client/0.3.7/source`
- `RC` is `https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source`
- `ZED` is `https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b`

## 1. What GPUI gives for async work

- `cx.background_spawn(future)` runs a `Send` future on the background
  executor and returns a `Task`
  ([`GP/src/app.rs#L3077-L3082`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L3077-L3082),
  [`GP/src/executor.rs#L101-L108`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/executor.rs#L101-L108)).
- `cx.spawn(async move |this, cx| ...)` in an entity context runs on the main
  thread. It gets a weak handle to the entity and an `AsyncApp` that it can
  hold across `.await`
  ([`GP/src/app/context.rs#L226-L236`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app/context.rs#L226-L236),
  [`GP/src/app.rs#L2036-L2048`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L2036-L2048)).
- On Linux the background executor is a pool of plain threads named
  `Worker-N`. There is one thread per CPU, with at least 2
  ([`GL/src/linux/dispatcher.rs#L28-L52`](https://docs.rs/crate/gpui-pre-linux/0.3.7/source/src/linux/dispatcher.rs#L28-L52)).
- `Task` is a future. The source says: "If you drop a task it will be
  cancelled immediately. Calling `Task::detach` allows the task to continue
  running"
  ([`GS/src/executor.rs#L385-L392`](https://docs.rs/crate/gpui-pre-scheduler/0.3.7/source/src/executor.rs#L385-L392),
  [`#L561-L569`](https://docs.rs/crate/gpui-pre-scheduler/0.3.7/source/src/executor.rs#L561-L569)).
- `background_executor().timer(duration)` gives a timer task. A timeout or a
  retry delay needs no tokio
  ([`GP/src/executor.rs#L183`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/executor.rs#L183)).
- gpui-kit adds no executor and no HTTP client. Its base crate only re-exports
  the `smol` channels
  ([`gpui-base/src/async_util.rs#L4`](https://docs.rs/crate/gpui-base/0.7.0/source/src/async_util.rs#L4)).

Inference: the view keeps each `Task` in a field. A new search replaces the
field. That drops the old task and cancels the old request.

## 2. GPUI has an HTTP trait, not an HTTP client

- The trait `HttpClient` has `send(http::Request<AsyncBody>)`. It returns a
  boxed future of `http::Response<AsyncBody>`
  ([`HC/src/http_client.rs#L123-L131`](https://docs.rs/crate/gpui-pre-http-client/0.3.7/source/src/http_client.rs#L123-L131)).
- `AsyncBody` implements `futures::AsyncRead`, so you read the body in chunks
  ([`HC/src/async_body.rs#L14`](https://docs.rs/crate/gpui-pre-http-client/0.3.7/source/src/async_body.rs#L14),
  [`#L114`](https://docs.rs/crate/gpui-pre-http-client/0.3.7/source/src/async_body.rs#L114)).
- A new app gets `NullHttpClient`. Every request fails with
  "No HttpClient available"
  ([`GP/src/app.rs#L178-L183`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L178-L183),
  [`#L3216-L3228`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L3216-L3228)).
- The app sets a client with `Application::with_http_client` or
  `cx.set_http_client`, and reads it with `cx.http_client()`
  ([`GP/src/app.rs#L217-L223`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L217-L223),
  [`#L1742-L1750`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L1742-L1750)).
- With the `test-support` feature, the crate has `FakeHttpClient::create`. It
  answers each request from a closure
  ([`HC/src/http_client.rs#L423-L458`](https://docs.rs/crate/gpui-pre-http-client/0.3.7/source/src/http_client.rs#L423-L458)).
  Tests can use it with no network.

## 3. How Zed runs reqwest beside GPUI

- Zed starts a tokio runtime with 2 workers at startup. The comment says:
  "Since we now have two executors, let's try to keep our footprint small"
  ([`ZED/crates/gpui_tokio/src/gpui_tokio.rs#L12-L25`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_tokio/src/gpui_tokio.rs#L12-L25)).
  Zed then builds `ReqwestClient` inside that runtime and installs it as the
  app client
  ([`ZED/crates/zed/src/main.rs#L493`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/zed/src/main.rs#L493),
  [`#L509-L515`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/zed/src/main.rs#L509-L515)).
- If no tokio runtime is current, `ReqwestClient` makes its own runtime with 1
  worker thread
  ([`RC/src/reqwest_client.rs#L123-L147`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs#L123-L147)).
  So a small app needs no tokio setup. Zed does not publish `gpui_tokio` as a
  gpui-pre crate (crates.io has no `gpui-pre-tokio` on 2026-10-10), and we do
  not need it.
- `send` spawns `request.send()` on tokio. A guard aborts that tokio task when
  the caller drops the future. Then `send` turns the reqwest body stream into
  an `AsyncBody`
  ([`RC/src/reqwest_client.rs#L268-L320`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs#L268-L320)).
  The caller reads that body on any executor, for example a GPUI worker.
- `gpui_tokio::Tokio::spawn` uses the same abort-on-drop guard for other tokio
  work. The comment says: "the Tokio task will be cancelled if the GPUI task
  is dropped"
  ([`ZED/crates/gpui_tokio/src/gpui_tokio.rs#L53-L73`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/gpui_tokio/src/gpui_tokio.rs#L53-L73)).
- The gpui-kit demo app installs the same client with
  `reqwest_client::ReqwestClient::user_agent("gpui-component/story")`
  ([gpui-kit `crates/story/src/lib.rs#L235-L240`](https://github.com/longbridge/gpui-kit/blob/ed27c6293b585acc40c10be7138534243bee5841/crates/story/src/lib.rs#L235-L240)).
  gpui-base 0.7.0 lists `gpui-pre-reqwest-client = "=0.3.7"` as a dev
  dependency
  ([`gpui-base Cargo.toml`](https://docs.rs/crate/gpui-base/0.7.0/source/Cargo.toml)).

## 4. Streaming, cancelling, TLS

### Streaming a large response

Read `response.body_mut()` with `AsyncReadExt::read` in a loop inside
`cx.background_spawn`. Each chunk arrives when reqwest receives it. Nothing
holds the whole body in memory (see the `bytes_stream` to `into_async_read`
chain in
[`RC/src/reqwest_client.rs#L311-L315`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs#L311-L315)).
The proof in section 7 streams 16 MiB this way.

For Silo the large data is video, and mpv fetches the video itself (see
`docs/research/silo-linux-client.md`). Inference: the app streams only JSON
pages and images, so streaming matters little in version 1.

### Cancelling a request

- If the headers did not arrive yet, drop the future or the `Task`. The guard
  aborts the tokio task, and the connection closes.
- If the body is streaming, drop the `Response` or its `AsyncBody`. The proof
  shows that the server stops sending.
- To set a timeout, put a `RequestTimeout` in the request extensions.
  `ReqwestClient` maps it to a reqwest timeout
  ([`RC/src/reqwest_client.rs#L286-L288`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs#L286-L288)).

### TLS

- `ReqwestClient` calls `use_rustls_tls()`. It never uses native-tls
  ([`RC/src/reqwest_client.rs#L33-L36`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs#L33-L36)).
  Its reqwest is the Zed fork, published as `gpui-pre-reqwest`, with the
  feature `rustls-tls-native-roots`
  ([`ZED/Cargo.toml#L805-L813`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/Cargo.toml#L805-L813)).
  Native roots means the system CA store. Inference: a self-hosted Silo
  server with a CA that the user added to the system works.
- The constructor `proxy_and_user_agent` also uses `rustls-platform-verifier`
  with the `aws_lc_rs` crypto provider. If the verifier fails to load, it uses
  the bundled webpki roots
  ([`RC/src/reqwest_client.rs#L113-L115`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs#L113-L115),
  [`ZED/crates/http_client_tls/src/http_client_tls.rs#L8-L29`](https://github.com/zed-industries/zed/blob/1a28cff4b409169bac058bca40dfbfeb7621d19b/crates/http_client_tls/src/http_client_tls.rs#L8-L29)).
- `read_proxy_from_env()` reads `all_proxy`, `https_proxy` and `http_proxy`
  ([`HC/src/http_client.rs#L357-L371`](https://docs.rs/crate/gpui-pre-http-client/0.3.7/source/src/http_client.rs#L357-L371)).
  To use the system proxy, pass its result to `proxy_and_user_agent`.

## 5. Images and `img()`

- `img("https://...")` becomes `Resource::Uri` when the string parses as a
  URL. Any other string is an embedded asset path. A `PathBuf` becomes a file
  path
  ([`GP/src/elements/img.rs#L42-L104`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/img.rs#L42-L104)).
- For a URI, the loader calls `cx.http_client().get(uri, empty body, follow
  redirects)`. It reads the whole body, makes sure that the status is a
  success, and decodes the image. For a file path, the loader calls
  `fs::read`
  ([`GP/src/elements/img.rs#L620-L656`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/img.rs#L620-L656)).
  So `img(url)` uses the app client. With the default `NullHttpClient`, every
  remote image fails.
- The loader sends no custom headers
  ([`HC/src/http_client.rs#L133-L152`](https://docs.rs/crate/gpui-pre-http-client/0.3.7/source/src/http_client.rs#L133-L152)).
  Silo artwork URLs are signed or public, and the official clients send no
  credentials with them (`docs/research/silo-linux-client.md`, section 1.5).
  So images need no auth header. Inference: if one becomes necessary, a small
  type that implements `HttpClient` can wrap the client and add headers for
  the Silo host.
- GPUI caches images only in memory. A loaded image stays in the app asset
  cache until `remove_asset`
  ([`GP/src/app.rs#L2764-L2771`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/app.rs#L2764-L2771)).
  A view can set its own cache with `.image_cache(...)` through the
  `ImageCache` trait. The built-in `RetainAllImageCache` keeps every image
  ([`GP/src/elements/image_cache.rs#L197-L222`](https://docs.rs/crate/gpui-pre/0.3.7/source/src/elements/image_cache.rs#L197-L222)).
- GPUI has no disk cache. Inference: the simplest disk cache is a background
  task that downloads the poster to `~/.cache/silo-omarchy/artwork/<key>`.
  The view then shows `img(path)`. Use the artwork key as the file name, not
  the full signed URL, because a signature can change. The poster ticket on
  the map decides the details.

## 6. Other clients

| Client | Works beside GPUI? | Notes |
| --- | --- | --- |
| `gpui-pre-reqwest-client` | Yes. Zed and the gpui-kit demo use it. | 1 extra tokio thread. Abort on drop. rustls. It implements the GPUI trait already. |
| reqwest direct, own tokio runtime | Yes | Same cost. We would rewrite the adapter that `ReqwestClient` already has, and `img()` needs that adapter too. |
| ureq 3 | Yes, on worker threads | The README says: "It uses blocking I/O instead of async I/O" ([ureq README](https://docs.rs/crate/ureq/3.4.2/source/README.md)). Each request blocks a GPUI worker thread. Inference: many poster loads at once fill the small worker pool, and a blocked read cannot stop when its `Task` drops. We also need our own `HttpClient` adapter. |
| async-compat | Yes | It still starts tokio. The docs say: "a new single-threaded runtime will be created on demand" ([async-compat docs](https://docs.rs/async-compat/latest/async_compat/)). Same cost as reqwest-client, and no gain. |
| hyper with a small tokio runtime | Yes | reqwest already wraps hyper. More code for the same result. |
| isahc | Not studied | It needs libcurl. No GPUI app in the sources uses it. |

### Cost of the second runtime

- `ReqwestClient` starts one tokio worker thread
  ([`RC/src/reqwest_client.rs#L123-L132`](https://docs.rs/crate/gpui-pre-reqwest-client/0.3.7/source/src/reqwest_client.rs#L123-L132)).
  Tokio also starts blocking-pool threads when it needs them, for example for
  DNS lookups, and stops them when they are idle
  ([tokio `Builder::max_blocking_threads`](https://docs.rs/tokio/1.53.1/tokio/runtime/struct.Builder.html#method.max_blocking_threads)).
- The proof binary (reqwest fork, hyper, tokio, rustls, aws-lc) is 8.3 MB in
  release mode, not stripped. A clean release build of only those crates took
  47 seconds on this machine.
- Inference from section 3: Zed ships this setup on every platform, so it is a
  tested path.

## 7. Proof

I wrote a small program in the session scratchpad, outside this repo. It used
`gpui-pre-reqwest-client =0.3.7` and `gpui-pre-http-client =0.3.7`. The
dependency versions came from the `Cargo.lock` of gpui-base 0.7.0, so every
crate was older than 7 days. The program talked only to a local
`TcpListener`. It ran on a plain thread with `futures::executor::block_on`
and no tokio runtime.

Output:

```text
stream ok: 16777216 bytes
body cancel ok: server stopped after 2/256 chunks
send cancel ok: connection closed after drop
```

The output shows three facts:

1. The client works from a thread outside tokio. It makes its own runtime.
2. A 16 MiB body streams in 64 KiB reads.
3. A dropped body stops the transfer. A dropped send future closes the
   connection before any headers arrive.

The proof did not run inside a GPUI window. Inference: the GPUI worker
threads are plain threads (section 1), the same as the proof thread.

## 8. Recommendation

1. Add `reqwest_client = { package = "gpui-pre-reqwest-client", version =
   "=0.3.7" }`. Bump it together with `gpui-pre` every time, because it pins
   `gpui-pre-http-client` to the exact same version. Version 0.3.8, for
   gpui-kit 0.7.1, came out on 2026-10-05. The age rule allows it from
   2026-10-12.
2. At startup, build `ReqwestClient::proxy_and_user_agent(read_proxy_from_env(),
   "silo-omarchy/<version>")` and call `cx.set_http_client(Arc::new(client))`.
3. Write the Silo API client on top of `Arc<dyn HttpClient>`. It adds the
   bearer token and the `X-Silo-*` headers to each request. It parses JSON
   with serde inside `cx.background_spawn`.
4. Keep each request `Task` in the view that owns it. To cancel, replace or
   drop it.
5. Show posters with `img(url)` first. Add the disk cache from section 5 in
   the poster ticket.
6. In tests, use `FakeHttpClient` (feature `test-support`). Never call a real
   server.
