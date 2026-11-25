use crate::CoreEvent;
#[cfg(target_os = "linux")]
use crate::ime_status::linux::launch_keytao_ime;

use crate::{Core, CoreError, ime_host::ImeHost};
use keytao_core::{
    DeployProgress, DeployStep, RuntimeDirs, isolated_deploy::PackageDeployTarget,
    runtime_log::AppOperation,
};
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
    deploy: impl FnMut() -> F,
) -> Result<(), String> {
    deploy_isolated_android(core, root, IsolatedDeploy::All, deploy).await
}

#[cfg(target_os = "android")]
async fn deploy_isolated_android<F: std::future::Future<Output = String>>(
    core: &Core,
    root: &Path,
    mode: IsolatedDeploy<'_>,
    mut deploy: impl FnMut() -> F,
) -> Result<(), String> {
    let _operation = AppOperation::start("deploy");
    let active_before = RuntimeDirs::active_package_key(root);
    match deploy_isolated_android_once(core, root, mode, &mut deploy).await {
        Ok(()) => Ok(()),
        Err(failure) if !failure.failed_packages.is_empty() => {
            if let Err(repair_err) =
                repair_failed_packages(core, root, &failure.failed_packages).await
            {
                tracing::warn!(%repair_err, "auto-repair after Android deploy failure did not complete");
                return Err(failure.error);
            }
            let retry_mode = retry_mode_after_repair(mode);
            deploy_isolated_android_once(core, root, retry_mode, &mut deploy)
                .await
                .map_err(|retry_failure| retry_failure.error)?;
            restore_previous_active_if_needed(core, root, retry_mode, active_before).await?;
            Ok(())
        }
        Err(failure) => Err(failure.error),
    }
}

#[cfg(target_os = "android")]
async fn deploy_isolated_android_once<F: std::future::Future<Output = String>>(
    core: &Core,
    root: &Path,
    mode: IsolatedDeploy<'_>,
    deploy: &mut impl FnMut() -> F,
) -> Result<(), DeployAttemptFailure> {
    use std::{
        fs,
        io::{BufRead, BufReader},
        time::Duration,
    };

    crate::package_migration::ensure_before_write(core, root)
        .await
        .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
    let _plan_lock = isolated_plan_lock(root)?;
    let shared = deploy_shared_data_dir(core, root);
    let (targets, _staged_inputs) =
        crate::runtime_materialize::prepare_deploy_targets(core, root, &shared, mode.target())
            .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
    if targets.is_empty() {
        if matches!(mode, IsolatedDeploy::All) && active_uses_libjd(core, root) {
            keytao_core::ReloadStamp::write(root)?;
            return Ok(());
        }
        return Err("没有可部署的方案包".to_owned().into());
    }
    if let IsolatedDeploy::Switch { schema_id, .. } = mode {
        match switch_cached_package(core, root, &shared, &targets[0], schema_id).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                return Err(DeployAttemptFailure {
                    error,
                    failed_packages: vec![targets[0].package_key.clone()],
                });
            }
        }
    }
    let journal = crate::packages::safe_path(root, ANDROID_DEPLOY_PROGRESS_FILE)?;
    keytao_core::write_file_atomic(&journal, b"").map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(fs::File::open(journal).map_err(|error| error.to_string())?);
    let plan = crate::packages::safe_path(root, ANDROID_DEPLOY_PLAN_FILE)?;
    let content = serde_json::to_vec(&targets).map_err(|error| error.to_string())?;
    keytao_core::write_file_atomic(&plan, &content).map_err(|error| error.to_string())?;

    let mut active_step = None;
    let mut completed = 0;
    let mut failed_packages = Vec::new();
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
                if progress.step == DeployStep::Failed {
                    push_unique_failed(&mut failed_packages, progress.package_key.clone());
                }
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
    let fallback_failed = |mut list: Vec<String>| {
        if list.is_empty() {
            for target in &targets {
                if target.materialize_error.is_some() {
                    push_unique_failed(&mut list, target.package_key.clone());
                }
            }
        }
        if list.is_empty() && targets.len() == 1 {
            push_unique_failed(&mut list, targets[0].package_key.clone());
        }
        list
    };
    if !error.is_empty() {
        return Err(DeployAttemptFailure {
            error,
            failed_packages: fallback_failed(failed_packages),
        });
    }
    if completed != targets.len()
        || !matches!(active_step, Some(DeployStep::Done | DeployStep::Skip))
    {
        return Err(DeployAttemptFailure {
            error: "Android package deployment did not report all results".into(),
            failed_packages: fallback_failed(failed_packages),
        });
    }
    let first = targets[0].package_key.clone();
    finish_isolated_target(core, root, &shared, mode, &first)
        .await
        .map_err(|error| DeployAttemptFailure {
            error,
            failed_packages: vec![first],
        })
}

