#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "ios"
))]
use crate::CoreEvent;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
use crate::ime_paths::*;
#[cfg(target_os = "linux")]
use crate::ime_status::linux::{launch_keytao_ime, linux_app_shared_data_dir};
#[cfg(target_os = "macos")]
use crate::ime_status::macos::macos_app_shared_data_dir;
#[cfg(target_os = "windows")]
use crate::ime_status::windows::{windows_app_shared_data_dir, windows_deploy_rime_blocking};
use crate::{Core, CoreError, ime_host::ImeHost};
use serde::Serialize;
#[cfg(target_os = "ios")]
use std::path::Path;

#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) fn render_user_schema_logos(user_dir: &std::path::Path) {
    let mut schemas = std::collections::BTreeMap::new();
    for dir in [user_dir.join("build"), user_dir.to_path_buf()] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Some(id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".schema.yaml"))
            else {
                continue;
            };
            let schema = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_yaml::from_str::<serde_yaml::Value>(&text).ok());
            let Some(schema) = schema else {
                tracing::warn!("Cannot read schema logo metadata: {}", path.display());
                continue;
            };
            let id = schema["schema"]["schema_id"].as_str().unwrap_or(id);
            let name = schema["schema"]["name"].as_str().unwrap_or("");
            schemas
                .entry(id.to_owned())
                .or_insert_with(|| name.to_owned());
        }
    }
    keytao_schema_logo::render_schema_logos(
        user_dir,
        &schemas.into_iter().collect::<Vec<_>>(),
        192,
    );
}

