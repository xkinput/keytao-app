//! Engine boundary shared by the TSF state and key callbacks.

use crate::globals::dll_related_dirs;
use keytao_core::{
    default_shared_data_dir, default_user_data_dir, ImeRuntime, ImeRuntimeSession, ImeState,
    InputContextPolicy, KeyProcessResult, ReloadStamp, ReloadStampCache,
    WINDOWS_IME_ENGINE_INIT_MUTEX_NAME,
};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{
            CloseHandle, FreeLibrary, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::{
            LibraryLoader::{
                LoadLibraryExW, LOAD_LIBRARY_FLAGS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            },
            Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        },
    },
};

const ENGINE_INIT_MUTEX_TIMEOUT_MS: u32 = 30_000;

pub(crate) fn stamp_changed(path: Option<&Path>, signature: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    // A missing stamp is not a reload request, matching ReloadStampWatcher.
    let Some(current) = ReloadStamp::signature_at(path) else {
        return false;
    };
    signature != Some(current.as_str())
}

pub struct EngineHandle {
    pub schema_id: String,
}

pub trait ImeEngineBackend: Send + Sync {
    /// Build only on the warmup/reload worker.
    fn build(&self) -> Result<EngineHandle, String>;
    fn reload_needed(&self) -> bool;
    fn status_error(&self) -> Option<&'static str> {
        None
    }
    fn drop_session(&self);
    fn process_key(&self, keysym: u32, mods: u32) -> Option<KeyProcessResult>;
    fn process_keys(&self, keys: &[(u32, u32)]) -> Option<KeyProcessResult>;
    fn process_enter(&self) -> Option<KeyProcessResult>;
    fn select_candidate_on_page(&self, index: usize) -> Option<ImeState>;
    fn change_page(&self, backward: bool) -> Option<ImeState>;
    fn reset(&self) -> Option<ImeState>;
    fn state(&self) -> ImeState;
    fn set_input_policy(&self, policy: InputContextPolicy) -> Option<ImeState>;
    fn set_english_mode(&self, enabled: bool) -> Result<ImeState, String>;
}

pub(crate) fn build_backend() -> Result<
    (
        Arc<dyn ImeEngineBackend>,
        EngineHandle,
        Option<PathBuf>,
        Option<String>,
    ),
    String,
> {
    if read_engine_mode() == crate::engine_config::EngineMode::Pipe {
        let pipe = crate::engine_pipe::PipeEngine::default();
        match pipe.build() {
            Ok(handle) => {
                return Ok((Arc::new(pipe), handle, None, None));
            }
            Err(error) => keytao_core::rt_log!(
                keytao_core::runtime_log::Level::Info,
                "ipc",
                "inproc_fallback",
                msg = error
            ),
        }
        let engine = Arc::new(InProcEngine::no_learning());
        let handle = engine.build().map_err(|error| {
            keytao_core::rt_log!(
                keytao_core::runtime_log::Level::Info,
                "ipc",
                "no_learning_fallback_failed",
                msg = error
            );
            if error.contains("需要重新部署") {
                error
            } else {
                "server unavailable".to_string()
            }
        })?;
        start_pipe_retry(&engine)?;
        let (path, signature) = engine.reload_stamp();
        return Ok((engine, handle, path, signature));
    }
    let engine = Arc::new(InProcEngine::default());
    let handle = engine.build()?;
    let (path, signature) = engine.reload_stamp();
    Ok((engine, handle, path, signature))
}

fn read_engine_mode() -> crate::engine_config::EngineMode {
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, REG_VALUE_TYPE, RRF_RT_REG_DWORD,
    };
    let mut value = [0u8; 4];
    let mut size = value.len() as u32;
    let mut kind = REG_VALUE_TYPE::default();
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::w!("Software\\KeyTao\\Ime"),
            windows::core::w!("EngineMode"),
            RRF_RT_REG_DWORD,
            Some(&mut kind),
            Some(value.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    crate::engine_config::EngineMode::from_registry(
        (result.is_ok() && size == 4).then_some((kind.0, value.as_slice())),
    )
}

#[derive(Default)]
pub struct InProcEngine {
    inner: Mutex<InProcState>,
    no_learning: bool,
    pipe_ready: AtomicBool,
}

