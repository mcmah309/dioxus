#![cfg_attr(not(feature = "axum"), allow(dead_code))]

use bytes::Bytes;
use std::{
    collections::HashMap,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tempfile::{NamedTempFile, TempPath};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, thiserror::Error, PartialEq)]
pub(crate) enum UploadError {
    #[error("unknown or expired LiveView file upload")]
    UnknownOrExpired,
    #[error("LiveView file upload has already started")]
    AlreadyStarted,
    #[error("LiveView file upload was canceled")]
    Canceled,
    #[error("failed to store LiveView file upload: {0}")]
    StorageFailed(String),
    #[error("LiveView file upload exceeds the connection's upload data limit")]
    LimitExceeded,
    #[error("LiveView file upload contains too many files")]
    FileCountLimitExceeded,
    #[error("failed to read LiveView file upload body")]
    BodyReadFailed,
    #[error("LiveView file upload size did not match the declared size")]
    SizeMismatch,
    #[error("LiveView did not receive every file through its HTTP upload handler; mount axum_file_upload at the WebSocket path followed by /upload/{{token}} using the same LiveViewPool")]
    Incomplete,
}

fn storage_failed(error: impl ToString) -> UploadError {
    UploadError::StorageFailed(error.to_string())
}

#[derive(Clone)]
pub(crate) struct FileUploadRegistry {
    uploads: Arc<Mutex<HashMap<String, RegisteredUpload>>>,
    data_limit: u64,
    file_limit: usize,
    timeout: Duration,
}

pub(crate) struct UploadSession {
    reserved_bytes: Arc<AtomicU64>,
    data_limit: u64,
}

struct RegisteredUpload {
    state: Arc<Mutex<UploadState>>,
    group: Arc<UploadGroup>,
    reservation: Arc<UploadReservation>,
}

struct UploadGroup {
    expires: Instant,
    started: AtomicBool,
    canceled: CancellationToken,
}

impl UploadGroup {
    fn is_expired(&self, now: Instant) -> bool {
        !self.started.load(Ordering::Acquire) && self.expires <= now
    }
}

struct UploadReservation {
    bytes: u64,
    reserved_bytes: Arc<AtomicU64>,
}

struct TemporaryUpload {
    file: NamedTempFile,
    reservation: Arc<UploadReservation>,
}

pub(crate) struct StoredFile {
    pub(crate) path: TempPath,
    _reservation: Arc<UploadReservation>,
}

enum UploadState {
    Pending,
    Uploading,
    Complete(Arc<StoredFile>),
    Failed,
}

pub(crate) struct UploadWriter {
    expected_size: u64,
    written: u64,
    file: Option<Arc<TemporaryUpload>>,
    group: Arc<UploadGroup>,
    state: Arc<Mutex<UploadState>>,
    finished: bool,
}

impl Default for FileUploadRegistry {
    fn default() -> Self {
        Self::new(crate::DEFAULT_UPLOAD_LIMIT)
    }
}

impl FileUploadRegistry {
    pub(crate) fn new(data_limit: u64) -> Self {
        Self {
            uploads: Default::default(),
            data_limit,
            file_limit: crate::DEFAULT_UPLOAD_FILE_LIMIT,
            timeout: crate::DEFAULT_UPLOAD_TIMEOUT,
        }
    }

    pub(crate) fn with_limit(self, data_limit: u64) -> Self {
        Self {
            uploads: Default::default(),
            data_limit,
            file_limit: self.file_limit,
            timeout: self.timeout,
        }
    }

    pub(crate) fn with_timeout(self, timeout: Duration) -> Self {
        Self {
            uploads: Default::default(),
            data_limit: self.data_limit,
            file_limit: self.file_limit,
            timeout,
        }
    }

    pub(crate) fn with_file_limit(self, file_limit: usize) -> Self {
        Self {
            uploads: Default::default(),
            data_limit: self.data_limit,
            file_limit,
            timeout: self.timeout,
        }
    }

    pub(crate) fn new_session(&self) -> UploadSession {
        UploadSession {
            reserved_bytes: Default::default(),
            data_limit: self.data_limit,
        }
    }

