use crate::{
    LiveViewError,
    document::init_document,
    element::LiveviewElement,
    events::SerializedHtmlEventConverter,
    query::{QueryEngine, QueryResult},
};

use crate::{
    file_data::LiveviewFormData,
    upload::{StoredFile, UploadSession},
};
use dioxus_core::{Element, Event, ScopeId, VirtualDom, provide_context};
use dioxus_html::{EventData, HtmlEvent, PlatformEventData};
use dioxus_interpreter_js::MutationState;
use futures_util::{SinkExt, StreamExt, pin_mut};
use serde::{Deserialize, Serialize};
use std::{any::Any, rc::Rc, sync::Arc};
use tokio_util::task::LocalPoolHandle;

#[derive(Deserialize, Debug)]
struct FileUploadStart {
    size: u64,
    event: Box<HtmlEvent>,
}

#[derive(Deserialize, Debug)]
struct FileUploadCancel {}

#[derive(Deserialize, Debug)]
struct FileUploadComplete {}

#[derive(Deserialize, Debug)]
#[serde(tag = "method", content = "params")]
enum IpcMessage {
    #[serde(rename = "user_event")]
    Event(Box<HtmlEvent>),
    // Validate upload metadata in the handler so malformed requests receive an error response.
    #[serde(rename = "file_upload")]
    FileUpload(serde_json::Value),
    #[serde(rename = "file_upload_cancel")]
    FileUploadCancel(FileUploadCancel),
    #[serde(rename = "file_upload_complete")]
    FileUploadComplete(FileUploadComplete),
    #[serde(rename = "query")]
    Query(QueryResult),
}

struct PendingFileUpload {
    event: Option<Box<HtmlEvent>>,
    file_count: usize,
    tokens: Vec<String>,
    uploads: crate::upload::FileUploadRegistry,
}

impl PendingFileUpload {
    fn new(
        upload: FileUploadStart,
        uploads: crate::upload::FileUploadRegistry,
        session: &UploadSession,
    ) -> Result<Self, String> {
        let EventData::Form(form) = &upload.event.data else {
            return Err("file upload did not contain a form event".to_string());
        };
        let mut size = 0_u64;
        let mut sizes = Vec::new();
        for value in &form.values {
            let Some(file) = value.file.as_ref() else {
                continue;
            };
            size = size
                .checked_add(file.size)
                .ok_or_else(|| "file upload size overflowed".to_string())?;
            sizes.push(file.size);
        }
        if sizes.is_empty() {
            return Err("file upload did not contain any files".to_string());
        }
        if size != upload.size {
            return Err("file upload size did not match its file metadata".to_string());
        }
        let tokens = uploads
            .register(session, &sizes)
            .map_err(|error| error.to_string())?;

        Ok(Self {
            event: Some(upload.event),
            file_count: sizes.len(),
            tokens,
            uploads,
        })
    }

    fn credentials(&self) -> Vec<String> {
        self.tokens.clone()
    }

    fn cancel(&mut self) {
        self.uploads.cancel(&self.tokens);
        self.tokens.clear();
    }

    fn finish(mut self) -> Result<(Box<HtmlEvent>, Vec<Arc<StoredFile>>), String> {
        let files = self
            .uploads
            .take_completed(&self.tokens)
            .map_err(|error| error.to_string())?;
        self.tokens.clear();
        if files.len() != self.file_count {
            return Err("file upload response did not contain every file".to_string());
        }
        Ok((self.event.take().unwrap(), files))
    }
}

