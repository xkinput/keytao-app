use super::*;

impl ProcessRimeState {
    pub(super) fn using_libjd(&self) -> bool {
        #[cfg(not(target_os = "ios"))]
        return lock_ignore_poison(&self.libjd).is_some();
        #[cfg(target_os = "ios")]
        false
    }

    pub(super) fn clear_libjd(&self) -> bool {
        #[cfg(not(target_os = "ios"))]
        return lock_ignore_poison(&self.libjd).take().is_some();
        #[cfg(target_os = "ios")]
        false
    }

    pub(super) fn finalize_backend(&self) -> Result<(), String> {
        if self.clear_libjd() {
            return Ok(());
        }
        desktop::finalize_rime()
    }

    pub(super) fn create_backend(&self, root: &Path) -> Result<Box<dyn ImeBackend>, String> {
        #[cfg(not(target_os = "ios"))]
        if let Some(backend) = lock_ignore_poison(&self.libjd).as_ref() {
            return Ok(Box::new(backend.new_session()?));
        }
        Ok(Box::new(RimeBackend::new_with_user_data_dir(root)?))
    }

    pub(super) fn schema_root(&self, root: &Path) -> PathBuf {
        if self.using_libjd() {
            root.to_path_buf()
        } else {
            desktop::loaded_schema_root(root)
        }
    }
}

impl ImeRuntime {
    #[cfg(target_os = "ios")]
    pub(super) fn activate_libjd(&self, _: &RimeDataDirs, _: bool) -> Result<bool, String> {
        Ok(false)
    }

    #[cfg(not(target_os = "ios"))]
    pub(super) fn activate_libjd(&self, dirs: &RimeDataDirs, reload: bool) -> Result<bool, String> {
        use engine_choice::PackageEngine;
        if !self.0.ime_host || libjd::platform().is_none() {
            return Ok(false);
        }
        #[cfg(windows)]
        if !self.0.libjd_server {
            return Ok(false);
        }
        #[cfg(target_os = "android")]
        if !android_libjd_enabled() {
            return Ok(false);
        }
        let key = engine_choice::active_package(&dirs.user);
        let engine = engine_choice::read(&dirs.user, key.as_deref());
        if engine.is_rime() {
            return Ok(false);
        }
        if !RuntimeDirs::needs_migration(&dirs.user)
            && ReloadStamp::package_key(&ReloadStamp::path(&dirs.user))
                .is_some_and(|requested| Some(requested.as_str()) != key.as_deref())
        {
            return Err(PACKAGE_SWITCH_PENDING.into());
        }
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        if !reload && initialized.as_ref() == Some(dirs) && PROCESS_RIME.using_libjd() {
            return Ok(true);
        }
        runtime_log::init_for_engine(&dirs.user);
        let library_path = installed_library(&dirs.user, engine);
        let page_size =
            runtime_dirs::read_regular_file(&dirs.user.join(english_mode::SETTINGS_FILE))
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|settings| settings["candidatePageSize"].as_u64())
                .filter(|size| (1..=10).contains(size))
                .unwrap_or(5) as usize;
        let backend = library_path
            .ok_or_else(|| "verified libjd installation missing".to_string())
            // The installation resolver requires the digest-verified publication manifest.
            .and_then(|path| unsafe {
                if engine == PackageEngine::LibjdEngine {
                    let (dictionary, schema) =
                        imported_dictionary(&dirs.user, None, key.as_deref())
                            .ok_or("converted ime-jd dictionary is missing")?;
                    jd_backend::JdBackend::load_dictionary(&path, &dictionary, page_size, schema)
                } else {
                    jd_backend::JdBackend::load_with_page_size(&path, page_size)
                }
            });
        let backend = match backend {
            Ok(backend) => backend,
            Err(error) => {
                crate::rt_log!(Level::Info, "error", "libjd_fallback", msg = &error);
                eprintln!("KeyTao: libjd could not load, falling back to Rime: {error}");
                return Ok(false);
            }
        };

        let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
        PROCESS_RIME.shutdown_for_dir_change(&mut initialized, dirs, true)?;
        *lock_ignore_poison(&PROCESS_RIME.libjd) = Some(backend);
        *initialized = Some(dirs.clone());
        Ok(true)
    }
}

#[cfg(target_os = "android")]
static ANDROID_DATA_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[cfg(target_os = "android")]
pub fn set_android_libjd_data_root(data: PathBuf) {
    let _ = ANDROID_DATA_ROOT.set(data);
}

#[cfg(target_os = "android")]
pub fn android_libjd_enabled() -> bool {
    ANDROID_DATA_ROOT.get().is_some()
}

#[cfg(target_os = "android")]
pub fn android_libjd_ready(root: &Path) -> bool {
    package_libjd_ready(root, engine_choice::active_package(root).as_deref())
}

pub(crate) fn package_libjd_ready(root: &Path, key: Option<&str>) -> bool {
    package_libjd_ready_in_store(root, None, key)
}

pub(crate) fn package_libjd_ready_in_store(
    root: &Path,
    data_root: Option<&Path>,
    key: Option<&str>,
) -> bool {
    #[cfg(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android"
    ))]
    {
        use engine_choice::PackageEngine;
        let engine = engine_choice::read(root, key);
        let Some(platform) = engine_platform(engine) else {
            return false;
        };
        let ready = |data: &Path| {
            platform.installed_library(data).is_some()
                && (engine != PackageEngine::LibjdEngine
                    || key
                        .and_then(|key| libjd::imported_dictionary(data, key))
                        .is_some())
        };
        return data_root.is_some_and(ready)
            || runtime_data_root(root).as_deref().is_some_and(ready);
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        let _ = (root, data_root, key);
        false
    }
}

pub(crate) fn imported_dictionary(
    root: &Path,
    data: Option<&Path>,
    key: Option<&str>,
) -> Option<(PathBuf, SchemaInfo)> {
    let key = key?;
    data.and_then(|data| libjd::imported_dictionary(data, key))
        .or_else(|| libjd::imported_dictionary(&runtime_data_root(root)?, key))
}

fn runtime_data_root(root: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    {
        let _ = root;
        ANDROID_DATA_ROOT.get().cloned()
    }
    #[cfg(not(target_os = "android"))]
    Some(RuntimeDirs::app_data_root(root).unwrap_or_else(|| root.to_path_buf()))
}

#[cfg(not(target_os = "ios"))]
fn engine_platform(engine: engine_choice::PackageEngine) -> Option<libjd::LibjdPlatform> {
    use engine_choice::PackageEngine;
    match engine {
        PackageEngine::Rime => None,
        PackageEngine::Libjd => libjd::platform(),
        PackageEngine::LibjdEngine => libjd::platform().map(libjd::LibjdPlatform::engine_only),
    }
}

#[cfg(not(target_os = "ios"))]
fn installed_library(root: &Path, engine: engine_choice::PackageEngine) -> Option<PathBuf> {
    engine_platform(engine)?.installed_library(&runtime_data_root(root)?)
}
