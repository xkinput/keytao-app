//! Engine boundary shared by the TSF state and key callbacks.

use keytao_core::{
    default_shared_data_dir, default_user_data_dir, ImeRuntime, ImeRuntimeSession, ImeState,
    InputContextPolicy, KeyProcessResult, ReloadStamp, WINDOWS_IME_ENGINE_INIT_MUTEX_NAME,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, FreeLibrary, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0},
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

pub struct EngineHandle {
    pub schema_id: String,
}

pub trait ImeEngineBackend: Send + Sync {
    /// Build only on the warmup/reload worker.
    fn build(&self) -> Result<EngineHandle, String>;
    fn reload_needed(&self) -> bool;
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

#[derive(Default)]
pub struct InProcEngine {
    inner: Mutex<InProcState>,
}

#[derive(Default)]
struct InProcState {
    runtime: Option<ImeRuntime>,
    session: Option<ImeRuntimeSession>,
    reload_stamp_path: Option<PathBuf>,
    reload_stamp_signature: Option<String>,
    rime_dll: Option<LoadedRimeDll>,
}

impl InProcEngine {
    /// Retain stamp checks when a failed reload has already dropped its engine.
    pub(crate) fn with_reload_stamp(path: Option<PathBuf>, signature: Option<String>) -> Self {
        Self {
            inner: Mutex::new(InProcState {
                reload_stamp_path: path,
                reload_stamp_signature: signature,
                ..InProcState::default()
            }),
        }
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

    fn try_build_engine() -> Result<InProcState, String> {
        let sandboxed = crate::appcontainer::is_appcontainer()
            .map_err(|error| format!("inspect host AppContainer token: {error}"))?;
        let _init_guard = EngineInitGuard::acquire(sandboxed)?;
        let sandbox = if sandboxed {
            Some(crate::appcontainer::prepare_data()?)
        } else {
            None
        };
        let user_dir = match &sandbox {
            Some(data) => data.user_dir.clone(),
            None => default_user_data_dir().ok_or("cannot determine keytao data directory")?,
        };
        let shared = bundled_shared_data_dir().unwrap_or_else(default_shared_data_dir);
        let rime_dll = preload_rime_dll(&shared)?;
        let reload_stamp_path = sandbox.as_ref().map_or_else(
            || ReloadStamp::path(&user_dir),
            |data| data.reload_stamp_path.clone(),
        );
        let needs_deploy = sandbox.as_ref().is_some_and(|data| {
            data.changed
                || !keytao_core::schema_install_state(&user_dir).deployed
                || keytao_core::windows_rime_build_repair_required(&user_dir)
        });
        let runtime = ImeRuntime::with_dirs(&user_dir, shared).with_ime_host();
        if needs_deploy {
            // This function only runs on the warmup/reload worker, never the
            // host's input thread. Public package updates preserve private userdb.
            runtime.reload()?;
            sandbox.as_ref().unwrap().mark_deployed()?;
        } else {
            runtime.init_without_deploy()?;
        }
        let session = runtime.create_session()?;
        let reload_stamp_signature = sandbox.as_ref().map_or_else(
            || ReloadStamp::signature_at(&reload_stamp_path),
            |data| data.reload_stamp_signature.clone(),
        );
        Ok(InProcState {
            runtime: Some(runtime),
            session: Some(session),
            reload_stamp_path: Some(reload_stamp_path),
            reload_stamp_signature,
            rime_dll,
        })
    }
}

impl ImeEngineBackend for InProcEngine {
    fn build(&self) -> Result<EngineHandle, String> {
        let inner = Self::try_build_engine()?;
        let schema_id = inner.session.as_ref().unwrap().state().schema_id;
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = inner;
        Ok(EngineHandle { schema_id })
    }

    fn reload_needed(&self) -> bool {
        let (path, signature) = self.reload_stamp();
        let Some(path) = path else {
            return false;
        };
        // A missing stamp is not a reload request, matching ReloadStampWatcher.
        let Some(current) = ReloadStamp::signature_at(&path) else {
            return false;
        };
        signature.as_deref() != Some(current.as_str())
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

    fn set_input_policy(&self, policy: InputContextPolicy) -> Option<ImeState> {
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
        let mut name: Vec<u16> = mutex_name.encode_utf16().collect();
        name.push(0);
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
            .map_err(|error| format!("create engine initialization mutex: {error}"))?;
        let wait = unsafe { WaitForSingleObject(handle, ENGINE_INIT_MUTEX_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            unsafe {
                let _ = CloseHandle(handle);
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

fn dll_related_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(hmodule) = crate::globals::DLL_INSTANCE.get().copied() {
        let mut buf = vec![0u16; 32768];
        let len = unsafe {
            windows::Win32::System::LibraryLoader::GetModuleFileNameW(
                windows::Win32::Foundation::HMODULE(hmodule as _),
                &mut buf,
            )
        } as usize;
        if len > 0 {
            if let Some(parent) = PathBuf::from(String::from_utf16_lossy(&buf[..len])).parent() {
                dirs.push(parent.to_path_buf());
            }
        }
    }

    if let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        dirs.push(parent);
    }

    dirs
}