#[cfg(target_os = "ios")]
fn ios_app_shared_data_dir(core: &Core, user_root: &Path) -> Option<String> {
    let mut candidates = vec![
        user_root.to_path_buf(),
        user_root.join("rime-data"),
        user_root.join("shared"),
    ];

    {
        let resource_dir = core.env().resource_dir.clone();
        candidates.extend([
            resource_dir.join("rime-data"),
            resource_dir.join("runtime").join("rime-data"),
            resource_dir.join("SharedSupport"),
        ]);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.extend([
                exe_dir.join("rime-data"),
                exe_dir.join("runtime").join("rime-data"),
                exe_dir.join("..").join("rime-data"),
            ]);
        }
    }

    candidates
        .into_iter()
        .find(|dir| dir.join("default.yaml").is_file())
        .map(|dir| dir.to_string_lossy().into_owned())
}
#[derive(Serialize, Clone)]
pub struct DeployResult {
    pub success: bool,
    pub message: String,
}
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub async fn rime_deploy_default(
    core: &Core,
    host: impl ImeHost,
    #[cfg(target_os = "windows")] actions: &crate::app_actions::AppActions,
) -> Result<DeployResult, CoreError> {
    let result: Result<_, String> = async {
        #[cfg(target_os = "windows")]
        let _guard = actions.begin_deploy(None)?;
        rime_deploy_default_inner(core, host).await
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub(crate) async fn rime_deploy_default_inner(
    core: &Core,
    host: impl ImeHost,
) -> Result<DeployResult, String> {
    let dest =
        keytao_core::default_user_data_dir().ok_or("Cannot determine keytao data directory")?;
    #[cfg(not(target_os = "windows"))]
    let user = dest.to_string_lossy().into_owned();
    #[cfg(target_os = "windows")]
    let shared =
        windows_app_shared_data_dir(core).unwrap_or_else(keytao_core::default_shared_data_dir);
    #[cfg(target_os = "macos")]
    let shared =
        macos_app_shared_data_dir(core).unwrap_or_else(keytao_core::default_shared_data_dir);
    #[cfg(target_os = "linux")]
    let shared =
        linux_app_shared_data_dir(core).unwrap_or_else(keytao_core::default_shared_data_dir);

    core.emit(CoreEvent::DeployProgress("正在部署 librime...".into()));

    #[cfg(target_os = "linux")]
    let _deploy_lock = tokio::task::spawn_blocking(move || {
        keytao_core::linux_deploy_lock(&dest, std::time::Duration::from_secs(300))?
            .ok_or_else(|| "Timed out waiting 5 minutes for the librime deployment lock".to_owned())
    })
    .await
    .map_err(|error| format!("Deployment lock task failed: {error}"))??;

    // The test-input session must be closed before the deployment, not replaced
    // after it: librime keeps its config and dictionary caches alive as long as
    // a session references them, so deploying underneath a live session would
    // rebuild the artifacts and keep serving the stale ones.
    #[cfg(target_os = "linux")]
    let runtime = { host.suspend_for_deploy(user, shared) };

    #[cfg(target_os = "windows")]
    let deploy_result =
        tokio::task::spawn_blocking(move || windows_deploy_rime_blocking(dest, shared)).await;
    #[cfg(target_os = "macos")]
    let deploy_result =
        tokio::task::spawn_blocking(move || keytao_core::deploy(user, shared)).await;
    #[cfg(target_os = "linux")]
    let (deploy_result, _deploy_lock) = match tokio::task::spawn_blocking(move || {
        let result = runtime.reload();
        (result, _deploy_lock)
    })
    .await
    {
        Ok((result, lock)) => (Ok(result), Some(lock)),
        Err(error) => (Err(error), None),
    };

    // Reopen the test input, on the new schemas after a successful deployment
    // and on the previous ones after a failed one — either way it must not stay
    // closed until the next setup.
    #[cfg(target_os = "linux")]
    {
        if let Err(error) = host.resume_after_deploy() {
            tracing::warn!("test input session unavailable after deploy: {error}");
        }
    }

    match deploy_result {
        Ok(Ok(())) => {
            core.emit(CoreEvent::DeployProgress("部署完成".into()));
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            match write_keytao_ime_reload_stamp() {
                Ok(()) => {
                    core.emit(CoreEvent::DeployProgress("已通知系统输入法重载".into()));
                    tracing::info!("IME reload stamp written after deploy");
                }
                Err(e) => {
                    core.emit(CoreEvent::DeployProgress(format!(
                        "系统输入法重载通知失败：{e}"
                    )));
                    tracing::warn!("IME reload stamp failed after deploy: {e}");
                }
            }
            #[cfg(target_os = "windows")]
            core.emit(CoreEvent::DeployProgress("已通知系统输入法重载".into()));
            #[cfg(target_os = "linux")]
            match launch_keytao_ime(core, &host, true) {
                Ok(status) => {
                    core.emit(CoreEvent::DeployProgress(status.message));
                }
                Err(error) => {
                    core.emit(CoreEvent::DeployProgress(format!(
                        "Linux 输入法启动失败：{error}"
                    )));
                }
            }
            Ok(DeployResult {
                success: true,
                message: "部署成功".into(),
            })
        }
        Ok(Err(e)) => Err(format!("部署失败: {e}")),
        Err(e) => Err(format!("任务错误: {e}")),
    }
}

#[cfg(target_os = "android")]
pub async fn rime_deploy_default(
    _core: &Core,
    host: impl ImeHost,
) -> Result<DeployResult, CoreError> {
    let result: Result<_, String> = async {
        let result: serde_json::Value = host.deploy_ime_data()?;
        // The Kotlin side owns the data directory (app-specific storage), so there is
        // no path constant to fall back to here.
        let schema_name = result["schemaName"].as_str().unwrap_or("KeyTao");
        let message = match result["path"].as_str() {
            Some(path) => format!("Android RIME 已部署 {schema_name}，并通知输入法重载：{path}"),
            None => format!("Android RIME 已部署 {schema_name}，并通知输入法重载"),
        };
        Ok(DeployResult {
            success: true,
            message,
        })
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(target_os = "ios")]
pub async fn rime_deploy_default(
    core: &Core,
    host: impl ImeHost,
) -> Result<DeployResult, CoreError> {
    let result: Result<_, String> = async {
        let dest = host.user_root()?;
        let user = dest.to_string_lossy().into_owned();
        let shared = ios_app_shared_data_dir(core, &dest)
            .unwrap_or_else(keytao_core::default_shared_data_dir);

        core.emit(CoreEvent::DeployProgress("正在部署 librime...".into()));

        let logo_root = dest.clone();
        match tokio::task::spawn_blocking(move || {
            keytao_core::deploy(user, shared)?;
            render_user_schema_logos(&logo_root);
            Ok::<(), String>(())
        })
        .await
        {
            Ok(Ok(())) => {
                core.emit(CoreEvent::DeployProgress("部署完成".into()));
                match write_ios_reload_stamp(&dest) {
                    Ok(stamp) => {
                        let message = format!("已通知 iOS 输入法重载：{}", stamp.display());
                        core.emit(CoreEvent::DeployProgress(message.clone()));
                        Ok(DeployResult {
                            success: true,
                            message,
                        })
                    }
                    Err(e) => {
                        let message = format!("部署完成，但 iOS 输入法重载通知失败：{e}");
                        core.emit(CoreEvent::DeployProgress(message.clone()));
                        Ok(DeployResult {
                            success: true,
                            message,
                        })
                    }
                }
            }
            Ok(Err(e)) => Err(format!("部署失败: {e}")),
            Err(e) => Err(format!("任务错误: {e}")),
        }
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
)))]
pub async fn rime_deploy_default(
    _core: &Core,
    _host: impl ImeHost,
) -> Result<DeployResult, CoreError> {
    let result: Result<_, String> =
        async { Err("librime deployment is not supported on this platform yet".into()) }.await;
    result.map_err(CoreError::Other)
}
