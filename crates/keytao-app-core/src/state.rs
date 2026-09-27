use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, path::Path};

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
    fn old_user_rule_requires_first_run_and_installed_scheme() {
        assert!(should_complete_onboarding(true, true));
        assert!(!should_complete_onboarding(true, false));
        assert!(!should_complete_onboarding(false, true));
        assert!(!should_complete_onboarding(false, false));
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppState {
    pub auth_token: Option<String>,
    pub user: Option<serde_json::Value>,
    pub onboarding_completed: bool,
    // Existing files already finalized onboarding; fresh/recovered defaults have
    // not. Persist this distinction when auth writes precede root resolution.
    #[serde(default = "existing_onboarding_initialized")]
    pub onboarding_initialized: bool,
    pub legacy_imported: bool,
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
