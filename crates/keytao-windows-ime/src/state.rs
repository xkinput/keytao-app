//! Shared state between all TSF COM objects.

use keytao_core::{
    default_shared_data_dir, default_user_data_dir, ImeRuntime, ImeRuntimeSession, ImeState,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{FreeLibrary, HANDLE, HMODULE},
        System::LibraryLoader::{
            LoadLibraryExW, LOAD_LIBRARY_FLAGS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        },
        UI::TextServices::{ITfComposition, ITfKeyEventSink, ITfThreadMgr},
    },
};

const RELOAD_STAMP_FILE: &str = "keytao-ime.reload";

// ── Shared TsfState ───────────────────────────────────────────────────────────

pub struct TsfState {
    pub runtime: Option<ImeRuntime>,
    pub session: Option<ImeRuntimeSession>,
    rime_dll: Option<LoadedRimeDll>,
    pub engine_building: bool,
    pub engine_error: Option<String>,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub client_id: u32,
    pub key_sink: Option<ITfKeyEventSink>,
    pub composition: Option<ITfComposition>,
    pub ime_state: Option<ImeState>,
    pub candidate_win: crate::candidate_win::CandidateWindow,
    pub mode_hint_win: crate::candidate_win::CandidateWindow,
    pub ascii_mode: bool,
    pub reload_stamp_path: Option<PathBuf>,
    pub reload_stamp_signature: Option<String>,
    pub reload_in_progress: bool,
    pub reload_clear_pending: bool,
    pub shift_pressed_without_key: bool,
}

pub(crate) struct EngineBundle {
    runtime: ImeRuntime,
    session: ImeRuntimeSession,
    reload_stamp_path: PathBuf,
    reload_stamp_signature: String,
    rime_dll: Option<LoadedRimeDll>,
}

struct LoadedRimeDll(HMODULE);

// SAFETY: The handle is retained only to keep the lazily loaded module alive
// while the TSF text service owns the librime runtime. It is not dereferenced.
unsafe impl Send for LoadedRimeDll {}
unsafe impl Sync for LoadedRimeDll {}

impl Drop for LoadedRimeDll {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

// SAFETY: TSF TIPs run in COM STA; all calls are on the same thread.
unsafe impl Send for TsfState {}
unsafe impl Sync for TsfState {}

impl TsfState {
    pub fn new() -> Self {
        Self {
            runtime: None,
            session: None,
            rime_dll: None,
            engine_building: false,
            engine_error: None,
            thread_mgr: None,
            client_id: 0,
            key_sink: None,
            composition: None,
            ime_state: None,
            candidate_win: crate::candidate_win::CandidateWindow::new(),
            mode_hint_win: crate::candidate_win::CandidateWindow::new(),
            ascii_mode: false,
            reload_stamp_path: None,
            reload_stamp_signature: None,
            reload_in_progress: false,
            reload_clear_pending: false,
            shift_pressed_without_key: false,
        }
    }

    pub(crate) fn build_engine() -> Result<EngineBundle, String> {
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let shared = bundled_shared_data_dir().unwrap_or_else(default_shared_data_dir);
        let rime_dll = preload_rime_dll(&shared)?;
        let reload_stamp_path = user_dir.join(RELOAD_STAMP_FILE);
        let runtime = ImeRuntime::with_dirs(user_dir, shared);
        runtime.init()?;
        let session = runtime.create_session()?;
        let reload_stamp_signature = reload_stamp_signature(&reload_stamp_path);
        Ok(EngineBundle {
            runtime,
            session,
            reload_stamp_path,
            reload_stamp_signature,
            rime_dll,
        })
    }

    pub(crate) fn install_engine(&mut self, bundle: EngineBundle) {
        self.runtime = Some(bundle.runtime);
        self.session = Some(bundle.session);
        self.reload_stamp_signature = Some(bundle.reload_stamp_signature);
        self.reload_stamp_path = Some(bundle.reload_stamp_path);
        self.rime_dll = bundle.rime_dll;
        self.engine_building = false;
        self.engine_error = None;
    }

    pub(crate) fn engine_ready(&self) -> bool {
        self.session.is_some()
    }

    pub(crate) fn begin_engine_build(&mut self) -> bool {
        if self.engine_ready() || self.engine_building {
            return false;
        }
        self.engine_building = true;
        self.engine_error = None;
        true
    }

    pub(crate) fn finish_engine_build_error(&mut self, error: String) {
        self.engine_building = false;
        self.engine_error = Some(error);
    }