    pub(crate) fn register(
        &self,
        session: &UploadSession,
        sizes: &[u64],
    ) -> Result<Vec<String>, UploadError> {
        if sizes.len() > self.file_limit {
            return Err(UploadError::FileCountLimitExceeded);
        }
        {
            let now = Instant::now();
            let mut registry = self.uploads.lock().unwrap();
            registry.retain(|_, upload| {
                if upload.group.is_expired(now) {
                    upload.group.canceled.cancel();
                    false
                } else {
                    true
                }
            });
        }
        let reservations = session.reserve(sizes)?;
        let group = Arc::new(UploadGroup {
            expires: Instant::now() + self.timeout,
            started: AtomicBool::new(false),
            canceled: CancellationToken::new(),
        });
        let mut registry = self.uploads.lock().unwrap();
        Ok(reservations
            .into_iter()
            .map(|reservation| {
                let token = Uuid::new_v4().to_string();
                registry.insert(
                    token.clone(),
                    RegisteredUpload {
                        group: group.clone(),
                        reservation: Arc::new(reservation),
                        state: Arc::new(Mutex::new(UploadState::Pending)),
                    },
                );
                token
            })
            .collect())
    }

    pub(crate) async fn wait_for_cleanup(&self, tokens: &[String]) {
        let (mut deadline, canceled) = {
            let registry = self.uploads.lock().unwrap();
            let Some(upload) = tokens.iter().find_map(|token| registry.get(token)) else {
                return;
            };
            (
                (!upload.group.started.load(Ordering::Acquire)).then_some(upload.group.expires),
                upload.group.canceled.clone(),
            )
        };
        loop {
            tokio::select! {
                _ = canceled.cancelled() => {
                    self.cancel(tokens);
                    return;
                }
                _ = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => {
                    let mut registry = self.uploads.lock().unwrap();
                    let now = Instant::now();
                    if tokens.iter().all(|token| registry.get(token).is_none_or(|upload| upload.group.is_expired(now))) {
                        for token in tokens {
                            if let Some(upload) = registry.remove(token) {
                                upload.group.canceled.cancel();
                            }
                        }
                        return;
                    }
                    deadline = None;
                }
            }
        }
    }

    pub(crate) async fn begin(
        &self,
        token: &str,
        content_length: Option<u64>,
    ) -> Result<UploadWriter, UploadError> {
        let (group, state, reservation) = {
            let registry = self.uploads.lock().unwrap();
            let upload = registry.get(token).ok_or(UploadError::UnknownOrExpired)?;
            if upload.group.is_expired(Instant::now()) {
                upload.group.canceled.cancel();
                return Err(UploadError::UnknownOrExpired);
            }
            if upload.group.canceled.is_cancelled() {
                return Err(UploadError::Canceled);
            }
            upload.group.started.store(true, Ordering::Release);
            let mut state = upload.state.lock().unwrap();
            if !matches!(*state, UploadState::Pending) {
                return Err(UploadError::AlreadyStarted);
            }
            if content_length.is_some_and(|size| size != upload.reservation.bytes) {
                *state = UploadState::Failed;
                upload.group.canceled.cancel();
                return Err(UploadError::SizeMismatch);
            }
            *state = UploadState::Uploading;
            (
                upload.group.clone(),
                upload.state.clone(),
                upload.reservation.clone(),
            )
        };
        let mut writer = UploadWriter {
            expected_size: reservation.bytes,
            written: 0,
            file: None,
            group,
            state,
            finished: false,
        };
        writer.file = Some(
            tokio::task::spawn_blocking(move || {
                let file = tempfile::Builder::new()
                    .prefix("dioxus-liveview-")
                    .tempfile()
                    .map_err(storage_failed)?;
                Ok::<_, UploadError>(Arc::new(TemporaryUpload { file, reservation }))
            })
            .await
            .map_err(storage_failed)??,
        );
        if writer.group.canceled.is_cancelled() {
            return Err(UploadError::Canceled);
        }
        Ok(writer)
    }

    pub(crate) fn take_completed(
        &self,
        tokens: &[String],
    ) -> Result<Vec<Arc<StoredFile>>, UploadError> {
        let mut registry = self.uploads.lock().unwrap();
        for token in tokens {
            let upload = registry.get(token).ok_or(UploadError::UnknownOrExpired)?;
            if upload.group.canceled.is_cancelled() {
                return Err(UploadError::Canceled);
            }
            if !matches!(*upload.state.lock().unwrap(), UploadState::Complete(_)) {
                return Err(UploadError::Incomplete);
            }
        }
        Ok(tokens
            .iter()
            .map(|token| {
                let upload = registry.remove(token).unwrap();
                let mut state = upload.state.lock().unwrap();
                let UploadState::Complete(file) =
                    std::mem::replace(&mut *state, UploadState::Failed)
                else {
                    unreachable!("completed uploads were checked before removal")
                };
                file
            })
            .collect())
    }

