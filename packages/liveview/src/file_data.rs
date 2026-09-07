use crate::upload::StoredFile;
use bytes::Bytes;
use dioxus_core::CapturedError;
use dioxus_html::{
    FileData, FormValue, HasFileData, HasFormData, NativeFileData, SerializedFileData,
    SerializedFormData,
};
use futures_util::Stream;
use std::{any::Any, future::Future, io::Read, path::PathBuf, pin::Pin, sync::Arc};

#[derive(Clone)]
pub(crate) struct LiveviewFormData {
    value: String,
    valid: bool,
    values: Vec<(String, FormValue)>,
}

impl LiveviewFormData {
    pub(crate) fn new(form: SerializedFormData, files: Vec<Arc<StoredFile>>) -> Self {
        let mut files = files.into_iter();
        let values = form
            .values
            .into_iter()
            .map(|value| {
                let file = value.file.map(|mut metadata| {
                    metadata.contents = None;
                    UploadedFileData {
                        metadata,
                        storage: files.next(),
                    }
                });
                let data = if let Some(text) = value.text {
                    FormValue::Text(text)
                } else if let Some(file) =
                    file.filter(|file| !file.metadata.path.as_os_str().is_empty())
                {
                    FormValue::File(Some(FileData::new(file)))
                } else {
                    FormValue::File(None)
                };
                (value.key, data)
            })
            .collect();
        Self {
            value: form.value,
            valid: form.valid,
            values,
        }
    }
}

impl HasFormData for LiveviewFormData {
    fn value(&self) -> String {
        self.value.clone()
    }

    fn valid(&self) -> bool {
        self.valid
    }

