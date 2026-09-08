use crate::upload::{FileUploadRegistry, StoredFile, UploadSession};
use futures_util::{
    FutureExt,
    future::{AbortHandle, BoxFuture, Shared},
};
use std::sync::Arc;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

type TransferResult = Result<Arc<StoredFile>, String>;

/// Owns a browser File independently of the input that selected it. The shared future starts
/// the HTTP transfer on its first poll and caches the result for every subsequent reader.
pub(crate) struct RemoteFile {
    id: u64,
    commands: UnboundedSender<FileCommand>,
    transfer: Shared<BoxFuture<'static, TransferResult>>,
    retained: bool,
}

impl RemoteFile {
    pub(crate) fn new(
        id: u64,
        size: u64,
        session: &UploadSession,
        uploads: FileUploadRegistry,
        commands: UnboundedSender<FileCommand>,
    ) -> Self {
        let reservation = uploads.reserve(session, size);
        // A rejected handle still exposes metadata, but reading it reports the quota error.
        let retained = reservation.is_ok();
        if !retained {
            let _ = commands.send(FileCommand::Release { id });
        }
        let requests = commands.clone();
        let transfer = async move {
            let reservation = reservation.map_err(|error| error.to_string())?;
            let token = uploads.register_reserved(reservation);
            let mut guard = TransferGuard {
                token: Some(token.clone()),
                uploads: uploads.clone(),
                commands: requests.clone(),
            };
            let (result, receiver) = oneshot::channel();
            requests
                .send(FileCommand::Read {
                    id,
                    upload: PendingFileUpload::new(token, uploads, result),
                })
                .map_err(|_| "LiveView connection closed before reading the file".to_string())?;
            let result = receiver
                .await
                .map_err(|_| "LiveView connection closed during a file upload".to_string())?;
            if result.is_ok() {
                guard.token = None;
            }
            result
        }
        .boxed()
        .shared();
        Self {
            id,
            commands,
            transfer,
            retained,
        }
    }

    pub(crate) async fn read(&self) -> TransferResult {
        self.transfer.clone().await
    }

    pub(crate) fn stored(&self) -> Option<&Arc<StoredFile>> {
        self.transfer.peek().and_then(|result| result.as_ref().ok())
    }
}

impl Drop for RemoteFile {
    fn drop(&mut self) {
        if self.retained {
            let _ = self.commands.send(FileCommand::Release { id: self.id });
        }
    }
}

pub(crate) enum FileCommand {
    Read { id: u64, upload: PendingFileUpload },
    Cancel { token: String },
    Release { id: u64 },
}

struct TransferGuard {
    token: Option<String>,
    uploads: FileUploadRegistry,
    commands: UnboundedSender<FileCommand>,
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            self.uploads.cancel(&token);
            let _ = self.commands.send(FileCommand::Cancel { token });
        }
    }
}

/// The connection owns only the response sender, so it cannot keep an abandoned file alive.
pub(crate) struct PendingFileUpload {
    pub(crate) token: String,
    uploads: FileUploadRegistry,
    result: Option<oneshot::Sender<TransferResult>>,
    pub(crate) cleanup: Option<AbortHandle>,
}

impl PendingFileUpload {
    pub(crate) fn new(
        token: String,
        uploads: FileUploadRegistry,
        result: oneshot::Sender<TransferResult>,
    ) -> Self {
        Self {
            token,
            uploads,
            result: Some(result),
            cleanup: None,
        }
    }

    pub(crate) fn finish(mut self, error: Option<String>) {
        let result = match error {
            Some(error) => Err(error),
            None => self
                .uploads
                .take_completed(&self.token)
                .map_err(|error| error.to_string()),
        };
        let _ = self.result.take().unwrap().send(result);
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.result.as_ref().is_none_or(oneshot::Sender::is_closed)
    }
}

impl Drop for PendingFileUpload {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup.abort();
        }
        self.uploads.cancel(&self.token);
        if let Some(result) = self.result.take() {
            let _ = result.send(Err(
                "LiveView file upload was canceled or expired".to_string()
            ));
        }
    }
}
