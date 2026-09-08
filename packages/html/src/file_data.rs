use bytes::Bytes;
use futures_util::Stream;
use std::{path::PathBuf, pin::Pin, prelude::rust_2024::Future};

#[derive(Clone)]
pub struct FileData {
    inner: std::sync::Arc<dyn NativeFileData>,
}

impl FileData {
    pub fn new(inner: impl NativeFileData + 'static) -> Self {
        Self {
            inner: std::sync::Arc::new(inner),
        }
    }

    pub fn content_type(&self) -> Option<String> {
        self.inner.content_type()
    }

    pub fn name(&self) -> String {
        self.inner.name()
    }

    pub fn size(&self) -> u64 {
        self.inner.size()
    }

    pub fn last_modified(&self) -> u64 {
        self.inner.last_modified()
    }

    pub async fn read_bytes(&self) -> Result<Bytes, dioxus_core::CapturedError> {
        self.inner.read_bytes().await
    }

    pub async fn read_string(&self) -> Result<String, dioxus_core::CapturedError> {
        self.inner.read_string().await
    }

    pub fn byte_stream(
        &self,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, dioxus_core::CapturedError>> + Send + 'static>>
    {
        self.inner.byte_stream()
    }

    pub fn inner(&self) -> &dyn std::any::Any {
        self.inner.inner()
    }

    pub fn path(&self) -> PathBuf {
        self.inner.path()
    }
}

impl PartialEq for FileData {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
            && self.size() == other.size()
            && self.last_modified() == other.last_modified()
            && self.path() == other.path()
    }
}

pub trait NativeFileData: Send + Sync {
    fn name(&self) -> String;
    fn size(&self) -> u64;
    fn last_modified(&self) -> u64;
    fn path(&self) -> PathBuf;
    fn content_type(&self) -> Option<String>;
    fn read_bytes(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes, dioxus_core::CapturedError>> + 'static>>;
    fn byte_stream(
        &self,
    ) -> Pin<
        Box<
            dyn futures_util::Stream<Item = Result<Bytes, dioxus_core::CapturedError>>
                + 'static
                + Send,
        >,
    >;
    fn read_string(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<String, dioxus_core::CapturedError>> + 'static>>;
    fn inner(&self) -> &dyn std::any::Any;
}

impl std::fmt::Debug for FileData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileData")
            .field("name", &self.inner.name())
            .field("size", &self.inner.size())
            .field("last_modified", &self.inner.last_modified())
            .finish()
    }
}

pub trait HasFileData: std::any::Any {
    fn files(&self) -> Vec<FileData>;
}

#[cfg(feature = "serialize")]
pub use serialize::*;

#[cfg(feature = "serialize")]
mod serialize {
    use super::*;
    use serde::Deserialize;
    use std::cell::RefCell;

    /// A serializable representation of file metadata
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
    pub struct SerializedFileData {
        pub path: PathBuf,
        pub size: u64,
        pub last_modified: u64,
        pub content_type: Option<String>,
    }

    impl SerializedFileData {
        /// Create a new empty serialized file data object
        pub fn empty() -> Self {
            Self {
                path: PathBuf::new(),
                size: 0,
                last_modified: 0,
                content_type: None,
            }
        }

        /// Create serialized file metadata without eagerly reading the file contents.
        pub(crate) fn from_file_data(file_data: &FileData) -> Self {
            Self {
                path: file_data.path(),
                size: file_data.size(),
                last_modified: file_data.last_modified(),
                content_type: file_data.content_type(),
            }
        }
    }

    impl NativeFileData for SerializedFileData {
        fn name(&self) -> String {
            self.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        }

        fn size(&self) -> u64 {
            self.size
        }

        fn last_modified(&self) -> u64 {
            self.last_modified
        }

