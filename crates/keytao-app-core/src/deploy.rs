use crate::CoreEvent;
#[cfg(target_os = "linux")]
use crate::ime_status::linux::launch_keytao_ime;

use crate::{Core, CoreError, ime_host::ImeHost};
use keytao_core::{DeployProgress, DeployStep, RuntimeDirs, isolated_deploy::PackageDeployTarget};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub fn deploy_shared_data_dir(core: &Core, root: &Path) -> PathBuf {
    #[cfg(target_os = "android")]
    {
        let _ = core;
        root.join("rime-data")
    }
    #[cfg(target_os = "ios")]
    {
        ios_app_shared_data_dir(core, root)
            .unwrap_or_else(keytao_core::default_shared_data_dir)
            .into()
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = root;
        crate::ime_status::app_shared_data_dir(core).into()
    }
}

pub fn emit_legacy_progress(core: &Core, step: DeployStep, message: impl Into<String>) {
    emit_flat_progress(core, &flat_target(core, None), step, message);
}

fn flat_target(core: &Core, root: Option<&Path>) -> PackageDeployTarget {
    let package_key = root
        .and_then(|root| {
            let activations = crate::package_activation::read_all(core, root).ok()?;
            crate::runtime_materialize::in_use_package(root, &activations)
        })
        .or_else(|| core.lock_state().active_scheme.clone())
        .unwrap_or_else(|| "keytao".into());
    let package_name = crate::runtime_materialize::package_name(core, &package_key);
    PackageDeployTarget {
        package_key,
        package_name,
        materialize_error: None,
        staged_inputs: None,
    }
}

fn emit_flat_progress(
    core: &Core,
    target: &PackageDeployTarget,
    step: DeployStep,
    message: impl Into<String>,
) {
    core.emit(CoreEvent::DeployProgress(DeployProgress::flat(
        &target.package_key,
        &target.package_name,
        step,
        message,
    )));
}

#[cfg(target_os = "linux")]
fn report_linux_ime_launch(
    core: &Core,
    host: &impl ImeHost,
    target: &PackageDeployTarget,
    restart: bool,
) {
    let message = match launch_keytao_ime(core, host, restart) {
        Ok(status) => status.message,
        Err(error) => format!("Linux 输入法启动失败：{error}"),
    };
    emit_flat_progress(core, target, DeployStep::Done, message);
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeployRecord {
    pub root: std::path::PathBuf,
    pub scheme_key: Option<String>,
    pub success: bool,
}

pub fn record_result(core: &Core, root: &Path, success: bool) -> Result<(), CoreError> {
    persist_result(core, root, success).map(|_| ())
}

fn persist_result(
    core: &Core,
    root: &Path,
    success: bool,
) -> Result<std::path::PathBuf, CoreError> {
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
            scheme_key: RuntimeDirs::active_package_key(&root).or_else(|| state.active_scheme.clone()),
            success,
        });
    })?;
    Ok(root)
}

#[cfg(test)]
pub fn record_android_result(
    core: &Core,
    root: &std::path::Path,
    success: bool,
) -> Result<(), CoreError> {
    record_result(core, root, false)?;
    if !success {
        return Ok(());
    }
    verify_android_deployment(root)?;
    record_result(core, root, true)
}

#[cfg(target_os = "android")]
pub(crate) const ANDROID_DEPLOY_PLAN_FILE: &str = ".keytao-android-deploy-plan.json";
#[cfg(target_os = "android")]
pub(crate) const ANDROID_DEPLOY_PROGRESS_FILE: &str = ".keytao-android-deploy-progress.jsonl";

#[cfg(target_os = "android")]
pub async fn deploy_with_android_host<F: std::future::Future<Output = String>>(
    core: &Core,
    root: &Path,
    deploy: impl FnOnce() -> F,
) -> Result<(), String> {
    deploy_isolated_android(core, root, IsolatedDeploy::All, deploy).await
}

