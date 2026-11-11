use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::Path,
};

const STATE_FILE: &str = "app-state.json";

#[derive(Serialize)]
pub struct OnboardingState {
    pub completed: bool,
}

pub(crate) fn should_complete_onboarding(first_run: bool, local_scheme_installed: bool) -> bool {
    first_run && local_scheme_installed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_installed_scheme_migrates_and_saves_only_package_state() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(STATE_FILE),
            r#"{
                "auth_token": "test-token",
                "user": {"id": 7},
                "onboarding_completed": true,
                "legacy_imported": true,
                "installed_scheme": "xmjd-feiding",
                "installed_scheme_version": "v1"
            }"#,
        )
        .unwrap();

        let state = AppState::load(dir.path()).unwrap();
        state.save(dir.path()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(
            saved["installed_packages"],
            serde_json::json!({"xmjd-feiding": {"version": "v1"}})
        );
        assert_eq!(saved["active_scheme"], "xmjd-feiding");
        assert!(saved.get("installed_scheme").is_none());
        assert!(saved.get("installed_scheme_version").is_none());
        assert_eq!(saved["auth_token"], "test-token");
        assert_eq!(saved["user"], serde_json::json!({"id": 7}));
        assert_eq!(saved["onboarding_initialized"], true);
        assert!(AppState::load(dir.path()).unwrap() == state);
        assert!(serde_json::from_str::<AppState>("{}").is_err());
    }

    #[test]
    fn state_without_installed_scheme_deserializes() {
        let state: AppState = serde_json::from_str(
            r#"{
                "auth_token": "test-token",
                "user": {"id": 7},
                "onboarding_completed": true,
                "onboarding_initialized": true,
                "legacy_imported": true
            }"#,
        )
        .unwrap();
        assert!(state.installed_packages.is_empty());
        assert!(state.active_scheme.is_none());
    }

    #[test]
    fn old_user_rule_requires_first_run_and_installed_scheme() {
        assert!(should_complete_onboarding(true, true));
        assert!(!should_complete_onboarding(true, false));
        assert!(!should_complete_onboarding(false, true));
        assert!(!should_complete_onboarding(false, false));
    }

    #[test]
    fn dismissed_scheme_versions_survive_save_and_legacy_state_defaults_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(STATE_FILE),
            r#"{"auth_token":null,"user":null,"onboarding_completed":true,"legacy_imported":true,"dismissed_scheme_updates":{"xmjd":"20261105"}}"#,
        ).unwrap();
        let state = AppState::load(dir.path()).unwrap();
        state.save(dir.path()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(
            saved["dismissed_scheme_updates"],
            serde_json::json!({"xmjd":"20261105"})
        );
        fs::write(
            dir.path().join(STATE_FILE),
            r#"{"auth_token":null,"user":null,"onboarding_completed":true,"legacy_imported":true}"#,
        )
        .unwrap();
        AppState::load(dir.path())
            .unwrap()
            .save(dir.path())
            .unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(STATE_FILE)).unwrap()).unwrap();
        assert_eq!(saved["dismissed_scheme_updates"], serde_json::json!({}));
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub version: Option<String>,
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredAppState")]
pub struct AppState {
    pub auth_token: Option<String>,
    pub user: Option<serde_json::Value>,
    pub onboarding_completed: bool,
    // Existing files already finalized onboarding; fresh/recovered defaults have
    // not. Persist this distinction when auth writes precede root resolution.
    #[serde(default = "existing_onboarding_initialized")]
    pub onboarding_initialized: bool,
    pub legacy_imported: bool,
    pub installed_packages: BTreeMap<String, InstalledPackage>,
    pub active_scheme: Option<String>,
    pub dismissed_scheme_updates: BTreeMap<String, String>,
    pub scheme_sync_settings: serde_json::Value,
    pub follow_account_sync: bool,
    pub synced_schemes: BTreeSet<String>,
    pub last_scheme_sync_at: Option<String>,
    pub scheme_cloud_ids: BTreeMap<String, String>,
    pub last_deploy: Option<crate::deploy::DeployRecord>,
}

#[derive(Deserialize)]
struct StoredAppState {
    auth_token: Option<String>,
    user: Option<serde_json::Value>,
    onboarding_completed: bool,
    #[serde(default = "existing_onboarding_initialized")]
    onboarding_initialized: bool,
    legacy_imported: bool,
    installed_packages: Option<BTreeMap<String, InstalledPackage>>,
    active_scheme: Option<String>,
    #[serde(default)]
    dismissed_scheme_updates: BTreeMap<String, String>,
    #[serde(default)]
    scheme_sync_settings: serde_json::Value,
    #[serde(default)]
    follow_account_sync: bool,
    #[serde(default)]
    synced_schemes: BTreeSet<String>,
    last_scheme_sync_at: Option<String>,
    #[serde(default)]
    scheme_cloud_ids: BTreeMap<String, String>,
    last_deploy: Option<crate::deploy::DeployRecord>,
    installed_scheme: Option<String>,
    installed_scheme_version: Option<String>,
}

impl From<StoredAppState> for AppState {
    fn from(state: StoredAppState) -> Self {
        let (installed_packages, active_scheme) = match state.installed_packages {
            Some(packages) => (packages, state.active_scheme),
            None => {
                let mut packages = BTreeMap::new();
                if let Some(scheme) = state.installed_scheme.as_ref() {
                    packages.insert(
                        scheme.clone(),
                        InstalledPackage {
                            version: state.installed_scheme_version,
                        },
                    );
                }
                (packages, state.installed_scheme)
            }
        };
        Self {
            auth_token: state.auth_token,
            user: state.user,
            onboarding_completed: state.onboarding_completed,
            onboarding_initialized: state.onboarding_initialized,
            legacy_imported: state.legacy_imported,
            installed_packages,
            active_scheme,
            dismissed_scheme_updates: state.dismissed_scheme_updates,
            scheme_sync_settings: state.scheme_sync_settings,
            follow_account_sync: state.follow_account_sync,
            synced_schemes: state.synced_schemes,
            last_scheme_sync_at: state.last_scheme_sync_at,
            scheme_cloud_ids: state.scheme_cloud_ids,
            last_deploy: state.last_deploy,
        }
    }
}

fn existing_onboarding_initialized() -> bool {
    true
}

impl AppState {
    pub fn load(data_dir: &Path) -> Result<Self, CoreError> {
        match fs::read(data_dir.join(STATE_FILE)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn save(&self, data_dir: &Path) -> Result<(), CoreError> {
        fs::create_dir_all(data_dir)?;
        // A same-directory temporary file keeps replacement on one filesystem.
        // tempfile creates private (0600) files on Unix and removes them on error.
        let mut temporary = tempfile::Builder::new()
            .prefix(".app-state-")
            .suffix(".tmp")
            .tempfile_in(data_dir)?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), self)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(data_dir.join(STATE_FILE))
            .map_err(|error| CoreError::from(error.error))?;
        Ok(())
    }
}