    pub(crate) fn cancel(&self, tokens: &[String]) {
        let mut registry = self.uploads.lock().unwrap();
        for token in tokens {
            if let Some(upload) = registry.remove(token) {
                upload.group.canceled.cancel();
                *upload.state.lock().unwrap() = UploadState::Failed;
            }
        }
    }
}

impl UploadSession {
    fn reserve(&self, sizes: &[u64]) -> Result<Vec<UploadReservation>, UploadError> {
        let total = sizes.iter().try_fold(0_u64, |total, size| {
            total.checked_add(*size).ok_or(UploadError::LimitExceeded)
        })?;
        let mut reserved = self.reserved_bytes.load(Ordering::Acquire);
        loop {
            let next = reserved
                .checked_add(total)
                .ok_or(UploadError::LimitExceeded)?;
            if next > self.data_limit {
                return Err(UploadError::LimitExceeded);
            }
            match self.reserved_bytes.compare_exchange_weak(
                reserved,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(sizes
                        .iter()
                        .map(|bytes| UploadReservation {
                            bytes: *bytes,
                            reserved_bytes: self.reserved_bytes.clone(),
                        })
                        .collect());
                }
                Err(current) => reserved = current,
            }
        }
    }
}

impl Drop for UploadReservation {
    fn drop(&mut self) {
        self.reserved_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

impl UploadWriter {
    pub(crate) async fn cancelled(&self) {
        self.group.canceled.cancelled().await;
    }

    pub(crate) async fn write(&mut self, bytes: Bytes) -> Result<(), UploadError> {
        if self.group.canceled.is_cancelled() {
            return Err(UploadError::Canceled);
        }
        let next = self
            .written
            .checked_add(bytes.len() as u64)
            .filter(|size| *size <= self.expected_size)
            .ok_or(UploadError::SizeMismatch)?;
        let file = self
            .file
            .as_ref()
            .ok_or(UploadError::AlreadyStarted)?
            .clone();
        // The disk operation keeps the file and its quota alive even if this future is dropped.
        tokio::task::spawn_blocking(move || file.file.as_file().write_all(&bytes))
            .await
            .map_err(storage_failed)?
            .map_err(storage_failed)?;
        if self.group.canceled.is_cancelled() {
            return Err(UploadError::Canceled);
        }
        self.written = next;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(), UploadError> {
        if self.group.canceled.is_cancelled() {
            return Err(UploadError::Canceled);
        }
        if self.written != self.expected_size {
            return Err(UploadError::SizeMismatch);
        }
        let mut state = self.state.lock().unwrap();
        if !matches!(*state, UploadState::Uploading) {
            return Err(UploadError::Canceled);
        }
        let file = Arc::try_unwrap(self.file.take().ok_or(UploadError::AlreadyStarted)?)
            .map_err(|_| storage_failed("upload is still writing"))?;
        *state = UploadState::Complete(Arc::new(StoredFile {
            path: file.file.into_temp_path(),
            _reservation: file.reservation,
        }));
        self.finished = true;
        Ok(())
    }
}

impl Drop for UploadWriter {
    fn drop(&mut self) {
        if !self.finished {
            self.group.canceled.cancel();
            *self.state.lock().unwrap() = UploadState::Failed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DEFAULT_UPLOAD_FILE_LIMIT, DEFAULT_UPLOAD_LIMIT, DEFAULT_UPLOAD_TIMEOUT, LiveViewPool,
    };

    #[test]
    fn registration_does_not_allocate_the_file_size() {
        let registry = FileUploadRegistry::new(u64::MAX);
        let session = registry.new_session();
        let tokens = registry.register(&session, &[u64::MAX]).unwrap();
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), u64::MAX);
        assert_eq!(
            registry.register(&session, &[1]),
            Err(UploadError::LimitExceeded)
        );
        registry.cancel(&tokens);
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
        assert_eq!(
            registry.register(&session, &[u64::MAX, 1]),
            Err(UploadError::LimitExceeded)
        );
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn registration_limits_zero_byte_file_count() {
        let registry = FileUploadRegistry::default().with_file_limit(2);
        let session = registry.new_session();
        assert_eq!(
            registry.register(&session, &[0, 0, 0]),
            Err(UploadError::FileCountLimitExceeded)
        );
        assert!(registry.uploads.lock().unwrap().is_empty());
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);

        let tokens = registry.register(&session, &[0, 0]).unwrap();
        assert_eq!(tokens.len(), 2);
        registry.cancel(&tokens);
    }

    #[tokio::test]
    async fn retained_files_count_toward_their_connections_limit() {
        let registry = FileUploadRegistry::new(3);
        let session = registry.new_session();
        let other_session = registry.new_session();
        let tokens = registry.register(&session, &[2, 1]).unwrap();
        for (token, contents) in tokens.iter().zip([&b"ab"[..], &b"c"[..]]) {
            let mut upload = registry.begin(token, None).await.unwrap();
            upload.write(Bytes::from_static(contents)).await.unwrap();
            upload.finish().unwrap();
        }
        let mut files = registry.take_completed(&tokens).unwrap();
        let paths: Vec<_> = files.iter().map(|file| file.path.to_path_buf()).collect();
        let first = files.remove(0);
        let retained = first.clone();
        drop(first);
        assert!(paths.iter().all(|path| path.exists()));
        assert_eq!(
            registry.register(&session, &[1]),
            Err(UploadError::LimitExceeded)
        );
        let other_tokens = registry.register(&other_session, &[3]).unwrap();
        drop(retained);
        assert!(!paths[0].exists());
        assert!(paths[1].exists());
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 1);
        let incoming = registry.register(&session, &[2]).unwrap();
        assert_eq!(
            registry.register(&session, &[1]),
            Err(UploadError::LimitExceeded)
        );
        drop(files);
        assert!(!paths[1].exists());
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 2);
        registry.cancel(&incoming);
        registry.cancel(&other_tokens);
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn incomplete_uploads_delete_their_temporary_files() {
        let registry = FileUploadRegistry::new(3);
        let session = registry.new_session();
        let tokens = registry.register(&session, &[3]).unwrap();
        let mut upload = registry.begin(&tokens[0], None).await.unwrap();
        let path = upload.file.as_ref().unwrap().file.path().to_path_buf();
        upload.write(Bytes::from_static(b"ab")).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"ab");
        assert_eq!(upload.finish(), Err(UploadError::SizeMismatch));
        assert!(!path.exists());
        registry.wait_for_cleanup(&tokens).await;
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn canceled_disk_writes_keep_their_file_and_quota_until_they_stop() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap()
            .block_on(async {
                let registry = FileUploadRegistry::new(3);
                let session = registry.new_session();
                let tokens = registry.register(&session, &[3]).unwrap();
                let mut upload = registry.begin(&tokens[0], None).await.unwrap();
                let path = upload.file.as_ref().unwrap().file.path().to_path_buf();
                let (started_tx, started_rx) = tokio::sync::oneshot::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let blocker = tokio::task::spawn_blocking(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                });
                started_rx.await.unwrap();
                {
                    let write = upload.write(Bytes::from_static(b"abc"));
                    futures_util::pin_mut!(write);
                    assert!(futures_util::poll!(&mut write).is_pending());
                }
                registry.cancel(&tokens);
                drop(upload);
                assert!(path.exists());
                assert_eq!(
                    registry.register(&session, &[1]),
                    Err(UploadError::LimitExceeded)
                );
                release_tx.send(()).unwrap();
                blocker.await.unwrap();
                tokio::task::spawn_blocking(|| {}).await.unwrap();
                assert!(!path.exists());
                assert!(registry.register(&session, &[3]).is_ok());
            });
    }

    #[tokio::test(start_paused = true)]
    async fn omitted_upload_settings_keep_their_defaults() {
        let pool = LiveViewPool::new();
        let timeout_only = pool.clone().with_upload_timeout(Duration::from_secs(1));
        assert_eq!(
            timeout_only.uploads.register(
                &timeout_only.uploads.new_session(),
                &[DEFAULT_UPLOAD_LIMIT + 1]
            ),
            Err(UploadError::LimitExceeded)
        );
        assert_eq!(
            timeout_only.uploads.register(
                &timeout_only.uploads.new_session(),
                &[0; DEFAULT_UPLOAD_FILE_LIMIT + 1],
            ),
            Err(UploadError::FileCountLimitExceeded)
        );

        let file_limit_only = pool.clone().with_upload_file_limit(2);
        assert_eq!(
            file_limit_only
                .uploads
                .register(&file_limit_only.uploads.new_session(), &[0, 0, 0]),
            Err(UploadError::FileCountLimitExceeded)
        );

        let limit_only = pool.with_upload_limit(2);
        let session = limit_only.uploads.new_session();
        let tokens = limit_only.uploads.register(&session, &[2]).unwrap();
        let expiration = limit_only.uploads.wait_for_cleanup(&tokens);
        futures_util::pin_mut!(expiration);
        assert!(futures_util::poll!(&mut expiration).is_pending());
        tokio::time::advance(DEFAULT_UPLOAD_TIMEOUT - Duration::from_secs(1)).await;
        assert!(futures_util::poll!(&mut expiration).is_pending());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(futures_util::poll!(&mut expiration).is_ready());
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn pools_can_use_independent_upload_limits_and_timeouts() {
        let first = LiveViewPool::new()
            .with_upload_limit(2)
            .with_upload_timeout(Duration::from_secs(10));
        let second = first
            .clone()
            .with_upload_timeout(Duration::from_secs(20))
            .with_upload_limit(3);
        let first_session = first.uploads.new_session();
        let second_session = second.uploads.new_session();
        let first_tokens = first.uploads.register(&first_session, &[2]).unwrap();
        let second_tokens = second.uploads.register(&second_session, &[3]).unwrap();
        for (pool, session) in [(&first, &first_session), (&second, &second_session)] {
            assert_eq!(
                pool.uploads.register(session, &[1]),
                Err(UploadError::LimitExceeded)
            );
        }

        let first_expiration = first.uploads.wait_for_cleanup(&first_tokens);
        let second_expiration = second.uploads.wait_for_cleanup(&second_tokens);
        futures_util::pin_mut!(first_expiration, second_expiration);
        assert!(futures_util::poll!(&mut first_expiration).is_pending());
        assert!(futures_util::poll!(&mut second_expiration).is_pending());

        tokio::time::advance(Duration::from_secs(11)).await;
        assert!(futures_util::poll!(&mut first_expiration).is_ready());
        assert!(futures_util::poll!(&mut second_expiration).is_pending());
        assert_eq!(first_session.reserved_bytes.load(Ordering::Acquire), 0);
        assert_eq!(second_session.reserved_bytes.load(Ordering::Acquire), 3);

        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(futures_util::poll!(&mut second_expiration).is_ready());
        assert_eq!(second_session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn uploads_validate_size_and_are_one_time() {
        let registry = FileUploadRegistry::default();
        let session = registry.new_session();
        let token = registry.register(&session, &[3]).unwrap().pop().unwrap();
        let mut upload = registry.begin(&token, Some(3)).await.unwrap();
        upload.write(Bytes::from_static(&[0, 255])).await.unwrap();
        upload.write(Bytes::from_static(&[128])).await.unwrap();
        upload.finish().unwrap();

        let files = registry
            .take_completed(std::slice::from_ref(&token))
            .unwrap();
        assert_eq!(std::fs::read(&files[0].path).unwrap(), vec![0, 255, 128]);
        assert!(matches!(
            registry.begin(&token, Some(3)).await,
            Err(UploadError::UnknownOrExpired)
        ));
    }

    #[tokio::test]
    async fn completion_requires_every_http_upload_to_finish() {
        let registry = FileUploadRegistry::default();
        let session = registry.new_session();
        let tokens = registry.register(&session, &[1, 0]).unwrap();
        assert!(matches!(
            registry.take_completed(&tokens),
            Err(UploadError::Incomplete)
        ));

        let mut first = registry.begin(&tokens[0], Some(1)).await.unwrap();
        assert!(matches!(
            registry.take_completed(&tokens),
            Err(UploadError::Incomplete)
        ));
        first.write(Bytes::from_static(b"x")).await.unwrap();
        first.finish().unwrap();
        assert!(matches!(
            registry.take_completed(&tokens),
            Err(UploadError::Incomplete)
        ));

        registry
            .begin(&tokens[1], Some(0))
            .await
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(registry.take_completed(&tokens).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_invalid_file_cancels_the_upload() {
        let registry = FileUploadRegistry::default();
        let session = registry.new_session();
        let tokens = registry.register(&session, &[3, 1]).unwrap();
        let mut upload = registry.begin(&tokens[0], None).await.unwrap();
        assert_eq!(
            upload.write(Bytes::from_static(&[1, 2, 3, 4])).await,
            Err(UploadError::SizeMismatch)
        );
        drop(upload);
        assert_eq!(
            registry.begin(&tokens[1], Some(1)).await.err(),
            Some(UploadError::Canceled)
        );
        assert!(matches!(
            registry.take_completed(&tokens),
            Err(UploadError::Canceled)
        ));
        registry.wait_for_cleanup(&tokens).await;
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn cancel_stops_an_active_http_request() {
        let registry = FileUploadRegistry::default();
        let session = registry.new_session();
        let token = registry.register(&session, &[3]).unwrap().pop().unwrap();
        let mut upload = registry.begin(&token, Some(3)).await.unwrap();
        registry.cancel(std::slice::from_ref(&token));

        assert_eq!(
            upload.write(Bytes::from_static(&[1])).await,
            Err(UploadError::Canceled)
        );
        assert_eq!(upload.finish(), Err(UploadError::Canceled));
    }

    #[tokio::test]
    async fn quota_limit_covers_all_live_upload_files() {
        let registry = FileUploadRegistry::new(3);
        let session = registry.new_session();
        let token = registry.register(&session, &[3]).unwrap().pop().unwrap();
        let upload = registry.begin(&token, Some(3)).await.unwrap();
        registry.cancel(std::slice::from_ref(&token));

        assert_eq!(
            registry.register(&session, &[1]),
            Err(UploadError::LimitExceeded)
        );
        drop(upload);
        assert!(registry.register(&session, &[1]).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn expired_uploads_release_quota_before_registration() {
        let registry = FileUploadRegistry::new(4);
        let session = registry.new_session();
        let expired = registry.register(&session, &[3, 1]).unwrap();
        tokio::time::advance(DEFAULT_UPLOAD_TIMEOUT + Duration::from_secs(1)).await;

        let tokens = registry.register(&session, &[4]).unwrap();
        for token in expired {
            assert_eq!(
                registry.begin(&token, None).await.err(),
                Some(UploadError::UnknownOrExpired)
            );
        }
        registry.cancel(&tokens);
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn unused_uploads_release_quota_at_expiration() {
        let registry = FileUploadRegistry::new(4);
        let session = registry.new_session();
        let tokens = registry.register(&session, &[3, 1]).unwrap();
        let expiration = registry.wait_for_cleanup(&tokens);
        futures_util::pin_mut!(expiration);

        assert!(futures_util::poll!(&mut expiration).is_pending());
        tokio::time::advance(DEFAULT_UPLOAD_TIMEOUT - Duration::from_secs(1)).await;
        assert!(futures_util::poll!(&mut expiration).is_pending());
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 4);

        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(futures_util::poll!(&mut expiration).is_ready());
        assert!(registry.uploads.lock().unwrap().is_empty());
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn active_uploads_keep_remaining_credentials_valid() {
        let registry = FileUploadRegistry::new(5);
        let session = registry.new_session();
        let tokens = registry.register(&session, &[3, 1]).unwrap();
        let expiration = registry.wait_for_cleanup(&tokens);
        futures_util::pin_mut!(expiration);
        assert!(futures_util::poll!(&mut expiration).is_pending());

        let mut first = registry.begin(&tokens[0], Some(3)).await.unwrap();
        tokio::time::advance(DEFAULT_UPLOAD_TIMEOUT + Duration::from_secs(1)).await;
        assert!(futures_util::poll!(&mut expiration).is_pending());
        let other = registry.register(&session, &[1]).unwrap();

        first
            .write(Bytes::from_static(&[0, 255, 128]))
            .await
            .unwrap();
        first.finish().unwrap();
        let mut second = registry.begin(&tokens[1], Some(1)).await.unwrap();
        second.write(Bytes::from_static(&[42])).await.unwrap();
        second.finish().unwrap();

        let files = registry.take_completed(&tokens).unwrap();
        assert_eq!(std::fs::read(&files[0].path).unwrap(), vec![0, 255, 128]);
        assert_eq!(std::fs::read(&files[1].path).unwrap(), vec![42]);
        drop(files);
        registry.cancel(&other);
        assert_eq!(session.reserved_bytes.load(Ordering::Acquire), 0);
    }
}
