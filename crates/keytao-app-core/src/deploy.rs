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
use crate::ime_status::linux::launch_keytao_ime;
#[cfg(target_os = "windows")]
use crate::ime_status::windows::windows_deploy_rime_blocking;
use crate::{Core, CoreError, ime_host::ImeHost};
use serde::Serialize;
#[cfg(target_os = "ios")]
use std::path::Path;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeployRecord {
    pub root: std::path::PathBuf,
    pub scheme_key: Option<String>,
    pub success: bool,
}

pub fn record_result(core: &Core, root: &std::path::Path, success: bool) -> Result<(), CoreError> {
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error)
            if !success && error.kind() == std::io::ErrorKind::NotFound && root.is_absolute() =>
        {
            root.to_path_buf()
        }
        Err(error) => return Err(error.into()),
    };
    core.update_state(|state| {
        state.last_deploy = Some(DeployRecord {
            root: root.clone(),
            scheme_key: state.active_scheme.clone(),
            success,
        });
    })?;
    // Hosts also use this preparation before deploying through a native IME process.
    if !success && root.is_dir() {
        crate::packages::prepare_active_rime_lua(core, &root)?;
    }
    Ok(())
}

pub fn record_android_result(
    core: &Core,
    root: &std::path::Path,
    success: bool,
) -> Result<(), CoreError> {
    record_result(core, root, false)?;
    if !success {
        return Ok(());
    }
    let config = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
        .ok_or_else(|| CoreError::Other("Android RIME 部署缺少方案配置".into()))?;
    let schemas = crate::scheme_files::parse_schema_list(&config);
    crate::upload_bundle::verify_deploy_artifacts(root, Some(&root.join("rime-data")), &schemas)?;
    record_result(core, root, true)
}

pub fn render_user_schema_logos(user_dir: &std::path::Path) {
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

pub fn active_schema_logo_path(user_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let schema_id = keytao_core::deployed_schema_id_from_dir(user_dir)?;
    render_user_schema_logos(user_dir);
    let output = user_dir.canonicalize().ok()?.join(".keytao-logo");
    let path = output.join(format!("{schema_id}.png")).canonicalize().ok()?;
    (path.starts_with(&output) && path.is_file()).then_some(path)
}

#[cfg(target_os = "ios")]
pub(crate) fn ios_app_shared_data_dir(core: &Core, user_root: &Path) -> Option<String> {
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
    pub warning: Option<String>,
}

#[cfg(target_os = "macos")]
fn macos_signing_team_id(output: &str) -> Result<&str, String> {
    let mut teams = output
        .lines()
        .filter_map(|line| line.strip_prefix("TeamIdentifier="));
    let team = teams.next().ok_or("Code signature has no Team ID")?;
    if teams.next().is_some()
        || team.len() != 10
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err("Code signature has an invalid Team ID".into());
    }
    Ok(team)
}

#[cfg(target_os = "macos")]
fn verified_macos_signing_team(executable: &std::path::Path) -> Result<String, String> {
    use std::process::{Command, Stdio};

    let display = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4"])
        .arg(executable)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "Cannot read code signature for {}: {error}",
                executable.display()
            )
        })?;
    if !display.status.success() {
        return Err(format!(
            "Cannot read code signature for {}: {}",
            executable.display(),
            String::from_utf8_lossy(&display.stderr).trim()
        ));
    }
    let metadata = String::from_utf8(display.stderr)
        .map_err(|error| format!("Invalid code signature metadata: {error}"))?;
    let team = macos_signing_team_id(&metadata)?;
    // Bind the reported Team ID to an Apple-issued signing certificate.
    let requirement = format!("anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
    let verify = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "-R", &format!("={requirement}")])
        .arg(executable)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "Cannot verify code signature for {}: {error}",
                executable.display()
            )
        })?;
    if !verify.status.success() {
        return Err(format!(
            "Code signature verification failed for {}: {}",
            executable.display(),
            String::from_utf8_lossy(&verify.stderr).trim()
        ));
    }
    Ok(team.to_owned())
}