#[cfg(any(all(target_os = "android", debug_assertions), test))]
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
    Switch {
        key: &'a str,
        schema_id: Option<&'a str>,
    },
    Refresh(&'a str),
}

impl<'a> IsolatedDeploy<'a> {
    fn target(self) -> Option<&'a str> {
        match self {
            Self::All => None,
            Self::Switch { key, .. } | Self::Refresh(key) => Some(key),
        }
    }

    fn publishes_all(self) -> bool {
        matches!(self, Self::All)
    }
}

pub(crate) fn active_uses_libjd(core: &Core, root: &Path) -> bool {
    RuntimeDirs::active_package_key(root).is_some_and(|key| {
        keytao_core::engine_choice::read(root, Some(&key))
            == keytao_core::engine_choice::PackageEngine::LibjdEngine
            && crate::libjd::imported_package_ready(core, &key)
    })
}

pub fn ensure_not_using_libjd(core: &Core, root: &Path) -> Result<(), CoreError> {
    if active_uses_libjd(core, root) {
        return Err(CoreError::Other("此功能适用于 Rime 引擎".into()));
    }
    Ok(())
}

struct DeployAttemptFailure {
    error: String,
    failed_packages: Vec<String>,
}

impl From<String> for DeployAttemptFailure {
    fn from(error: String) -> Self {
        Self {
            error,
            failed_packages: Vec::new(),
        }
    }
}

fn push_unique_failed(list: &mut Vec<String>, key: impl Into<String>) {
    let key = key.into();
    if !key.is_empty() && !list.contains(&key) {
        list.push(key);
    }
}

fn retry_mode_after_repair<'a>(mode: IsolatedDeploy<'a>) -> IsolatedDeploy<'a> {
    match mode {
        IsolatedDeploy::Switch {
            key: "builtin/keytao",
            schema_id,
        } => IsolatedDeploy::Switch {
            key: "keytao",
            schema_id,
        },
        IsolatedDeploy::Refresh("builtin/keytao") => IsolatedDeploy::Refresh("keytao"),
        other => other,
    }
}

async fn restore_previous_active_if_needed(
    core: &Core,
    root: &Path,
    mode: IsolatedDeploy<'_>,
    active_before: Option<String>,
) -> Result<(), String> {
    if !matches!(mode, IsolatedDeploy::All) {
        return Ok(());
    }
    let Some(previous) = active_before else {
        return Ok(());
    };
    let normalized = previous.strip_prefix("builtin/").unwrap_or(&previous);
    if RuntimeDirs::active_package_key(root).as_deref() == Some(normalized) {
        return Ok(());
    }
    let shared = deploy_shared_data_dir(core, root);
    if RuntimeDirs::for_package(root, &shared, normalized)
        .is_some_and(|dirs| keytao_core::package_build_ready(root, &dirs))
    {
        switch_prepared_package(
            core, root, &shared, Some(&core.env().data_dir), normalized, None,
        )
        .await?;
        persist_result(core, root, true).map_err(|error| error.to_string())?;
        render_user_schema_logos(root);
    }
    Ok(())
}

async fn repair_failed_packages(
    core: &Core,
    root: &Path,
    failed_packages: &[String],
) -> Result<Option<String>, String> {
    let mut repaired = Vec::new();
    for key in failed_packages {
        let name = crate::runtime_materialize::package_name(core, key);
        core.emit(CoreEvent::InstallProgress(crate::events::InstallProgress {
            stage: "repairing".into(),
            percent: 10,
            message: format!("检测到 {name} 部署异常，正在重新准备方案来源…"),
        }));
        crate::packages::repair_package_source_for_redeploy(core, root, key)
            .await
            .map_err(|error| error.to_string())?;
        repaired.push(format!("已重新部署方案 {name}"));
    }
    Ok((!repaired.is_empty()).then(|| repaired.join("\n")))
}