impl Drop for PendingFileUpload {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn file_upload_failed(error: impl ToString) -> LiveViewError {
    LiveViewError::FileUploadFailed(error.to_string())
}

fn dispatch_event(
    vdom: &VirtualDom,
    query_engine: &QueryEngine,
    event: Box<HtmlEvent>,
    files: Vec<Arc<StoredFile>>,
) {
    let HtmlEvent {
        element,
        name,
        bubbles,
        data,
    } = *event;
    // Intercept the mounted event and insert a custom element type.
    let event = if let EventData::Mounted = &data {
        let element = LiveviewElement::new(element, query_engine.clone());
        Event::new(
            Rc::new(PlatformEventData::new(Box::new(element))) as Rc<dyn Any>,
            bubbles,
        )
    } else if let EventData::Form(form) = data {
        Event::new(
            Rc::new(PlatformEventData::new(Box::new(LiveviewFormData::new(
                form, files,
            )))) as Rc<dyn Any>,
            bubbles,
        )
    } else {
        Event::new(data.into_any(), bubbles)
    };
    vdom.runtime().handle_event(&name, event, element);
}

#[derive(Clone)]
pub struct LiveViewPool {
    pub(crate) pool: LocalPoolHandle,
    pub(crate) uploads: crate::upload::FileUploadRegistry,
}

impl Default for LiveViewPool {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveViewPool {
    pub fn new() -> Self {
        // Set the event converter
        dioxus_html::set_event_converter(Box::new(SerializedHtmlEventConverter));

        LiveViewPool {
            pool: LocalPoolHandle::new(
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1),
            ),
            uploads: Default::default(),
        }
    }

    /// Set the total bytes each LiveView connection may hold in incoming and completed uploads.
    ///
    /// Files are streamed to temporary files. Their declared sizes count toward the limit from
    /// registration until the last file handle or reader is dropped, including handles retained
    /// by the application after the event. Defaults to [`crate::DEFAULT_UPLOAD_LIMIT`] (1 GiB).
    pub fn with_upload_limit(mut self, limit: u64) -> Self {
        self.uploads = self.uploads.with_limit(limit);
        self
    }

    /// Set the maximum number of files accepted in one LiveView upload batch.
    ///
    /// This limit also applies to zero-byte files so a batch cannot consume unbounded registry
    /// entries or temporary-file metadata without counting toward the byte limit. Defaults to
    /// [`crate::DEFAULT_UPLOAD_FILE_LIMIT`] (1024 files).
    pub fn with_upload_file_limit(mut self, limit: usize) -> Self {
        self.uploads = self.uploads.with_file_limit(limit);
        self
    }

    /// Set how long a registered upload batch may wait for its first HTTP request.
    ///
    /// Unused batches release their quota reservations when this timeout expires. Once any file
    /// starts uploading, the batch remains valid until completion or cancellation by the websocket.
    /// Defaults to [`crate::DEFAULT_UPLOAD_TIMEOUT`] (five minutes).
    pub fn with_upload_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.uploads = self.uploads.with_timeout(timeout);
        self
    }

    /// Run an existing VirtualDom on the caller's executor using this pool's upload registry.
    ///
    /// Pass a clone of this pool to the HTTP upload handler so it can receive files for this
    /// connection. The returned future is not `Send`; await it on a local executor. Use
    /// [`Self::launch_virtualdom`] to create and run a VirtualDom on the pool's threads instead.
    pub async fn run(
        &self,
        vdom: VirtualDom,
        ws: impl LiveViewSocket,
    ) -> Result<(), LiveViewError> {
        run_with_uploads(vdom, ws, self.uploads.clone()).await
    }

    pub async fn launch(
        &self,
        ws: impl LiveViewSocket,
        app: fn() -> Element,
    ) -> Result<(), LiveViewError> {
        self.launch_with_props(ws, |app| app(), app).await
    }

    pub async fn launch_with_props<T: Clone + Send + 'static>(
        &self,
        ws: impl LiveViewSocket,
        app: fn(T) -> Element,
        props: T,
    ) -> Result<(), LiveViewError> {
        self.launch_virtualdom(ws, move || VirtualDom::new_with_props(app, props))
            .await
    }

    pub async fn launch_virtualdom<F: FnOnce() -> VirtualDom + Send + 'static>(
        &self,
        ws: impl LiveViewSocket,
        make_app: F,
    ) -> Result<(), LiveViewError> {
        let uploads = self.uploads.clone();
        match self
            .pool
            .spawn_pinned(move || run_with_uploads(make_app(), ws, uploads))
            .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(LiveViewError::SendingFailed),
        }
    }
}

