use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, path::Path};

const STATE_FILE: &str = "app-state.json";

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppState {
    pub auth_token: Option<String>,
    pub user: Option<serde_json::Value>,
    pub onboarding_completed: bool,
    pub legacy_imported: bool,
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
