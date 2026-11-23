use serde::{Deserialize, Serialize};
use std::path::Path;

pub const PACKAGE_KEY: &str = "keytao";

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
    let Some(package) = package else {
        return PackageEngine::Rime;
    };
    crate::runtime_dirs::read_regular_file(&root.join(crate::english_mode::SETTINGS_FILE))
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|settings| serde_json::from_value(settings["engines"][package].clone()).ok())
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
    fn engine_choice_applies_only_to_keytao() {
        let root =
            std::env::temp_dir().join(format!("keytao-engine-choice-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        assert_eq!(read(&root, Some(PACKAGE_KEY)), PackageEngine::Rime);
        std::fs::write(
            root.join(crate::english_mode::SETTINGS_FILE),
            br#"{"engines":{"keytao":"libjd","xmjd":"libjd","builtin/keytao":"libjd"}}"#,
        )
        .unwrap();
        assert_eq!(read(&root, Some(PACKAGE_KEY)), PackageEngine::Libjd);
        for key in [None, Some("xmjd"), Some("builtin/keytao")] {
            assert_eq!(read(&root, key), PackageEngine::Rime);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