#[cfg(target_os = "macos")]
fn verified_macos_deploy_executable() -> Result<std::path::PathBuf, String> {
    let executable =
        std::path::PathBuf::from("/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME");
    if !executable.is_file() {
        return Err(
            "KeyTao macOS input method is not installed; install it before deploying".into(),
        );
    }
    let app = std::env::current_exe()
        .map_err(|error| format!("Cannot determine the App executable: {error}"))?;
    let app_team = verified_macos_signing_team(&app)?;
    let ime_team = verified_macos_signing_team(&executable)?;
    if app_team != ime_team {
        return Err("KeyTao input method signature Team ID does not match the App".into());
    }
    Ok(executable)
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
    crate::remove_filter::ensure_remove_filter(&dest)?;
    crate::ime_settings::prepare_candidate_page_size(&dest)?;
    record_result(core, &dest, false).map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    let user = dest.to_string_lossy().into_owned();
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let shared = crate::ime_status::app_shared_data_dir(core);
    #[cfg(not(target_os = "windows"))]
    let logo_root = dest.clone();

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
    #[cfg(target_os = "linux")]
    let (mut deploy_command, deploy_executable) =
        crate::ime_status::linux::resolve_keytao_ime_command(core);

    #[cfg(target_os = "windows")]
    let deploy_result =
        tokio::task::spawn_blocking(move || windows_deploy_rime_blocking(dest, shared)).await;
    #[cfg(target_os = "macos")]
    let deploy_result = tokio::task::spawn_blocking(move || {
        let executable = verified_macos_deploy_executable()?;
        keytao_core::deploy_child::run_deploy_child(
            &executable,
            std::time::Duration::from_secs(600),
        )?;
        render_user_schema_logos(&logo_root);
        Ok::<(), String>(())
    })
    .await;
    #[cfg(target_os = "linux")]
    let (deploy_result, _deploy_lock) = match tokio::task::spawn_blocking(move || {
        keytao_core::deploy_child::inherit_linux_deploy_lock(&mut deploy_command, &_deploy_lock);
        let result = keytao_core::deploy_child::run_deploy_child_command_with_progress(
            &mut deploy_command,
            std::time::Duration::from_secs(600),
            |_| {},
        )
        .map_err(|error| {
            format!(
                "Cannot deploy with keytao-ime ({deploy_executable}); ensure the Linux input method is installed: {error}"
            )
        })
        .and_then(|()| runtime.init_without_deploy());
        if result.is_ok() {
            render_user_schema_logos(&logo_root);
        }
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

    #[cfg(not(target_os = "windows"))]
    let deploy_result = deploy_result.map(|result| result.map(|()| None::<String>));

    match deploy_result {
        Ok(Ok(warning)) => {
            let root = keytao_core::default_user_data_dir()
                .ok_or("Cannot determine keytao data directory")?;
            record_result(core, &root, true).map_err(|e| e.to_string())?;
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
            if let Some(warning) = &warning {
                core.emit(CoreEvent::DeployProgress(format!(
                    "系统搜索框方案未更新：{warning}"
                )));
            }
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
                warning,
            })
        }
        Ok(Err(e)) => Err(format!("部署失败: {e}")),
        Err(e) => Err(format!("任务错误: {e}")),
    }
}