/// A LiveViewSocket is a Sink and Stream of bytes that Dioxus uses to communicate with the client.
///
/// Most websockets from most HTTP frameworks can be converted into a LiveViewSocket using the appropriate adapter.
///
/// You can also convert your own socket into a LiveViewSocket by implementing this trait. This trait is an auto trait,
/// meaning that as long as your type implements Stream and Sink, you can use it as a LiveViewSocket.
///
/// For example, the axum implementation is a really small transform:
///
/// ```rust
/// use axum::extract::ws::{Message, WebSocket};
/// use dioxus_liveview::{LiveViewError, LiveViewSocket};
/// use futures_util::{SinkExt, StreamExt};
///
/// pub fn axum_socket(ws: WebSocket) -> impl LiveViewSocket {
///     ws.map(transform_rx)
///         .with(transform_tx)
///         .sink_map_err(|_| LiveViewError::SendingFailed)
/// }
///
/// fn transform_rx(message: Result<Message, axum::Error>) -> Result<Vec<u8>, LiveViewError> {
///     message
///         .map_err(|_| LiveViewError::SendingFailed)?
///         .into_text()
///         .map(|text| text.as_str().into())
///         .map_err(|_| LiveViewError::SendingFailed)
/// }
///
/// async fn transform_tx(message: Vec<u8>) -> Result<Message, axum::Error> {
///     Ok(Message::Binary(message.into()))
/// }
/// ```
pub trait LiveViewSocket:
    SinkExt<Vec<u8>, Error = LiveViewError>
    + StreamExt<Item = Result<Vec<u8>, LiveViewError>>
    + Send
    + 'static
{
}

impl<S> LiveViewSocket for S where
    S: SinkExt<Vec<u8>, Error = LiveViewError>
        + StreamExt<Item = Result<Vec<u8>, LiveViewError>>
        + Send
        + 'static
{
}

/// The primary event loop for the VirtualDom waiting for user input
///
/// This function makes it easy to integrate Dioxus LiveView with any socket-based framework.
///
/// As long as your framework can provide a Sink and Stream of Bytes, you can use this function.
///
/// You might need to transform the error types of the web backend into the LiveView error type.
///
/// For file uploads, use [`LiveViewPool::run`] with the same pool as your HTTP upload handler.
/// This standalone function cannot share its upload registry with that handler.
#[deprecated(note = "Use LiveViewPool::run with the same pool as the HTTP upload handler")]
pub async fn run(vdom: VirtualDom, ws: impl LiveViewSocket) -> Result<(), LiveViewError> {
    run_with_uploads(vdom, ws, Default::default()).await
}