#[derive(Default)]
struct InProcState {
    runtime: Option<ImeRuntime>,
    session: Option<ImeRuntimeSession>,
    reload_stamp_path: Option<PathBuf>,
    reload_stamp_signature: Option<String>,
    reload_stamp_cache: ReloadStampCache,
    rime_dll: Option<Arc<LoadedRimeDll>>,
}

impl InProcEngine {
    fn no_learning() -> Self {
        Self {
            no_learning: true,
            ..Self::default()
        }
    }

    /// The server always initializes the desktop directories, including for AC clients.
    pub fn build_server(&self) -> Result<EngineHandle, String> {
        self.install(Self::try_build_engine(false)?)
    }

    /// Load externally deployed artifacts without running deployment in this process.
    pub fn reload_server(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let _init_guard = EngineInitGuard::acquire(false)?;
        let signature = inner
            .reload_stamp_path
            .as_ref()
            .and_then(|path| ReloadStamp::signature_at(path));
        inner
            .runtime
            .as_ref()
            .ok_or("server runtime is not ready")?
            .reload_without_deploy()?;
        inner.reload_stamp_signature = signature;
        Ok(())
    }

    /// Release mapped dictionaries before the child invalidates their files on Windows.
    pub fn prepare_server_deploy(&self) -> Result<(), String> {
        let _inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let _init_guard = EngineInitGuard::acquire(false)?;
        let user_dir = default_user_data_dir().ok_or("cannot determine KeyTao data directory")?;
        keytao_core::reinitialize(
            user_dir.to_string_lossy().into_owned(),
            server_shared_data_dir(),
        )
    }

    /// Only the short-lived `--deploy` entry point may call this.
    pub fn deploy_server(deploy: impl FnOnce() -> i32) -> Result<i32, String> {
        let _init_guard = EngineInitGuard::acquire(false)?;
        let shared = server_shared_data_dir();
        let _rime_dll = preload_rime_dll(&shared)?;
        // The generic child must use the same bundled data as the server.
        std::env::set_var("KEYTAO_RIME_SHARED_DATA_DIR", shared);
        Ok(deploy())
    }

    /// A server owns one initialized runtime; each connection gets its own session.
    pub fn fork_session(&self) -> Result<Self, String> {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let runtime = inner.runtime.clone().ok_or("server runtime is not ready")?;
        let session = runtime.create_session()?;
        Ok(Self {
            inner: Mutex::new(InProcState {
                runtime: Some(runtime),
                session: Some(session),
                reload_stamp_path: inner.reload_stamp_path.clone(),
                reload_stamp_signature: inner.reload_stamp_signature.clone(),
                reload_stamp_cache: inner.reload_stamp_cache.clone(),
                rime_dll: inner.rime_dll.clone(),
            }),
            ..Self::default()
        })
    }

