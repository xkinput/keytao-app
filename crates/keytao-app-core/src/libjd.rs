use crate::{Core, CoreError, download::emit_progress};
use keytao_core::{
    RuntimeDirs,
    engine_choice::{self, PackageEngine},
    libjd::{self, BUNDLED_ENGINE_VERSION},
};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct LibjdStatus {
    pub available: bool,
    pub installed_version: Option<String>,
}

fn bundled_library() -> Option<PathBuf> {
    #[cfg(windows)]
    return crate::ime_status::windows::bundled_libjd_library();
    #[cfg(not(windows))]
    libjd::bundled_engine_library()
}

pub fn libjd_status() -> LibjdStatus {
    let available = bundled_library().is_some();
    LibjdStatus {
        available,
        installed_version: (available || libjd::bundled_engine_library().is_some())
            .then(|| BUNDLED_ENGINE_VERSION.into()),
    }
}

pub fn input_engine(root: &Path) -> PackageEngine {
    if bundled_library().is_some() {
        engine_choice::preferred(root)
    } else {
        PackageEngine::Rime
    }
}

pub fn imported_package_ready(core: &Core, key: &str) -> bool {
    bundled_library().is_some() && libjd::imported_dictionary(&core.env().data_dir, key).is_some()
}

fn engine_settings(root: &Path) -> Result<(PathBuf, Option<Vec<u8>>), CoreError> {
    let path = crate::packages::safe_path(root, keytao_core::english_mode::SETTINGS_FILE)
        .map_err(CoreError::Other)?;
    let previous = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok((path, previous))
}