        fn read_bytes(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Bytes, dioxus_core::CapturedError>> + 'static>>
        {
            let path = self.path.clone();

            Box::pin(async move {
                #[cfg(not(target_arch = "wasm32"))]
                if path.exists() {
                    return Ok(std::fs::read(path).map(Bytes::from)?);
                }

                Err(dioxus_core::CapturedError::msg(
                    "File contents not available",
                ))
            })
        }

        fn read_string(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<String, dioxus_core::CapturedError>> + 'static>>
        {
            let path = self.path.clone();

            Box::pin(async move {
                #[cfg(not(target_arch = "wasm32"))]
                if path.exists() {
                    return Ok(std::fs::read_to_string(path)?);
                }

                Err(dioxus_core::CapturedError::msg(
                    "File contents not available",
                ))
            })
        }

        fn byte_stream(
            &self,
        ) -> Pin<
            Box<
                dyn futures_util::Stream<Item = Result<Bytes, dioxus_core::CapturedError>>
                    + 'static
                    + Send,
            >,
        > {
            let path = self.path.clone();

            Box::pin(futures_util::stream::once(async move {
                #[cfg(not(target_arch = "wasm32"))]
                if path.exists() {
                    return Ok(std::fs::read(path).map(Bytes::from)?);
                }

                Err(dioxus_core::CapturedError::msg(
                    "File contents not available",
                ))
            }))
        }

        fn inner(&self) -> &dyn std::any::Any {
            self
        }

        fn path(&self) -> PathBuf {
            self.path.clone()
        }

        fn content_type(&self) -> Option<String> {
            self.content_type.clone()
        }
    }

    pub(crate) const FILE_DATA_NEWTYPE: &str = "$dioxus::FileData";

    thread_local! {
        static FORM_FILES: RefCell<Vec<(FileData, SerializedFileData)>> = const { RefCell::new(Vec::new()) };
    }

    // Serde's visitors only carry data-model values. During synchronous form parsing, the
    // private FileData newtype transports an index into these handles instead of a raw path.
    // Restore the previous context on errors, panics, and nested calls to parsed_values().
    pub(crate) fn with_form_files<T>(
        files: Vec<(FileData, SerializedFileData)>,
        parse: impl FnOnce() -> T,
    ) -> T {
        struct RestoreFiles(Vec<(FileData, SerializedFileData)>);
        impl Drop for RestoreFiles {
            fn drop(&mut self) {
                FORM_FILES.set(std::mem::take(&mut self.0));
            }
        }
        let _restore = RestoreFiles(FORM_FILES.replace(files));
        parse()
    }

    impl<'de> serde::Deserialize<'de> for FileData {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct FileVisitor;
            impl<'de> serde::de::Visitor<'de> for FileVisitor {
                type Value = FileData;

                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("file metadata or an owned form file")
                }

                fn visit_u64<E: serde::de::Error>(self, index: u64) -> Result<FileData, E> {
                    FORM_FILES.with(|files| {
                        usize::try_from(index)
                            .ok()
                            .and_then(|index| {
                                files.borrow().get(index).map(|(file, _)| file.clone())
                            })
                            .ok_or_else(|| E::custom("form file is no longer available"))
                    })
                }

                fn visit_newtype_struct<D: serde::Deserializer<'de>>(
                    self,
                    deserializer: D,
                ) -> Result<FileData, D::Error> {
                    let metadata = SerializedFileData::deserialize(deserializer)?;
                    // Untagged enums may buffer metadata before invoking this visitor. Match
                    // the snapshot captured before parsing: a lazy file's path can change when
                    // a concurrent read finishes. Unread files can have identical metadata, so
                    // reject ambiguity instead of returning a different file's handle.
                    FORM_FILES.with(|files| {
                        let files = files.borrow();
                        let mut matches = files.iter().filter(|(_, snapshot)| *snapshot == metadata);
                        let Some((file, _)) = matches.next() else {
                            return Ok(FileData::new(metadata));
                        };
                        if matches.any(|(other, _)| !std::sync::Arc::ptr_eq(&file.inner, &other.inner)) {
                            return Err(serde::de::Error::custom(
                                "buffered file metadata is ambiguous; deserialize FileData fields directly",
                            ));
                        }
                        Ok(file.clone())
                    })
                }
            }
            deserializer.deserialize_newtype_struct(FILE_DATA_NEWTYPE, FileVisitor)
        }
    }
}