#[cfg(target_os = "android")]
async fn deploy_isolated_android<F: std::future::Future<Output = String>>(
    core: &Core,
    root: &Path,
    mode: IsolatedDeploy<'_>,
    deploy: impl FnOnce() -> F,
) -> Result<(), String> {
    use std::{
        fs,
        io::{BufRead, BufReader},
        time::Duration,
    };

    crate::package_migration::ensure_before_write(core, root).await.map_err(|error| error.to_string())?;
    let _plan_lock = isolated_plan_lock(root)?;
    let shared = deploy_shared_data_dir(core, root);
    let (targets, _staged_inputs) =
        crate::runtime_materialize::prepare_deploy_targets(core, root, &shared, mode.target())
            .map_err(|error| error.to_string())?;
    if targets.is_empty() {
        return Err("没有可部署的方案包".into());
    }
    if matches!(mode, IsolatedDeploy::Switch(_))
        && switch_cached_package(root, &shared, &targets[0]).await?
    {
        return Ok(());
    }
    let journal = crate::packages::safe_path(root, ANDROID_DEPLOY_PROGRESS_FILE)?;
    keytao_core::write_file_atomic(&journal, b"").map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(fs::File::open(journal).map_err(|error| error.to_string())?);
    let plan = crate::packages::safe_path(root, ANDROID_DEPLOY_PLAN_FILE)?;
    let content = serde_json::to_vec(&targets).map_err(|error| error.to_string())?;
    keytao_core::write_file_atomic(&plan, &content).map_err(|error| error.to_string())?;

    let mut active_step = None;
    let mut completed = 0;
    let mut line = Vec::new();
    let mut drain_progress = || -> Result<(), String> {
        while reader
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?
            > 0
            && line.ends_with(b"\n")
        {
            let progress: DeployProgress =
                serde_json::from_slice(&line).map_err(|error| error.to_string())?;
            line.clear();
            if matches!(
                progress.step,
                DeployStep::Done | DeployStep::Skip | DeployStep::Failed
            ) {
                completed += 1;
                if progress.index == 1 {
                    active_step = Some(progress.step);
                }
            }
            core.emit(CoreEvent::DeployProgress(progress));
        }
        Ok(())
    };
    let result = deploy();
    tokio::pin!(result);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let error = loop {
        tokio::select! {
            result = &mut result => break result,
            _ = tick.tick() => drain_progress()?,
        }
    };
    drain_progress()?;
    let _ = fs::remove_file(plan);
    if mode.publishes_all() && matches!(active_step, Some(DeployStep::Done | DeployStep::Skip)) {
        persist_result(core, root, true).map_err(|error| error.to_string())?;
    }
    if !error.is_empty() {
        return Err(error);
    }
    if completed != targets.len()
        || !matches!(active_step, Some(DeployStep::Done | DeployStep::Skip))
    {
        return Err("Android package deployment did not report all results".into());
    }
    finish_isolated_target(core, root, &shared, mode, &targets[0].package_key).await
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn verify_android_deployment(root: &std::path::Path) -> Result<(), CoreError> {
    let dirs = keytao_core::RuntimeDirs::resolve(root, &root.join("rime-data"))
        .map_err(CoreError::Other)?;
    let config = [
        dirs.staging_dir.join("default.yaml"),
        dirs.user_data_dir.join("default.custom.yaml"),
        dirs.user_data_dir.join("default-custom.yaml"),
    ]
    .into_iter()
    .find_map(|path| std::fs::read_to_string(path).ok())
    .ok_or_else(|| CoreError::Other("Android RIME 部署缺少方案配置".into()))?;
    let schemas = crate::scheme_files::parse_schema_list(&config);
    crate::upload_bundle::verify_deploy_artifacts(root, Some(&dirs.shared_data_dir), &schemas)
}

pub fn render_user_schema_logos(user_dir: &std::path::Path) {
    let Ok(dirs) = keytao_core::RuntimeDirs::resolve(user_dir, user_dir) else {
        return;
    };
    let mut schemas = std::collections::BTreeMap::new();
    for dir in [dirs.staging_dir, dirs.user_data_dir] {
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
    let schema_id = keytao_core::current_schema_id_from_dir(user_dir)?;
    render_user_schema_logos(user_dir);
    let output = user_dir.canonicalize().ok()?.join(".keytao-logo");
    let path = output.join(format!("{schema_id}.png")).canonicalize().ok()?;
    (path.starts_with(&output) && path.is_file()).then_some(path)
}

#[cfg(target_os = "ios")]
pub(crate) fn ios_app_shared_data_dir(core: &Core, user_root: &Path) -> Option<String> {
    let mut candidates = vec![user_root.join("rime-data"), user_root.join("shared")];

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

#[derive(Clone, Copy)]
enum IsolatedDeploy<'a> {
    All,
    Switch(&'a str),
    Refresh(&'a str),
}

impl<'a> IsolatedDeploy<'a> {
    fn target(self) -> Option<&'a str> {
        match self {
            Self::All => None,
            Self::Switch(key) | Self::Refresh(key) => Some(key),
        }
    }

    fn publishes_all(self) -> bool {
        matches!(self, Self::All)
    }
}

pub async fn switch_package<#[cfg(target_os = "android")] F: std::future::Future<Output = String> + Send>(
    core: &Core,
    root: &Path,
    key: &str,
    #[cfg(target_os = "android")] deploy: impl FnOnce() -> F,
) -> Result<(), String> {
    deploy_package_mode(
        core,
        root,
        IsolatedDeploy::Switch(key),
        #[cfg(target_os = "android")]
        deploy,
    )
    .await
}

pub async fn deploy_package<#[cfg(target_os = "android")] F: std::future::Future<Output = String> + Send>(
    core: &Core,
    root: &Path,
    key: &str,
    #[cfg(target_os = "android")] deploy: impl FnOnce() -> F,
) -> Result<(), String> {
    deploy_package_mode(
        core,
        root,
        IsolatedDeploy::Refresh(key),
        #[cfg(target_os = "android")]
        deploy,
    )
    .await
}

async fn deploy_package_mode<#[cfg(target_os = "android")] F: std::future::Future<Output = String> + Send>(
    core: &Core,
    root: &Path,
    mode: IsolatedDeploy<'_>,
    #[cfg(target_os = "android")] deploy: impl FnOnce() -> F,
) -> Result<(), String> {

    #[cfg(target_os = "android")]
    return deploy_isolated_android(core, root, mode, deploy).await;
    #[cfg(target_os = "ios")]
    {
        let shared = deploy_shared_data_dir(core, root);
        return deploy_isolated_in_process_target(core, root, shared, mode)
            .await
            .map(|_| ());
    }
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if keytao_core::default_user_data_dir().as_deref() != Some(root) {
            let shared = deploy_shared_data_dir(core, root);
            return deploy_isolated_in_process_target(core, root, shared, mode)
                .await
                .map(|_| ());
        }
        deploy_isolated_desktop(core, root, mode).await?;
        Ok(())
    }
}

