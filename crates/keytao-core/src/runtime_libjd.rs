use super::*;

pub(super) struct ActiveJd {
    source: jd_backend::PreparedJd,
    first_session: Option<jd_backend::JdBackend>,
}

impl ActiveJd {
    fn new(backend: jd_backend::JdBackend) -> Self {
        Self {
            source: backend.source(),
            first_session: Some(backend),
        }
    }

    fn new_session(&mut self) -> Result<jd_backend::JdBackend, String> {
        match self.first_session.take() {
            Some(backend) => Ok(backend),
            None => self.source.clone().start(),
        }
    }
}

impl ProcessRimeState {
    pub(super) fn using_libjd(&self) -> bool {
        lock_ignore_poison(&self.libjd).is_some()
    }

    pub(super) fn clear_libjd(&self) -> bool {
        lock_ignore_poison(&self.libjd).take().is_some()
    }

    pub(super) fn finalize_backend(&self) -> Result<(), String> {
        self.clear_libjd();
        desktop::finalize_rime()
    }

    pub(super) fn create_backend(&self, root: &Path) -> Result<Box<dyn ImeBackend>, String> {
        if let Some(backend) = lock_ignore_poison(&self.libjd).as_mut() {
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
        // Start the validated target before releasing the current runtime so
        // an unreadable dictionary or failed allocation leaves typing intact.
        let backend = imported
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
                    jd_backend::JdBackend::load_dictionary(&path, &dictionary, page_size, schema)
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
        if initialized.is_some() {
            PROCESS_RIME.shutdown_for_dir_change(&mut initialized, dirs, true)?;
        } else if desktop::loaded_runtime().is_some() {
            desktop::finalize_rime()?;
        }
        // Without a converted scheme the host remains in passthrough mode.
        if let Some(backend) = backend {
            *lock_ignore_poison(&PROCESS_RIME.libjd) = Some(ActiveJd::new(backend));
            *initialized = Some(dirs.clone());
        }
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
    active_libjd_schema(root).is_some()
}

pub(crate) fn package_libjd_ready(root: &Path, key: Option<&str>) -> bool {
    package_libjd_ready_in_store(root, None, key)
}

pub fn active_libjd_schema(root: &Path) -> Option<SchemaInfo> {
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
        target_os = "android",
        target_os = "ios"
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
        target_os = "android",
        target_os = "ios"
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