    pub(crate) fn reload_stamp(&self) -> (Option<PathBuf>, Option<String>) {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            inner.reload_stamp_path.clone(),
            inner.reload_stamp_signature.clone(),
        )
    }

    fn session(&self) -> Option<ImeRuntimeSession> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .session
            .clone()
    }

    fn install(&self, inner: InProcState) -> Result<EngineHandle, String> {
        let schema_id = inner.session.as_ref().unwrap().state().schema_id;
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = inner;
        Ok(EngineHandle { schema_id })
    }

    fn try_build_engine(sandboxed: bool) -> Result<InProcState, String> {
        let _init_guard = EngineInitGuard::acquire(sandboxed)?;
        let shared = server_shared_data_dir();
        let rime_dll = preload_rime_dll(&shared)?.map(Arc::new);
        let sandbox = if sandboxed {
            let version = keytao_core::librime_runtime_version().ok_or("需要重新部署")?;
            Some(crate::appcontainer::prepare_data(&version, &shared)?)
        } else {
            None
        };
        let user_dir = match &sandbox {
            Some(data) => data.user_dir.clone(),
            None => default_user_data_dir().ok_or("cannot determine keytao data directory")?,
        };
        let reload_stamp_path = sandbox.as_ref().map_or_else(
            || ReloadStamp::path(&user_dir),
            |data| data.reload_stamp_path.clone(),
        );
        let runtime = ImeRuntime::with_dirs(&user_dir, shared).with_ime_host();
        // Reload after copying prebuilt files into the reinitialized sandbox runtime.
        if sandbox.is_some() && runtime.is_initialized() {
            runtime.reload_without_deploy()?;
        } else {
            runtime.init_without_deploy()?;
        }
        let session = runtime.create_session()?;
        if let Some(data) = sandbox.as_ref().filter(|data| data.changed) {
            data.mark_loaded()?;
        }
        let reload_stamp_signature = sandbox.as_ref().map_or_else(
            || ReloadStamp::signature_at(&reload_stamp_path),
            |data| data.reload_stamp_signature.clone(),
        );
        Ok(InProcState {
            runtime: Some(runtime),
            session: Some(session),
            reload_stamp_path: Some(reload_stamp_path),
            reload_stamp_signature,
            reload_stamp_cache: ReloadStampCache::default(),
            rime_dll,
        })
    }

    fn try_build_no_learning(sandboxed: bool) -> Result<InProcState, String> {
        let _init_guard = EngineInitGuard::acquire_no_learning(sandboxed)?;
        let (temporary_root, deployed, path, reload_stamp_signature) =
            crate::appcontainer::fallback_directories(sandboxed)?;
        let bundled = server_shared_data_dir();
        let rime_dll = preload_rime_dll(&bundled)?.map(Arc::new);
        let version = keytao_core::librime_runtime_version().ok_or("需要重新部署")?;
        crate::appcontainer::validate_prebuilt(&deployed, &version)?;
        let generation = deployed
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid public schema generation")?;
        let user_dir = crate::appcontainer::cache_directory(
            &temporary_root.join("keytao-no-learning"),
            generation,
            &version,
        );
        let stamp = format!("{}\n{version}\n{generation}", env!("CARGO_PKG_VERSION"));
        let marker = user_dir.join(".keytao-no-learning.version");
        let stale = std::fs::read_to_string(&marker).ok().as_deref() != Some(stamp.as_str())
            || !keytao_core::schema_install_state(&user_dir).deployed
            || crate::appcontainer::validate_prebuilt(&user_dir, &version).is_err()
            || keytao_core::windows_rime_build_repair_required(&user_dir);
        let runtime = ImeRuntime::with_dirs(&user_dir, bundled);
        if stale {
            std::fs::create_dir_all(&user_dir).map_err(|error| error.to_string())?;
            copy_fallback_data(&deployed, &user_dir)?;
            keytao_core::complete_deployment(&user_dir)?;
            keytao_core::mark_windows_rime_build_repair_complete(&user_dir)?;
        }
        crate::appcontainer::prune_cache(
            &temporary_root.join("keytao-no-learning"),
            Some(&user_dir),
        );
        if let Ok(entries) = std::fs::read_dir(&temporary_root) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("keytao-no-learning-")
                    && entry
                        .file_type()
                        .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                {
                    crate::appcontainer::prune_cache(&entry.path(), None);
                }
            }
        }
        if stale && runtime.is_initialized() {
            runtime.reload_without_deploy()?;
        } else {
            runtime.init_without_deploy()?;
        }
        let session = runtime.create_session()?;
        session.set_input_policy(InputContextPolicy {
            learning: false,
            ..InputContextPolicy::default()
        });
        if stale {
            std::fs::write(marker, stamp).map_err(|error| error.to_string())?;
        }
        Ok(InProcState {
            runtime: Some(runtime),
            session: Some(session),
            reload_stamp_signature,
            reload_stamp_path: Some(path),
            reload_stamp_cache: ReloadStampCache::default(),
            rime_dll,
        })
    }
}