async fn switch_cached_package(
    root: &Path,
    shared: &Path,
    target: &PackageDeployTarget,
) -> Result<bool, String> {
    if let Some(error) = &target.materialize_error {
        return Err(error.clone());
    }
    if target.staged_inputs.is_some() {
        return Ok(false);
    }
    let key = &target.package_key;
    let dirs = RuntimeDirs::for_package(root, shared, key)
        .ok_or_else(|| format!("无法读取方案包：{key}"))?;
    if !keytao_core::package_build_ready(root, &dirs) {
        return Ok(false);
    }
    switch_prepared_package(root, shared, key).await?;
    Ok(true)
}

async fn switch_prepared_package(root: &Path, shared: &Path, key: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            crate::windows_public_schemas::publish_isolated_package(&key, false)
        })
        .await
        .map_err(|error| error.to_string())??;
    }
    keytao_core::switch_ready_package(root, shared, key)?;
    #[cfg(target_os = "windows")]
    crate::windows_public_schemas::activate_cached_isolated_package(key)
        .map_err(|error| format!("方案包已切换，系统搜索框方案未更新：{error}"))?;
    Ok(())
}

async fn finish_isolated_target(
    core: &Core,
    root: &Path,
    shared: &Path,
    mode: IsolatedDeploy<'_>,
    first: &str,
) -> Result<(), String> {
    match mode {
        IsolatedDeploy::All if RuntimeDirs::active_package_key(root).is_none() => {
            switch_prepared_package(root, shared, first).await?;
            persist_result(core, root, true).map_err(|error| error.to_string())?;
            render_user_schema_logos(root);
        }
        IsolatedDeploy::Refresh(key) if RuntimeDirs::active_package_key(root).is_none() => {
            switch_prepared_package(root, shared, key).await?;
            persist_result(core, root, true).map_err(|error| error.to_string())?;
            render_user_schema_logos(root);
        }
        IsolatedDeploy::Switch(key) => switch_prepared_package(root, shared, key).await?,
        #[cfg(not(target_os = "android"))]
        IsolatedDeploy::Refresh(key) => finish_package_refresh(root, key).await?,
        _ => {}
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
async fn finish_package_refresh(root: &Path, key: &str) -> Result<(), String> {
    keytao_core::isolated_deploy::with_isolated_package_lock(root, || {
        if RuntimeDirs::active_package_key(root).as_deref() == Some(key) {
            keytao_core::ReloadStamp::write_package(root, key)?;
        }
        Ok(())
    })?;
    #[cfg(target_os = "windows")]
    if RuntimeDirs::active_package_key(root).as_deref() == Some(key) {
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || {
            crate::windows_public_schemas::publish_isolated_package(&key, true)
        })
        .await
        .map_err(|error| error.to_string())??;
    }
    Ok(())
}

fn package_switch_result() -> DeployResult {
    DeployResult {
        success: true,
        message: "已切换方案包".into(),
        warning: None,
    }
}

#[cfg(target_os = "macos")]
fn macos_signing_team_id(output: &str) -> Result<Option<&str>, String> {
    let mut teams = output
        .lines()
        .filter_map(|line| line.strip_prefix("TeamIdentifier="));
    let team = teams.next().ok_or("Code signature has no Team ID")?;
    if teams.next().is_some() {
        return Err("Code signature has an invalid Team ID".into());
    }
    if team == "not set" {
        return Ok(None);
    }
    if team.len() != 10
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err("Code signature has an invalid Team ID".into());
    }
    Ok(Some(team))
}