async fn run_with_uploads(
    mut vdom: VirtualDom,
    ws: impl LiveViewSocket,
    uploads: crate::upload::FileUploadRegistry,
) -> Result<(), LiveViewError> {
    #[cfg(all(feature = "devtools", debug_assertions))]
    let mut hot_reload_rx = {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        dioxus_devtools::connect(move |template| _ = tx.send(template));
        rx
    };

    let mut mutations = MutationState::default();

    // Create the a proxy for query engine
    let (query_tx, mut query_rx) = tokio::sync::mpsc::unbounded_channel();
    let query_engine = QueryEngine::new(query_tx);
    vdom.runtime().in_scope(ScopeId::ROOT, || {
        provide_context(query_engine.clone());
        init_document();
    });

    // pin the futures so we can use select!
    pin_mut!(ws);

    if let Some(edits) = {
        vdom.rebuild(&mut mutations);
        take_edits(&mut mutations)
    } {
        // send the initial render to the client
        ws.send(edits).await?;
    }

    let upload_session = uploads.new_session();
    let mut pending_file_upload: Option<PendingFileUpload> = None;

    loop {
        #[cfg(all(feature = "devtools", debug_assertions))]
        let hot_reload_wait = hot_reload_rx.recv();
        #[cfg(not(all(feature = "devtools", debug_assertions)))]
        let hot_reload_wait: std::future::Pending<Option<()>> = std::future::pending();

        tokio::select! {
            // poll any futures or suspense
            _ = vdom.wait_for_work() => {}

            _ = async {
                match &pending_file_upload {
                    Some(upload) => uploads.wait_for_cleanup(&upload.tokens).await,
                    None => std::future::pending().await,
                }
            } => {
                pending_file_upload = None;
            }

            evt = ws.next() => {
                match evt.as_ref().map(|o| o.as_deref()) {
                    // respond with a pong every ping to keep the websocket alive
                    Some(Ok(b"__ping__")) => {
                        ws.send(text_frame("__pong__")).await?;
                    }
                    Some(Ok(evt)) => {
                        if let Ok(message) = serde_json::from_str::<IpcMessage>(&String::from_utf8_lossy(evt)) {
                            match message {
                                IpcMessage::Event(evt) => {
                                    dispatch_event(&vdom, &query_engine, evt, Vec::new());
                                }
                                IpcMessage::FileUpload(upload) => {
                                    if pending_file_upload.is_some() {
                                        return Err(file_upload_failed(
                                            "received a new file upload before the previous upload completed",
                                        ));
                                    }
                                    let upload = serde_json::from_value::<FileUploadStart>(upload)
                                        .map_err(|error| format!("invalid file upload metadata: {error}"))
                                        .and_then(|upload| PendingFileUpload::new(upload, uploads.clone(), &upload_session));
                                    let response = match upload {
                                        Ok(upload) => {
                                            let credentials = upload.credentials();
                                            pending_file_upload = Some(upload);
                                            ClientUpdate::FileUpload(credentials)
                                        }
                                        Err(error) => ClientUpdate::FileUploadError(error),
                                    };
                                    ws.send(text_frame(
                                        &serde_json::to_string(&response).unwrap(),
                                    ))
                                    .await?;
                                }
                                IpcMessage::FileUploadCancel(_) => {
                                    pending_file_upload = None;
                                }
                                IpcMessage::FileUploadComplete(_) => {
                                    let result = pending_file_upload.take().ok_or_else(|| {
                                        "received file upload completion without a pending upload".to_string()
                                    }).and_then(PendingFileUpload::finish);
                                    let response = match result {
                                        Ok((event, files)) => {
                                            dispatch_event(&vdom, &query_engine, event, files);
                                            ClientUpdate::FileUploadComplete
                                        }
                                        Err(error) => ClientUpdate::FileUploadError(error),
                                    };
                                    ws.send(text_frame(
                                        &serde_json::to_string(&response).unwrap(),
                                    )).await?;
                                }
                                IpcMessage::Query(result) => {
                                    query_engine.send(result);
                                },
                            }
                        }
                    }
                    // log this I guess? when would we get an error here?
                    Some(Err(_e)) => {}
                    None => return Ok(()),
                }
            }

            // handle any new queries
            Some(query) = query_rx.recv() => {
                ws.send(text_frame(&serde_json::to_string(&ClientUpdate::Query(query)).unwrap())).await?;
            }

            Some(msg) = hot_reload_wait => {
                #[cfg(all(feature = "devtools", debug_assertions))]
                match msg {
                    dioxus_devtools::DevserverMsg::HotReload(msg)=> {
                        dioxus_devtools::apply_changes(&vdom, &msg);
                    }
                    dioxus_devtools::DevserverMsg::Shutdown => {
                        std::process::exit(0);
                    },
                    dioxus_devtools::DevserverMsg::FullReloadCommand
                    | dioxus_devtools::DevserverMsg::FullReloadStart
                    | dioxus_devtools::DevserverMsg::FullReloadFailed => {
                        // usually only web gets this message - what are we supposed to do?
                        // Maybe we could just binary patch ourselves in place without losing window state?
                    },
                    _ => {}
                }
                #[cfg(not(all(feature = "devtools", debug_assertions)))]
                let () = msg;
            }
        }

        // render the vdom
        vdom.render_immediate(&mut mutations);

        if let Some(edits) = take_edits(&mut mutations) {
            ws.send(edits).await?;
        }
    }
}

