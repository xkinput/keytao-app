//! Tauri adapter layer — thin glue between keytao-core and Tauri's IPC.
//! All platform logic, IME state types, and librime calls live in `keytao-core`.

use keytao_core::{
    default_user_data_dir, ImeRuntime, ImeRuntimeSession,
};
use std::sync::{Mutex, MutexGuard};

// ── Managed state ─────────────────────────────────────────────────────────────

/// The in-app test input, running on the process-wide [`ImeRuntime`].
///
/// It deliberately does not keep an `Engine` of its own: librime holds its
/// compiled config and its dictionaries behind `weak_ptr`s and only re-reads a
/// deployment once the last session referencing them is gone, so an engine
/// outside the runtime's session registry would keep the app serving the old
/// dictionaries after every deploy.
#[derive(Default)]
pub struct RimeEngine {
    inner: Mutex<TestInput>,
}

#[derive(Default)]
struct TestInput {
    runtime: Option<ImeRuntime>,
    session: Option<ImeRuntimeSession>,
}

impl RimeEngine {
    /// Close the test input and hand back the runtime to deploy on.
    ///
    /// The session has to be gone *before* the deployment rather than replaced
    /// after it: a redeploy under a live session rebuilds the artifacts and
    /// then keeps serving the cached ones.
    pub fn suspend_for_deploy(&self, user_data_dir: String, shared_data_dir: String) -> ImeRuntime {
        let runtime = ImeRuntime::with_dirs(user_data_dir, shared_data_dir);
        let mut inner = self.lock();
        inner.session = None;
        inner.runtime = Some(runtime.clone());
        runtime
    }

    /// Open the test input again on the runtime the last deployment ran on.
    pub fn resume(&self) -> Result<(), String> {
        let mut inner = self.lock();
        let runtime = inner
            .runtime
            .clone()
            .ok_or("Rime runtime not initialised")?;
        inner.session = Some(runtime.create_session()?);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, TestInput> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[tauri::command]
pub fn rime_get_data_dir() -> Option<String> {
    default_user_data_dir().map(|p| p.to_string_lossy().into_owned())
}
