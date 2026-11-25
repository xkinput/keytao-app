pub mod account;
pub mod cloud_packages;
pub mod upload_bundle;
pub mod scheme_sync;
pub mod submission;
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
pub mod theme_library;
pub mod system_fonts;
pub mod rime_sync;
pub mod ime_status;
pub mod download;
pub mod libjd;
mod libjd_dictionary;
pub mod install;
mod package_store;
mod package_fs;
mod addon_store;
mod downloaded_addon;
mod generic_rime;
mod pinyin_addons;
mod shape_addons;
mod package_activation;
pub mod package_orphans;
pub mod package_migration;
#[cfg(test)]
mod package_activation_tests;
pub mod packages;
pub mod package_schemas;
pub mod scheme;
pub mod scheme_files;
pub mod scheme_host;
pub mod scheme_types;
pub mod update;
pub mod user_words;
mod remove_filter;
mod candidate_page_size;
mod managed_custom;
pub mod runtime_materialize;
pub mod wanxiang;
pub mod rime_ice;
pub mod rime_mint;
pub mod rime_frost;
pub mod rime_snow;
pub mod rime_lssp;
#[cfg(any(target_os = "windows", test))]
pub mod windows_public_schemas;
pub use scheme_types::*;

pub mod env;
pub mod error;
pub mod events;
pub mod state;

pub use env::{AppEnv, Platform};
pub use error::CoreError;
pub use events::{CoreEvent, EventSink};
pub use state::AppState;

#[cfg(test)]
fn run_librime_test_in_child(name: &str) -> bool {
    const CASE: &str = "KEYTAO_APP_CORE_LIBRIME_TEST";
    if std::env::var(CASE).as_deref() == Ok(name) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CASE, name)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed; 0 failed;"),
        "{name}: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

use keytao_core::runtime_log::AppOperation;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Core {
    self_ref: Weak<Self>,
    env: AppEnv,
    sink: Arc<dyn EventSink>,
    state: Mutex<AppState>,
    download_cancel: tokio::sync::watch::Sender<u64>,
    app_update_package: tokio::sync::Mutex<Option<update::VerifiedPackage>>,
    recovered_state_file: Option<PathBuf>,
    pub(crate) pending_package: Mutex<()>,
    pub(crate) user_words: Mutex<user_words::UserWordsState>,
    package_word_support: Mutex<BTreeMap<String, package_schemas::PackageWordSupport>>,
}

impl Core {
    pub fn new(env: AppEnv, sink: Arc<dyn EventSink>) -> Result<Arc<Core>, CoreError> {
        Self::new_with_download_cancel(env, sink, tokio::sync::watch::channel(0).0)
    }

    fn new_with_download_cancel(
        env: AppEnv,
        sink: Arc<dyn EventSink>,
        download_cancel: tokio::sync::watch::Sender<u64>,
    ) -> Result<Arc<Core>, CoreError> {
        let loaded = {
            let _operation = AppOperation::start("app_state_load");
            AppState::load(&env.data_dir)
        };
        let (state, recovered_state_file) = match loaded {
            Ok(state) => (state, None),
            Err(CoreError::Parse(_)) => {
                let _operation = AppOperation::start("app_state_quarantine");
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
        let core = Arc::new_cyclic(|self_ref| Self {
            self_ref: self_ref.clone(),
            env,
            sink,
            state: Mutex::new(state),
            download_cancel,
            app_update_package: tokio::sync::Mutex::new(None),
            recovered_state_file,
            pending_package: Mutex::new(()),
            user_words: Mutex::new(user_words::UserWordsState::default()),
            package_word_support: Mutex::default(),
        });
        {
            let _operation = AppOperation::start("package_activation_recover");
            package_activation::recover(&core)?;
        }
        {
            let _operation = AppOperation::start("app_state_reload");
            *core.lock_state() = AppState::load(&core.env.data_dir)?;
        }
        {
            let _operation = AppOperation::start("package_store_migrate");
            if let Err(error) = package_store::PackageStore::new(&core).migrate() {
                tracing::warn!(%error, "Package store migration will retry at next startup");
            }
        }
        {
            let _operation = AppOperation::start("snow_package_migrate");
            if let Err(error) = addon_store::migrate_snow_package(&core) {
                tracing::warn!(%error, "Snow package migration will retry at next startup");
            }
        }
        update::prune_app_update_cache(&core);
        packages::package_logos::prune_package_logo_cache(&core);
        Ok(core)
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

    pub fn cancel_download(&self) {
        self.download_cancel.send_modify(|generation| *generation += 1);
    }

    pub fn state(&self) -> AppState {
        self.lock_state().clone()
    }

    /// Classify fresh/recovered state once a local IME root can be resolved.
    /// Keep unknown roots pending across both auth writes and restarts.
    pub fn initialize_onboarding(&self, local_schema_dir: Option<PathBuf>) -> Result<(), CoreError> {
        if self.lock_state().onboarding_initialized {
            return Ok(());
        }
        let Some(local_schema_dir) = local_schema_dir else {
            return Ok(());
        };
        let _operation = AppOperation::start("initialize_onboarding");
        let installed = scheme::check_local_schema(self, Some(local_schema_dir))?.installed;
        let mut state = self.lock_state();
        if state.onboarding_initialized {
            return Ok(());
        }
        let mut next = state.clone();
        next.onboarding_completed |= state::should_complete_onboarding(true, installed);
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
        if next.active_scheme != state.active_scheme {
            next.last_deploy = None;
        }
        if state.user.as_ref().and_then(|user| user["id"].as_i64())
            != next.user.as_ref().and_then(|user| user["id"].as_i64())
        {
            next.scheme_cloud_ids.clear();
        }
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
