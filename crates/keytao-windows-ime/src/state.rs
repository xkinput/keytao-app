//! Shared state between all TSF COM objects.

use keytao_core::{
    default_shared_data_dir, default_user_data_dir, ImeRuntime, ImeRuntimeSession, ImeState,
    KeyProcessResult,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use windows::Win32::UI::TextServices::{ITfComposition, ITfKeyEventSink, ITfThreadMgr};

const RELOAD_STAMP_FILE: &str = "keytao-ime.reload";

// ── Shared TsfState ───────────────────────────────────────────────────────────

pub struct TsfState {
    pub runtime: Option<ImeRuntime>,
    pub session: Option<ImeRuntimeSession>,
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
    pub reload_clear_pending: bool,
    pub shift_pressed_without_key: bool,
}

// SAFETY: TSF TIPs run in COM STA; all calls are on the same thread.
unsafe impl Send for TsfState {}
unsafe impl Sync for TsfState {}

impl TsfState {
    pub fn new() -> Self {
        Self {
            runtime: None,
            session: None,
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
            reload_clear_pending: false,
            shift_pressed_without_key: false,
        }
    }

    pub fn init_engine(&mut self) -> Result<(), String> {
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let shared = bundled_shared_data_dir().unwrap_or_else(default_shared_data_dir);
        let reload_stamp_path = user_dir.join(RELOAD_STAMP_FILE);
        let runtime = ImeRuntime::with_dirs(user_dir, shared);
        runtime.init()?;
        let session = runtime.create_session()?;
        self.runtime = Some(runtime);
        self.session = Some(session);
        self.reload_stamp_signature = Some(reload_stamp_signature(&reload_stamp_path));
        self.reload_stamp_path = Some(reload_stamp_path);
        Ok(())
    }

    pub fn ensure_engine(&mut self) -> bool {
        if self.session.is_some() {
            return true;
        }
        match self.init_engine() {
            Ok(()) => true,
            Err(e) => {
                tracing::error!("librime init failed: {e}");
                false
            }
        }
    }

    pub fn check_reload_stamp(&mut self) -> bool {
        let Some(path) = &self.reload_stamp_path else {
            return false;
        };
        let signature = reload_stamp_signature(path);
        if self.reload_stamp_signature.as_deref() == Some(signature.as_str()) {
            return false;
        }
        self.reload_stamp_signature = Some(signature);

        let Some(runtime) = &self.runtime else {
            return false;
        };
        match runtime.reload() {
            Ok(()) => {
                self.reload_clear_pending = true;
                tracing::info!("librime redeployed after reload stamp change");
                true
            }
            Err(e) => {
                tracing::error!("librime reload failed: {e}");
                false
            }
        }
    }

    pub fn take_reload_clear_pending(&mut self) -> bool {
        std::mem::take(&mut self.reload_clear_pending)
    }

    pub fn current_state(&self) -> Option<ImeState> {
        self.session.as_ref().map(ImeRuntimeSession::state)
    }

    pub fn process_key_result(&self, keysym: u32, mods: u32) -> Option<KeyProcessResult> {
        self.session.as_ref()?.process_key_result(keysym, mods)
    }

    pub fn select_candidate(&self, index: usize) -> Option<ImeState> {
        self.session.as_ref()?.select_candidate(index)
    }

    pub fn reset_session(&self) -> Option<ImeState> {
        self.session.as_ref()?.reset()
    }
}

pub type SharedState = Arc<Mutex<TsfState>>;

pub fn new_shared_state() -> SharedState {
    Arc::new(Mutex::new(TsfState::new()))
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
