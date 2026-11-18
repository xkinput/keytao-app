use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewCandidate {
    pub text: String,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatePreview {
    pub candidates: Vec<PreviewCandidate>,
    pub page_size: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    pub page_size: Option<u8>,
}

pub(crate) const PREVIEW_DIRECTORY_ENV: &str = "KEYTAO_CANDIDATE_PREVIEW_WORKSPACE";
const PREVIEW_DIRECTORY_PREFIX: &str = "keytao-candidate-preview-";

impl PreviewRequest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.page_size == Some(0) {
            return Err("candidate preview page size must be positive".into());
        }
        Ok(())
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub(crate) struct PreviewDirectory(std::path::PathBuf);

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
impl PreviewDirectory {
    pub(crate) fn create() -> Result<Self, String> {
        use std::{
            sync::atomic::{AtomicU64, Ordering},
            time::{SystemTime, UNIX_EPOCH},
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self::create_at(std::env::temp_dir().join(format!(
            "{PREVIEW_DIRECTORY_PREFIX}{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        )))
    }

    fn create_at(path: std::path::PathBuf) -> Result<Self, String> {
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&path)
        };
        #[cfg(not(unix))]
        let result = std::fs::create_dir(&path);
        result.map_err(|e| format!("create preview directory: {e}"))?;
        Ok(Self(path))
    }

    pub(crate) fn snapshot(source: &std::path::Path) -> Result<Self, String> {
        use std::fs;
        let directory = match std::env::var(PREVIEW_DIRECTORY_ENV) {
            Ok(name) => {
                let suffix = name
                    .strip_prefix(PREVIEW_DIRECTORY_PREFIX)
                    .filter(|suffix| {
                        !suffix.is_empty()
                            && suffix
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || byte == b'-')
                    })
                    .ok_or("invalid candidate preview workspace")?;
                let path = std::env::temp_dir().join(format!("{PREVIEW_DIRECTORY_PREFIX}{suffix}"));
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|e| format!("inspect preview workspace: {e}"))?;
                if !metadata.file_type().is_dir() {
                    return Err("candidate preview workspace is not a private directory".into());
                }
                Self(path)
            }
            Err(std::env::VarError::NotPresent) => Self::create()?,
            Err(error) => return Err(format!("invalid candidate preview workspace: {error}")),
        };
        fs::create_dir(directory.0.join("build")).map_err(|e| e.to_string())?;
        fs::create_dir(directory.0.join("log")).map_err(|e| e.to_string())?;
        // Only Rime inputs are copied: never inspect the App state or credentials.
        for entry in fs::read_dir(source).map_err(|e| format!("read preview inputs: {e}"))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_file()
                && matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("yaml" | "lua" | "txt")
                )
            {
                fs::copy(&path, directory.0.join(name.as_ref()))
                    .map_err(|e| format!("copy preview input: {e}"))?;
            } else if path.is_dir() && (name == "lua" || name.ends_with(".userdb")) {
                copy_tree(&path, &directory.0.join(name.as_ref()))?;
            }
        }
        for entry in fs::read_dir(source.join("build"))
            .map_err(|e| format!("read deployed preview config: {e}"))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("yaml") {
                fs::copy(
                    entry.path(),
                    directory.0.join("build").join(entry.file_name()),
                )
                .map_err(|e| format!("copy deployed preview config: {e}"))?;
            }
        }
        Ok(directory)
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
fn copy_tree(source: &std::path::Path, destination: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir(destination).map_err(|e| format!("create preview input directory: {e}"))?;
    for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        } else if entry.path().is_file() {
            std::fs::copy(entry.path(), destination.join(entry.file_name()))
                .map_err(|e| format!("copy preview input: {e}"))?;
        }
    }
    Ok(())
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
impl Drop for PreviewDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "windows", target_os = "macos")
))]
mod tests {
    use super::*;

    #[test]
    fn existing_workspace_is_preserved_when_creation_fails() {
        let directory = PreviewDirectory::create().unwrap();
        let marker = directory.path().join("keep.txt");
        std::fs::write(&marker, "keep").unwrap();
        assert!(PreviewDirectory::create_at(directory.path().to_path_buf()).is_err());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "keep");
    }
}
