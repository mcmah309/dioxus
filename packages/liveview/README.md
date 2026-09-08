# Dioxus Liveview

[![Crates.io][crates-badge]][crates-url]
[![MIT licensed][mit-badge]][mit-url]
[![Build Status][actions-badge]][actions-url]
[![Discord chat][discord-badge]][discord-url]

[crates-badge]: https://img.shields.io/crates/v/dioxus-liveview.svg
[crates-url]: https://crates.io/crates/dioxus-liveview
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/dioxuslabs/dioxus/blob/main/LICENSE-MIT
[actions-badge]: https://github.com/dioxuslabs/dioxus/actions/workflows/main.yml/badge.svg
[actions-url]: https://github.com/dioxuslabs/dioxus/actions?query=workflow%3ACI+branch%3Amaster
[discord-badge]: https://img.shields.io/discord/899851952891002890.svg?logo=discord&style=flat-square
[discord-url]: https://discord.gg/XgGxMSkvUM

[Website](https://dioxuslabs.com) |
[Guides](https://dioxuslabs.com/learn/0.7/) |
[API Docs](https://docs.rs/dioxus-liveview/latest/dioxus_liveview) |
[Chat](https://discord.gg/XgGxMSkvUM)

## Overview

`dioxus-liveview` provides adapters for running the Dioxus VirtualDom over a WebSocket connection.

The current backend frameworks supported include:

- Axum

Dioxus-LiveView exports some primitives to wire up an app into an existing backend framework.

- A ThreadPool for spawning the `!Send` VirtualDom and interacting with it from WebSockets
- An adapter for transforming various socket types into the `LiveViewSocket` type
- The glue to load the interpreter into your app

## File uploads

Files stream over HTTP into files in the system temporary directory. Each LiveView
connection defaults to a 1 GiB cap across incoming and retained uploads, each batch
is limited to 1024 files, and registered batches have five minutes to start
uploading. Set any of these values before cloning the pool for the WebSocket and
HTTP upload routes:

```rust
use dioxus_liveview::LiveViewPool;
use std::time::Duration;

let view = LiveViewPool::new()
    .with_upload_limit(256 * 1024 * 1024)
    .with_upload_file_limit(100)
    .with_upload_timeout(Duration::from_secs(60));
```

If your app builds its own Axum router, mount the HTTP upload handler alongside
the WebSocket route. For a WebSocket at `/ws`, add this route using a clone of the
same pool that runs the WebSocket connection. These Axum examples require the
`axum` feature on `dioxus-liveview`:

```rust
# #[cfg(feature = "axum")]
# {
use dioxus_liveview::LiveViewPool;

let view = LiveViewPool::new();
let router: axum::Router = axum::Router::new().route(
    "/ws/upload/{token}",
    dioxus_liveview::axum_file_upload(view.clone()),
);
# }
```

Append `/upload/{token}` to your actual WebSocket path, including any route prefix.
The default LiveView router already mounts this handler. A fallback page or a
redirect at the upload URL cannot receive the file, even if it returns HTTP success.
LiveView confirms receipt over the WebSocket before dispatching the form event;
failed uploads report an error and leave the connection available for retrying.

Multiple upload events can run concurrently on one connection. Each batch has its
own credentials, completion response, cancellation, and timeout; all batches and
retained files share the connection's data limit. The browser sends up to four
files concurrently within each batch and preserves the selection's file order in
the delivered event.

Clicks, text edits, and other events proceed while files upload. Events carrying
files are dispatched only after their own batch completes, so they can arrive
after later UI events or faster uploads, including uploads from the same input.

Omitted settings keep their defaults. A file's declared size counts toward the cap
from registration until its last `FileData` handle or reader is dropped. Dropping
the last handle deletes the temporary file and releases its quota. Canceled and
failed uploads also clean up their temporary files. Each connection has its own
budget; the pool does not identify accounts across connections.

The timeout releases unused upload reservations without waiting for another
upload. Once a batch starts uploading, it remains valid until completion or
cancellation. Files retained by the app remain available until released.

`FileData::byte_stream()` reads the temporary file in bounded chunks. `read_bytes()`
and `read_string()` load its contents into memory only when the app requests them.
`name()` preserves the browser's filename; `path()` returns the server's temporary
path after an upload. Metadata-only events do not expose the browser-supplied name
as a server path. Keep a `FileData` handle alive while using a temporary path.

If you construct your own `VirtualDom`, call `view.run(vdom, socket).await` on your
local executor and pass `view.clone()` to `axum_file_upload`. Both handlers must use
the same pool to share upload credentials and temporary files. The standalone `run` function
is deprecated because its upload registry is inaccessible to the HTTP handler.

### Cross-origin WebSocket URLs

Uploads use the HTTP equivalent of the configured WebSocket URL and include the
destination origin's credentials. If that URL is cross-origin, configure CORS on
the upload router with the exact page origin. Credentialed CORS cannot use a
wildcard origin. The upload request uses `PUT` with `Content-Type`,
`Content-Disposition`, `X-Content-Size`, and `X-Request-Client` headers, so the CORS
layer must allow that method and those headers. Enable the `cors` feature on
`tower-http` for this example:

```rust
# #[cfg(feature = "axum")]
# {
use axum::http::{header, HeaderName, HeaderValue, Method};
use dioxus_liveview::LiveViewPool;
use tower_http::cors::CorsLayer;

let view = LiveViewPool::new();
let upload_cors = CorsLayer::new()
    .allow_origin(HeaderValue::from_static("https://app.example.com"))
    .allow_methods([Method::PUT])
    .allow_headers([
        header::CONTENT_TYPE,
        header::CONTENT_DISPOSITION,
        HeaderName::from_static("x-content-size"),
        HeaderName::from_static("x-request-client"),
    ])
    .allow_credentials(true);

let upload_router: axum::Router = axum::Router::new()
    .route("/ws/upload/{token}", dioxus_liveview::axum_file_upload(view))
    .layer(upload_cors);
# }
```

## File downloads

With the `axum` feature, call `download_file` from a LiveView event handler or Dioxus
task to offer a server-side file to the browser:

```rust,no_run
# #[cfg(feature = "axum")]
# async fn export() -> Result<(), Box<dyn std::error::Error>> {
let file = dioxus_fullstack::FileStream::from_path("report.pdf").await?;
dioxus_liveview::download_file(file)?;
# Ok(())
# }
```

`FileStream::from_raw` also accepts generated contents or a stream. Only a
single-use token travels over the websocket. The browser requests the file via
HTTP GET, and its download manager receives the stream directly, without building
a JavaScript blob or transferring file bytes through the LiveView event loop.
The HTTP response supplies the attachment filename, including Unicode filenames.

The default router includes the handler. Custom routers must mount it beside
the upload route, using the same pool as the websocket:

```rust
# #[cfg(feature = "axum")]
# {
let view = dioxus_liveview::LiveViewPool::new();
let router: axum::Router = axum::Router::new()
    .route("/ws/upload/{token}", dioxus_liveview::axum_file_upload(view.clone()))
    .route("/ws/download/{token}", dioxus_liveview::axum_file_download(view.clone()));
# }
```

Append `/download/{token}` to the actual websocket path, including any route
prefix. As with uploads, the browser uses the HTTP equivalent of the websocket
URL. Downloads use browser navigation, so they do not require fetch CORS headers.

Success from `download_file` means the instruction was queued, not that the user
saved the file. Pending downloads default to 128 per connection and expire after
five minutes; configure these before cloning the pool with
`with_download_file_limit` and `with_download_timeout`. Expiration and websocket
disconnects release unrequested streams. Once an HTTP request claims the token,
the transfer can finish independently of the websocket. Tokens cannot be reused;
retrying requires another `download_file` call. Range requests and resume are not
supported, and HEAD requests are rejected without consuming a token.

This API is for saving files. It does not rewrite bytes embedded in `document::eval`,
data URLs, or component attributes, and is not a persistent URL for a PDF viewer.
File generation and other blocking work must still run off the LiveView thread;
the stream itself is polled by the HTTP server.

The `axum` example includes a **Download count** button alongside its counter.

## Contributing

- Report issues on our [issue tracker](https://github.com/dioxuslabs/dioxus/issues).
- Join the discord and ask questions!

## License

This project is licensed under the [MIT license].

[mit license]: https://github.com/dioxuslabs/dioxus/blob/main/LICENSE-MIT

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in Dioxus by you shall be licensed as MIT without any additional
terms or conditions.
