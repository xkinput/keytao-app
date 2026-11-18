//! Built-in Snow Jiandao package preparation and release discovery.

use crate::{Core, CoreError, shape_addons::Package, wanxiang::RemotePackage};
use std::{
    fs,
    io::{Cursor, Read},
};

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
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes))
        .map_err(|error| CoreError::Other(error.to_string()))?;
    let upstream = archive
        .file_names()
        .any(|name| name == "snow_jiandao.schema.yaml")
        && archive
            .file_names()
            .any(|name| name == "radical_pinyin.schema.yaml");
    if !upstream {
        drop(archive);
        return Ok(bytes);
    }
    #[derive(serde::Deserialize)]
    struct Schema {
        schema: Info,
    }
    #[derive(serde::Deserialize)]
    struct Info {
        version: String,
    }
    let version = serde_yaml::from_reader::<_, Schema>(
        archive
            .by_name("snow_jiandao.schema.yaml")
            .map_err(|error| CoreError::Other(error.to_string()))?
            .take(1024 * 1024),
    )
    .map(|schema| schema.schema.version)
    .unwrap_or_default();
    fs::create_dir_all(&core.env().cache_dir)?;
    let work = tempfile::tempdir_in(&core.env().cache_dir)?;
    let path = work.path().join("snow.zip");
    fs::write(&path, &bytes)?;
    let stage = work.path().join("files");
    let package = RemotePackage {
        version,
        source: format!("https://github.com/{REPO}"),
        download_url: String::new(),
        download_bytes: None,
    };
    let extracted = Package::Snow
        .extract(&path, &stage, &package)
        .map_err(CoreError::Other)?;
    let mut files = extracted
        .into_iter()
        .map(|file| Ok((file.path.clone(), fs::read(stage.join(file.path))?)))
        .collect::<Result<Vec<_>, CoreError>>()?;
    files.push(default_custom());
    crate::packages::archive_from_files(&files)
}

pub(crate) async fn remove_legacy(
    core: &Core,
    host: &impl crate::scheme_host::SchemeHost,
) -> Result<(), CoreError> {
    let root = host.user_root().map_err(CoreError::Other)?;
    let Some(active) = crate::addon_store::snow_activation(core, &root)? else {
        return Ok(());
    };
    if active.owned.is_empty() && active.generated.is_empty() {
        return crate::addon_store::finish_snow_takeover(core, &root);
    }
    crate::shape_addons::SNOW
        .manage(core, false, host, |receipt| async move { Ok(receipt) })
        .await?;
    Ok(())
}
