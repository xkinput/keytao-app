use crate::Core;
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub fn macos_app_shared_data_dir(core: &Core) -> Option<String> {
    let mut bases = vec![core.env().resource_dir.clone()];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                bases.push(contents_dir.join("Resources"));
            }
        }
    }
    let mut candidates = Vec::new();
    for base in bases {
        candidates.extend(
            [
                "rime-data",
                "SharedSupport",
                "KeyTao.app/Contents/Resources/rime-data",
                "KeyTao.app/Contents/SharedSupport",
            ]
            .map(|suffix| base.join(suffix)),
        );
    }
    candidates.extend([
        PathBuf::from("/Library/Input Methods/KeyTao.app/Contents/Resources/rime-data"),
        PathBuf::from("/Library/Input Methods/KeyTao.app/Contents/SharedSupport"),
    ]);

    candidates
        .into_iter()
        .find(|dir| dir.join("default.yaml").is_file())
        .map(|dir| dir.to_string_lossy().into_owned())
}
#[derive(Serialize)]
pub struct MacosImeStatus {
    pub installed: bool,
    pub app_path: Option<String>,
    pub message: String,
}

#[cfg(target_os = "macos")]
fn macos_ime_app_path() -> PathBuf {
    PathBuf::from("/Library/Input Methods/KeyTao.app")
}

#[cfg(target_os = "macos")]
fn macos_ime_status_inner() -> MacosImeStatus {
    let path = macos_ime_app_path();
    let installed = path.join("Contents/MacOS/KeyTaoIME").is_file();
    MacosImeStatus {
        installed,
        app_path: Some(path.to_string_lossy().into_owned()),
        message: if installed {
            "KeyTao macOS input method is installed".into()
        } else {
            "KeyTao macOS input method is not installed".into()
        },
    }
}

/// Check whether the KeyTao.app input method is installed.
pub fn macos_ime_status(_core: &Core) -> MacosImeStatus {
    #[cfg(target_os = "macos")]
    {
        macos_ime_status_inner()
    }
    #[cfg(not(target_os = "macos"))]
    {
        MacosImeStatus {
            installed: false,
            app_path: None,
            message: "macOS only".into(),
        }
    }
}