fn copy_fallback_data(source: &Path, target: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !crate::appcontainer::public_asset(Path::new(&entry.file_name())) {
            continue;
        }
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            return Err("symbolic link in fallback schema data".into());
        }
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            std::fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
            copy_fallback_data(&entry.path(), &destination)?;
        } else if kind.is_file() {
            let mut bytes = std::fs::read(entry.path()).map_err(|error| error.to_string())?;
            if entry
                .file_name()
                .to_string_lossy()
                .ends_with(".schema.yaml")
            {
                bytes = disable_schema_learning(&bytes)?;
            }
            if std::fs::read(&destination).ok().as_deref() != Some(bytes.as_slice()) {
                keytao_core::write_file_atomic(&destination, &bytes)
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

fn disable_schema_learning(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut schema: serde_yaml::Value =
        serde_yaml::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut namespaces = vec!["translator".to_string()];
    if let Some(translators) = schema["engine"]["translators"].as_sequence() {
        for translator in translators.iter().filter_map(|value| value.as_str()) {
            if let Some((_, namespace)) = translator.split_once('@') {
                namespaces.push(namespace.to_owned());
            }
        }
    }
    // Policy alone cannot stop librime learning in third-party translators.
    for namespace in namespaces {
        schema[namespace]["enable_user_dict"] = serde_yaml::Value::Bool(false);
    }
    serde_yaml::to_string(&schema)
        .map(String::into_bytes)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_learning_copies_prebuilt_and_disables_every_translator() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("build")).unwrap();
        let schema = "engine:\n  translators:\n    - table_translator\n    - script_translator@english\ntranslator:\n  enable_user_dict: true\nenglish:\n  enable_user_dict: true\n";
        std::fs::write(source.path().join("build/keytao.schema.yaml"), schema).unwrap();
        std::fs::write(source.path().join("build/keytao.table.bin"), b"prebuilt").unwrap();
        std::fs::create_dir_all(source.path().join("keytao.userdb")).unwrap();
        std::fs::write(source.path().join("keytao.userdb/words"), b"private").unwrap();
        copy_fallback_data(source.path(), target.path()).unwrap();
        let schema: serde_yaml::Value = serde_yaml::from_slice(
            &std::fs::read(target.path().join("build/keytao.schema.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            schema["translator"]["enable_user_dict"].as_bool(),
            Some(false)
        );
        assert_eq!(schema["english"]["enable_user_dict"].as_bool(), Some(false));
        assert_eq!(
            std::fs::read(target.path().join("build/keytao.table.bin")).unwrap(),
            b"prebuilt"
        );
        assert!(!target.path().join("keytao.userdb").exists());
        assert!(
            std::fs::read_to_string(source.path().join("build/keytao.schema.yaml"))
                .unwrap()
                .contains("enable_user_dict: true")
        );
    }
}

fn start_pipe_retry(engine: &Arc<InProcEngine>) -> Result<(), String> {
    crate::globals::pin_module().map_err(|error| error.to_string())?;
    let engine = Arc::downgrade(engine);
    std::thread::Builder::new()
        .name("keytao-pipe-retry".into())
        .spawn(move || {
            let pipe = crate::engine_pipe::PipeEngine::default();
            let mut failures = 1;
            loop {
                std::thread::sleep(crate::engine_status::retry_delay(failures));
                let Some(engine) = engine.upgrade() else {
                    return;
                };
                if pipe.build().is_ok() {
                    pipe.drop_session();
                    engine.pipe_ready.store(true, Ordering::Release);
                    return;
                }
                failures = failures.saturating_add(1);
            }
        })
        .map(|_| ())
        .map_err(|error| format!("start pipe reconnect worker: {error}"))
}

impl ImeEngineBackend for InProcEngine {
    fn build(&self) -> Result<EngineHandle, String> {
        let sandboxed = crate::appcontainer::is_appcontainer()
            .map_err(|error| format!("inspect host AppContainer token: {error}"))?;
        self.install(if self.no_learning {
            Self::try_build_no_learning(sandboxed)?
        } else {
            Self::try_build_engine(sandboxed)?
        })
    }

    fn reload_needed(&self) -> bool {
        if self.pipe_ready.load(Ordering::Acquire) {
            return true;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let InProcState {
            reload_stamp_path,
            reload_stamp_signature,
            reload_stamp_cache,
            ..
        } = &mut *inner;
        let Some(current) = reload_stamp_path
            .as_deref()
            .and_then(|path| reload_stamp_cache.signature_at(path))
        else {
            return false;
        };
        reload_stamp_signature.as_deref() != Some(current.as_str())
    }

    fn status_error(&self) -> Option<&'static str> {
        self.no_learning.then_some("server unavailable")
    }

    fn drop_session(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.session = None;
        inner.runtime = None;
        inner.rime_dll = None;
    }

    fn process_key(&self, keysym: u32, mods: u32) -> Option<KeyProcessResult> {
        self.session()?.process_key_result(keysym, mods)
    }

    fn process_keys(&self, keys: &[(u32, u32)]) -> Option<KeyProcessResult> {
        let mut result = None;
        for &(keysym, mods) in keys {
            result = self.process_key(keysym, mods);
        }
        result
    }

    fn process_enter(&self) -> Option<KeyProcessResult> {
        self.session()?.process_enter()
    }

    fn select_candidate_on_page(&self, index: usize) -> Option<ImeState> {
        self.session()?.select_candidate_on_page(index)
    }

    fn change_page(&self, backward: bool) -> Option<ImeState> {
        self.session()?.change_page(backward)
    }

    fn reset(&self) -> Option<ImeState> {
        self.session()?.reset()
    }

    fn state(&self) -> ImeState {
        self.session()
            .map_or_else(ImeState::empty, |session| session.state())
    }

    fn set_input_policy(&self, mut policy: InputContextPolicy) -> Option<ImeState> {
        if self.no_learning {
            policy.learning = false;
        }
        self.session()?.set_input_policy(policy)
    }

    fn set_english_mode(&self, enabled: bool) -> Result<ImeState, String> {
        self.session()
            .ok_or_else(|| "Rime session is unavailable".to_string())?
            .set_english_mode(enabled)
    }
}

struct LoadedRimeDll(HMODULE);

struct EngineInitGuard(HANDLE);

// SAFETY: The handle is retained only to keep the lazily loaded module alive
// while the TSF text service owns the librime runtime. It is not dereferenced.
unsafe impl Send for LoadedRimeDll {}
unsafe impl Sync for LoadedRimeDll {}

unsafe impl Send for EngineInitGuard {}

impl EngineInitGuard {
    fn acquire(appcontainer: bool) -> Result<Self, String> {
        // An unprefixed name uses the AppContainer's own object namespace;
        // the desktop Local\\ mutex is neither needed nor accessible there.
        let mutex_name = if appcontainer {
            "KeyTao.WindowsIme.EngineInit"
        } else {
            WINDOWS_IME_ENGINE_INIT_MUTEX_NAME
        };
        Self::acquire_named(mutex_name)
    }

    fn acquire_no_learning(appcontainer: bool) -> Result<Self, String> {
        Self::acquire_named(if appcontainer {
            "KeyTao.WindowsIme.NoLearning"
        } else {
            "Local\\KeyTao.WindowsIme.NoLearning"
        })
    }

    fn acquire_named(mutex_name: &str) -> Result<Self, String> {
        let mut name: Vec<u16> = mutex_name.encode_utf16().collect();
        name.push(0);
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
            .map_err(|error| format!("create engine initialization mutex: {error}"))?;
        let wait = unsafe { WaitForSingleObject(handle, ENGINE_INIT_MUTEX_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            unsafe {
                let _ = CloseHandle(handle);
            }
            if wait == WAIT_TIMEOUT {
                return Err("另一个部署正在进行，请稍后重试".into());
            }
            return Err(format!(
                "wait for engine initialization mutex: 0x{:08x}",
                wait.0
            ));
        }
        Ok(Self(handle))
    }
}

impl Drop for EngineInitGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

impl Drop for LoadedRimeDll {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

fn has_rime_base_data(dir: &Path) -> bool {
    dir.join("default.yaml").is_file()
}

fn preload_rime_dll(shared_data_dir: &str) -> Result<Option<LoadedRimeDll>, String> {
    let Some(runtime_dir) = Path::new(shared_data_dir).parent() else {
        return Ok(None);
    };
    let rime_dll = if cfg!(target_arch = "aarch64") {
        runtime_dir.join("rime-arm64.dll")
    } else {
        runtime_dir.join("rime.dll")
    };
    if !rime_dll.is_file() {
        return Ok(None);
    }

    let mut wide: Vec<u16> = rime_dll.to_string_lossy().encode_utf16().collect();
    wide.push(0);
    let flags =
        LOAD_LIBRARY_FLAGS(LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR.0 | LOAD_LIBRARY_SEARCH_SYSTEM32.0);
    let module = unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), HANDLE::default(), flags) }
        .map_err(|e| format!("load bundled rime.dll from {}: {e}", rime_dll.display()))?;
    Ok(Some(LoadedRimeDll(module)))
}

fn bundled_shared_data_dir() -> Option<String> {
    for base in dll_related_dirs() {
        for candidate in [
            base.join("rime-data"),
            base.join("resources").join("rime-data"),
            base.join("share").join("rime-data"),
        ] {
            if has_rime_base_data(&candidate) {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn server_shared_data_dir() -> String {
    bundled_shared_data_dir().unwrap_or_else(default_shared_data_dir)
}