fn text_frame(text: &str) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend(text.as_bytes());
    bytes
}

fn take_edits(mutations: &mut MutationState) -> Option<Vec<u8>> {
    // Add an extra one at the beginning to tell the shim this is a binary frame
    let mut bytes = vec![1];
    mutations.write_memory_into(&mut bytes);
    (bytes.len() > 1).then_some(bytes)
}

#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum ClientUpdate {
    #[serde(rename = "query")]
    Query(String),
    #[serde(rename = "file_upload")]
    FileUpload(Vec<String>),
    #[serde(rename = "file_upload_complete")]
    FileUploadComplete,
    #[serde(rename = "file_upload_error")]
    FileUploadError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_upload(uploads: crate::upload::FileUploadRegistry) -> PendingFileUpload {
        let message: IpcMessage = serde_json::from_value(serde_json::json!({
            "method": "file_upload",
            "params": {
                "size": 3,
                "event": {
                    "element": 0,
                    "name": "change",
                    "bubbles": true,
                    "data": {
                        "values": [
                            { "key": "description", "text": "upload" },
                            {
                                "key": "files",
                                "file": {
                                    "path": "hello.bin",
                                    "size": 3,
                                    "last_modified": 123,
                                    "content_type": "application/octet-stream"
                                }
                            },
                            {
                                "key": "files",
                                "file": {
                                    "path": "empty.bin",
                                    "size": 0,
                                    "last_modified": 456,
                                    "content_type": "application/octet-stream"
                                }
                            }
                        ]
                    }
                }
            }
        }))
        .unwrap();
        let IpcMessage::FileUpload(upload) = message else {
            unreachable!()
        };
        PendingFileUpload::new(
            serde_json::from_value(upload).unwrap(),
            uploads.clone(),
            &uploads.new_session(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn http_uploads_are_attached_to_the_form_event() {
        let uploads = crate::upload::FileUploadRegistry::default();
        let upload = pending_upload(uploads.clone());
        let credentials = upload.credentials();
        let mut file = uploads.begin(&credentials[0], Some(3)).await.unwrap();
        file.write(bytes::Bytes::from_static(&[0, 255, 128]))
            .await
            .unwrap();
        file.finish().unwrap();
        uploads
            .begin(&credentials[1], Some(0))
            .await
            .unwrap()
            .finish()
            .unwrap();

        let (event, files) = upload.finish().unwrap();
        let EventData::Form(form) = event.data else {
            unreachable!()
        };
        let form = dioxus_html::FormData::new(LiveviewFormData::new(form, files));
        assert_eq!(form.get_first("description").unwrap(), "upload");
        let files = form.files();
        assert_eq!(files[0].name(), "hello.bin");
        assert_eq!(files[0].size(), 3);
        assert_eq!(files[0].last_modified(), 123);
        assert_eq!(
            files[0].content_type().as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(
            files[0].read_bytes().await.unwrap().as_ref(),
            &[0, 255, 128]
        );
        assert!(files[1].read_bytes().await.unwrap().is_empty());
        assert_eq!(files[1].name(), "empty.bin");
        assert!(files[0].path().is_file());
        let paths: Vec<_> = files.iter().map(|file| file.path()).collect();
        drop(form);
        assert!(paths.iter().all(|path| path.exists()));
        drop(files);
        assert!(paths.iter().all(|path| !path.exists()));
    }

    #[test]
    fn upload_size_must_match_the_file_metadata() {
        let uploads = crate::upload::FileUploadRegistry::default();
        let mut pending = pending_upload(uploads);
        let mut upload = pending.event.take().unwrap();
        let EventData::Form(form) = &mut upload.data else {
            unreachable!()
        };
        form.values[1].file.as_mut().unwrap().size = 4;
        let upload = FileUploadStart {
            size: 3,
            event: upload,
        };
        let uploads = crate::upload::FileUploadRegistry::default();
        assert!(PendingFileUpload::new(upload, uploads.clone(), &uploads.new_session()).is_err());
    }
}
