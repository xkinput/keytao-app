//! Shared librime engine bootstrap and per-input-context sessions.

use keytao_core::{
    default_shared_data_dir, default_user_data_dir, deploy, Engine, ImeState, KeyProcessResult,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

#[derive(Clone)]
pub struct CoreEngine(Arc<CoreEngineState>);

#[derive(Clone)]
pub struct ImeSession {
    shared: Arc<CoreEngineState>,
    inner: Arc<Mutex<ImeSessionInner>>,
}

struct CoreEngineState {
    initialized: Mutex<bool>,
    generation: AtomicU64,
}

struct ImeSessionInner {
    engine: Engine,
    generation: u64,
}

impl CoreEngine {
    pub fn new() -> Self {
        Self(Arc::new(CoreEngineState {
            initialized: Mutex::new(false),
            generation: AtomicU64::new(0),
        }))
    }

    pub fn init(&self) -> Result<(), String> {
        let mut initialized = self.0.initialized.lock().unwrap();
        if *initialized {
            return Ok(());
        }

        self.deploy_locked()?;
        *initialized = true;
        Ok(())
    }

    pub fn reload(&self) -> Result<(), String> {
        let mut initialized = self.0.initialized.lock().unwrap();
        self.deploy_locked()?;
        *initialized = true;
        self.0.generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn deploy_locked(&self) -> Result<(), String> {
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let user = user_dir.to_string_lossy().into_owned();
        let shared = default_shared_data_dir();

        deploy(user, shared)
    }

    pub fn create_session(&self) -> Result<ImeSession, String> {
        let initialized = *self.0.initialized.lock().unwrap();
        if !initialized {
            self.init()?;
        }
        let generation = self.0.generation.load(Ordering::SeqCst);
        Ok(ImeSession {
            shared: self.0.clone(),
            inner: Arc::new(Mutex::new(ImeSessionInner {
                engine: Engine::new()?,
                generation,
            })),
        })
    }
}

impl ImeSession {
    pub fn state(&self) -> ImeState {
        let mut inner = self.inner.lock().unwrap();
        if self.refresh_if_needed(&mut inner).is_err() {
            return ImeState::empty();
        }
        inner.engine.state()
    }

    pub fn process_key_result(&self, keycode: u32, mask: u32) -> Option<KeyProcessResult> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.process_key_result(keycode, mask))
    }

    pub fn select_candidate(&self, index: usize) -> Option<ImeState> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.select_candidate(index))
    }

    pub fn reset(&self) -> Option<ImeState> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.reset())
    }

    fn refresh_if_needed(&self, inner: &mut ImeSessionInner) -> Result<(), String> {
        let current = self.shared.generation.load(Ordering::SeqCst);
        if inner.generation == current {
            return Ok(());
        }
        inner.engine = Engine::new()?;
        inner.generation = current;
        tracing::info!("Rime session refreshed after dictionary reload");
        Ok(())
    }
}