pub async fn switch_package<#[cfg(target_os = "android")] F: std::future::Future<Output = String> + Send>(
    core: &Core,
    root: &Path,
    key: &str,
    #[cfg(target_os = "android")] deploy: impl FnMut() -> F,
) -> Result<(), String> {
    switch_package_with_schema(
        core,
        root,
        key,
        None,
        #[cfg(target_os = "android")]
        deploy,
    )
    .await
}

pub async fn switch_package_with_schema<#[cfg(target_os = "android")] F: std::future::Future<Output = String> + Send>(
    core: &Core,
    root: &Path,
    key: &str,
    schema_id: Option<&str>,
    #[cfg(target_os = "android")] deploy: impl FnMut() -> F,
) -> Result<(), String> {
    if crate::libjd::input_engine(root) == keytao_core::engine_choice::PackageEngine::LibjdEngine {
        let schema = keytao_core::libjd::imported_dictionary(&core.env().data_dir, key)
            .map(|(_, schema)| schema.id);
        if !schema.is_some_and(|schema| schema_id.is_none_or(|id| id == schema)) {
            return Err("请先转换此方案的词库".into());
        }
        crate::package_migration::ensure_before_write(core, root)
            .await
            .map_err(|error| error.to_string())?;
        let shared = deploy_shared_data_dir(core, root);
        return switch_prepared_package(core, root, &shared, Some(&core.env().data_dir), key, schema_id)
            .await;
    }
    deploy_package_mode(
        core,
        root,
        IsolatedDeploy::Switch { key, schema_id },
        #[cfg(target_os = "android")]
        deploy,
    )
    .await
}

pub async fn deploy_package<#[cfg(target_os = "android")] F: std::future::Future<Output = String> + Send>(
    core: &Core,
    root: &Path,
    key: &str,
    #[cfg(target_os = "android")] deploy: impl FnMut() -> F,
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
    #[cfg(target_os = "android")] deploy: impl FnMut() -> F,
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
    core: &Core,
    root: &Path,
    shared: &Path,
    target: &PackageDeployTarget,
    schema_id: Option<&str>,
) -> Result<bool, String> {
    if let Some(error) = &target.materialize_error {
        return Err(error.clone());
    }
    if target.staged_inputs.is_some() {
        return Ok(false);
    }
    let key = &target.package_key;
    let dirs = RuntimeDirs::for_package(root, shared, key)
        .ok_or_else(|| format!("无法读取方案包：{}", target.package_name))?;
    if !keytao_core::isolated_deploy::rime_package_build_ready(root, &dirs) {
        return Ok(false);
    }
    switch_prepared_package(core, root, shared, None, key, schema_id).await?;
    Ok(true)
}