fn update_settings(
    root: &Path,
    update: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), CoreError>,
    activate: impl FnOnce() -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let (path, previous) = engine_settings(root)?;
    let mut settings = previous
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?
        .unwrap_or_default();
    update(&mut settings)?;
    crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec_pretty(&settings)?)?;
    if let Err(error) = activate() {
        let restore = match previous {
            Some(bytes) => crate::scheme_files::write_file_atomic(&path, &bytes),
            None => fs::remove_file(&path),
        };
        if let Err(restore) = restore {
            return Err(CoreError::Other(format!(
                "{error}; restore engine settings: {restore}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn activate_imported(core: &Core, root: &Path, key: &str) -> Result<(), CoreError> {
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    keytao_core::isolated_deploy::switch_ready_package_in_store(
        root,
        &shared,
        Some(&core.env().data_dir),
        key,
        None,
    )
    .map_err(CoreError::Other)
}

pub fn set_input_engine(core: &Core, root: &Path, engine: &str) -> Result<(), CoreError> {
    let converted = match engine {
        "rime" => Vec::new(),
        "libjd_engine" => {
            if bundled_library().is_none() {
                return Err(CoreError::Unsupported(
                    "当前安装包不包含 ime-jd 引擎".into(),
                ));
            }
            crate::runtime_materialize::isolated_packages(core, root)?
                .into_iter()
                .map(|package| package.package_key)
                .filter(|key| imported_package_ready(core, key))
                .collect()
        }
        _ => return Err(CoreError::Other("未知输入引擎".into())),
    };
    let active = RuntimeDirs::active_package_key(root);
    let target = converted
        .first()
        .filter(|_| !active.as_ref().is_some_and(|key| converted.contains(key)));
    update_settings(
        root,
        |settings| {
            settings.insert("engine".into(), engine.into());
            if !converted.is_empty() {
                let engines = converted_engines(settings)?;
                for key in &converted {
                    engines.insert(key.clone(), PackageEngine::LibjdEngine.as_str().into());
                }
            }
            Ok(())
        },
        || {
            if let Some(key) = target {
                activate_imported(core, root, key)?;
            } else {
                keytao_core::ReloadStamp::write(root).map_err(CoreError::Other)?;
            }
            Ok(())
        },
    )
}

fn converted_engines(
    settings: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<&mut serde_json::Map<String, serde_json::Value>, CoreError> {
    settings
        .entry("engines")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| CoreError::Other("输入引擎设置格式无效".into()))
}

pub async fn import_package(
    core: &Arc<Core>,
    root: &Path,
    key: &str,
    schema_id: Option<&str>,
) -> Result<usize, CoreError> {
    if engine_choice::preferred(root) != PackageEngine::LibjdEngine {
        return Err(CoreError::Other("请先在首页选择 ime-jd 引擎".into()));
    }
    let library = bundled_library()
        .ok_or_else(|| CoreError::Unsupported("当前安装包不包含 ime-jd 引擎".into()))?;
    let data = &core.env().data_dir;
    let destination = libjd::dictionary_directory(data, key)
        .ok_or_else(|| CoreError::Other("请先下载或导入此 Rime 方案".into()))?;
    let core = Arc::clone(core);
    let root = root.to_path_buf();
    let key = key.to_owned();
    let schema_id = schema_id.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        emit_progress(&core, "converting", 0, "正在准备转换词库…");
        let converted = crate::libjd_dictionary::convert(
            &core,
            &root,
            &key,
            schema_id.as_deref(),
            |index, total| {
                let percent = (index.saturating_sub(1) * 90 / total.max(1)) as u32;
                emit_progress(
                    &core,
                    "converting",
                    percent,
                    &format!("正在转换词库 {index}/{total}"),
                );
            },
        )?;
        let parent = destination.parent().unwrap();
        fs::create_dir_all(parent)?;
        let stage = tempfile::Builder::new()
            .prefix(".libjd-")
            .tempdir_in(parent)?;
        emit_progress(&core, "converting", 92, "正在编译并校验 ime-jd 词库…");
        build_dictionary(&library, &converted.text, stage.path())?;
        let schema = keytao_core::SchemaInfo {
            id: converted.schema_id,
            name: converted.schema_name,
        };
        crate::scheme_files::write_file_atomic(
            &stage.path().join("schema.json"),
            &serde_json::to_vec(&schema)?,
        )?;
        publish_dictionary(&destination, stage.path(), || {
            emit_progress(&core, "converting", 98, "正在切换方案…");
            update_settings(
                &root,
                |settings| {
                    converted_engines(settings)?
                        .insert(key.clone(), PackageEngine::LibjdEngine.as_str().into());
                    Ok(())
                },
                || activate_imported(&core, &root, &key),
            )
        })?;
        emit_progress(&core, "done", 100, "已转换词库");
        Ok(converted.skipped)
    })
    .await
    .map_err(|error| CoreError::Other(error.to_string()))?
}

fn publish_dictionary(
    destination: &Path,
    stage: &Path,
    activate: impl FnOnce() -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let old = destination.with_file_name(".old-libjd-dictionary");
    match fs::symlink_metadata(&old) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(CoreError::Other("ime-jd 旧词库目录无效".into()));
        }
        Ok(_) if destination.try_exists()? => crate::package_fs::remove_dir_all(&old)?,
        Ok(_) => crate::package_fs::rename(&old, destination)?,
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    crate::package_store::publish_directory_then(
        destination,
        stage,
        crate::package_fs::rename,
        activate,
    )
}

pub(crate) fn preserve_imported_dictionary(data: &Path, key: &str) -> Result<(), CoreError> {
    let Some(destination) = libjd::dictionary_directory(data, key) else {
        return Ok(());
    };
    let Some((dictionary, schema)) = libjd::imported_dictionary(data, key) else {
        return Ok(());
    };
    let parent = destination.parent().unwrap();
    if dictionary.parent().and_then(Path::parent) == Some(parent) {
        return Ok(());
    }
    fs::create_dir_all(parent)?;
    let stage = tempfile::tempdir_in(parent)?;
    fs::copy(dictionary, stage.path().join("dictionary.bin"))?;
    crate::scheme_files::write_file_atomic(
        &stage.path().join("schema.json"),
        &serde_json::to_vec(&schema)?,
    )?;
    publish_dictionary(&destination, stage.path(), || Ok(()))
}

#[cfg(windows)]
fn build_dictionary(_: &Path, text: &[u8], stage: &Path) -> Result<(), CoreError> {
    let input = stage.join("dictionary.txt");
    fs::write(&input, text)?;
    crate::ime_status::windows::build_libjd_dictionary(&input).map_err(CoreError::Other)?;
    fs::remove_file(input)?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
fn build_dictionary(library: &Path, text: &[u8], stage: &Path) -> Result<(), CoreError> {
    // The library is a verified build-time dependency located in the application bundle.
    unsafe {
        keytao_core::jd_backend::JdBackend::build_dictionary(
            library,
            text,
            &stage.join("dictionary.bin"),
        )
    }
    .map_err(CoreError::Other)
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "linux",
    target_os = "android"
)))]
fn build_dictionary(_: &Path, _: &[u8], _: &Path) -> Result<(), CoreError> {
    Err(CoreError::Unsupported("当前平台不支持 ime-jd".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires the bundled engine and XMJD_TEST_ARCHIVE"]
    async fn bundled_engine_imports_an_undeployed_scheme_and_switches_back() {
        use crate::{AppEnv, CoreEvent, EventSink, Platform};

        struct Sink;
        impl EventSink for Sink {
            fn emit(&self, _: CoreEvent) {}
        }

        let archive = fs::read(std::env::var_os("XMJD_TEST_ARCHIVE").unwrap()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let core = Core::new(
            AppEnv {
                data_dir: root.into(),
                cache_dir: root.join("cache"),
                resource_dir: root.join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            Arc::new(Sink),
        )
        .unwrap();
        crate::package_migration::ensure_before_write(&core, root)
            .await
            .unwrap();
        let previous = RuntimeDirs::active_package_key(root);
        set_input_engine(&core, root, "libjd_engine").unwrap();
        assert_eq!(RuntimeDirs::active_package_key(root), previous);

        crate::packages::store_cloud_archive(&core, "xmjd", None, "official", None, &archive, root)
            .unwrap();
        import_package(&core, root, "xmjd", Some("xmjd6"))
            .await
            .unwrap();
        assert_eq!(
            RuntimeDirs::active_package_key(root).as_deref(),
            Some("xmjd")
        );
        assert_eq!(
            keytao_core::current_schema_id_from_dir(root).as_deref(),
            Some("xmjd6")
        );

        let runtime =
            keytao_core::ImeRuntime::with_dirs(root, root.to_string_lossy()).with_ime_host();
        runtime.init_without_deploy().unwrap();
        let session = runtime.create_session().unwrap();
        session.process_key_result(b'a'.into(), 0).unwrap();
        assert!(
            !session
                .process_key_result(b'a'.into(), 0)
                .unwrap()
                .state
                .candidates
                .is_empty()
        );

        set_input_engine(&core, root, "rime").unwrap();
        runtime.reload_without_deploy().unwrap();
        assert!(!keytao_core::schema_install_state(root).deployed);
        assert!(session.process_key_result(b'a'.into(), 0).is_none());
        assert!(imported_package_ready(&core, "xmjd"));

        set_input_engine(&core, root, "libjd_engine").unwrap();
        runtime.reload_without_deploy().unwrap();
        assert_eq!(session.state().schema_id, "xmjd6");
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn failed_activation_restores_the_existing_dictionary() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("libjd-dictionary");
        let stage = root.path().join("stage");
        fs::create_dir(&destination).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(destination.join("dictionary.bin"), b"previous").unwrap();
        fs::write(stage.join("dictionary.bin"), b"replacement").unwrap();
        let result = publish_dictionary(&destination, &stage, || {
            assert_eq!(
                fs::read(destination.join("dictionary.bin")).unwrap(),
                b"replacement"
            );
            Err(CoreError::Other("activation failed".into()))
        });
        assert_eq!(result.unwrap_err().to_string(), "activation failed");
        assert_eq!(
            fs::read(destination.join("dictionary.bin")).unwrap(),
            b"previous"
        );
    }

    #[test]
    fn preference_preserves_per_package_conversions_and_rolls_back_failed_switch() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (path, _) = engine_settings(root).unwrap();
        let original =
            br#"{"engine":"rime","engines":{"keytao":"libjd_engine"},"candidatePageSize":7}"#;
        fs::write(&path, original).unwrap();
        let select_jd = |settings: &mut serde_json::Map<String, serde_json::Value>| {
            settings.insert("engine".into(), "libjd_engine".into());
            Ok(())
        };
        assert!(
            update_settings(root, select_jd, || Err(CoreError::Other(
                "test failure".into()
            )))
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), original);
        update_settings(root, select_jd, || Ok(())).unwrap();
        assert_eq!(engine_choice::preferred(root), PackageEngine::LibjdEngine);
        update_settings(
            root,
            |settings| {
                settings.insert("engine".into(), "rime".into());
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
        assert_eq!(engine_choice::preferred(root), PackageEngine::Rime);
        assert_eq!(
            engine_choice::configured(root, Some("keytao")),
            PackageEngine::LibjdEngine
        );
        assert_eq!(crate::ime_settings::read_candidate_page_size(root), Some(7));
    }
}
