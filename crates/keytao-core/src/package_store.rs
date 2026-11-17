use std::path::{Path, PathBuf};

pub fn root(data_dir: &Path) -> PathBuf {
    data_dir.join("scheme-packages")
}

pub fn active_manifest(data_dir: &Path) -> PathBuf {
    root(data_dir).join("active-manifest.json")
}
