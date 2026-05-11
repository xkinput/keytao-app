//! Shared librime engine bootstrap and per-input-context sessions.

use keytao_core::{
    default_shared_data_dir, default_user_data_dir, deploy, Engine, ImeState, KeyProcessResult,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct CoreEngine(Arc<Mutex<bool>>);

#[derive(Clone)]
pub struct ImeSession(Arc<Mutex<Engine>>);

impl CoreEngine {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(false)))
    }

    pub fn init(&self) -> Result<(), String> {
        let mut initialized = self.0.lock().unwrap();
        if *initialized {
            return Ok(());
        }

        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let user = user_dir.to_string_lossy().into_owned();
        let shared = default_shared_data_dir();

        deploy(user.clone(), shared)?;
        *initialized = true;
        Ok(())
    }

    pub fn create_session(&self) -> Result<ImeSession, String> {
        let initialized = *self.0.lock().unwrap();
        if !initialized {
            self.init()?;
        }
        Ok(ImeSession(Arc::new(Mutex::new(Engine::new()?))))
    }
}

impl ImeSession {
    pub fn state(&self) -> ImeState {
        self.0.lock().unwrap().state()
    }

    pub fn process_key_result(&self, keycode: u32, mask: u32) -> Option<KeyProcessResult> {
        Some(self.0.lock().unwrap().process_key_result(keycode, mask))
    }

    pub fn select_candidate(&self, index: usize) -> Option<ImeState> {
        Some(self.0.lock().unwrap().select_candidate(index))
    }

    pub fn reset(&self) -> Option<ImeState> {
        Some(self.0.lock().unwrap().reset())
    }
}
