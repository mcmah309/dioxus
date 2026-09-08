use axum::{
    body::Body,
    http::{HeaderValue, header},
    response::Response,
};
use dioxus_core::{Runtime, ScopeId};
use dioxus_fullstack::FileStream;
use futures_util::TryStreamExt;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc::UnboundedSender, time::Instant};
use tokio_util::task::AbortOnDropHandle;
use uuid::Uuid;

/// The default number of downloads awaiting an HTTP request per LiveView connection.
pub const DEFAULT_DOWNLOAD_FILE_LIMIT: usize = 128;

/// How long the browser has to start a queued download by default.
pub const DEFAULT_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// A file could not be queued for download.
#[derive(Debug, thiserror::Error)]
#[error("LiveView file download failed: {message}")]
pub struct FileDownloadError {
    message: &'static str,
}

/// Ask the browser to download a server-side file over HTTP.
///
/// Call this from a LiveView event handler or Dioxus task. Construct the stream with
/// [`FileStream::from_path`] or [`FileStream::from_raw`]. Only a short-lived, single-use token
/// crosses the websocket; the HTTP handler polls the file stream independently of the VirtualDom.
/// File generation and other blocking work must still run off the LiveView thread.
///
/// Success means the download instruction was queued, not that the browser saved the file.
/// Unrequested files expire or are released when the connection closes. Once HTTP streaming
/// starts, it may finish even if the websocket disconnects. Downloads do not support resume.
///
/// Custom routers must mount [`crate::axum_file_download`] at the websocket path followed by
/// `/download/{token}`, using the same [`crate::LiveViewPool`].
pub fn download_file(mut file: FileStream) -> Result<(), FileDownloadError> {
    let context = Runtime::try_current()
        .and_then(|runtime| runtime.consume_context::<DownloadContext>(ScopeId::ROOT))
        .ok_or(FileDownloadError {
            message: "no active LiveView connection",
        })?;

    let content_type = HeaderValue::from_str(
        file.content_type().unwrap_or("application/octet-stream"),
    )
    .map_err(|_| FileDownloadError {
        message: "invalid file content type",
    })?;
    // Never interpolate a filename into a quoted HTTP header. Encode UTF-8 bytes, including
    // quotes and control characters, and discard any directory components.
    let filename = file
        .file_name()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file");
    let mut disposition = String::from("attachment; filename=\"download\"; filename*=UTF-8''");
    for byte in filename.as_bytes() {
        use std::fmt::Write;
        write!(disposition, "%{byte:02X}").unwrap();
    }
    let size = file.size();
    let body = file.body_mut().ok_or(FileDownloadError {
        message: "expected a server-side FileStream",
    })?;
    let body = std::mem::replace(body, Body::empty().into_data_stream());
    let mut response = Response::new(Body::from_stream(body.inspect_err(|error| {
        tracing::error!(%error, "Failed to stream LiveView file download");
    })));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(header::CONTENT_DISPOSITION, disposition.parse().unwrap());
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(size) = size {
        headers.insert(header::CONTENT_LENGTH, size.into());
    }

    context.register(response)
}

#[derive(Clone)]
pub(crate) struct FileDownloadRegistry {
    state: Arc<Mutex<RegistryState>>,
    file_limit: usize,
    timeout: Duration,
}

#[derive(Default)]
struct RegistryState {
    files: HashMap<String, PendingDownload>,
    sessions: HashMap<Uuid, usize>,
}

struct PendingDownload {
    session: Uuid,
    expires: Instant,
    response: Response,
    _cleanup: AbortOnDropHandle<()>,
}

impl RegistryState {
    fn remove(&mut self, token: &str) -> Option<PendingDownload> {
        let file = self.files.remove(token)?;
        if let Some(count) = self.sessions.get_mut(&file.session) {
            *count -= 1;
        }
        Some(file)
    }
}

impl Default for FileDownloadRegistry {
    fn default() -> Self {
        Self {
            state: Default::default(),
            file_limit: DEFAULT_DOWNLOAD_FILE_LIMIT,
            timeout: DEFAULT_DOWNLOAD_TIMEOUT,
        }
    }
}

impl FileDownloadRegistry {
    pub(crate) fn with_file_limit(self, file_limit: usize) -> Self {
        Self {
            state: Default::default(),
            file_limit,
            ..self
        }
    }

    pub(crate) fn with_timeout(self, timeout: Duration) -> Self {
        Self {
            state: Default::default(),
            timeout,
            ..self
        }
    }

    pub(crate) fn new_session(&self, sender: UnboundedSender<String>) -> DownloadSession {
        let id = Uuid::new_v4();
        self.state.lock().unwrap().sessions.insert(id, 0);
        DownloadSession(DownloadContext {
            registry: self.clone(),
            id,
            sender,
        })
    }

    pub(crate) fn take(&self, token: &str) -> Option<Response> {
        let file = self.state.lock().unwrap().remove(token)?;
        (file.expires > Instant::now()).then_some(file.response)
    }
}

#[derive(Clone)]
pub(crate) struct DownloadContext {
    registry: FileDownloadRegistry,
    id: Uuid,
    sender: UnboundedSender<String>,
}

impl DownloadContext {
    fn register(&self, response: Response) -> Result<(), FileDownloadError> {
        let token = Uuid::new_v4().to_string();
        let expires = Instant::now() + self.registry.timeout;
        {
            let mut state = self.registry.state.lock().unwrap();
            let count = state.sessions.get_mut(&self.id).ok_or(FileDownloadError {
                message: "the LiveView connection is closed",
            })?;
            if *count >= self.registry.file_limit {
                return Err(FileDownloadError {
                    message: "too many downloads awaiting an HTTP request",
                });
            }
            *count += 1;
            // Cancel the expiry waiter as soon as this file is requested or discarded. The
            // waiter itself must hold neither the stream nor the registry alive.
            let registry = Arc::downgrade(&self.registry.state);
            let cleanup_token = token.clone();
            let cleanup = AbortOnDropHandle::new(tokio::spawn(async move {
                tokio::time::sleep_until(expires).await;
                if let Some(registry) = registry.upgrade() {
                    registry.lock().unwrap().remove(&cleanup_token);
                }
            }));
            state.files.insert(
                token.clone(),
                PendingDownload {
                    session: self.id,
                    expires,
                    response,
                    _cleanup: cleanup,
                },
            );
        }
        if self.sender.send(token.clone()).is_err() {
            self.registry.state.lock().unwrap().remove(&token);
            return Err(FileDownloadError {
                message: "the LiveView connection is closed",
            });
        }
        Ok(())
    }
}

/// Held by the websocket future so cleanup also runs if that future is canceled.
pub(crate) struct DownloadSession(DownloadContext);

impl DownloadSession {
    pub(crate) fn context(&self) -> DownloadContext {
        self.0.clone()
    }
}

impl Drop for DownloadSession {
    fn drop(&mut self) {
        let mut state = self.0.registry.state.lock().unwrap();
        state.sessions.remove(&self.0.id);
        state.files.retain(|_, file| file.session != self.0.id);
    }
}
