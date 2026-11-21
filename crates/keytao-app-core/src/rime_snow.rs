//! Built-in Snow Jiandao package preparation and release discovery.

use crate::{
    Core, CoreError,
    generic_rime::{self, DEFAULT_MAX_EXPANDED_BYTES},
    wanxiang::RemotePackage,
};
use std::fs;

pub const SCHEMA_ID: &str = "snow_jiandao";
const REPO: &str = "rimeinn/rime-snow-pinyin";

#[cfg(test)]
pub(crate) fn package_from_location(location: &str) -> Result<RemotePackage, String> {
    crate::rime_frost::release_package_from_location(location, REPO, "snow-pinyin.zip")
}

pub(crate) async fn latest_package() -> Result<RemotePackage, String> {
    crate::rime_frost::latest_release(REPO, "snow-pinyin.zip").await
}

pub(crate) fn default_custom() -> (String, Vec<u8>) {
    (
        "default.custom.yaml".into(),
        b"patch:\n  schema_list:\n    - schema: snow_jiandao\n".to_vec(),
    )
}

pub(crate) fn prepare_archive(core: &Core, bytes: Vec<u8>) -> Result<Vec<u8>, CoreError> {
    fs::create_dir_all(&core.env().cache_dir)?;
    let work = tempfile::tempdir_in(&core.env().cache_dir)?;
    let path = work.path().join("snow.zip");
    fs::write(&path, &bytes)?;
    let stage = work.path().join("files");
    let extracted = generic_rime::extract_archive(
        &path,
        &stage,
        Some(SCHEMA_ID),
        DEFAULT_MAX_EXPANDED_BYTES,
    )
    .map_err(CoreError::Other)?;
    let mut files = extracted
        .into_iter()
        .map(|file| Ok((file.path.clone(), fs::read(stage.join(file.path))?)))
        .collect::<Result<Vec<_>, CoreError>>()?;
    if crate::package_schemas::default_schemas(&files)?.is_empty() {
        files.retain(|(name, _)| name != "default.custom.yaml");
        files.push(default_custom());
    }
    crate::packages::archive_from_files(&files)
}
