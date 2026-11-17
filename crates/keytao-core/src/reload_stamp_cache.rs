use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, PartialEq)]
struct StampMetadata {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(windows)]
    identity: (u32, u64),
    #[cfg(unix)]
    identity: (u64, u64),
}

impl StampMetadata {
    fn read(path: &Path) -> Option<Self> {
        #[cfg(windows)]
        let (metadata, identity) = {
            use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
            use windows::Win32::{
                Foundation::HANDLE,
                Storage::FileSystem::{
                    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES,
                    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
                },
            };

            let file = std::fs::OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES.0)
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
                .open(path)
                .ok()?;
            let metadata = file.metadata().ok()?;
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
            (
                metadata,
                (
                    info.dwVolumeSerialNumber,
                    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
                ),
            )
        };
        #[cfg(not(windows))]
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(any(windows, unix))]
            identity,
        })
    }

    fn signature(&self, hash: u64) -> String {
        let modified = self
            .modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        format!("{}:{modified}:{hash:016x}", self.len)
    }
}

/// Cache a stamp's content hash while its file metadata is unchanged.
#[derive(Clone, Default)]
pub struct ReloadStampCache {
    cached: Option<(PathBuf, StampMetadata, String)>,
}

impl ReloadStampCache {
    pub fn signature_at(&mut self, path: &Path) -> Option<String> {
        self.signature_at_with(path, |path| std::fs::read(path))
    }

    fn signature_at_with(
        &mut self,
        path: &Path,
        read: impl FnOnce(&Path) -> std::io::Result<Vec<u8>>,
    ) -> Option<String> {
        let Some(metadata) = StampMetadata::read(path) else {
            self.cached = None;
            return None;
        };
        if let Some((cached_path, cached_metadata, signature)) = &self.cached {
            if cached_path == path && cached_metadata == &metadata {
                return Some(signature.clone());
            }
        }
        let bytes = match read(path) {
            Ok(bytes) => bytes,
            Err(_) => {
                // Match ReloadStamp's error signature, but retry the read on the next check.
                self.cached = None;
                return Some(metadata.signature(0));
            }
        };
        let signature = metadata.signature(super::fnv1a64(&bytes));
        self.cached = Some((path.to_owned(), metadata, signature.clone()));
        Some(signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn temporary_stamp(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "keytao-stamp-cache-{name}-{}-{suffix}",
            std::process::id()
        ))
    }

    #[test]
    fn unchanged_metadata_skips_read_and_changed_metadata_rehashes() {
        let path = temporary_stamp("changes");
        std::fs::write(&path, "first").unwrap();
        let reads = Cell::new(0);
        let read = |path: &Path| {
            reads.set(reads.get() + 1);
            std::fs::read(path)
        };
        let mut cache = ReloadStampCache::default();
        let first = cache.signature_at_with(&path, &read).unwrap();
        assert_eq!(
            Some(first.clone()),
            super::super::ReloadStamp::signature_at(&path)
        );
        assert_eq!(cache.signature_at_with(&path, &read), Some(first.clone()));
        assert_eq!(reads.get(), 1);

        std::fs::write(&path, "next deployment").unwrap();
        let changed = cache.signature_at_with(&path, &read).unwrap();
        assert_ne!(first, changed);
        assert_eq!(
            Some(changed.clone()),
            super::super::ReloadStamp::signature_at(&path)
        );
        assert_eq!(cache.signature_at_with(&path, &read), Some(changed));
        assert_eq!(reads.get(), 2);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_and_recreated_stamps_invalidate_the_cache() {
        let path = temporary_stamp("recreate");
        let reads = Cell::new(0);
        let read = |path: &Path| {
            reads.set(reads.get() + 1);
            std::fs::read(path)
        };
        let mut cache = ReloadStampCache::default();
        assert_eq!(cache.signature_at_with(&path, &read), None);
        assert_eq!(reads.get(), 0);
        std::fs::write(&path, "first").unwrap();
        assert!(cache.signature_at_with(&path, &read).is_some());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(cache.signature_at_with(&path, &read), None);
        std::fs::write(&path, "first").unwrap();
        assert!(cache.signature_at_with(&path, &read).is_some());
        assert_eq!(reads.get(), 2);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn replacement_with_identical_size_and_mtime_rehashes() {
        let path = temporary_stamp("replacement");
        std::fs::write(&path, "first").unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut cache = ReloadStampCache::default();
        let first = cache.signature_at(&path).unwrap();
        let replacement = path.with_extension("new");
        std::fs::write(&replacement, "other").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        assert_ne!(cache.signature_at(&path).unwrap(), first);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_reads_are_retried_without_a_metadata_change() {
        let path = temporary_stamp("retry");
        std::fs::write(&path, "first").unwrap();
        let mut cache = ReloadStampCache::default();
        let failed = cache.signature_at_with(&path, |_| {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        });
        assert_eq!(
            failed,
            Some(StampMetadata::read(&path).unwrap().signature(0))
        );
        let reads = Cell::new(0);
        let read = |path: &Path| {
            reads.set(reads.get() + 1);
            std::fs::read(path)
        };
        let current = cache.signature_at_with(&path, &read);
        assert_ne!(current, failed);
        assert_eq!(cache.signature_at_with(&path, &read), current);
        assert_eq!(reads.get(), 1);
        std::fs::remove_file(path).unwrap();
    }
}
