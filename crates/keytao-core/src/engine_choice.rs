use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub const PACKAGE_KEY: &str = "keytao";
pub const ENABLED_FILE: &str = ".keytao-engine-enabled";

pub fn is_enabled(root: &Path) -> bool {
    crate::runtime_dirs::read_regular_file(&root.join(ENABLED_FILE)).as_deref() != Some(b"0\n")
}

pub fn set_enabled(root: &Path, enabled: bool) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".keytao-engine-enabled.lock"))
        .map_err(|error| error.to_string())?;
    crate::lock_file(&lock, true).map_err(|error| error.to_string())?;

    let path = root.join(ENABLED_FILE);
    let previous = crate::runtime_dirs::read_regular_file(&path);
    crate::write_file_atomic(&path, if enabled { b"1\n" } else { b"0\n" })
        .map_err(|error| format!("save input engine state: {error}"))?;
    if let Err(error) = crate::ReloadStamp::write(root) {
        match previous {
            Some(bytes) => crate::write_file_atomic(&path, &bytes),
            None => fs::remove_file(&path),
        }
        .map_err(|restore| format!("{error}; restore input engine state: {restore}"))?;
        return Err(error);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageEngine {
    #[default]
    Rime,
    Libjd,
    #[serde(rename = "libjd_engine")]
    LibjdEngine,
}

impl PackageEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rime => "rime",
            Self::Libjd => "libjd",
            Self::LibjdEngine => "libjd_engine",
        }
    }

    pub fn is_rime(&self) -> bool {
        *self == Self::Rime
    }
}

pub fn read(root: &Path, package: Option<&str>) -> PackageEngine {
    let settings = settings(root);
    if configured_in(&settings, package) == PackageEngine::LibjdEngine {
        preferred_in(root, &settings)
    } else {
        PackageEngine::Rime
    }
}

pub fn preferred(root: &Path) -> PackageEngine {
    preferred_in(root, &settings(root))
}

pub fn configured(root: &Path, package: Option<&str>) -> PackageEngine {
    configured_in(&settings(root), package)
}

fn settings(root: &Path) -> serde_json::Value {
    crate::runtime_dirs::read_regular_file(&root.join(crate::english_mode::SETTINGS_FILE))
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn preferred_in(root: &Path, settings: &serde_json::Value) -> PackageEngine {
    match settings.get("engine") {
        Some(engine) if engine.as_str() == Some(PackageEngine::LibjdEngine.as_str()) => {
            PackageEngine::LibjdEngine
        }
        // Keep upgraded users on their converted scheme. The old bundled dictionary is retired.
        None if configured_in(settings, active_package(root).as_deref()) == PackageEngine::LibjdEngine => {
            PackageEngine::LibjdEngine
        }
        _ => PackageEngine::Rime,
    }
}

fn configured_in(settings: &serde_json::Value, package: Option<&str>) -> PackageEngine {
    let Some(package) = package else {
        return PackageEngine::Rime;
    };
    serde_json::from_value(settings["engines"][package].clone())
        .ok()
        .filter(|engine| *engine != PackageEngine::Libjd || package == PACKAGE_KEY)
        .unwrap_or_default()
}

pub fn active_package(root: &Path) -> Option<String> {
    use crate::RuntimeDirs;
    if RuntimeDirs::needs_migration(root) {
        RuntimeDirs::flat_active_package_key(root)
    } else {
        RuntimeDirs::active_package_key(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_state_is_unchanged_when_reload_cannot_be_signalled() {
        let root = std::env::temp_dir().join(format!("keytao-engine-state-{}", std::process::id()));
        let stamp = crate::ReloadStamp::path(&root);
        fs::create_dir_all(&stamp).unwrap();
        assert!(set_enabled(&root, false).is_err());
        assert!(is_enabled(&root));
        assert!(!root.join(ENABLED_FILE).exists());

        fs::remove_dir(&stamp).unwrap();
        set_enabled(&root, false).unwrap();
        fs::remove_file(&stamp).unwrap();
        fs::create_dir(&stamp).unwrap();
        assert!(set_enabled(&root, true).is_err());
        assert!(!is_enabled(&root));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_preference_and_per_package_conversion_stay_independent() {
        let root = std::env::temp_dir().join(format!("keytao-engine-choice-{}", std::process::id()));
        let store = crate::package_store::root(&root);
        std::fs::create_dir_all(&store).unwrap();
        assert_eq!(read(&root, Some(PACKAGE_KEY)), PackageEngine::Rime);
        let settings = root.join(crate::english_mode::SETTINGS_FILE);
        let active = store.join("active-manifest.json");
        std::fs::write(&active, br#"{"active_package":"keytao"}"#).unwrap();
        std::fs::write(
            &settings,
            br#"{"engines":{"keytao":"libjd","xmjd":"libjd_engine"}}"#,
        )
        .unwrap();
        assert_eq!(configured(&root, Some(PACKAGE_KEY)), PackageEngine::Libjd);
        assert_eq!(read(&root, Some(PACKAGE_KEY)), PackageEngine::Rime);
        assert_eq!(read(&root, Some("xmjd")), PackageEngine::Rime);
        std::fs::write(&active, br#"{"active_package":"xmjd"}"#).unwrap();
        assert_eq!(preferred(&root), PackageEngine::LibjdEngine);
        assert_eq!(read(&root, Some("xmjd")), PackageEngine::LibjdEngine);
        std::fs::write(
            &settings,
            br#"{"engine":"rime","engines":{"xmjd":"libjd_engine"}}"#,
        )
        .unwrap();
        assert_eq!(preferred(&root), PackageEngine::Rime);
        assert_eq!(read(&root, Some("xmjd")), PackageEngine::Rime);
        assert_eq!(configured(&root, Some("xmjd")), PackageEngine::LibjdEngine);
        std::fs::write(&settings, br#"{"engine":"libjd_engine"}"#).unwrap();
        assert_eq!(preferred(&root), PackageEngine::LibjdEngine);
        assert_eq!(read(&root, Some("xmjd")), PackageEngine::Rime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
