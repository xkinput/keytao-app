//! Shared state between all TSF COM objects.

use keytao_core::{default_shared_data_dir, default_user_data_dir, deploy, Engine, ImeState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows::Win32::UI::TextServices::{ITfComposition, ITfKeyEventSink, ITfThreadMgr};

// ── Shared TsfState ───────────────────────────────────────────────────────────

pub struct TsfState {
    pub engine: Option<Engine>,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub client_id: u32,
    pub key_sink: Option<ITfKeyEventSink>,
    pub composition: Option<ITfComposition>,
    pub ime_state: Option<ImeState>,
    pub candidate_win: crate::candidate_win::CandidateWindow,
}

// SAFETY: TSF TIPs run in COM STA; all calls are on the same thread.
unsafe impl Send for TsfState {}
unsafe impl Sync for TsfState {}

impl TsfState {
    pub fn new() -> Self {
        Self {
            engine: None,
            thread_mgr: None,
            client_id: 0,
            key_sink: None,
            composition: None,
            ime_state: None,
            candidate_win: crate::candidate_win::CandidateWindow::new(),
        }
    }

    pub fn init_engine(&mut self) -> Result<(), String> {
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let shared = bundled_shared_data_dir().unwrap_or_else(default_shared_data_dir);
        deploy(user_dir.to_string_lossy().into_owned(), shared)?;
        self.engine = Some(Engine::new()?);
        Ok(())
    }

    pub fn engine(&self) -> Option<&Engine> {
        self.engine.as_ref()
    }
}

pub type SharedState = Arc<Mutex<TsfState>>;

pub fn new_shared_state() -> SharedState {
    Arc::new(Mutex::new(TsfState::new()))
}

fn has_rime_base_data(dir: &Path) -> bool {
    dir.join("default.yaml").is_file()
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