    fn values(&self) -> Vec<(String, FormValue)> {
        self.values.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl HasFileData for LiveviewFormData {
    fn files(&self) -> Vec<FileData> {
        self.values
            .iter()
            .filter_map(|(_, value)| match value {
                FormValue::File(file) => file.clone(),
                FormValue::Text(_) => None,
            })
            .collect()
    }
}

struct UploadedFileData {
    metadata: SerializedFileData,
    storage: Option<Arc<StoredFile>>,
}

fn require_storage(storage: Option<Arc<StoredFile>>) -> Result<Arc<StoredFile>, CapturedError> {
    storage.ok_or_else(|| CapturedError::msg("File contents not available"))
}

impl NativeFileData for UploadedFileData {
    fn name(&self) -> String {
        self.metadata
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    fn size(&self) -> u64 {
        self.metadata.size
    }

    fn last_modified(&self) -> u64 {
        self.metadata.last_modified
    }

    fn path(&self) -> PathBuf {
        self.storage
            .as_ref()
            .map_or_else(PathBuf::new, |file| file.path.to_path_buf())
    }

    fn content_type(&self) -> Option<String> {
        self.metadata.content_type.clone()
    }

    fn read_bytes(&self) -> Pin<Box<dyn Future<Output = Result<Bytes, CapturedError>>>> {
        let storage = self.storage.clone();
        Box::pin(async move {
            let storage = require_storage(storage)?;
            Ok(
                tokio::task::spawn_blocking(move || std::fs::read(&storage.path))
                    .await??
                    .into(),
            )
        })
    }

    fn read_string(&self) -> Pin<Box<dyn Future<Output = Result<String, CapturedError>>>> {
        let storage = self.storage.clone();
        Box::pin(async move {
            let storage = require_storage(storage)?;
            Ok(
                tokio::task::spawn_blocking(move || std::fs::read_to_string(&storage.path))
                    .await??,
            )
        })
    }

    fn byte_stream(&self) -> Pin<Box<dyn Stream<Item = Result<Bytes, CapturedError>> + Send>> {
        let state = (None::<std::fs::File>, self.storage.clone());
        Box::pin(futures_util::stream::try_unfold(
            state,
            |(file, storage)| async move {
                let storage = require_storage(storage)?;
                tokio::task::spawn_blocking(move || {
                    let mut file = match file {
                        Some(file) => file,
                        None => std::fs::File::open(&storage.path)?,
                    };
                    let mut bytes = vec![0; 64 * 1024];
                    let count = file.read(&mut bytes)?;
                    if count == 0 {
                        return Ok(None);
                    }
                    bytes.truncate(count);
                    Ok::<_, CapturedError>(Some((Bytes::from(bytes), (Some(file), Some(storage)))))
                })
                .await?
            },
        ))
    }

    fn inner(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upload::{FileUploadRegistry, UploadError, UploadSession};
    use futures_util::StreamExt;

    async fn uploaded_file(contents: Bytes) -> (FileData, FileUploadRegistry, UploadSession) {
        let registry = FileUploadRegistry::new(contents.len() as u64);
        let session = registry.new_session();
        let tokens = registry
            .register(&session, &[contents.len() as u64])
            .unwrap();
        let mut writer = registry.begin(&tokens[0], None).await.unwrap();
        writer.write(contents.clone()).await.unwrap();
        writer.finish().unwrap();
        let storage = registry.take_completed(&tokens).unwrap().pop().unwrap();
        let file = FileData::new(UploadedFileData {
            metadata: SerializedFileData {
                path: "browser-name.txt".into(),
                size: contents.len() as u64,
                ..SerializedFileData::empty()
            },
            storage: Some(storage),
        });
        (file, registry, session)
    }

    #[tokio::test]
    async fn byte_stream_keeps_file_and_quota_alive_and_reads_in_chunks() {
        let contents = Bytes::from(vec![42; 150_000]);
        let (file, registry, session) = uploaded_file(contents.clone()).await;
        let path = file.path();
        let mut stream = file.byte_stream();
        drop(file);
        assert!(path.exists());
        assert_eq!(
            registry.register(&session, &[1]),
            Err(UploadError::LimitExceeded)
        );
        let mut total = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            assert!(!chunk.is_empty() && chunk.len() <= 64 * 1024);
            assert_eq!(chunk, contents.slice(total..total + chunk.len()));
            total += chunk.len();
            assert!(path.exists());
            assert_eq!(
                registry.register(&session, &[1]),
                Err(UploadError::LimitExceeded)
            );
        }
        assert_eq!(total, contents.len());
        drop(stream);
        assert!(!path.exists());
        assert!(
            registry
                .register(&session, &[contents.len() as u64])
                .is_ok()
        );
    }

    #[tokio::test]
    async fn dropping_a_partially_read_stream_deletes_the_file() {
        let (file, registry, session) = uploaded_file(Bytes::from(vec![42; 150_000])).await;
        let path = file.path();
        let mut stream = file.byte_stream();
        drop(file);
        assert_eq!(stream.next().await.unwrap().unwrap().len(), 64 * 1024);
        assert!(path.exists());
        drop(stream);
        assert!(!path.exists());
        assert!(registry.register(&session, &[150_000]).is_ok());
    }

    #[tokio::test]
    async fn read_futures_keep_the_temporary_file_alive() {
        let (file, registry, session) = uploaded_file(Bytes::from_static(b"hello")).await;
        let path = file.path();
        let native = file.inner().downcast_ref::<UploadedFileData>().unwrap();
        let bytes = native.read_bytes();
        let text = native.read_string();
        drop(file);
        assert!(path.exists());
        assert_eq!(bytes.await.unwrap(), b"hello"[..]);
        assert_eq!(
            registry.register(&session, &[1]),
            Err(UploadError::LimitExceeded)
        );
        assert_eq!(text.await.unwrap(), "hello");
        assert!(!path.exists());
        assert!(registry.register(&session, &[5]).is_ok());
    }

    #[tokio::test]
    async fn metadata_only_events_cannot_read_client_supplied_server_paths() {
        let server_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(server_file.path(), b"server data").unwrap();
        let form = LiveviewFormData::new(
            SerializedFormData::new(
                String::new(),
                vec![dioxus_html::SerializedFormObject {
                    key: "file".to_string(),
                    text: None,
                    file: Some(SerializedFileData {
                        path: server_file.path().to_path_buf(),
                        size: 11,
                        contents: Some(Bytes::from_static(b"inline data")),
                        ..SerializedFileData::empty()
                    }),
                }],
            ),
            Vec::new(),
        );
        let file = form.files().pop().unwrap();
        assert!(file.path().as_os_str().is_empty());
        assert!(file.read_bytes().await.is_err());
        assert!(file.read_string().await.is_err());
        assert!(file.byte_stream().next().await.unwrap().is_err());
    }
}
