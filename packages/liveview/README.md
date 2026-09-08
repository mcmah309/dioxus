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

LiveView uploads the contents when the file is first read with `read_bytes()`, `read_string()`, or `byte_stream()`; accessing its metadata does not start an upload.

```rust
use dioxus_html::{FormData, FormValue};

#[derive(serde::Deserialize)]
struct Fields {
    description: String,
}

async fn submit(form: &FormData) -> Result<(), dioxus_core::CapturedError> {
    let fields: Fields = form.deserialize_values()?;
    if let Some(FormValue::File(Some(file))) = form.get_first("upload") {
        let contents = file.read_bytes().await?;
        println!("{}: {} bytes for {}", file.name(), contents.len(), fields.description);
    }
    Ok(())
}
```

`FormData::deserialize_values()` can deserialize file metadata as
`SerializedFileData` or `Option<SerializedFileData>`. Retrieve the `FileData` handle
separately with `get_first()`, `get()`, or `files()` when you need to read its contents.

The default LiveView router configures uploads automatically. If you build a custom
Axum router, enable the `axum` feature and mount `axum_file_upload` at the WebSocket
path followed by `/upload/{token}`. Use the same `LiveViewPool` for the WebSocket and
upload handlers:

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

Uploads default to 1 GiB and 1024 files per connection with a five-minute upload
timeout. Configure different limits before cloning the pool:

```rust
use dioxus_liveview::LiveViewPool;
use std::time::Duration;

let view = LiveViewPool::new()
    .with_upload_storage_limit(256 * 1024 * 1024)
    .with_upload_file_limit(100)
    .with_upload_timeout(Duration::from_secs(60));
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
