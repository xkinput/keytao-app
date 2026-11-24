use crate::Core;
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

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
pub async fn set_input_engine_enabled(root: &Path, enabled: bool) -> Result<(), String> {
    use keytao_core::engine_choice;
    use std::time::Duration;
    use tokio::process::Command;

    let lock_root = root.to_owned();
    let _control = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&lock_root).map_err(|error| error.to_string())?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_root.join(".keytao-macos-engine-control.lock"))
            .map_err(|error| error.to_string())?;
        keytao_core::lock_file(&file, true).map_err(|error| error.to_string())?;
        Ok::<_, String>(file)
    })
    .await
    .map_err(|error| error.to_string())??;

    let app = macos_ime_app_path();
    let helper = app.join("Contents/MacOS/KeyTaoIME");
    let supports_shutdown = if helper.is_file() {
        let capability = Command::new("/usr/bin/plutil")
            .args(["-extract", "KeyTaoSupportsInputMethodShutdown", "raw"])
            .arg(app.join("Contents/Info.plist"))
            .output()
            .await
            .map_err(|error| error.to_string())?;
        capability.status.success() && capability.stdout.trim_ascii() == b"true"
    } else {
        false
    };

    let previously_enabled = engine_choice::is_enabled(root);
    engine_choice::set_enabled(root, enabled)?;
    if !supports_shutdown {
        return Ok(());
    }

    let command = if enabled {
        "--enable-input-source"
    } else {
        "--stop-input-method"
    };
    let result = match tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(helper)
            .arg(command)
            .arg(root)
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => {
            let detail = if output.stderr.trim_ascii().is_empty() {
                &output.stdout
            } else {
                &output.stderr
            };
            Err(format!(
                "macOS 输入法操作失败（{}）{}",
                output.status,
                String::from_utf8_lossy(detail).trim()
            ))
        }
        Ok(Err(error)) => Err(format!("无法启动 macOS 输入法助手：{error}")),
        Err(_) => Err("macOS 输入法操作超时".into()),
    };
    if result.is_err() && enabled && !previously_enabled {
        engine_choice::set_enabled(root, false)?;
    }
    result
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
