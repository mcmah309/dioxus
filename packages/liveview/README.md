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

Files stay in the browser until application code reads their contents. Reads transfer
files over HTTP into the system temporary directory. Each LiveView connection defaults
to a 1 GiB storage cap and a 1024-file cap across unread, incoming, and retained files. Once a
read requests a transfer, the browser has five minutes to start its HTTP request.
Set these values before cloning the pool for the WebSocket and HTTP upload routes:

```rust
use dioxus_liveview::LiveViewPool;
use std::time::Duration;

let view = LiveViewPool::new()
    .with_upload_storage_limit(256 * 1024 * 1024)
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
LiveView verifies receipt before resolving the read; a failed transfer returns an
error through the file's read API.

Form events deliver metadata and file handles immediately. Accessing
`name()`, `size()`, `files()`, or `FormData::parsed_values()` does not transfer contents.
The first awaited `read_bytes()` or `read_string()`, or the first poll of
`byte_stream()`, requests the file. Submitting a form only uploads files that its
handler reads; reading from `onchange` intentionally starts the transfer earlier.

Concurrent readers share one transfer, and later reads reuse its result. Events
referencing the same retained browser File share its storage and quota reservation.
The browser sends up to four requested files concurrently per connection. Other UI
events continue to run during transfers.

A file handle owns the selected browser File independently of its input. Removing or
replacing the input does not invalidate a retained handle or cancel its active readers.
Dropping all handles and readers releases the browser reference, cancels an unfinished
transfer, deletes any temporary file, and releases its quota. Disconnecting cancels
unfinished transfers; already downloaded files remain readable while retained.

Omitted settings keep their defaults. A file's declared size and one file slot count
toward the connection's limits from creation of its handle, including unread and
zero-byte files. Handles that exceed either limit still expose metadata, but reads
return the quota error. Failed transfers release their reservations. Each connection
has its own budget; the pool does not identify accounts across connections.

The timeout starts when a read requests the transfer, so an unread selection can be
retained until submission. An expired request releases its reservation and reports an
error to its readers. Once HTTP uploading starts, it remains valid until completion
or cancellation. A handle caches either the completed file or the transfer error.

`FileData::byte_stream()` starts the transfer on its first poll, waits for the complete
temporary file, then reads it in bounded chunks. `read_bytes()` and `read_string()`
load the downloaded contents into memory. `name()` preserves the browser's filename;
`path()` is empty until the transfer completes successfully, then returns the
server's temporary path. Calling `path()` does not start or wait for a transfer;
it stays empty while uploading or if the transfer fails. A read through another
handle to the same file can also make the path available. Keep an original
`FileData` handle alive while using that path. Browser-supplied paths are never
treated as server filesystem paths.

`FormData::parsed_values()` deserializes text fields and file metadata. Use
`SerializedFileData` for metadata fields. Its `name` preserves the original browser
filename even when `path` is empty or points to a temporary server file.
For an optional upload, use `Option<SerializedFileData>`: an unselected input
parses as `None`, while a selected zero-byte file parses as `Some(metadata)`.
`FileData` does not implement `Deserialize`; structs parsed with `parsed_values()`
must use `SerializedFileData` for file metadata.
Obtain the original handles with `get_first("input_name")`, `get("input_name")`, or
`files()`. These handles retain lazy reads, filenames, and quota ownership after the
form event is dropped:

```rust
use dioxus_html::{FormData, FormValue};

#[derive(serde::Deserialize)]
struct Fields {
    description: String,
}

async fn submit(form: &FormData) -> Result<(), dioxus_core::CapturedError> {
    let fields: Fields = form.parsed_values()?;
    if let Some(FormValue::File(Some(file))) = form.get_first("upload") {
        let contents = file.read_bytes().await?;
        println!("{}: {} bytes for {}", file.name(), contents.len(), fields.description);
    }
    Ok(())
}
```

If you construct your own `VirtualDom`, call `view.run(vdom, socket).await` on your
local executor and pass `view.clone()` to `axum_file_upload`. Both handlers must use
the same pool to share upload credentials and temporary files.

### Cross-origin WebSocket URLs

Uploads use the HTTP equivalent of the configured WebSocket URL and include the
destination origin's credentials. If that URL is cross-origin, configure CORS on
the upload router with the exact page origin. Credentialed CORS cannot use a
wildcard origin. The upload request uses `PUT` with `Content-Type` and
`X-Content-Size` headers, so the CORS layer must allow that method and those
headers. Enable the `cors` feature on
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
        HeaderName::from_static("x-content-size"),
    ])
    .allow_credentials(true);

let upload_router: axum::Router = axum::Router::new()
    .route("/ws/upload/{token}", dioxus_liveview::axum_file_upload(view))
    .layer(upload_cors);
# }
```

## Contributing

- Report issues on our [issue tracker](https://github.com/dioxuslabs/dioxus/issues).
- Join the discord and ask questions!

## License

This project is licensed under the [MIT license].

[mit license]: https://github.com/dioxuslabs/dioxus/blob/main/LICENSE-MIT

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in Dioxus by you shall be licensed as MIT without any additional
terms or conditions.
