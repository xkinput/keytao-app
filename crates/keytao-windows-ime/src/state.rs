//! Shared state between all TSF COM objects.

use keytao_core::{default_shared_data_dir, default_user_data_dir, deploy, Engine, ImeState};
use std::sync::{Arc, Mutex};
use windows::Win32::UI::TextServices::{ITfComposition, ITfKeyEventSink, ITfThreadMgr};

// ── Shared TsfState ───────────────────────────────────────────────────────────

pub struct TsfState {
    pub engine: Option<Engine>,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub client_id: u32,
    pub thread_mgr_cookie: u32,
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
            thread_mgr_cookie: 0,
            key_sink: None,
            composition: None,
            ime_state: None,
            candidate_win: crate::candidate_win::CandidateWindow::new(),
        }
    }

    pub fn init_engine(&mut self) -> Result<(), String> {
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let shared = default_shared_data_dir();
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