#[cfg(target_os = "android")]
pub async fn rime_deploy_default(
    core: &Core,
    host: impl ImeHost,
) -> Result<DeployResult, CoreError> {
    let result: Result<_, String> = async {
        let root = host.user_root()?;
        crate::remove_filter::ensure_remove_filter(&root)?;
        crate::ime_settings::prepare_candidate_page_size(&root)?;
        record_result(core, &root, false).map_err(|e| e.to_string())?;
        let result: serde_json::Value = host.deploy_ime_data()?;
        if result["deployed"].as_bool() != Some(true) {
            return Err("Android Rime deployment did not succeed".into());
        }
        record_result(core, &root, true).map_err(|e| e.to_string())?;
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
            warning: None,
        })
    }
    .await;
    result.map_err(CoreError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn signing_team_id_rejects_missing_ambiguous_or_injected_values() {
        assert_eq!(
            macos_signing_team_id("Executable=/example\nTeamIdentifier=AB12CD34EF\n"),
            Ok("AB12CD34EF")
        );
        for output in [
            "",
            "TeamIdentifier=not set\n",
            "TeamIdentifier=\n",
            "TeamIdentifier=AB12CD34E\n",
            "TeamIdentifier=ab12cd34ef\n",
            "TeamIdentifier=AB12CD34EF\nTeamIdentifier=ZZ12CD34EF\n",
            "TeamIdentifier=AB12CD34EF\" or true\n",
        ] {
            assert!(macos_signing_team_id(output).is_err(), "{output:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn signing_verification_rejects_unsigned_executable_without_running_it() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("untrusted-ime");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        assert!(verified_macos_signing_team(&executable).is_err());
    }

    #[test]
    fn active_schema_logo_path_uses_rendered_schema_and_selected_schema() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(
            root.join("build/default.yaml"),
            "schema_list:\n  - schema: xmjd6\n  - schema: custom_schema\n",
        )
        .unwrap();
        for (id, name) in [("xmjd6", "Xingmao"), ("custom_schema", "Custom")] {
            std::fs::write(
                root.join("build").join(format!("{id}.schema.yaml")),
                format!("schema:\n  schema_id: {id}\n  name: {name}\n"),
            )
            .unwrap();
            std::fs::write(
                root.join(format!("{name}.svg")),
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="#84cc16"/></svg>"##,
            )
            .unwrap();
        }
        let output = root.canonicalize().unwrap().join(".keytao-logo");
        assert_eq!(active_schema_logo_path(root), Some(output.join("xmjd6.png")));
        std::fs::write(
            root.join("user.yaml"),
            "var:\n  previously_selected_schema: custom_schema\n",
        )
        .unwrap();
        let selected = output.join("custom_schema.png");
        assert_eq!(active_schema_logo_path(root), Some(selected.clone()));
        assert!(std::fs::read(&selected).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        std::fs::remove_file(selected).unwrap();
        assert!(active_schema_logo_path(root).is_some());
    }

    #[test]
    fn active_schema_logo_path_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("user");
        std::fs::create_dir_all(root.join(".keytao-logo")).unwrap();
        std::fs::write(root.join("outside.png"), b"outside").unwrap();
        std::fs::write(
            root.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: ../outside\n",
        )
        .unwrap();
        assert!(active_schema_logo_path(&root).is_none());
    }
}

#[cfg(target_os = "ios")]
pub async fn rime_deploy_default(
    core: &Core,
    host: impl ImeHost,
) -> Result<DeployResult, CoreError> {
    let result: Result<_, String> = async {
        let dest = host.user_root()?;
        crate::remove_filter::ensure_remove_filter(&dest)?;
        crate::ime_settings::prepare_candidate_page_size(&dest)?;
        record_result(core, &dest, false).map_err(|e| e.to_string())?;
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
                record_result(core, &dest, true).map_err(|e| e.to_string())?;
                core.emit(CoreEvent::DeployProgress("部署完成".into()));
                match write_ios_reload_stamp(&dest) {
                    Ok(stamp) => {
                        let message = format!("已通知 iOS 输入法重载：{}", stamp.display());
                        core.emit(CoreEvent::DeployProgress(message.clone()));
                        Ok(DeployResult {
                            success: true,
                            message,
                            warning: None,
                        })
                    }
                    Err(e) => {
                        let message = format!("部署完成，但 iOS 输入法重载通知失败：{e}");
                        core.emit(CoreEvent::DeployProgress(message.clone()));
                        Ok(DeployResult {
                            success: true,
                            message,
                            warning: None,
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