#[cfg(target_os = "macos")]
fn verified_macos_signing_team(
    executable: &std::path::Path,
    require_apple_anchor: bool,
) -> Result<Option<String>, String> {
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
    let mut verify = Command::new("/usr/bin/codesign");
    verify.args(["--verify", "--strict"]);
    if let (true, Some(team)) = (require_apple_anchor, team) {
        // Bind the reported Team ID to an Apple-issued signing certificate.
        verify.args([
            "-R",
            &format!("=anchor apple generic and certificate leaf[subject.OU] = \"{team}\""),
        ]);
    }
    let verify = verify
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
    Ok(team.map(str::to_owned))
}

#[cfg(target_os = "macos")]
fn validate_macos_deploy_signature(
    app_team: Option<&str>,
    ime_signature: Result<Option<String>, String>,
) -> Result<(), String> {
    let ime_team = ime_signature
        .map_err(|error| format!("KeyTao input method signature is invalid: {error}"))?;
    if app_team.is_some() && ime_team.as_deref() != app_team {
        return Err("KeyTao input method signature Team ID does not match the App".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn verified_macos_deploy_executable(app: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let executable =
        std::path::PathBuf::from("/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME");
    if !executable.is_file() {
        return Err(
            "KeyTao macOS input method is not installed; install it before deploying".into(),
        );
    }
    let canonical = executable
        .canonicalize()
        .map_err(|error| format!("Cannot resolve KeyTao input method path: {error}"))?;
    if canonical != executable {
        return Err("KeyTao input method path must not contain symlinks".into());
    }
    let app_team = verified_macos_signing_team(app, true)
        .map_err(|error| format!("KeyTao App signature is invalid: {error}"))?;
    let ime_signature = verified_macos_signing_team(&executable, app_team.is_some());
    validate_macos_deploy_signature(app_team.as_deref(), ime_signature)?;
    Ok(executable)
}

/// Read candidates without keeping a desktop librime engine in the App.
pub async fn preview_candidates(
    core: &Core,
    host: impl ImeHost,
    size: Option<u8>,
) -> Result<keytao_core::CandidatePreview, String> {
    if size.is_some_and(|size| !(1..=10).contains(&size)) {
        return Err("Candidate page size must be between 1 and 10".into());
    }
    let root = host.user_root()?;
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    if keytao_core::default_user_data_dir().as_ref() != Some(&root) {
        return Err("Desktop candidate previews require the default user root".into());
    }
    #[cfg(target_os = "linux")]
    let (mut command, _) = crate::ime_status::linux::resolve_keytao_ime_command(core);
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let runtime = {
        #[cfg(target_os = "ios")]
        let shared = ios_app_shared_data_dir(core, &root)
            .unwrap_or_else(keytao_core::default_shared_data_dir);
        #[cfg(target_os = "android")]
        let shared = root.join("rime-data").to_string_lossy().into_owned();
        keytao_core::ImeRuntime::with_dirs(root, shared)
    };
    #[cfg(not(target_os = "linux"))]
    let _ = core;
    tokio::task::spawn_blocking(move || {
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        let request = keytao_core::PreviewRequest { page_size: size };
        #[cfg(target_os = "macos")]
        {
            let app = std::env::current_exe()
                .map_err(|error| format!("Cannot determine the App executable: {error}"))?;
            let executable = verified_macos_deploy_executable(&app)?;
            keytao_core::deploy_child::run_preview_child(
                &executable,
                &request,
                std::time::Duration::from_secs(10),
            )
        }
        #[cfg(target_os = "windows")]
        {
            crate::ime_status::windows::preview_candidates_blocking(&request)
        }
        #[cfg(target_os = "linux")]
        {
            keytao_core::deploy_child::run_preview_child_command(
                &mut command,
                &request,
                std::time::Duration::from_secs(10),
            )
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            runtime.preview_candidates(size)
        }
    })
    .await
    .map_err(|error| format!("Candidate preview task failed: {error}"))?
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
async fn deploy_isolated_desktop(
    core: &Core,
    root: &Path,
    mode: IsolatedDeploy<'_>,
) -> Result<DeployResult, String> {
    #[cfg(not(target_os = "windows"))]
    use keytao_core::deploy_child;
    #[cfg(target_os = "macos")]
    use std::process::Command;
    #[cfg(not(target_os = "windows"))]
    use std::time::Duration;

    crate::package_migration::ensure_before_write(core, root).await.map_err(|error| error.to_string())?;
    let plan_lock = isolated_plan_lock(root)?;
    let shared = deploy_shared_data_dir(core, root);
    let shared_dir = shared.as_path();
    let (targets, staged_inputs) =
        crate::runtime_materialize::prepare_deploy_targets(core, root, shared_dir, mode.target())
            .map_err(|error| error.to_string())?;
    if targets.is_empty() {
        return Err("没有可部署的方案包".into());
    }
    if matches!(mode, IsolatedDeploy::Switch(_))
        && switch_cached_package(root, shared_dir, &targets[0]).await?
    {
        return Ok(package_switch_result());
    }
    #[cfg(target_os = "linux")]
    let linux_lock = {
        let root = root.to_owned();
        tokio::task::spawn_blocking(move || {
            keytao_core::linux_deploy_lock(&root, Duration::from_secs(300))?.ok_or_else(|| {
                "Timed out waiting 5 minutes for the librime deployment lock".to_owned()
            })
        })
        .await
        .map_err(|error| error.to_string())??
    };
    let expected = targets.len();
    let publish_deploy = mode.publishes_all() && RuntimeDirs::active_package_key(root).is_some();
    #[cfg(not(target_os = "windows"))]
    let child_shared = shared.clone();
    let plan = serde_json::to_string(&targets).map_err(|error| error.to_string())?;
    #[cfg(target_os = "linux")]
    let (mut command, _executable) = crate::ime_status::linux::resolve_keytao_ime_command(core);
    #[cfg(target_os = "macos")]
    let mut command = Command::new(verified_macos_deploy_executable(
        &std::env::current_exe().map_err(|error| error.to_string())?,
    )?);

    let (sender, events) = tokio::sync::mpsc::unbounded_channel::<DeployProgress>();
    #[cfg(target_os = "windows")]
    let worker = {
        let root = root.to_owned();
        tokio::task::spawn_blocking(move || {
            let _staged_inputs = staged_inputs;
            let result = crate::ime_status::windows::windows_deploy_packages_blocking(
                &root,
                &plan,
                publish_deploy,
                |progress| {
                    let _ = sender.send(progress.clone());
                },
            );
            (result, plan_lock)
        })
    };
    #[cfg(not(target_os = "windows"))]
    let worker = tokio::task::spawn_blocking(move || {
        let _staged_inputs = staged_inputs;
        command.env(deploy_child::PACKAGE_TARGETS_ENV, plan);
        if !child_shared.as_os_str().is_empty() {
            command.env("KEYTAO_RIME_SHARED_DATA_DIR", child_shared);
        }
        #[cfg(target_os = "linux")]
        deploy_child::inherit_linux_deploy_lock(&mut command, &linux_lock);
        let result = deploy_child::run_deploy_child_command_with_progress(
            &mut command,
            Duration::from_secs(600),
            |progress| {
                let _ = sender.send(progress.clone());
            },
        );
        #[cfg(target_os = "linux")]
        drop(linux_lock);
        (result.map(|()| None), plan_lock)
    });
    let (result, _plan_lock) =
        finish_isolated_deploy(core, root, expected, events, worker, publish_deploy).await?;
    finish_isolated_target(core, root, shared_dir, mode, &targets[0].package_key).await?;
    Ok(result)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "ios"))]
pub async fn deploy_isolated_in_process(
    core: &Core,
    root: &Path,
    shared: std::path::PathBuf,
) -> Result<DeployResult, String> {
    deploy_isolated_in_process_target(core, root, shared, IsolatedDeploy::All).await
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "ios"))]
async fn deploy_isolated_in_process_target(
    core: &Core,
    root: &Path,
    shared: std::path::PathBuf,
    mode: IsolatedDeploy<'_>,
) -> Result<DeployResult, String> {
    crate::package_migration::migrate_if_needed(core, root, &shared).map_err(|error| error.to_string())?;
    let plan_lock = isolated_plan_lock(root)?;
    let (targets, staged_inputs) =
        crate::runtime_materialize::prepare_deploy_targets(core, root, &shared, mode.target())
            .map_err(|error| error.to_string())?;
    if targets.is_empty() {
        return Err("没有可部署的方案包".into());
    }
    if matches!(mode, IsolatedDeploy::Switch(_))
        && switch_cached_package(root, &shared, &targets[0]).await?
    {
        return Ok(package_switch_result());
    }
    let expected = targets.len();
    let first = targets[0].package_key.clone();
    let (sender, events) = tokio::sync::mpsc::unbounded_channel();
    let worker_root = root.to_owned();
    let worker_shared = shared.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _staged_inputs = staged_inputs;
        let result = keytao_core::isolated_deploy::deploy_packages(
            &worker_root,
            &worker_shared,
            None,
            &targets,
            |progress| {
                let _ = sender.send(progress);
            },
        );
        (result.map(|()| None), plan_lock)
    });
    let publish_deploy = mode.publishes_all() && RuntimeDirs::active_package_key(root).is_some();
    let (result, _plan_lock) =
        finish_isolated_deploy(core, root, expected, events, worker, publish_deploy).await?;
    finish_isolated_target(core, root, &shared, mode, &first).await?;
    Ok(result)
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "ios"
))]
async fn finish_isolated_deploy(
    core: &Core,
    root: &Path,
    expected: usize,
    mut events: tokio::sync::mpsc::UnboundedReceiver<DeployProgress>,
    worker: tokio::task::JoinHandle<(Result<Option<String>, String>, std::fs::File)>,
    publish_deploy: bool,
) -> Result<(DeployResult, std::fs::File), String> {
    let mut active_step = None;
    let mut completed = 0;
    let mut reload_warning = None;
    while let Some(progress) = events.recv().await {
        if progress.legacy_message.is_none()
            && matches!(
                progress.step,
                DeployStep::Done | DeployStep::Skip | DeployStep::Failed
            )
        {
            completed += 1;
            if progress.index == 1 {
                active_step = Some(progress.step);
                if publish_deploy
                    && !cfg!(target_os = "windows")
                    && progress.step == DeployStep::Done
                {
                    reload_warning = keytao_core::ReloadStamp::write(root)
                        .err()
                        .map(|error| format!("系统输入法重载通知失败：{error}"));
                }
            }
        }
        core.emit(CoreEvent::DeployProgress(progress));
    }
    let (result, plan_lock) = worker.await.map_err(|error| error.to_string())?;
    if publish_deploy && matches!(active_step, Some(DeployStep::Done | DeployStep::Skip)) {
        persist_result(core, root, true).map_err(|error| error.to_string())?;
        if active_step == Some(DeployStep::Done) {
            render_user_schema_logos(root);
        }
    }
    let worker_warning = result.map_err(|error| format!("部署失败: {error}"))?;
    if completed != expected || !matches!(active_step, Some(DeployStep::Done | DeployStep::Skip)) {
        return Err("方案包部署未返回完整结果".into());
    }
    Ok((
        DeployResult {
            success: true,
            message: "部署成功".into(),
            warning: reload_warning.or(worker_warning),
        },
        plan_lock,
    ))
}