async fn switch_prepared_package(
    core: &Core,
    root: &Path,
    shared: &Path,
    data_root: Option<&Path>,
    key: &str,
    schema_id: Option<&str>,
) -> Result<(), String> {
    let _operation = AppOperation::start("activate_scheme_package");
    #[cfg(target_os = "windows")]
    let public = if crate::windows_public_schemas::is_default_user_root(root) {
        // Prepare the snapshot before the private reload needs the Windows engine lock.
        Some(publish_windows_search_package(key, false).await)
    } else {
        None
    };
    keytao_core::isolated_deploy::switch_ready_package_in_store(
        root,
        shared,
        data_root,
        key,
        schema_id,
    )?;
    #[cfg(target_os = "windows")]
    if let Some(result) = public
        && let Err(error) = result
            .and_then(|()| crate::windows_public_schemas::activate_cached_isolated_package(key))
    {
        report_windows_search_warning(core, &error);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = core;
    Ok(())
}

async fn finish_isolated_target(
    core: &Core,
    root: &Path,
    shared: &Path,
    mode: IsolatedDeploy<'_>,
    first: &str,
) -> Result<(), String> {
    let data_root = Some(core.env().data_dir.as_path());
    match mode {
        IsolatedDeploy::All if RuntimeDirs::active_package_key(root).is_none() => {
            switch_prepared_package(core, root, shared, data_root, first, None).await?;
            persist_result(core, root, true).map_err(|error| error.to_string())?;
            render_user_schema_logos(root);
        }
        IsolatedDeploy::Refresh(key) if RuntimeDirs::active_package_key(root).is_none() => {
            switch_prepared_package(core, root, shared, data_root, key, None).await?;
            persist_result(core, root, true).map_err(|error| error.to_string())?;
            render_user_schema_logos(root);
        }
        IsolatedDeploy::Switch { key, schema_id } => {
            switch_prepared_package(core, root, shared, data_root, key, schema_id).await?
        }
        #[cfg(not(target_os = "android"))]
        IsolatedDeploy::Refresh(key) => finish_package_refresh(core, root, key).await?,
        _ => {}
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
async fn finish_package_refresh(core: &Core, root: &Path, key: &str) -> Result<(), String> {
    keytao_core::isolated_deploy::with_isolated_package_lock(root, || {
        if RuntimeDirs::active_package_key(root).as_deref() == Some(key) {
            keytao_core::ReloadStamp::write_package(root, key)?;
        }
        Ok(())
    })?;
    #[cfg(target_os = "windows")]
    if crate::windows_public_schemas::is_default_user_root(root)
        && RuntimeDirs::active_package_key(root).as_deref() == Some(key)
        && let Err(error) = publish_windows_search_package(key, true).await
    {
        report_windows_search_warning(core, &error);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = core;
    Ok(())
}

#[cfg(target_os = "windows")]
async fn publish_windows_search_package(key: &str, activate: bool) -> Result<(), String> {
    let key = key.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::windows_public_schemas::publish_isolated_package(&key, activate)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(target_os = "windows")]
fn report_windows_search_warning(core: &Core, error: &str) {
    let warning = crate::windows_public_schemas::public_warning("系统搜索框方案未更新", error);
    emit_legacy_progress(core, DeployStep::Done, warning);
}

#[cfg(not(target_os = "android"))]
fn package_switch_result(root: &Path, shared: &Path) -> DeployResult {
    DeployResult {
        success: true,
        message: "已切换方案包".into(),
        warning: keytao_core::deployment_warning_with_shared(root, shared),
    }
}

#[cfg(not(target_os = "android"))]
fn merge_warnings(warnings: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let joined = warnings
        .into_iter()
        .flatten()
        .map(|w| w.trim().to_owned())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!joined.is_empty()).then_some(joined)
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
#[derive(PartialEq, Eq)]
struct MacosExecutableStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(target_os = "macos")]
impl MacosExecutableStamp {
    fn read(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("Cannot inspect {}: {error}", path.display()))?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[cfg(target_os = "macos")]
fn verify_macos_deploy_signatures(app: &Path, ime: &Path) -> Result<(), String> {
    use std::{
        collections::HashMap,
        sync::{LazyLock, Mutex},
    };

    type Signatures = HashMap<(PathBuf, PathBuf), [MacosExecutableStamp; 2]>;
    static VERIFIED: LazyLock<Mutex<Signatures>> = LazyLock::new(Mutex::default);

    let paths = (app.to_owned(), ime.to_owned());
    let stamps = || {
        Ok::<_, String>([
            MacosExecutableStamp::read(app)?,
            MacosExecutableStamp::read(ime)?,
        ])
    };
    let mut verified = VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = stamps()?;
    if verified.get(&paths) == Some(&before) {
        return Ok(());
    }
    verified.remove(&paths);
    let app_team = verified_macos_signing_team(app, true)
        .map_err(|error| format!("KeyTao App signature is invalid: {error}"))?;
    let ime_signature = verified_macos_signing_team(ime, app_team.is_some());
    validate_macos_deploy_signature(app_team.as_deref(), ime_signature)?;
    if stamps()? != before {
        return Err("KeyTao executables changed during signature verification".into());
    }
    verified.insert(paths, before);
    Ok(())
}

#[cfg(target_os = "macos")]
fn verified_macos_deploy_executable(app: &Path) -> Result<PathBuf, String> {
    let executable = PathBuf::from("/Library/Input Methods/KeyTao.app/Contents/MacOS/KeyTaoIME");
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
    verify_macos_deploy_signatures(app, &executable)?;
    Ok(executable)
}

/// Read candidates without keeping an input engine in the App.
pub async fn preview_candidates(
    core: &Core,
    host: impl ImeHost,
    size: Option<u8>,
    #[cfg(target_os = "macos")] install_lock: &'static tokio::sync::Mutex<()>,
) -> Result<keytao_core::CandidatePreview, String> {
    let _operation = AppOperation::start("preview_candidates_native");
    if size.is_some_and(|size| !(1..=10).contains(&size)) {
        return Err("Candidate page size must be between 1 and 10".into());
    }
    #[cfg(target_os = "macos")]
    let install_guard = install_lock.lock().await;
    let root = host.user_root()?;
    if !keytao_core::engine_choice::is_enabled(&root) {
        return Err("算法引擎已关闭".into());
    }
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    if keytao_core::default_user_data_dir().as_ref() != Some(&root) {
        return Err("Desktop candidate previews require the default user root".into());
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    if crate::libjd::input_engine(&root) == keytao_core::engine_choice::PackageEngine::LibjdEngine {
        use keytao_core::{jd_backend::JdBackend, libjd};

        let key = keytao_core::engine_choice::active_package(&root)
            .ok_or("no active ime-jd scheme")?;
        let (dictionary, schema) = libjd::imported_dictionary(&core.env().data_dir, &key)
            .ok_or("no converted ime-jd dictionary")?;
        let library = libjd::bundled_engine_library().ok_or("bundled ime-jd engine is missing")?;
        let page_size = size
            .or_else(|| crate::ime_settings::read_candidate_page_size(&root))
            .map_or(keytao_core::jd_backend::DEFAULT_PAGE_SIZE, usize::from);
        return tokio::task::spawn_blocking(move || {
            // Load the App's own bundled library, which matches its architecture.
            let backend =
                unsafe { JdBackend::load_dictionary(&library, &dictionary, page_size, schema) }?;
            Ok(backend.preview_candidates())
        })
        .await
        .map_err(|error| format!("Candidate preview task failed: {error}"))?;
    }
    #[cfg(target_os = "macos")]
    drop(install_guard);
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
            keytao_core::deploy_child::run_preview_child_command_with_snapshot_lock(
                &mut std::process::Command::new(executable),
                &request,
                std::time::Duration::from_secs(10),
                || {
                    let guard = install_lock.blocking_lock();
                    if !keytao_core::engine_choice::is_enabled(&root)
                        || crate::libjd::input_engine(&root)
                            == keytao_core::engine_choice::PackageEngine::LibjdEngine
                    {
                        return Err("Candidate preview engine changed before reading inputs".into());
                    }
                    Ok(guard)
                },
                true,
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
    let _operation = AppOperation::start("deploy");
    let active_before = RuntimeDirs::active_package_key(root);
    match deploy_isolated_desktop_once(core, root, mode).await {
        Ok(result) => Ok(result),
        Err(failure) if !failure.failed_packages.is_empty() => {
            let notice = match repair_failed_packages(core, root, &failure.failed_packages).await {
                Ok(notice) => notice,
                Err(repair_err) => {
                    tracing::warn!(%repair_err, "auto-repair after desktop deploy failure did not complete");
                    return Err(failure.error);
                }
            };
            let retry_mode = retry_mode_after_repair(mode);
            let mut result = deploy_isolated_desktop_once(core, root, retry_mode)
                .await
                .map_err(|retry_failure| retry_failure.error)?;
            restore_previous_active_if_needed(core, root, retry_mode, active_before).await?;
            result.warning = merge_warnings([notice, result.warning]);
            Ok(result)
        }
        Err(failure) => Err(failure.error),
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
async fn deploy_isolated_desktop_once(
    core: &Core,
    root: &Path,
    mode: IsolatedDeploy<'_>,
) -> Result<DeployResult, DeployAttemptFailure> {
    #[cfg(not(target_os = "windows"))]
    use keytao_core::deploy_child;
    #[cfg(target_os = "macos")]
    use std::process::Command;
    #[cfg(not(target_os = "windows"))]
    use std::time::Duration;

    crate::package_migration::ensure_before_write(core, root)
        .await
        .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
    let plan_lock = isolated_plan_lock(root)?;
    let shared = deploy_shared_data_dir(core, root);
    let shared_dir = shared.as_path();
    let (targets, staged_inputs) =
        crate::runtime_materialize::prepare_deploy_targets(core, root, shared_dir, mode.target())
            .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
    if targets.is_empty() {
        if matches!(mode, IsolatedDeploy::All) && active_uses_libjd(core, root) {
            keytao_core::ReloadStamp::write(root)?;
            return Ok(DeployResult {
                success: true,
                message: "已更新 ime-jd 配置".into(),
                warning: None,
            });
        }
        return Err("没有可部署的方案包".to_owned().into());
    }
    if let IsolatedDeploy::Switch { schema_id, .. } = mode {
        match switch_cached_package(core, root, shared_dir, &targets[0], schema_id).await {
            Ok(true) => return Ok(package_switch_result(root, shared_dir)),
            Ok(false) => {}
            Err(error) => {
                return Err(DeployAttemptFailure {
                    error,
                    failed_packages: vec![targets[0].package_key.clone()],
                });
            }
        }
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
        .map_err(|error| DeployAttemptFailure::from(error.to_string()))??
    };
    let expected = targets.len();
    let first = targets[0].package_key.clone();
    let materialize_failed: Vec<String> = targets
        .iter()
        .filter(|target| target.materialize_error.is_some())
        .map(|target| target.package_key.clone())
        .collect();
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
    let (mut result, _plan_lock) = finish_isolated_deploy(
        core,
        root,
        expected,
        events,
        worker,
        publish_deploy,
        &materialize_failed,
        (expected == 1).then_some(first.as_str()),
    )
    .await?;
    finish_isolated_target(core, root, shared_dir, mode, &first)
        .await
        .map_err(|error| DeployAttemptFailure {
            error,
            failed_packages: vec![first],
        })?;
    result.warning = merge_warnings([
        keytao_core::deployment_warning_with_shared(root, shared_dir),
        result.warning,
    ]);
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
    let _operation = AppOperation::start("deploy");
    deploy_isolated_in_process_with_repair(core, root, shared, mode, |failed| async move {
        repair_failed_packages(core, root, &failed).await
    })
    .await
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "ios"))]
async fn deploy_isolated_in_process_with_repair<Repair, RepairFut>(
    core: &Core,
    root: &Path,
    shared: std::path::PathBuf,
    mode: IsolatedDeploy<'_>,
    repair: Repair,
) -> Result<DeployResult, String>
where
    Repair: FnOnce(Vec<String>) -> RepairFut,
    RepairFut: std::future::Future<Output = Result<Option<String>, String>>,
{
    let active_before = RuntimeDirs::active_package_key(root);
    match deploy_isolated_in_process_once(core, root, shared.clone(), mode).await {
        Ok(result) => Ok(result),
        Err(failure) if !failure.failed_packages.is_empty() => {
            let notice = match repair(failure.failed_packages).await {
                Ok(notice) => notice,
                Err(repair_err) => {
                    tracing::warn!(%repair_err, "auto-repair after in-process deploy failure did not complete");
                    return Err(failure.error);
                }
            };
            let retry_mode = retry_mode_after_repair(mode);
            let mut result = deploy_isolated_in_process_once(core, root, shared.clone(), retry_mode)
                .await
                .map_err(|retry_failure| retry_failure.error)?;
            restore_previous_active_if_needed(core, root, retry_mode, active_before).await?;
            result.warning = merge_warnings([notice, result.warning]);
            Ok(result)
        }
        Err(failure) => Err(failure.error),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "ios"))]
async fn deploy_isolated_in_process_once(
    core: &Core,
    root: &Path,
    shared: std::path::PathBuf,
    mode: IsolatedDeploy<'_>,
) -> Result<DeployResult, DeployAttemptFailure> {
    crate::package_migration::migrate_if_needed(core, root, &shared)
        .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
    let plan_lock = isolated_plan_lock(root)?;
    let (targets, staged_inputs) =
        crate::runtime_materialize::prepare_deploy_targets(core, root, &shared, mode.target())
            .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
    if targets.is_empty() {
        if matches!(mode, IsolatedDeploy::All) && active_uses_libjd(core, root) {
            keytao_core::ReloadStamp::write(root)?;
            return Ok(DeployResult {
                success: true,
                message: "已更新 ime-jd 配置".into(),
                warning: None,
            });
        }
        return Err("没有可部署的方案包".to_owned().into());
    }
    if let IsolatedDeploy::Switch { schema_id, .. } = mode {
        match switch_cached_package(core, root, &shared, &targets[0], schema_id).await {
            Ok(true) => return Ok(package_switch_result(root, &shared)),
            Ok(false) => {}
            Err(error) => {
                return Err(DeployAttemptFailure {
                    error,
                    failed_packages: vec![targets[0].package_key.clone()],
                });
            }
        }
    }
    let expected = targets.len();
    let first = targets[0].package_key.clone();
    let materialize_failed: Vec<String> = targets
        .iter()
        .filter(|target| target.materialize_error.is_some())
        .map(|target| target.package_key.clone())
        .collect();
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
    let (mut result, _plan_lock) = finish_isolated_deploy(
        core,
        root,
        expected,
        events,
        worker,
        publish_deploy,
        &materialize_failed,
        (expected == 1).then_some(first.as_str()),
    )
    .await?;
    finish_isolated_target(core, root, &shared, mode, &first)
        .await
        .map_err(|error| DeployAttemptFailure {
            error,
            failed_packages: vec![first],
        })?;
    result.warning = merge_warnings([
        keytao_core::deployment_warning_with_shared(root, &shared),
        result.warning,
    ]);
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
    materialize_failed: &[String],
    single_target: Option<&str>,
) -> Result<(DeployResult, std::fs::File), DeployAttemptFailure> {
    let mut active_step = None;
    let mut completed = 0;
    let mut reload_warning = None;
    let mut failed_packages = Vec::new();
    while let Some(progress) = events.recv().await {
        if progress.legacy_message.is_none()
            && matches!(
                progress.step,
                DeployStep::Done | DeployStep::Skip | DeployStep::Failed
            )
        {
            completed += 1;
            if progress.step == DeployStep::Failed {
                push_unique_failed(&mut failed_packages, progress.package_key.clone());
            }
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
    let (result, plan_lock) = worker
        .await
        .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
    if publish_deploy && matches!(active_step, Some(DeployStep::Done | DeployStep::Skip)) {
        persist_result(core, root, true)
            .map_err(|error| DeployAttemptFailure::from(error.to_string()))?;
        if active_step == Some(DeployStep::Done) {
            render_user_schema_logos(root);
        }
    }
    let enrich_failed = |mut list: Vec<String>| {
        if list.is_empty() {
            for key in materialize_failed {
                push_unique_failed(&mut list, key.clone());
            }
        }
        if list.is_empty()
            && let Some(single) = single_target
        {
            push_unique_failed(&mut list, single);
        }
        list
    };
    let worker_warning = match result {
        Ok(warning) => warning,
        Err(error) => {
            drop(plan_lock);
            return Err(DeployAttemptFailure {
                error: format!("部署失败: {error}"),
                failed_packages: enrich_failed(failed_packages),
            });
        }
    };
    if completed != expected || !matches!(active_step, Some(DeployStep::Done | DeployStep::Skip)) {
        drop(plan_lock);
        return Err(DeployAttemptFailure {
            error: "方案包部署未返回完整结果".into(),
            failed_packages: enrich_failed(failed_packages),
        });
    }
    Ok((
        DeployResult {
            success: true,
            message: "部署成功".into(),
            warning: merge_warnings([reload_warning, worker_warning]),
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
    keytao_core::lock_file(&lock, false)
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
            git: None,
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
        deploy_isolated_in_process_target(
            &core,
            root,
            shared.clone(),
            IsolatedDeploy::Switch {
                key,
                schema_id: None,
            },
        )
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

        deploy_isolated_in_process_target(
            &core,
            root,
            shared,
            IsolatedDeploy::Switch {
                key,
                schema_id: None,
            },
        )
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
    fn cached_macos_signatures_are_invalidated_by_changes_to_either_executable() {
        use std::fs::{self, File, FileTimes};

        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        let ime = dir.path().join("ime");
        let executable = fs::read("/usr/bin/true").unwrap();
        fs::write(&app, &executable).unwrap();
        fs::write(&ime, &executable).unwrap();
        verify_macos_deploy_signatures(&app, &ime).unwrap();

        let mut invalid = executable.clone();
        invalid[0] ^= 0xff;
        for path in [&app, &ime] {
            let modified = fs::metadata(path).unwrap().modified().unwrap();
            fs::write(path, &invalid).unwrap();
            File::open(path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(modified))
                .unwrap();
            assert!(verify_macos_deploy_signatures(&app, &ime).is_err());
            fs::write(path, &executable).unwrap();
            verify_macos_deploy_signatures(&app, &ime).unwrap();
        }
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

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn deploy_failure_detects_broken_cloud_package_backs_up_and_redownloads_cleanly() {
        use std::{
            fs,
            io::{Cursor, Write},
            sync::Arc,
        };

        struct NullSink;
        impl crate::EventSink for NullSink {
            fn emit(&self, _: CoreEvent) {}
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rime");
        let shared = temp.path().join("rime-data");
        fs::create_dir_all(root.join("runtime")).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("default.yaml"), "config_version: '1'\n").unwrap();
        fs::write(
            root.join(RuntimeDirs::MIGRATION_MARKER),
            br#"{"version":1}"#,
        )
        .unwrap();

        let core = Core::new(
            crate::AppEnv {
                data_dir: temp.path().join("data"),
                cache_dir: temp.path().join("cache"),
                resource_dir: temp.path().to_path_buf(),
                platform: crate::Platform::MacOs,
                app_version: "fixture".into(),
            },
            Arc::new(NullSink),
        )
        .unwrap();

        let make_zip = |files: &[(&str, &str)]| {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, body) in files {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap().into_inner()
        };

        // Install a broken cloud package + leftover runtime file that causes compilation to fail.
        let broken_zip = make_zip(&[(
            "keytao.schema.yaml",
            "schema: {schema_id: keytao, name: Broken, version: '1'}\nengine:\n  translators: [table_translator]\ntranslator: {dictionary: missing_dict_xyz, enable_user_dict: false}\n",
        )]);
        crate::packages::store_cloud_archive(
            &core,
            "keytao",
            Some("v1-broken"),
            "official",
            None,
            &broken_zip,
            &root,
        )
        .unwrap();
        fs::create_dir_all(root.join("runtime/keytao")).unwrap();
        fs::write(root.join("runtime/keytao/leftover.custom.yaml"), b"bad: [\n").unwrap();

        let clean_zip = make_zip(&[
            (
                "default.custom.yaml",
                "patch:\n  schema_list:\n    - schema: keytao\n",
            ),
            (
                "keytao.schema.yaml",
                "schema: {schema_id: keytao, name: CleanKeytao, version: '2'}\nengine:\n  processors: [speller, selector, express_editor]\n  segmentors: [abc_segmentor, fallback_segmentor]\n  translators: [table_translator]\nspeller: {alphabet: abcde}\ntranslator: {dictionary: keytao, enable_user_dict: false}\n",
            ),
            (
                "keytao.dict.yaml",
                "---\nname: keytao\nversion: '2'\nsort: by_weight\nuse_preset_vocabulary: false\n...\n好\thao\t100\n",
            ),
        ]);

        let core_for_repair = core.clone();
        let root_for_repair = root.clone();
        let result = deploy_isolated_in_process_with_repair(
            &core,
            &root,
            shared.clone(),
            IsolatedDeploy::Switch {
                key: "keytao",
                schema_id: None,
            },
            move |failed| async move {
                assert_eq!(failed, vec!["keytao".to_owned()]);
                let backup = crate::packages::repair_package_for_redeploy_with(
                    &core_for_repair,
                    &root_for_repair,
                    "keytao",
                    crate::packages::RepairMode::BackUpRuntime,
                    async |core, root, key| {
                        assert_eq!(key, "keytao");
                        crate::packages::store_cloud_archive(
                            core,
                            &key,
                            Some("v2-clean"),
                            "official",
                            None,
                            &clean_zip,
                            root,
                        )?;
                        Ok(())
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok(backup)
            },
        )
        .await
        .unwrap();

        assert!(result.success);
        assert_eq!(RuntimeDirs::active_package_key(&root).as_deref(), Some("keytao"));
        assert!(root.join("runtime/keytao/build/keytao.schema.yaml").is_file());
        assert!(!root.join("runtime/keytao/leftover.custom.yaml").exists());
        assert!(root.join("migrations").is_dir());
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