    pub(crate) fn begin_reload_if_changed(&mut self) -> Option<ImeRuntime> {
        if self.reload_in_progress {
            return None;
        }
        let Some(path) = &self.reload_stamp_path else {
            return None;
        };
        let signature = reload_stamp_signature(path);
        if self.reload_stamp_signature.as_deref() == Some(signature.as_str()) {
            return None;
        }
        let runtime = self.runtime.clone()?;
        self.reload_stamp_signature = Some(signature);
        self.reload_in_progress = true;
        Some(runtime)
    }

    pub(crate) fn finish_reload(&mut self, ok: bool) {
        self.reload_in_progress = false;
        if ok {
            self.reload_clear_pending = true;
        }
    }

    pub(crate) fn take_reload_clear_pending(&mut self) -> bool {
        std::mem::take(&mut self.reload_clear_pending)
    }

    pub(crate) fn session(&self) -> Option<ImeRuntimeSession> {
        self.session.clone()
    }
}

pub type SharedState = Arc<Mutex<TsfState>>;

pub fn new_shared_state() -> SharedState {
    Arc::new(Mutex::new(TsfState::new()))
}

pub(crate) fn start_engine_warmup(shared_state: &SharedState) {
    let should_start = {
        let mut st = shared_state.lock().unwrap();
        st.begin_engine_build()
    };
    if !should_start {
        return;
    }

    let state = Arc::clone(shared_state);
    std::thread::spawn(move || match TsfState::build_engine() {
        Ok(bundle) => {
            let mut st = state.lock().unwrap();
            if !st.engine_ready() {
                st.install_engine(bundle);
            } else {
                st.engine_building = false;
            }
            tracing::info!("KeyTao Windows IME engine warmed up");
        }
        Err(error) => {
            tracing::error!("librime init failed: {error}");
            state.lock().unwrap().finish_engine_build_error(error);
        }
    });
}

pub(crate) fn start_reload_if_needed(shared_state: &SharedState) -> bool {
    let runtime = {
        let mut st = shared_state.lock().unwrap();
        st.begin_reload_if_changed()
    };

    let Some(runtime) = runtime else {
        return false;
    };

    let state = Arc::clone(shared_state);
    std::thread::spawn(move || {
        let ok = match runtime.reload() {
            Ok(()) => {
                tracing::info!("librime redeployed after reload stamp change");
                true
            }
            Err(error) => {
                tracing::error!("librime reload failed: {error}");
                false
            }
        };
        state.lock().unwrap().finish_reload(ok);
    });
    true
}

pub(crate) fn update_ime_windows(
    shared_state: &SharedState,
    ime_state: &ImeState,
    caret_x: i32,
    caret_y: i32,
    show_mode_hint: bool,
) {
    with_detached_windows(shared_state, |candidate_win, mode_hint_win| {
        let show = !ime_state.candidates.is_empty() || !ime_state.preedit.is_empty();
        if show {
            candidate_win.show(ime_state, caret_x, caret_y);
        } else {
            candidate_win.hide();
        }
        if show_mode_hint {
            mode_hint_win.show_mode_hint(ime_state.ascii_mode, caret_x, caret_y);
        }
    });
}

pub(crate) fn hide_ime_windows(shared_state: &SharedState) {
    with_detached_windows(shared_state, |candidate_win, mode_hint_win| {
        candidate_win.hide();
        mode_hint_win.hide();
    });
}

pub(crate) fn hide_candidate_window(shared_state: &SharedState) {
    with_detached_windows(shared_state, |candidate_win, _mode_hint_win| {
        candidate_win.hide();
    });
}

fn with_detached_windows<R>(
    shared_state: &SharedState,
    f: impl FnOnce(
        &mut crate::candidate_win::CandidateWindow,
        &mut crate::candidate_win::CandidateWindow,
    ) -> R,
) -> R {
    let (mut candidate_win, mut mode_hint_win) = {
        let mut st = shared_state.lock().unwrap();
        (
            std::mem::replace(
                &mut st.candidate_win,
                crate::candidate_win::CandidateWindow::new(),
            ),
            std::mem::replace(
                &mut st.mode_hint_win,
                crate::candidate_win::CandidateWindow::new(),
            ),
        )
    };

    let result = f(&mut candidate_win, &mut mode_hint_win);

    let mut st = shared_state.lock().unwrap();
    st.candidate_win = candidate_win;
    st.mode_hint_win = mode_hint_win;
    result
}

fn has_rime_base_data(dir: &Path) -> bool {
    dir.join("default.yaml").is_file()
}

fn reload_stamp_signature(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            format!("{}:{}", metadata.len(), modified)
        }
        Err(_) => "missing".to_string(),
    }
}

fn preload_rime_dll(shared_data_dir: &str) -> Result<Option<LoadedRimeDll>, String> {
    let Some(runtime_dir) = Path::new(shared_data_dir).parent() else {
        return Ok(None);
    };
    let rime_dll = runtime_dir.join("rime.dll");
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
