use crate::Core;
#[cfg(target_os = "macos")]
use crate::ime_paths::*;
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub fn macos_app_shared_data_dir(core: &Core) -> Option<String> {
    let mut candidates = Vec::new();

    {
        let resource_dir = core.env().resource_dir.clone();
        candidates.extend([
            resource_dir.join("rime-data"),
            resource_dir.join("SharedSupport"),
            resource_dir
                .join("KeyTao.app")
                .join("Contents")
                .join("Resources")
                .join("rime-data"),
            resource_dir
                .join("KeyTao.app")
                .join("Contents")
                .join("SharedSupport"),
        ]);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                let resources_dir = contents_dir.join("Resources");
                candidates.extend([
                    resources_dir.join("rime-data"),
                    resources_dir.join("SharedSupport"),
                    resources_dir
                        .join("KeyTao.app")
                        .join("Contents")
                        .join("Resources")
                        .join("rime-data"),
                    resources_dir
                        .join("KeyTao.app")
                        .join("Contents")
                        .join("SharedSupport"),
                ]);
            }
        }
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
    pub user_data_dir: Option<String>,
    pub shared_data_dir: Option<String>,
    pub shared_data_source: String,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub log_dir: Option<String>,
    pub message: String,
}

#[cfg(target_os = "macos")]
fn macos_ime_app_path() -> PathBuf {
    PathBuf::from("/Library/Input Methods/KeyTao.app")
}

#[cfg(target_os = "macos")]
fn macos_ime_status_inner(core: &Core) -> MacosImeStatus {
    let path = macos_ime_app_path();
    let installed = path.join("Contents/MacOS/KeyTaoIME").is_file();
    let (shared_data_dir, shared_data_source) = shared_data_status(macos_app_shared_data_dir(core));
    let (reload_stamp_path, reload_stamp_signature) = reload_stamp_status();
    MacosImeStatus {
        installed,
        app_path: Some(path.to_string_lossy().into_owned()),
        user_data_dir: default_user_data_dir_string(),
        shared_data_dir,
        shared_data_source,
        reload_stamp_path,
        reload_stamp_signature,
        log_dir: keytao_core::default_user_data_dir().map(|dir| path_string(dir.join("log"))),
        message: if installed {
            "KeyTao macOS input method is installed".into()
        } else {
            "KeyTao macOS input method is not installed".into()
        },
    }
}

/// Check whether the KeyTao.app input method is installed.
pub fn macos_ime_status(core: &Core) -> MacosImeStatus {
    #[cfg(target_os = "macos")]
    {
        macos_ime_status_inner(&core)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = core;
        MacosImeStatus {
            installed: false,
            app_path: None,
            user_data_dir: None,
            shared_data_dir: None,
            shared_data_source: "unsupported".into(),
            reload_stamp_path: None,
            reload_stamp_signature: None,
            log_dir: None,
            message: "macOS only".into(),
        }
    }
}