fn isolated_plan_lock(root: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let lock = crate::packages::safe_path(root, ".keytao-deploy-plan.lock")?;
    let lock = std::fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock)
        .map_err(|error| error.to_string())?;
    lock.try_lock()
        .map_err(|error| format!("Cannot acquire the package deployment plan lock: {error}"))?;
    Ok(lock)
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
    let result = deploy_isolated_desktop(core, &dest, IsolatedDeploy::All).await?;
    #[cfg(target_os = "linux")]
    report_linux_ime_launch(core, &host, &flat_target(core, None), result.warning.is_some());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn switching_to_cached_package_does_not_start_deployment() {
        use std::{
            fs,
            io::{Cursor, Write},
            sync::{Arc, Mutex},
        };

        #[derive(Default)]
        struct Steps(Mutex<Vec<DeployStep>>);
        impl crate::EventSink for Steps {
            fn emit(&self, event: CoreEvent) {
                if let CoreEvent::DeployProgress(progress) = event {
                    self.0.lock().unwrap().push(progress.step);
                }
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let shared = root.join("rime-data");
        fs::create_dir(&shared).unwrap();
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (file, content) in [
            ("default.yaml", "config_version: '1'\nschema_list: [{schema: keytao}]\n"),
            ("keytao.schema.yaml", "schema: {schema_id: keytao, name: Fixture, version: '1'}\nengine:\n  processors: [speller, selector, express_editor]\n  segmentors: [abc_segmentor, fallback_segmentor]\n  translators: [table_translator]\nspeller: {alphabet: abcde}\ntranslator: {dictionary: keytao, enable_user_dict: false}\n"),
            ("keytao.dict.yaml", "---\nname: keytao\nversion: '1'\nsort: by_weight\nuse_preset_vocabulary: false\n...\n甲\ta\t100\n"),
        ] {
            archive.start_file(file, zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        let archive = archive.finish().unwrap().into_inner();
        let steps = Arc::new(Steps::default());
        let core = Core::new(
            crate::AppEnv {
                data_dir: root.join("data"),
                cache_dir: root.join("cache"),
                resource_dir: root.to_path_buf(),
                platform: crate::Platform::MacOs,
                app_version: "fixture".into(),
            },
            steps.clone(),
        ).unwrap();
        let key = "local-switch";
        let meta = crate::packages::PackageMeta {
            scheme_key: key.into(),
            engine: Default::default(),
            version: None,
            installed_at_unix: 1,
            source: "local".into(),
            display_name: None,
            selection_base_version: None,
            manifest: Default::default(),
            zip_sha256: crate::packages::sha256_reader(archive.as_slice()).unwrap(),
        };
        crate::package_store::PackageStore::new(&core)
            .write_archive(Cursor::new(archive), &meta, None)
            .unwrap();
        core.lock_state().installed_packages.insert(
            key.into(),
            crate::state::InstalledPackage { version: None },
        );
        deploy_isolated_in_process_target(&core, root, shared.clone(), IsolatedDeploy::Switch(key))
            .await
            .unwrap();
        let resolved = RuntimeDirs::for_package(root, &shared, key).unwrap();
        assert_eq!(resolved.shared_data_dir, root.join("scheme-packages/local/local-switch/files"));
        assert!(steps.0.lock().unwrap().contains(&DeployStep::Compile));
        steps.0.lock().unwrap().clear();
        fs::write(
            keytao_core::package_store::active_manifest(root),
            br#"{"active_package":"builtin/other"}"#,
        ).unwrap();

        deploy_isolated_in_process_target(&core, root, shared, IsolatedDeploy::Switch(key))
            .await
            .unwrap();
        assert_eq!(RuntimeDirs::active_package_key(root).as_deref(), Some(key));
        assert!(steps.0.lock().unwrap().is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn signing_team_id_rejects_missing_ambiguous_or_injected_values() {
        assert_eq!(
            macos_signing_team_id("Executable=/example\nTeamIdentifier=AB12CD34EF\n"),
            Ok(Some("AB12CD34EF"))
        );
        for output in [
            "",
            "TeamIdentifier=\n",
            "TeamIdentifier=not Set\n",
            "TeamIdentifier=AB12CD34E\n",
            "TeamIdentifier=ab12cd34ef\n",
            "TeamIdentifier=AB12CD34EF\nTeamIdentifier=ZZ12CD34EF\n",
            "TeamIdentifier=not set\nTeamIdentifier=AB12CD34EF\n",
            "TeamIdentifier=AB12CD34EF\" or true\n",
        ] {
            assert!(macos_signing_team_id(output).is_err(), "{output:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn signing_team_id_parses_not_set_as_no_team() {
        assert_eq!(macos_signing_team_id("TeamIdentifier=not set\n"), Ok(None));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_deploy_signature_policy() {
        for (app_team, ime_team, allowed) in [
            (None, None, true),
            (None, Some("AB12CD34EF"), true),
            (Some("AB12CD34EF"), Some("AB12CD34EF"), true),
            (Some("AB12CD34EF"), None, false),
            (Some("AB12CD34EF"), Some("ZZ12CD34EF"), false),
        ] {
            assert_eq!(
                validate_macos_deploy_signature(app_team, Ok(ime_team.map(str::to_owned))).is_ok(),
                allowed,
                "App: {app_team:?}, IME: {ime_team:?}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_deploy_signature_policy_rejects_failed_verification() {
        assert_eq!(
            validate_macos_deploy_signature(None, Err("strict verification failed".into())),
            Err("KeyTao input method signature is invalid: strict verification failed".into())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn signing_verification_rejects_unsigned_executable_without_running_it() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("untrusted-ime");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        assert!(verified_macos_signing_team(&executable, true).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Read-only signature check requiring the installed KeyTao App and input method"]
    fn installed_macos_deploy_signature_check() {
        let executable = verified_macos_deploy_executable(std::path::Path::new(
            "/Applications/KeyTao.app/Contents/MacOS/KeyTao",
        ))
        .expect("Installed KeyTao deploy signature check failed");
        assert_eq!(
            executable,
            std::path::Path::new("/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME")
        );
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
    let root = host.user_root().map_err(CoreError::Other)?;
    let shared = deploy_shared_data_dir(core, &root);
    deploy_isolated_in_process(core, &root, shared).await.map_err(CoreError::Other)
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
