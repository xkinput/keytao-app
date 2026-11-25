use crate::{Core, CoreError, package_fs, package_store::PackageStore, packages::safe_path};
use keytao_core::{RuntimeDirs, engine_choice, isolated_deploy, libjd};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct PackageCacheSizes {
    pub build_bytes: u64,
    pub ime_jd_bytes: u64,
}

pub fn cache_sizes(core: &Core, root: &Path, scheme_key: &str) -> Result<PackageCacheSizes, CoreError> {
    let Some(source) = installed_source(core, root, scheme_key)? else {
        return Ok(PackageCacheSizes::default());
    };
    let build_bytes = if RuntimeDirs::active_package_key(root).as_deref() != Some(scheme_key)
        && let Ok(Some(files)) = isolated_deploy::rime_build_cache_files(root, scheme_key)
    {
        cache_bytes(&files)?
    } else {
        0
    };
    let ime_jd_bytes = if engine_choice::can_release_libjd_dictionary(root, scheme_key) {
        cache_bytes(&dictionary_files(core, root, scheme_key, &source)?)?
    } else {
        0
    };
    Ok(PackageCacheSizes { build_bytes, ime_jd_bytes })
}

pub fn release(core: &Core, root: &Path, scheme_key: &str, ime_jd: bool) -> Result<(), CoreError> {
    if !directory_exists(root)? {
        return Err(CoreError::Other("方案运行目录无效".into()));
    }
    let _engine = crate::libjd::lock_engine_selection();
    isolated_deploy::with_isolated_package_lock(root, || {
        let source = installed_source(core, root, scheme_key)
            .map_err(|error| error.to_string())?
            .ok_or("请先完成方案安装与迁移")?;
        if !ime_jd {
            if RuntimeDirs::active_package_key(root).as_deref() == Some(scheme_key) {
                return Err("请先切换到其他方案".into());
            }
            return isolated_deploy::release_rime_build_cache(root, scheme_key);
        }
        if !engine_choice::can_release_libjd_dictionary(root, scheme_key) {
            return Err("当前引擎或此方案仍选用了 ime-jd，无法释放词库".into());
        }
        let files = dictionary_files(core, root, scheme_key, &source)
            .map_err(|error| error.to_string())?;
        for file in files {
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(package_fs::context("删除缓存", &file, error).to_string()),
            }
        }
        Ok(())
    })
    .map_err(CoreError::Other)
}

fn directory_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn installed_source(core: &Core, root: &Path, key: &str) -> Result<Option<PathBuf>, CoreError> {
    crate::scheme::validate_scheme_key(key).map_err(CoreError::Other)?;
    if !directory_exists(root)?
        || !directory_exists(&core.env().data_dir)?
        || RuntimeDirs::needs_migration(root)
        || !core.lock_state().installed_packages.contains_key(key)
    {
        return Ok(None);
    }
    let source = PackageStore::new(core).checked_package_path(key)?;
    let files = safe_path(&source, "files").map_err(CoreError::Other)?;
    Ok(directory_exists(&files)?.then_some(source))
}

fn dictionary_files(
    core: &Core,
    _root: &Path,
    key: &str,
    source: &Path,
) -> Result<Vec<PathBuf>, CoreError> {
    let Some(stable) = libjd::dictionary_directory(&core.env().data_dir, key) else {
        return Ok(Vec::new());
    };
    let mut dictionaries = BTreeSet::from([stable, source.join("libjd-dictionary")]);
    #[cfg(target_os = "ios")]
    if let Some(projected) = libjd::projected_dictionary_directory(_root, key) {
        dictionaries.insert(projected);
    }
    dictionaries.extend(dictionaries.iter().map(|path| path.with_file_name(".old-libjd-dictionary")).collect::<Vec<_>>());
    let mut files = Vec::new();
    for directory in dictionaries {
        if !directory_exists(&directory)? {
            continue;
        }
        for name in ["dictionary.bin", "schema.json"] {
            let path = directory.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => files.push(path),
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(files)
}

fn cache_bytes(files: &[PathBuf]) -> io::Result<u64> {
    #[cfg(unix)]
    let mut linked = std::collections::HashMap::new();
    let mut bytes = 0u64;
    for file in files {
        let metadata = match fs::symlink_metadata(file) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() > 1 {
                let selected = linked.entry((metadata.dev(), metadata.ino())).or_insert(0);
                *selected += 1;
                if *selected != metadata.nlink() {
                    continue;
                }
            }
        }
        bytes = bytes.saturating_add(metadata.len());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme_host::tests::AddonHost;

    fn install_dictionary(core: &Core, key: &str) {
        let store = PackageStore::new(core);
        fs::create_dir_all(store.package_path(key).join("files")).unwrap();
        core.lock_state().installed_packages.insert(
            key.into(),
            crate::state::InstalledPackage { version: None },
        );
        let dictionary = libjd::dictionary_directory(&core.env().data_dir, key).unwrap();
        fs::create_dir_all(&dictionary).unwrap();
        fs::write(dictionary.join("dictionary.bin"), b"converted").unwrap();
        fs::write(dictionary.join("schema.json"), br#"{"id":"xmjd","name":"Fixture"}"#).unwrap();
    }

    #[test]
    fn corrupt_settings_prevent_dictionary_release() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let host = AddonHost::new(root);
        let core = &host.core;
        let key = "xmjd";
        install_dictionary(core, key);
        assert!(cache_sizes(core, root, key).unwrap().ime_jd_bytes > 0);

        fs::write(root.join(keytao_core::english_mode::SETTINGS_FILE), b"{").unwrap();
        assert_eq!(cache_sizes(core, root, key).unwrap().ime_jd_bytes, 0);
        assert!(release(core, root, key, true).is_err());
        assert!(libjd::imported_dictionary(&core.env().data_dir, key).is_some());
    }

    #[test]
    fn inactive_converted_package_keeps_its_dictionary_after_upgrade() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let host = AddonHost::new(root);
        let core = &host.core;
        let key = "xmjd";
        install_dictionary(core, key);
        let manifest = keytao_core::package_store::active_manifest(root);
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(manifest, br#"{"active_package":"keytao"}"#).unwrap();

        assert!(cache_sizes(core, root, key).unwrap().ime_jd_bytes > 0);

        let settings = root.join(keytao_core::english_mode::SETTINGS_FILE);
        for choice in [
            br#"{"engines":{"xmjd":"libjd_engine"}}"#.as_slice(),
            br#"{"engine":"rime","engines":{"xmjd":"libjd_engine"}}"#,
        ] {
            fs::write(&settings, choice).unwrap();
            assert_eq!(RuntimeDirs::active_package_key(root).as_deref(), Some("keytao"));
            assert!(engine_choice::preferred(root).is_rime());
            assert_eq!(cache_sizes(core, root, key).unwrap().ime_jd_bytes, 0);
            assert!(release(core, root, key, true).is_err());
            assert!(libjd::imported_dictionary(&core.env().data_dir, key).is_some());
            assert_eq!(fs::read(&settings).unwrap(), choice);
        }
    }
}
