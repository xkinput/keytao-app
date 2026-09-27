pub mod account;
pub mod addon;
#[cfg(target_os = "android")]
pub mod android_jni;
pub mod logs;
#[cfg(target_os = "windows")]
pub mod app_actions;
pub mod component_versions;
pub mod deploy;
pub mod ime_host;
pub mod ime_paths;
pub mod ime_settings;
pub mod ime_status;
pub mod download;
pub mod install;
pub mod scheme;
pub mod scheme_files;
pub mod scheme_host;
pub mod scheme_types;
pub mod update;
pub mod wanxiang;
pub use scheme_types::*;

pub mod env;
pub mod error;
pub mod events;
pub mod state;

pub use env::{AppEnv, Platform};
pub use error::CoreError;
pub use events::{CoreEvent, EventSink};
pub use state::AppState;

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Core {
    env: AppEnv,
    sink: Arc<dyn EventSink>,
    state: Mutex<AppState>,
    recovered_state_file: Option<PathBuf>,
}

impl Core {
    pub fn new(env: AppEnv, sink: Arc<dyn EventSink>) -> Result<Arc<Core>, CoreError> {
        let (state, recovered_state_file) = match AppState::load(&env.data_dir) {
            Ok(state) => (state, None),
            Err(CoreError::Parse(_)) => {
                let mut millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| CoreError::Other(error.to_string()))?
                    .as_millis();
                let mut quarantine = env
                    .data_dir
                    .join(format!("app-state.json.corrupt-{millis}"));
                // Preserve earlier quarantines if recovery repeats in one millisecond.
                while quarantine.try_exists()? {
                    millis += 1;
                    quarantine = env
                        .data_dir
                        .join(format!("app-state.json.corrupt-{millis}"));
                }
                fs::rename(env.data_dir.join("app-state.json"), &quarantine)?;
                (AppState::default(), Some(quarantine))
            }
            Err(error) => return Err(error),
        };
        Ok(Arc::new(Self {
            env,
            sink,
            state: Mutex::new(state),
            recovered_state_file,
        }))
    }

    /// The preserved corrupt state file, if this Core recovered during startup.
    pub fn recovered_state_file(&self) -> Option<PathBuf> {
        self.recovered_state_file.clone()
    }

    pub fn env(&self) -> &AppEnv {
        &self.env
    }

    pub fn emit(&self, event: CoreEvent) {
        self.sink.emit(event);
    }

    pub fn state(&self) -> AppState {
        self.lock_state().clone()
    }

    /// Classify fresh/recovered state once a local IME root can be resolved.
    /// Keep unknown roots pending across both auth writes and restarts.
    pub fn initialize_onboarding(&self, local_schema_dir: Option<PathBuf>) -> Result<(), CoreError> {
        let mut state = self.lock_state();
        let first_run = !state.onboarding_initialized;
        if !first_run {
            return Ok(());
        }
        let Some(local_schema_dir) = local_schema_dir else {
            return Ok(());
        };
        let installed = scheme::check_local_schema(self, Some(local_schema_dir))?.installed;
        let mut next = state.clone();
        next.onboarding_completed |= state::should_complete_onboarding(first_run, installed);
        next.onboarding_initialized = true;
        // Persist even a fresh user's false value so later installs do not turn
        // a subsequent startup into an old-user migration.
        next.save(&self.env.data_dir)?;
        *state = next;
        Ok(())
    }

    pub fn onboarding(&self) -> state::OnboardingState {
        state::OnboardingState { completed: self.lock_state().onboarding_completed }
    }

    pub fn complete_onboarding(&self) -> Result<(), CoreError> {
        self.update_state(|state| {
            state.onboarding_completed = true;
            state.onboarding_initialized = true;
        })
    }

    /// Serialize updates within this Core. The callback must not re-enter Core.
    /// Publish the snapshot only after the atomic file replacement succeeds.
    pub fn update_state(&self, update: impl FnOnce(&mut AppState)) -> Result<(), CoreError> {
        let mut state = self.lock_state();
        let mut next = state.clone();
        update(&mut next);
        if next != *state {
            next.save(&self.env.data_dir)?;
            *state = next;
        }
        Ok(())
    }

    pub fn import_legacy_state(
        &self,
        auth_token: Option<String>,
        user: Option<serde_json::Value>,
        onboarding_completed: bool,
    ) -> Result<(), CoreError> {
        let auth_token = auth_token.filter(|token| !token.trim().is_empty());
        let user = auth_token.as_ref().and(user);
        self.update_state(move |state| {
            if !state.legacy_imported {
                if state.auth_token.is_none() {
                    state.auth_token = auth_token;
                    state.user = user;
                }
                state.onboarding_completed |= onboarding_completed;
                state.legacy_imported = true;
            }
        })
    }

    pub fn clear_auth(&self) -> Result<(), CoreError> {
        self.update_state(|state| {
            state.auth_token = None;
            state.user = None;
            // A delayed WebView import must not resurrect a cleared session.
            state.legacy_imported = true;
        })
    }

    fn lock_state(&self) -> MutexGuard<'_, AppState> {
        // Callbacks only mutate a candidate snapshot, so a panic cannot publish
        // partial changes to either the persisted or in-memory state.
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod scheme_tests;

#[cfg(test)]
mod install_tests;
