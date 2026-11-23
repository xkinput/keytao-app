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
        self.clear_libjd();
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
    pub(super) fn libjd_requested(&self, _: &Path) -> bool {
        false
    }

    #[cfg(not(target_os = "ios"))]
    pub(super) fn libjd_requested(&self, root: &Path) -> bool {
        if !self.0.ime_host {
            return false;
        }
        #[cfg(windows)]
        if !self.0.libjd_server {
            return false;
        }
        #[cfg(target_os = "android")]
        if !android_libjd_enabled() {
            return false;
        }
        engine_choice::preferred(root) == engine_choice::PackageEngine::LibjdEngine
            && libjd::bundled_engine_library().is_some()
    }

    #[cfg(target_os = "ios")]
    pub(super) fn activate_libjd(&self, _: &RimeDataDirs, _: bool) -> Result<bool, String> {
        Ok(false)
    }

    #[cfg(not(target_os = "ios"))]
    pub(super) fn activate_libjd(&self, dirs: &RimeDataDirs, reload: bool) -> Result<bool, String> {
        use engine_choice::PackageEngine;
        if !self.libjd_requested(&dirs.user) {
            return Ok(false);
        }
        let generation = self.generation();
        let key = engine_choice::active_package(&dirs.user);
        let package_engine = engine_choice::read(&dirs.user, key.as_deref());
        let signature = ReloadStamp::current_signature(&dirs.user);
        if !RuntimeDirs::needs_migration(&dirs.user)
            && ReloadStamp::package_key(&ReloadStamp::path(&dirs.user))
                .is_some_and(|requested| Some(requested.as_str()) != key.as_deref())
        {
            return Err(PACKAGE_SWITCH_PENDING.into());
        }
        if !reload
            && lock_ignore_poison(&PROCESS_RIME.initialized).as_ref() == Some(dirs)
            && PROCESS_RIME.using_libjd()
        {
            return Ok(true);
        }
        let imported = (package_engine == PackageEngine::LibjdEngine)
            .then(|| imported_dictionary(&dirs.user, None, key.as_deref()))
            .flatten();
        let prepared = imported
            .map(|(dictionary, schema)| {
                runtime_log::init_for_engine(&dirs.user);
                let page_size =
                    runtime_dirs::read_regular_file(&dirs.user.join(english_mode::SETTINGS_FILE))
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                        .and_then(|settings| settings["candidatePageSize"].as_u64())
                        .filter(|size| (1..=10).contains(size))
                        .unwrap_or(jd_backend::DEFAULT_PAGE_SIZE as u64)
                        as usize;
                let path =
                    libjd::bundled_engine_library().ok_or("bundled ime-jd engine is missing")?;
                unsafe {
                    jd_backend::JdBackend::prepare_dictionary(&path, &dictionary, page_size, schema)
                }
            })
            .transpose()?;
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        if self.generation() != generation {
            return Err("Rime generation changed".into());
        }
        let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
        if deployment_in_progress(&dirs.user) {
            return Err("Rime deployment is busy".into());
        }
        if ReloadStamp::current_signature(&dirs.user) != signature
            || engine_choice::active_package(&dirs.user) != key
            || !self.libjd_requested(&dirs.user)
            || engine_choice::read(&dirs.user, key.as_deref()) != package_engine
        {
            return Err(PACKAGE_SWITCH_PENDING.into());
        }
        desktop::initialized_user_dir()?;
        let previous_dirs = initialized.clone();
        let previous_rime = desktop::loaded_runtime();
        let previous_jd = lock_ignore_poison(&PROCESS_RIME.libjd)
            .as_ref()
            .map(jd_backend::JdBackend::source);
        if initialized.is_some() {
            PROCESS_RIME.shutdown_for_dir_change(&mut initialized, dirs, true)?;
        } else if previous_rime.is_some() {
            desktop::finalize_rime()?;
        }
        let Some(prepared) = prepared else {
            // Keep the host in passthrough until a converted scheme is ready.
            return Ok(true);
        };
        match prepared.start() {
            Ok(backend) => {
                *lock_ignore_poison(&PROCESS_RIME.libjd) = Some(backend);
                *initialized = Some(dirs.clone());
                Ok(true)
            }
            Err(error) => {
                let restore = if let Some(source) = previous_jd {
                    source.start().map(|backend| {
                        *lock_ignore_poison(&PROCESS_RIME.libjd) = Some(backend);
                    })
                } else if let Some(rime) = previous_rime {
                    let root = previous_dirs.as_ref().map_or(&dirs.user, |dirs| &dirs.user);
                    desktop::setup_resolved_runtime(root, &rime, DeployProcess::InProcess)
                } else {
                    return Err(error);
                };
                if let Err(restore) = restore {
                    return Err(format!("{error}; restore previous input engine: {restore}"));
                }
                *initialized = previous_dirs;
                Err(error)
            }
        }
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
    active_libjd_schema(root).is_some()
}

pub(crate) fn package_libjd_ready(root: &Path, key: Option<&str>) -> bool {
    package_libjd_ready_in_store(root, None, key)
}

pub(crate) fn active_libjd_schema(root: &Path) -> Option<SchemaInfo> {
    package_libjd_schema_in_store(root, None, engine_choice::active_package(root).as_deref())
}

pub(crate) fn package_libjd_ready_in_store(
    root: &Path,
    data_root: Option<&Path>,
    key: Option<&str>,
) -> bool {
    package_libjd_schema_in_store(root, data_root, key).is_some()
}

pub(crate) fn package_libjd_schema_in_store(
    root: &Path,
    data_root: Option<&Path>,
    key: Option<&str>,
) -> Option<SchemaInfo> {
    #[cfg(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android"
    ))]
    {
        if engine_choice::read(root, key) != engine_choice::PackageEngine::LibjdEngine {
            return None;
        }
        libjd::bundled_engine_library()?;
        imported_dictionary(root, data_root, key).map(|(_, schema)| schema)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        let _ = (root, data_root, key);
        None
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
