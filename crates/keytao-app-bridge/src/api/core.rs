use crate::{api::types::*, frb_generated::StreamSink, host::InstallHost, runtime};
use flutter_rust_bridge::DartFnFuture;
use keytao_app_core::ime_host::ImeHost;
#[cfg(not(target_os = "android"))]
use keytao_app_core::{events::InstallProgress, CoreEvent};

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

/// Creates the process-wide Core, or returns its existing info when the resolved
/// directories, platform and user root match. Restart the process to change them.
pub fn init_core(config: BridgeConfig) -> Result<BridgeInfo, String> {
    runtime::initialize(config)
}

/// One active subscriber; registering again replaces/closes the previous stream.
/// Wait for Ready before triggering an action whose events must be observed.
pub fn core_events(sink: StreamSink<BridgeEvent>) -> Result<(), String> {
    runtime::state()?.events.register(sink)
}

pub fn check_local_schema() -> Result<LocalSchemaDto, String> {
    let state = runtime::state()?;
    let root = state.host.user_root()?;
    let local =
        keytao_app_core::scheme::check_local_schema(&state.core, Some(root.clone()))
            .map_err(|error| error.to_string())?;
    let app_state = state.core.state();
    let pending = keytao_core::RuntimeDirs::needs_migration(&root);
    let active_package = if pending {
        None
    } else {
        keytao_core::RuntimeDirs::active_package_key(&root)
    };
    let easy_english_enabled = active_package.as_deref()
        .filter(|key| *key != keytao_app_core::addon::EASY_EN_ADDON_ID)
        .map(|key| keytao_app_core::runtime_materialize::easy_english_enabled(&root, key))
        .transpose()
        .map_err(|error| error.to_string())?;
    let installed_scheme = if pending { app_state.active_scheme } else { active_package };
    let installed_scheme_version = installed_scheme
        .as_ref()
        .and_then(|scheme| app_state.installed_packages.get(scheme))
        .and_then(|package| package.version.clone());
    Ok(LocalSchemaDto {
        installed: local.installed,
        deployed: local.deployed,
        build_available: local.build_available,
        version: local.version,
        schemas: local.schemas,
        installed_scheme,
        installed_scheme_version,
        easy_english_enabled,
        deploy_warning: local.deploy_warning,
    })
}

pub fn active_schema_logo_path() -> Result<Option<String>, String> {
    let state = runtime::state()?;
    Ok(
        keytao_app_core::deploy::active_schema_logo_path(&state.host.user_root()?)
            .map(|path| path.to_string_lossy().into_owned()),
    )
}

pub async fn remote_scheme_logo(
    scheme_key: String,
    repo_url: String,
    branch: Option<String>,
) -> Result<Option<String>, String> {
    let state = runtime::state()?;
    keytao_app_core::packages::package_logos::remote_scheme_logo(
        &state.core,
        &scheme_key,
        &repo_url,
        branch.as_deref(),
    )
    .await
    .map_err(|error| error.to_string())
}

pub fn onboarding() -> Result<OnboardingDto, String> {
    Ok(OnboardingDto {
        completed: runtime::state()?.core.onboarding().completed,
    })
}

pub fn complete_onboarding() -> Result<(), String> {
    runtime::state()?
        .core
        .complete_onboarding()
        .map_err(|error| error.to_string())
}

pub fn package_schemas(scheme_key: String) -> Result<Vec<PackageSchemaDto>, String> {
    runtime::dto(
        keytao_app_core::package_schemas::package_schemas(&runtime::state()?.core, &scheme_key)
            .map_err(|error| error.to_string())?,
    )
}

pub async fn set_package_schemas(scheme_key: String, ids: Vec<String>) -> Result<bool, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    keytao_app_core::package_schemas::set_package_schemas(&state.core, &scheme_key, &ids)
        .map_err(|error| error.to_string())?;
    let is_addon = keytao_app_core::package_schemas::is_addon_package_key(&scheme_key);
    Ok(is_addon || keytao_core::RuntimeDirs::active_package_key(&root).as_deref() == Some(&scheme_key))
}

pub fn list_installed_packages() -> Result<Vec<InstalledPackageDto>, String> {
    let state = runtime::state()?;
    let root = state.host.user_root()?;
    let mut packages = keytao_app_core::packages::list_installed_packages(&state.core)
        .map_err(|error| error.to_string())?;
    if !keytao_core::RuntimeDirs::needs_migration(&root) {
        let key = keytao_core::RuntimeDirs::active_package_key(&root);
        for package in &mut packages {
            package.active = key.as_deref() == Some(package.scheme_key.as_str());
            package.in_use = package.active;
            let engine = package_engine(&state.core, &package.scheme_key);
            if engine == keytao_core::engine_choice::PackageEngine::LibjdEngine {
                package.engine = engine.as_str().into();
                if keytao_core::engine_choice::preferred(&root) == engine {
                    package.supports_user_words = false;
                }
            }
        }
    }
    runtime::dto(packages)
}

fn package_engine(
    core: &keytao_app_core::Core,
    key: &str,
) -> keytao_core::engine_choice::PackageEngine {
    use keytao_core::engine_choice::PackageEngine;
    if keytao_app_core::libjd::imported_package_ready(core, key) {
        PackageEngine::LibjdEngine
    } else {
        PackageEngine::Rime
    }
}

pub async fn isolated_packages() -> Result<Vec<IsolatedPackageDto>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    let core = state.core.clone();
    tokio::task::spawn_blocking(move || {
        let mut packages: Vec<IsolatedPackageDto> = runtime::dto(
            keytao_app_core::runtime_materialize::isolated_packages(&core, &root)
                .map_err(|error| error.to_string())?,
        )?;
        for package in &mut packages {
            package.engine = package_engine(&core, &package.package_key).as_str().into();
        }
        Ok(packages)
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn package_orphan_report() -> Result<PackageOrphanReportDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::package_orphans::report(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

pub fn package_migration_pending() -> Result<bool, String> {
    let root = runtime::state()?.host.user_root()?;
    Ok(keytao_core::RuntimeDirs::needs_migration(&root))
}

pub fn pending_migration_backup_dir() -> Result<Option<String>, String> {
    let root = runtime::state()?.host.user_root()?;
    keytao_app_core::package_migration::pending_backup_notice(&root)
        .map_err(|error| error.to_string())
}

pub fn dismiss_migration_backup_notice() -> Result<(), String> {
    let root = runtime::state()?.host.user_root()?;
    keytao_app_core::package_migration::dismiss_backup_notice(&root)
        .map_err(|error| error.to_string())
}

pub async fn migrate_to_isolated() -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    let core = state.core.clone();
    tokio::task::spawn_blocking(move || keytao_app_core::package_migration::start_app(&core, &root))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

pub async fn trash_package_orphans(
    files: Vec<PackageOrphanFileDto>,
    kept_backups: Option<Vec<PackageKeptBackupDto>>,
) -> Result<Vec<String>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let files: Vec<_> = files
        .into_iter()
        .map(|file| keytao_app_core::package_orphans::OrphanFile {
            path: file.path,
            sha256: file.sha256,
            size: file.size,
        })
        .collect();
    let kept_backups: Vec<_> = kept_backups
        .unwrap_or_default()
        .into_iter()
        .map(|backup| keytao_app_core::package_orphans::KeptBackup {
            path: backup.path,
            backup: backup.backup,
        })
        .collect();
    keytao_app_core::package_orphans::trash_selected_with_backups(
        &state.core,
        &state.host.user_root()?,
        &files,
        &kept_backups,
    )
    .map_err(|error| error.to_string())
}

pub async fn import_local_scheme(
    directory_path: String,
    display_name: String,
    scheme_key: Option<String>,
) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    keytao_app_core::package_migration::ensure_before_write(&state.core, &root)
        .await
        .map_err(|error| error.to_string())?;
    keytao_app_core::packages::import_local_scheme(
        &state.core,
        std::path::Path::new(&directory_path),
        &display_name,
        scheme_key.as_deref(),
    )
    .map_err(|error| error.to_string())
}

pub async fn import_git_scheme(
    repo_url: String,
    mode: String,
    branch: Option<String>,
    asset_name: Option<String>,
    display_name: String,
    scheme_key: Option<String>,
) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    let parsed_mode = keytao_app_core::packages::GitSchemeMode::parse(&mode)
        .map_err(|error| error.to_string())?;
    keytao_app_core::packages::import_git_scheme(
        &state.core,
        &root,
        keytao_app_core::packages::GitSchemeConfig {
            repo_url,
            mode: parsed_mode,
            branch,
            asset_name,
        },
        &display_name,
        scheme_key.as_deref(),
    )
    .await
    .map_err(|error| error.to_string())
}

pub fn get_scheme_sync_state() -> Result<String, String> {
    Ok(keytao_app_core::scheme_sync::sync_state(&runtime::state()?.core).to_string())
}

pub fn set_scheme_sync_preferences(
    follow_account_sync: Option<bool>,
    scheme_key: Option<String>,
    sync: Option<bool>,
    settings_json: Option<String>,
) -> Result<String, String> {
    let settings = settings_json
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| error.to_string())?;
    keytao_app_core::scheme_sync::set_preferences(
        &runtime::state()?.core,
        follow_account_sync,
        scheme_key.as_deref(),
        sync,
        settings,
    )
    .map(|value| value.to_string())
    .map_err(|error| error.to_string())
}

pub fn export_scheme_profile() -> Result<String, String> {
    keytao_app_core::scheme_sync::export_profile(&runtime::state()?.core)
        .map(|value| value.to_string())
        .map_err(|error| error.to_string())
}

pub async fn upload_scheme_profile() -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::scheme_sync::upload_profile(&state.core)
        .await
        .map(|value| value.to_string())
        .map_err(|error| error.to_string())
}

pub async fn fetch_scheme_profile() -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::scheme_sync::fetch_profile(&state.core)
        .await
        .map(|value| value.to_string())
        .map_err(|error| error.to_string())
}

/// Applies preferences and adds missing packages. Activation requires a separate confirmed call.
pub async fn apply_scheme_profile(profile_json: String) -> Result<String, String> {
    let profile = serde_json::from_str(&profile_json).map_err(|error| error.to_string())?;
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::scheme_sync::apply_profile(&state.core, profile, &state.host.user_root()?)
        .await
        .map(|value| value.to_string())
        .map_err(|error| error.to_string())
}

pub async fn list_cloud_packages() -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let packages = keytao_app_core::cloud_packages::list(&state.core)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&packages).map_err(|error| error.to_string())
}

pub async fn preview_cloud_package(scheme_key: String) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let preview = keytao_app_core::cloud_packages::preview(
        &state.core,
        &scheme_key,
        &state.host.user_root()?,
    )
    .map_err(|error| error.to_string())?;
    serde_json::to_string(&preview).map_err(|error| error.to_string())
}

pub async fn upload_cloud_package(
    scheme_key: String,
    expected_sha256: Option<String>,
) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let package = keytao_app_core::cloud_packages::upload(
        &state.core,
        &scheme_key,
        &state.host.user_root()?,
        expected_sha256.as_deref(),
    )
    .await
    .map_err(|error| error.to_string())?;
    serde_json::to_string(&package).map_err(|error| error.to_string())
}

/// Runs Android's deployImeData callback under the same lock as install and rollback.
pub async fn deploy_android(
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        keytao_app_core::deploy::deploy_with_android_host(
            &state.core,
            &state.host.user_root()?,
            android_deploy,
        )
        .await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = android_deploy;
        Err("Android deployment is only available on Android".into())
    }
}

pub async fn download_cloud_package(id: String) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::cloud_packages::download(&state.core, &id, &state.host.user_root()?)
        .await
        .map_err(|error| error.to_string())
}

pub async fn delete_cloud_package(id: String) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::cloud_packages::delete(&state.core, &id)
        .await
        .map_err(|error| error.to_string())
}

pub async fn list_user_words(scheme_key: String) -> Result<Vec<UserWordDto>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::user_words::list_user_words(
            &state.core,
            &scheme_key,
            &state.host.user_root()?,
        )
        .map_err(|error| error.to_string())?,
    )
}

pub async fn add_user_word(
    scheme_key: String,
    word: String,
    code: String,
    weight: Option<String>,
) -> Result<UserWordMutationDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::user_words::add_user_word(
            &state.core,
            &scheme_key,
            &state.host.user_root()?,
            &word,
            &code,
            weight,
        )
        .map_err(|error| error.to_string())?,
    )
}

pub async fn update_user_word(
    scheme_key: String,
    id: u32,
    expected_word: String,
    expected_code: String,
    word: String,
    code: String,
    weight: Option<String>,
) -> Result<UserWordMutationDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::user_words::update_user_word(
            &state.core,
            &scheme_key,
            &state.host.user_root()?,
            id,
            &expected_word,
            &expected_code,
            &word,
            &code,
            weight,
        )
        .map_err(|error| error.to_string())?,
    )
}

pub async fn delete_user_word(
    scheme_key: String,
    id: u32,
    expected_word: String,
    expected_code: String,
) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::user_words::delete_user_word(
        &state.core,
        &scheme_key,
        &state.host.user_root()?,
        id,
        &expected_word,
        &expected_code,
    )
    .map_err(|error| error.to_string())
}

pub async fn compare_user_words(scheme_key: String) -> Result<Vec<UserWordComparisonDto>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::user_words::compare_user_words(
            &state.core,
            &scheme_key,
            &state.host.user_root()?,
        )
        .map_err(|error| error.to_string())?,
    )
}

pub async fn lookup_user_word(
    scheme_key: String,
    word: String,
    code: String,
) -> Result<UserWordComparisonDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::user_words::lookup_user_word(&state.core, &scheme_key, &word, &code)
            .map_err(|error| error.to_string())?,
    )
}

pub async fn search_stock_words(
    scheme_key: String,
    query: String,
) -> Result<Vec<UserWordDto>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::user_words::search_stock_words(&state.core, &scheme_key, &query)
            .map_err(|error| error.to_string())?,
    )
}

pub async fn set_user_word_removed(
    scheme_key: String,
    word: String,
    code: String,
    remove: bool,
) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::user_words::set_user_word_removed(
        &state.core,
        &scheme_key,
        &state.host.user_root()?,
        &word,
        &code,
        remove,
    )
    .map_err(|error| error.to_string())
}

pub async fn take_user_words_report(scheme_key: String) -> Result<UserWordsReportDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    runtime::dto(
        keytao_app_core::user_words::take_user_words_report(&state.core, &scheme_key)
            .map_err(|error| error.to_string())?,
    )
}

pub async fn download_scheme_package(scheme_key: String, source: String) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::packages::download_scheme_package(
        &state.core,
        &scheme_key,
        &source,
        &state.host.user_root()?,
    )
    .await
    .map_err(|error| error.to_string())
}

pub async fn check_scheme_updates() -> Result<Vec<SchemeUpdateDto>, String> {
    runtime::dto(keytao_app_core::packages::check_scheme_updates(&runtime::state()?.core).await)
}

pub fn get_dismissed_scheme_updates() -> Result<std::collections::HashMap<String, String>, String> {
    Ok(runtime::state()?
        .core
        .state()
        .dismissed_scheme_updates
        .into_iter()
        .collect())
}

pub fn dismiss_scheme_update(scheme_key: String, version: String) -> Result<(), String> {
    let scheme_key = keytao_app_core::scheme::validate_scheme_key(&scheme_key)?;
    runtime::state()?
        .core
        .update_state(|state| {
            state
                .dismissed_scheme_updates
                .insert(scheme_key.into(), version);
        })
        .map_err(|error| error.to_string())
}

pub async fn switch_package(
    package_key: String,
    schema_id: Option<String>,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    #[cfg(target_os = "windows")]
    state.host.require_default_windows_root()?;
    #[cfg(not(target_os = "android"))]
    let _ = android_deploy;
    keytao_app_core::deploy::switch_package_with_schema(
        &state.core,
        &state.host.user_root()?,
        &package_key,
        schema_id.as_deref(),
        #[cfg(target_os = "android")]
        android_deploy,
    )
    .await
}

pub async fn set_package_easy_english(
    package_key: String,
    enabled: bool,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    keytao_app_core::addon::set_package_easy_english(&state.core, &package_key, enabled, &host)
        .await
}

pub fn libjd_status() -> Result<LibjdStatusDto, String> {
    let status = keytao_app_core::libjd::libjd_status();
    Ok(LibjdStatusDto {
        available: status.available,
        installed_version: status.installed_version,
    })
}

pub fn input_engine() -> Result<String, String> {
    let root = runtime::state()?.host.user_root()?;
    Ok(keytao_app_core::libjd::input_engine(&root).as_str().into())
}

pub fn input_engine_enabled() -> Result<bool, String> {
    let root = runtime::state()?.host.user_root()?;
    Ok(keytao_core::engine_choice::is_enabled(&root))
}

pub async fn set_input_engine_enabled(enabled: bool) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    #[cfg(target_os = "macos")]
    {
        keytao_app_core::ime_status::macos::set_input_engine_enabled(&root, enabled).await
    }
    #[cfg(target_os = "windows")]
    {
        tokio::task::spawn_blocking(move || {
            keytao_app_core::ime_status::windows::set_input_engine_enabled(&root, enabled)
        })
        .await
        .map_err(|error| error.to_string())?
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        keytao_core::engine_choice::set_enabled(&root, enabled)
    }
}

pub async fn set_input_engine(engine: String) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    #[cfg(target_os = "windows")]
    if engine == "libjd_engine" {
        state.host.require_default_windows_root()?;
    }
    let root = state.host.user_root()?;
    keytao_app_core::package_migration::ensure_before_write(&state.core, &root)
        .await
        .map_err(format_libjd_error)?;
    keytao_app_core::libjd::set_input_engine(&state.core, &root, &engine)
        .map_err(format_libjd_error)
}

pub async fn import_libjd_package(key: String, schema_id: Option<String>) -> Result<u32, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    #[cfg(target_os = "windows")]
    state.host.require_default_windows_root()?;
    let root = state.host.user_root()?;
    let skipped = keytao_app_core::libjd::import_package(&state.core, &root, &key, schema_id.as_deref())
        .await
        .map_err(format_libjd_error)?;
    Ok(u32::try_from(skipped).unwrap_or(u32::MAX))
}

fn format_libjd_error(error: keytao_app_core::CoreError) -> String {
    use keytao_app_core::CoreError;
    eprintln!("ime-jd operation failed: {error}");
    match error {
        CoreError::Network(reason) => format!("网络连接失败：{reason}"),
        CoreError::Io(_) | CoreError::Parse(_) => "无法保存 ime-jd 设置或文件".into(),
        CoreError::Other(message) | CoreError::Unsupported(message) => message,
        _ => "ime-jd 操作失败".into(),
    }
}

pub async fn remove_scheme_package(
    scheme_key: String,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<Vec<String>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    keytao_app_core::packages::remove_scheme_package(&state.core, &scheme_key, &host)
        .await
        .map_err(|error| error.to_string())
}

pub async fn repair_scheme_package(
    package_key: String,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<Option<String>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    #[cfg(target_os = "windows")]
    state.host.require_default_windows_root()?;
    let root = state.host.user_root()?;
    let was_active = keytao_core::RuntimeDirs::active_package_key(&root).is_some_and(|active| {
        active == package_key
            || active.strip_prefix("builtin/") == Some(package_key.as_str())
    });
    let backup = keytao_app_core::packages::repair_package_for_redeploy(
        &state.core,
        &root,
        &package_key,
    )
    .await
    .map_err(|error| error.to_string())?;
    let normalized = package_key
        .strip_prefix("builtin/")
        .unwrap_or(&package_key);
    #[cfg(not(target_os = "android"))]
    let _ = &android_deploy;
    if was_active {
        keytao_app_core::deploy::switch_package_with_schema(
            &state.core,
            &root,
            normalized,
            None,
            #[cfg(target_os = "android")]
            android_deploy,
        )
        .await?;
    } else {
        keytao_app_core::deploy::deploy_package(
            &state.core,
            &root,
            normalized,
            #[cfg(target_os = "android")]
            android_deploy,
        )
        .await?;
    }
    Ok(backup)
}

/// The persisted account, if a token is stored. The token never leaves Rust.
pub fn account_session() -> Result<Option<AccountUserDto>, String> {
    let state = runtime::state()?.core.state();
    match (state.auth_token, state.user) {
        (Some(_), Some(user)) => Ok(Some(runtime::dto(user)?)),
        _ => Ok(None),
    }
}

pub async fn get_account_stats() -> Result<AccountStatsDto, String> {
    runtime::dto(
        keytao_app_core::account::get_account_stats(&runtime::state()?.core)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub async fn account_login(name: String, password: String) -> Result<AccountUserDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let session = keytao_app_core::account::keytao_login(&state.core, name, password)
        .await
        .map_err(|error| error.to_string())?;
    runtime::dto(session.user)
}

/// Refreshes an aging token at startup without prompting for credentials.
pub async fn account_refresh() -> Result<Option<AccountUserDto>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::account::startup_refresh(&state.core)
        .await
        .map_err(|error| error.to_string())?;
    account_session()
}

pub async fn account_logout() -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    state.core.clear_auth().map_err(|error| error.to_string())
}

/// Merges the user dictionary in place without deploying.
pub async fn account_sync_user_dictionary(
    confirmed_deletions: Option<UserDictionaryDeletionConfirmationDto>,
) -> Result<UserDictionarySyncDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let token = state.core.state().auth_token.ok_or("未登录")?;
    let result = keytao_app_core::account::sync_user_dictionary(
        &state.core,
        &state.host,
        token,
        confirmed_deletions.map(|confirmed| {
            keytao_app_core::account::UserDictionaryDeletionConfirmation {
                local_deletions: confirmed
                    .local_deletions
                    .into_iter()
                    .map(|entry| keytao_app_core::account::UserDictionaryDeletion {
                        word: entry.word,
                        code: entry.code,
                        remove: entry.remove,
                    })
                    .collect(),
                server_deletions: confirmed
                    .server_deletions
                    .into_iter()
                    .map(|entry| keytao_app_core::account::UserDictionaryDeletion {
                        word: entry.word,
                        code: entry.code,
                        remove: entry.remove,
                    })
                    .collect(),
            }
        }),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(UserDictionarySyncDto {
        file_name: result.file_name,
        count: result.count as u64,
        message: result.message,
        pending: result.pending,
        local_deletions: result
            .local_deletions
            .into_iter()
            .map(|entry| UserDictionaryDeletionDto {
                word: entry.word,
                code: entry.code,
                remove: entry.remove,
            })
            .collect(),
        server_deletions: result
            .server_deletions
            .into_iter()
            .map(|entry| UserDictionaryDeletionDto {
                word: entry.word,
                code: entry.code,
                remove: entry.remove,
            })
            .collect(),
    })
}

pub async fn account_pending_words() -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let words = keytao_app_core::submission::pending_user_words(&state.core)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&words).map_err(|error| error.to_string())
}

pub async fn account_prepare_submission(entries_json: String) -> Result<String, String> {
    let entries: Vec<keytao_app_core::submission::SubmissionWord> =
        serde_json::from_str(&entries_json).map_err(|error| error.to_string())?;
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let preview = keytao_app_core::submission::prepare_submission(&state.core, entries)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&preview).map_err(|error| error.to_string())
}

pub async fn account_submit_words(preview_json: String) -> Result<String, String> {
    let preview = serde_json::from_str(&preview_json).map_err(|error| error.to_string())?;
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let result = keytao_app_core::submission::submit(&state.core, preview)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&result).map_err(|error| error.to_string())
}

pub async fn fetch_scheme_release(scheme: String) -> Result<SchemeReleaseDto, String> {
    let release = keytao_app_core::scheme::fetch_scheme_release(&runtime::state()?.core, scheme)
        .await
        .map_err(|error| error.to_string())?;
    Ok(SchemeReleaseDto {
        scheme: release.scheme,
        source_type: release.source_type,
        label: release.label,
        version: release.version,
        name: release.name,
        published_at: release.published_at,
        download_url: release.download_url,
        asset_name: release.asset_name,
    })
}

/// Fetches the latest release for the same scheme key accepted by Core,
/// downloads it, then installs into the host root. Does not deploy the IME.
pub async fn install_latest_scheme(scheme: String) -> Result<InstallResultDto, String> {
    #[cfg(target_os = "android")]
    {
        let _ = scheme;
        return Err("Use the platform installation flow on Android".into());
    }
    #[cfg(not(target_os = "android"))]
    {
        let state = runtime::state()?;
        state.core.emit(CoreEvent::InstallProgress(InstallProgress {
            stage: "fetching".into(),
            percent: 0,
            message: "正在获取方案版本...".into(),
        }));
        let release = fetch_scheme_release(scheme.clone()).await?;
        install_scheme_from_url(release.download_url, scheme, Some(release.version)).await
    }
}

pub async fn fetch_latest_release() -> Result<ReleaseInfoDto, String> {
    runtime::dto(
        keytao_app_core::scheme::fetch_latest_release(&runtime::state()?.core)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub async fn check_app_update() -> Result<AppUpdateDto, String> {
    runtime::dto(
        keytao_app_core::update::check_app_update(&runtime::state()?.core)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub async fn set_app_update_prereleases(enabled: bool) -> Result<(), String> {
    keytao_app_core::update::set_app_update_prereleases(&runtime::state()?.core, enabled)
        .await
        .map_err(|error| error.to_string())
}

pub fn set_website_settings(origin: String, hidden: bool) -> Result<(), String> {
    if !matches!(
        origin.as_str(),
        "https://keytao.rea.ink" | "https://keytao.vercel.app"
    ) {
        return Err("Unsupported KeyTao website origin".into());
    }
    runtime::state()?
        .core
        .update_state(|state| {
            state.website_origin = Some(origin);
            state.website_hidden = hidden;
        })
        .map_err(|error| error.to_string())
}

pub async fn download_app_update(version: String) -> Result<bool, String> {
    keytao_app_core::update::download_app_update(&runtime::state()?.core, &version).await
}

pub async fn open_app_update_package(version: String, restart: bool) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::update::open_app_update_package(&state.core, &version, restart).await
}

/// Reads a filesystem directory. Android SAF tree URIs use the readLocalSchemas
/// platform channel instead. Both Core-supported custom-config filenames count.
pub fn read_local_schemas(dir: String) -> Result<LocalSchemasDto, String> {
    let root = std::path::Path::new(&dir);
    let has_default_custom = ["default.custom.yaml", "default-custom.yaml"]
        .iter()
        .any(|name| root.join(name).is_file());
    Ok(LocalSchemasDto {
        has_default_custom,
        schemas: keytao_app_core::scheme::read_local_schemas(&runtime::state()?.core, dir)
            .map_err(|error| error.to_string())?,
    })
}

/// Returns Core's directory-first, name-sorted entries. SAF uses listFiles.
pub fn list_dir(dir: String) -> Result<Vec<FileItemDto>, String> {
    runtime::dto(
        keytao_app_core::scheme::list_dir(&runtime::state()?.core, dir)
            .map_err(|error| error.to_string())?,
    )
}

pub fn cancel_scheme_download() -> Result<(), String> {
    runtime::state()?.core.cancel_download();
    Ok(())
}

/// Downloads and smart-installs into the selected filesystem directory without
/// deploying. Android SAF uses download_scheme_archive and smartExtractZip.
pub async fn install_scheme_to_dir(
    url: String,
    dir: String,
    scheme_key: String,
    version: Option<String>,
) -> Result<InstallResultDto, String> {
    keytao_app_core::scheme::validate_scheme_key(&scheme_key)?;
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = runtime::absolute_dir(&dir)?;
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let active = root
        .canonicalize()
        .ok()
        .zip(state.host.user_root()?.canonicalize().ok())
        .is_some_and(|(root, default_root)| root == default_root);
    let archive = keytao_app_core::download::download_to_temp(&state.core, url)
        .await
        .map_err(|error| error.to_string())?;
    let result = if active {
        keytao_app_core::install::install_live_scheme(
            &state.core,
            archive.clone(),
            &root,
            &scheme_key,
            version.as_deref(),
        )
        .await
    } else {
        keytao_app_core::install::export_scheme(&state.core, archive.clone(), &root)
    };
    let _ = std::fs::remove_file(archive);
    let result = result.map_err(|error| error.to_string())?;
    Ok(install_result_dto(result))
}

/// Android SAF sequence: confirm storage permission, pickDirectory,
/// then listFiles/readLocalSchemas through the platform channel. Download here,
/// call smartExtractZip(zipPath, treeUri) with its progress handler, and pass
/// jsonEncode(result) to finish_android_install for the DTO and done event.
/// Always call remove_downloaded_archive in Dart's finally block. Do not deploy
/// or copy into the private/live root. Download progress uses core_events.
/// Dart must serialize this entire sequence with all other installations:
/// Core's fixed cache filename is protected only during each Rust call.
pub async fn download_scheme_archive(url: String) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::download::download_to_temp(&state.core, url)
        .await
        .map_err(|error| error.to_string())
}

/// Cleans up the download after extraction and package recording. Resolves
/// parent directories and rejects symlink files before deleting inside cache.
pub async fn remove_downloaded_archive(path: String) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let archive = downloaded_archive_path(state, &path)?;
    match std::fs::symlink_metadata(&archive) {
        Ok(metadata) if metadata.is_file() => match std::fs::remove_file(archive) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        },
        Ok(_) => Err("Downloaded archive must be a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn downloaded_archive_path(state: &runtime::BridgeState, path: &str) -> Result<std::path::PathBuf, String> {
    let path = runtime::absolute_dir(path)?;
    let cache = state
        .core
        .env()
        .cache_dir
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let parent = path
        .parent()
        .ok_or("Downloaded archive must have a parent directory")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let archive = parent.join(
        path.file_name()
            .ok_or("Downloaded archive must have a filename")?,
    );
    if !parent.starts_with(&cache) {
        return Err("Downloaded archive must be a file inside the bridge cache directory".into());
    }
    Ok(archive)
}

/// Installs a selected GitHub/Gitee or scheme release URL without deploying.
pub async fn install_scheme_from_url(
    url: String,
    scheme_key: String,
    version: Option<String>,
) -> Result<InstallResultDto, String> {
    keytao_app_core::scheme::validate_scheme_key(&scheme_key)?;
    #[cfg(target_os = "android")]
    {
        let _ = (url, scheme_key, version);
        Err("Use the platform installation flow on Android".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        let state = runtime::state()?;
        // Core's downloader uses a fixed cache filename; serialize installations.
        let _guard = state.install_lock.lock().await;
        let root = state.host.user_root()?;
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let archive = keytao_app_core::download::download_to_temp(&state.core, url)
            .await
            .map_err(|error| error.to_string())?;
        let result = keytao_app_core::install::install_live_scheme(
            &state.core,
            archive.clone(),
            &root,
            &scheme_key,
            version.as_deref(),
        )
        .await;
        let _ = std::fs::remove_file(archive);
        result.map(install_result_dto).map_err(|error| error.to_string())
    }
}

#[flutter_rust_bridge::frb(ignore)]
fn install_result_dto(result: keytao_app_core::scheme_types::InstallResult) -> InstallResultDto {
    InstallResultDto {
        merged_schemas: result.merged_schemas,
        logs: result.logs,
        verify: result
            .verify
            .into_iter()
            .map(|entry| VerifyEntryDto {
                path: entry.path,
                ok: entry.ok,
                note: entry.note,
            })
            .collect(),
    }
}

/// Call after storage permission. The Android channel resolves the destination;
/// finish_android_install performs the transactional installation in Core.
pub async fn prepare_android_install(url: String) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        keytao_app_core::install::prepare_android_install(
            &state.core,
            &state.host.user_root()?,
            url,
        )
        .await
        .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = url;
        Err("Android installation is only available on Android".into())
    }
}

/// Pass jsonEncode of the Android channel's smart-extract result unchanged.
pub async fn finish_android_install(
    result_json: String,
    scheme_key: String,
    version: Option<String>,
) -> Result<InstallResultDto, String> {
    keytao_app_core::scheme::validate_scheme_key(&scheme_key)?;
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value =
            serde_json::from_str(&result_json).map_err(|error| error.to_string())?;
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        let user_root = state.host.user_root()?;
        let active = result["packageRoot"]
            .as_str()
            .filter(|root| std::path::Path::new(root).is_absolute())
            .and_then(|root| std::path::Path::new(root).canonicalize().ok())
            .zip(user_root.canonicalize().ok())
            .is_some_and(|(root, default_root)| root == default_root);
        if result["coreInstall"].as_bool() == Some(true) {
            if !active {
                return Err("Android private installation must target the IME root".into());
            }
            let archive = result["packageZipPath"]
                .as_str()
                .ok_or("Android installation is missing its archive")?;
            let archive = downloaded_archive_path(state, archive)?;
            if !std::fs::symlink_metadata(&archive).is_ok_and(|metadata| metadata.is_file()) {
                return Err("Downloaded archive must be a regular file".into());
            }
            return keytao_app_core::install::install_live_scheme(
                &state.core,
                archive.to_string_lossy().into_owned(),
                &user_root,
                &scheme_key,
                version.as_deref(),
            )
            .await
            .map(install_result_dto)
            .map_err(|error| error.to_string());
        }
        Ok(install_result_dto(keytao_app_core::install::finish_android_export(&result)))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (result_json, scheme_key, version);
        Err("Android installation is only available on Android".into())
    }
}

/// Android Rime deployment belongs to the deployImeData platform channel.
pub async fn deploy_default() -> Result<DeployResultDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    use keytao_core::engine_choice::{self, PackageEngine};
    if keytao_app_core::libjd::input_engine(&root) == PackageEngine::LibjdEngine {
        let key = engine_choice::active_package(&root).ok_or("当前没有可重新部署的 ime-jd 方案")?;
        let (_, schema) = keytao_core::libjd::imported_dictionary(&state.core.env().data_dir, &key)
            .ok_or("请先转换此方案的词库")?;
        let skipped = keytao_app_core::libjd::import_package(
            &state.core,
            &root,
            &key,
            Some(&schema.id),
        )
        .await
        .map_err(format_libjd_error)?;
        return Ok(DeployResultDto {
            success: true,
            message: "已重新部署 ime-jd 词库".into(),
            warning: (skipped > 0).then(|| format!("跳过 {skipped} 条不兼容词条")),
        });
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios"
    ))]
    {
        runtime::dto(state.host.deploy_default().await?)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios"
    )))]
    Err("Use deployImeData on Android; deployment is unavailable on this platform".into())
}

pub async fn get_android_ime_input_settings() -> Result<AndroidImeInputSettingsDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::get_android_ime_input_settings(
            runtime::state()?.host.clone(),
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

/// Saves all writable fields; returned path/message fields are read-only.
pub async fn set_android_ime_input_settings(
    settings: AndroidImeInputSettingsDto,
) -> Result<AndroidImeInputSettingsDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::set_android_ime_input_settings(
            runtime::state()?.host.clone(),
            settings.haptics_enabled,
            settings.haptic_intensity,
            settings.enter_key_behavior,
            settings.key_preview_enabled,
            settings.long_press_delay_ms,
            settings.keyboard_height_scale,
            settings.delete_speed,
            settings.english_mode,
            settings.backspace_gesture_mode,
            settings.keyboard_height_dp,
            settings.candidate_bar_height_dp,
            settings.swipe_threshold_dp,
            settings.key_sound_enabled,
            settings.key_sound_volume,
            settings.key_hint_visible,
            settings.flick_keys_enabled,
            settings.number_row_enabled,
            settings.candidate_font_scale,
            settings.double_space_period_enabled,
            settings.symbol_auto_return,
            settings.phrases,
            settings.clipboard_history_enabled,
            settings.clipboard_history_limit,
            settings.clipboard_suggestion_enabled,
            settings.clipboard_suggestion_timeout_sec,
            settings.clipboard_return_after_paste,
            settings.clipboard_hide_sensitive,
            settings.floating_portrait_enabled,
            settings.floating_portrait_scale,
            settings.floating_landscape_enabled,
            settings.floating_landscape_scale,
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

pub fn get_schemas() -> Result<SchemaListDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::get_schemas(runtime::state()?.host.clone())
            .map_err(|error| error.to_string())?,
    )
}

pub fn get_rime_sync_settings() -> Result<RimeSyncSettingsDto, String> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    return runtime::dto(keytao_app_core::rime_sync::read_settings(
        &runtime::state()?.host.user_root()?,
    )?);
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    Err("Rime user data sync is only available on desktop".into())
}

pub async fn save_rime_sync_settings(
    installation_id: String,
    sync_dir: String,
) -> Result<RimeSyncSettingsDto, String> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        runtime::dto(keytao_app_core::rime_sync::save_settings(
            &state.host.user_root()?,
            &installation_id,
            &sync_dir,
        )?)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (installation_id, sync_dir);
        Err("Rime user data sync is only available on desktop".into())
    }
}

pub async fn sync_rime_user_data() -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        keytao_app_core::rime_sync::sync_user_data(state.host.clone()).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    Err("Rime user data sync is only available on desktop".into())
}

pub fn select_schema(schema_id: String) -> Result<(), String> {
    keytao_app_core::ime_settings::select_schema(runtime::state()?.host.clone(), &schema_id)
        .map_err(|error| error.to_string())
}

pub fn get_schema_switches() -> Result<Vec<SchemaSwitchDto>, String> {
    keytao_app_core::ime_settings::get_schema_switches(runtime::state()?.host.clone())
        .map(|switches| switches.into_iter().map(Into::into).collect())
        .map_err(|error| error.to_string())
}

pub fn set_schema_switch(
    schema_id: String,
    name: String,
    value: u32,
) -> Result<Vec<SchemaSwitchDto>, String> {
    keytao_app_core::ime_settings::set_schema_switch(
        runtime::state()?.host.clone(),
        &schema_id,
        &name,
        value as usize,
    )
    .map(|switches| switches.into_iter().map(Into::into).collect())
    .map_err(|error| error.to_string())
}

fn theme_library_host() -> Result<crate::host::BridgeHost, String> {
    let state = runtime::state()?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    state.host.require_default_theme_root()?;
    Ok(state.host.clone())
}

pub fn get_theme_library() -> Result<Vec<ThemeSummaryDto>, String> {
    runtime::dto(keytao_app_core::theme_library::get_theme_library(
        theme_library_host()?,
    )?)
}

pub fn get_theme_editor() -> Result<String, String> {
    keytao_app_core::theme_library::get_theme_editor(theme_library_host()?)
}

pub async fn list_system_font_families() -> Result<Vec<SystemFontFamilyDto>, String> {
    tokio::task::spawn_blocking(|| {
        keytao_app_core::system_fonts::list_system_font_families().map(|families| {
            families
                .into_iter()
                .map(|font| SystemFontFamilyDto {
                    family: font.family,
                    display_name: font.display_name,
                    weights: font.weights,
                })
                .collect()
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub async fn load_system_font_bytes(family: String) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        keytao_app_core::system_fonts::load_system_font_bytes(family)
    })
    .await
    .map_err(|error| error.to_string())?
}

pub fn update_theme_editor(
    target: String,
    path: String,
    value_json: Option<String>,
) -> Result<String, String> {
    keytao_app_core::theme_library::update_theme_editor(
        theme_library_host()?,
        &target,
        &path,
        value_json.as_deref(),
    )
}

pub fn set_theme_editor_color_scheme(color_scheme: Option<UiColorSchemeDto>) -> Result<String, String> {
    keytao_app_core::theme_library::set_theme_editor_color_scheme(
        theme_library_host()?,
        color_scheme.map(|color_scheme| match color_scheme {
            UiColorSchemeDto::Auto => keytao_theme::UiColorScheme::Auto,
            UiColorSchemeDto::Light => keytao_theme::UiColorScheme::Light,
            UiColorSchemeDto::Dark => keytao_theme::UiColorScheme::Dark,
        }),
    )
}

#[flutter_rust_bridge::frb(sync)]
pub fn resolve_theme_editor_preview(source_json: String, dark: bool) -> Result<String, String> {
    keytao_theme::editor::resolve_theme_editor_preview(&source_json, dark)
}

pub fn apply_theme(id: String) -> Result<(), String> {
    keytao_app_core::theme_library::apply_theme(theme_library_host()?, &id)
}

pub fn duplicate_theme(id: String) -> Result<String, String> {
    keytao_app_core::theme_library::duplicate_theme(theme_library_host()?, &id)
}

pub fn rename_theme(id: String, name: String) -> Result<(), String> {
    keytao_app_core::theme_library::rename_theme(theme_library_host()?, &id, &name)
}

pub fn delete_theme(id: String) -> Result<(), String> {
    keytao_app_core::theme_library::delete_theme(theme_library_host()?, &id)
}

pub fn export_theme(id: String) -> Result<ThemeExportDto, String> {
    runtime::dto(keytao_app_core::theme_library::export_theme(
        theme_library_host()?,
        &id,
    )?)
}

pub fn preview_theme_import(text: String) -> Result<ThemeImportPreviewDto, String> {
    runtime::dto(keytao_app_core::theme_library::preview_theme_import(
        theme_library_host()?,
        &text,
    )?)
}

pub fn import_theme(text: String, overwrite: bool) -> Result<ThemeImportResultDto, String> {
    runtime::dto(keytao_app_core::theme_library::import_theme(
        theme_library_host()?,
        &text,
        overwrite,
    )?)
}

pub fn get_ime_ui_settings() -> Result<ImeUiSettingsDto, String> {
    let state = runtime::state()?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    state.host.require_default_theme_root()?;
    runtime::dto(
        keytao_app_core::ime_settings::get_ime_ui_settings(
            #[cfg(any(target_os = "android", target_os = "ios"))]
            state.host.clone(),
        )
        .map_err(|error| error.to_string())?,
    )
}

/// Resolve the candidate preview using pending controls, without initializing Core.
#[flutter_rust_bridge::frb(sync)]
pub fn resolve_candidate_panel_preview(
    color_scheme: UiColorSchemeDto,
    accent_color: String,
    font_size: f32,
    system_dark: bool,
    corner_radius: Option<f32>,
    background_color: String,
) -> Result<String, String> {
    let theme = keytao_theme::resolve_candidate_panel_preview_from_paths(
        keytao_theme::default_bundled_theme_path().as_deref(),
        keytao_theme::default_user_theme_path().as_deref(),
        if system_dark {
            keytao_theme::EffectiveColorScheme::Dark
        } else {
            keytao_theme::EffectiveColorScheme::Light
        },
        match color_scheme {
            UiColorSchemeDto::Auto => keytao_theme::UiColorScheme::Auto,
            UiColorSchemeDto::Light => keytao_theme::UiColorScheme::Light,
            UiColorSchemeDto::Dark => keytao_theme::UiColorScheme::Dark,
        },
        &accent_color,
        font_size,
        corner_radius,
        &background_color,
    )?;
    keytao_theme::resolved_renderer_theme_json(&theme).map_err(|error| error.to_string())
}

pub fn set_ime_ui_settings(
    color_scheme: UiColorSchemeDto,
    orientation: PanelOrientationDto,
    accent_color: Option<String>,
    font_size: f32,
    corner_radius: Option<f32>,
    background_color: String,
    changed_fields: Option<u8>,
) -> Result<ImeUiSettingsDto, String> {
    let state = runtime::state()?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    state.host.require_default_theme_root()?;
    runtime::dto(
        keytao_app_core::ime_settings::set_ime_ui_settings(
            #[cfg(any(target_os = "android", target_os = "ios"))]
            state.host.clone(),
            runtime::dto(color_scheme)?,
            runtime::dto(orientation)?,
            accent_color,
            font_size,
            corner_radius,
            background_color,
            changed_fields,
        )
        .map_err(|error| error.to_string())?,
    )
}

pub fn set_ime_embedded_composition(embedded: bool) -> Result<ImeUiSettingsDto, String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        runtime::state()?.host.require_default_theme_root()?;
        runtime::dto(
            keytao_app_core::ime_settings::set_ime_embedded_composition(embedded)
                .map_err(|error| error.to_string())?,
        )
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = embedded;
        Err("Embedded composition settings are desktop only".into())
    }
}

pub fn set_ime_hide_candidate_panel(hide: bool) -> Result<ImeUiSettingsDto, String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        runtime::state()?.host.require_default_theme_root()?;
        runtime::dto(
            keytao_app_core::ime_settings::set_ime_hide_candidate_panel(hide)
                .map_err(|error| error.to_string())?,
        )
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = hide;
        Err("Candidate panel visibility settings are desktop only".into())
    }
}

pub fn set_ime_show_tray_mode_indicator(show: bool) -> Result<ImeUiSettingsDto, String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        runtime::state()?.host.require_default_theme_root()?;
        runtime::dto(
            keytao_app_core::ime_settings::set_ime_show_tray_mode_indicator(show)
                .map_err(|error| error.to_string())?,
        )
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = show;
        Err("Tray mode indicator settings are desktop only".into())
    }
}

pub fn get_desktop_english_mode() -> Result<EnglishModeDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::get_desktop_english_mode(runtime::state()?.host.clone())
            .map_err(|error| error.to_string())?,
    )
}

pub fn get_candidate_page_size() -> Result<Option<u8>, String> {
    Ok(keytao_app_core::ime_settings::read_candidate_page_size(
        &runtime::state()?.host.user_root()?,
    ))
}

pub async fn preview_candidates(size: Option<u8>) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let preview =
        keytao_app_core::deploy::preview_candidates(&state.core, state.host.clone(), size).await?;
    serde_json::to_string(&preview).map_err(|error| error.to_string())
}

pub async fn set_candidate_page_size(size: Option<u8>) -> Result<Option<u8>, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    let updated = keytao_app_core::ime_settings::set_candidate_page_size(&root, size)?;
    if keytao_core::engine_choice::read(
        &root,
        keytao_core::engine_choice::active_package(&root).as_deref(),
    ) == keytao_core::engine_choice::PackageEngine::Libjd
    {
        state.host.write_reload_stamp(&root)?;
    }
    Ok(updated)
}

pub async fn set_desktop_english_mode(mode: EnglishModeDto) -> Result<EnglishModeDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::set_desktop_english_mode(
            runtime::state()?.host.clone(),
            runtime::dto(mode)?,
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

#[derive(serde::Deserialize)]
pub struct AddonStatusDto {
    pub installed: bool,
    pub deployed: bool,
    pub version: String,
    pub variant: Option<String>,
    #[serde(default)]
    pub english_available: bool,
    pub latest_version: Option<String>,
}

pub fn addon_schema_status(id: String) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::addon::addon_schema_status(&state.core, || state.host.user_root(), id)
            .map_err(|error| error.to_string())?,
    )
}

/// android_deploy invokes deployImeData and returns "" on success or error text on failure,
/// catching Dart exceptions. Do not re-enter bridge install/deploy calls from
/// the callback: the install lock is held. macOS ignores the callback and uses
/// the existing bridge deploy. InstallProgress continues through core_events.
pub async fn addon_schema_install(
    id: String,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::addon::addon_schema_install(&state.core, id, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

/// Uses the same per-call deploy contract as addon_schema_install.
/// Core owns schema removal, English-mode reset and progress events.
pub async fn addon_schema_uninstall(
    id: String,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::addon::addon_schema_uninstall(&state.core, id, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub fn wanxiang_status() -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::wanxiang::wanxiang_status(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

#[derive(serde::Deserialize)]
pub struct WanxiangGrammarStatusDto {
    pub available: bool,
    pub installed: bool,
    pub version: String,
    pub size: u64,
}

pub fn wanxiang_grammar_status() -> Result<WanxiangGrammarStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::wanxiang::grammar_status_for(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

pub fn cancel_wanxiang_grammar_download() -> Result<(), String> {
    runtime::state()?.core.cancel_download();
    Ok(())
}

/// Uses the add-on deploy contract and holds the install lock through rollback.
pub async fn manage_wanxiang_grammar(
    installed: bool,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<WanxiangGrammarStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::wanxiang::manage_grammar(&state.core, installed, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub fn rime_ice_status() -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::rime_ice::rime_ice_status(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

pub fn rime_mint_status() -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::rime_mint::rime_mint_status(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

pub fn rime_frost_status() -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::rime_frost::rime_frost_status(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

pub fn rime_lssp_status() -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::rime_lssp::rime_lssp_status(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

/// A failed upstream check leaves the locally installed status available.
pub async fn check_addon_update(id: String) -> Result<AddonStatusDto, String> {
    let mut status = match id.as_str() {
        "easy_en" => addon_schema_status(id.clone())?,
        "wanxiang" => wanxiang_status()?,
        "rime_ice" => rime_ice_status()?,
        "rime_mint" => rime_mint_status()?,
        "rime_frost" => rime_frost_status()?,
        "triple_pinyin_lssp" => rime_lssp_status()?,
        _ => return Err(format!("Unsupported add-on: {id}")),
    };
    status.latest_version = match id.as_str() {
        "easy_en" => keytao_app_core::addon::latest_version().await.ok(),
        "rime_ice" => keytao_app_core::rime_ice::latest_version().await.ok(),
        "rime_mint" => keytao_app_core::rime_mint::latest_version().await.ok(),
        "rime_frost" => keytao_app_core::rime_frost::latest_version().await.ok(),
        "triple_pinyin_lssp" => keytao_app_core::rime_lssp::latest_version().await.ok(),
        _ => keytao_app_core::wanxiang::latest_version().await.ok(),
    };
    Ok(status)
}

/// Uses addon_schema_install's deploy contract. Core keeps its receipt/rollback
/// flow unchanged and may call android_deploy again after restoring files.
/// The install lock covers the whole operation, including rollback/deployment.
pub async fn manage_wanxiang(
    installed: bool,
    variant: String,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::wanxiang::manage_wanxiang(&state.core, installed, variant, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

/// Uses addon_schema_install's deploy contract and holds the install lock through rollback.
pub async fn manage_rime_ice(
    installed: bool,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::rime_ice::manage_rime_ice(&state.core, installed, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

/// Uses addon_schema_install's deploy contract and holds the install lock through rollback.
pub async fn manage_rime_mint(
    installed: bool,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::rime_mint::manage_rime_mint(&state.core, installed, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

/// Uses addon_schema_install's deploy contract and holds the install lock through rollback.
pub async fn manage_rime_frost(
    installed: bool,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::rime_frost::manage_rime_frost(&state.core, installed, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

/// Holds the install lock through deployment and rollback.
pub async fn manage_rime_lssp(
    installed: bool,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::rime_lssp::manage_rime_lssp(&state.core, installed, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub fn macos_ime_status() -> Result<MacosImeStatusDto, String> {
    runtime::dto(keytao_app_core::ime_status::macos::macos_ime_status(
        &runtime::state()?.core,
    ))
}

pub async fn windows_ime_status() -> Result<WindowsImeStatusDto, String> {
    #[cfg(target_os = "windows")]
    {
        runtime::dto(
            keytao_app_core::ime_status::windows::windows_ime_status(
                runtime::state()?.core.clone(),
            )
            .await
            .map_err(|error| error.to_string())?,
        )
    }
    #[cfg(not(target_os = "windows"))]
    Err("Windows IME is only available on Windows".into())
}

pub async fn windows_ime_ensure_registered() -> Result<WindowsImeStatusDto, String> {
    #[cfg(target_os = "windows")]
    {
        runtime::dto(
            keytao_app_core::ime_status::windows::windows_ime_ensure_registered(
                runtime::state()?.core.clone(),
            )
            .await
            .map_err(|error| error.to_string())?,
        )
    }
    #[cfg(not(target_os = "windows"))]
    Err("Windows IME is only available on Windows".into())
}

pub async fn windows_prepare_search_schemas() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        keytao_app_core::windows_public_schemas::prepare_search_schemas(&state.core).await
    }
    #[cfg(not(target_os = "windows"))]
    Err("Windows IME is only available on Windows".into())
}

/// Forward initial and single-instance arguments after init_core. Core retains
/// redeploy requests until acknowledged; the event wakes the Dart UI for open
/// and redeploy URLs. The native shell remains responsible for window focus.
pub fn handle_app_args(args: Vec<String>) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let state = runtime::state()?;
        state.host.actions.receive_args(args);
        state.core.emit(CoreEvent::WindowsImeAction(()));
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = args;
        Err("App URL actions are only available on Windows".into())
    }
}

pub fn windows_pending_ime_action() -> Result<Option<u32>, String> {
    #[cfg(target_os = "windows")]
    {
        Ok(runtime::state()?.host.actions.pending())
    }
    #[cfg(not(target_os = "windows"))]
    Err("Windows IME is only available on Windows".into())
}

pub fn windows_dismiss_ime_action(id: u32) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        runtime::state()?.host.actions.dismiss(id);
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = id;
        Err("Windows IME is only available on Windows".into())
    }
}

pub async fn windows_redeploy_ime_action(id: u32) -> Result<DeployResultDto, String> {
    #[cfg(target_os = "windows")]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
        state.host.require_default_windows_root()?;
        runtime::dto(
            keytao_app_core::app_actions::windows_redeploy_ime_action(
                &state.core,
                state.host.clone(),
                &state.host.actions,
                id,
            )
            .await
            .map_err(|error| error.to_string())?,
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = id;
        Err("Windows IME is only available on Windows".into())
    }
}

pub fn linux_ime_status() -> Result<LinuxImeStatusDto, String> {
    #[cfg(target_os = "linux")]
    {
        let state = runtime::state()?;
        runtime::dto(keytao_app_core::ime_status::linux::linux_ime_status(
            &state.core,
            &state.host,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    Err("Linux IME is only available on Linux".into())
}

/// The daemon outlives the app; there is intentionally no stop-on-exit hook.
pub fn linux_start_ime() -> Result<LinuxImeStatusDto, String> {
    #[cfg(target_os = "linux")]
    {
        let state = runtime::state()?;
        runtime::dto(keytao_app_core::ime_status::linux::launch_keytao_ime(
            &state.core,
            &state.host,
            false,
        )?)
    }
    #[cfg(not(target_os = "linux"))]
    Err("Linux IME is only available on Linux".into())
}

pub fn get_component_versions() -> Result<ComponentVersionsDto, String> {
    let state = runtime::state()?;
    let mut versions: ComponentVersionsDto =
        runtime::dto(keytao_app_core::component_versions::get_component_versions(
            &state.core,
            &state.host,
            option_env!("RIME_VERSION"),
            option_env!("OPENCC_VERSION"),
        ))?;
    versions.data_dir = Some(state.host.user_root()?.to_string_lossy().into_owned());
    Ok(versions)
}

pub async fn get_runtime_log_settings() -> Result<RuntimeLogSettingsDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::logs::get_runtime_log_settings(&state.core, &state.host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub async fn set_runtime_log_settings(
    enabled: bool,
    level: RuntimeLogLevelDto,
) -> Result<(), String> {
    let state = runtime::state()?;
    keytao_app_core::logs::set_runtime_log_settings(
        &state.core,
        &state.host,
        enabled,
        runtime::dto(level)?,
    )
    .await
    .map_err(|error| error.to_string())
}

pub async fn read_runtime_log(max_lines: Option<u32>) -> Result<DebugLogFileDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::logs::read_runtime_log(
            &state.core,
            &state.host,
            max_lines.map(|value| value as usize),
        )
        .await
        .map_err(|error| error.to_string())?,
    )
}

pub async fn read_debug_logs() -> Result<DebugLogsDto, String> {
    runtime::dto(
        keytao_app_core::logs::read_debug_logs(&runtime::state()?.core)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub async fn clear_runtime_log() -> Result<(), String> {
    let state = runtime::state()?;
    keytao_app_core::logs::clear_runtime_log(&state.core, &state.host)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_preview_resolves_pending_settings_and_rejects_invalid_inputs() {
        let json = resolve_candidate_panel_preview(
            UiColorSchemeDto::Auto,
            "#123456".into(),
            15.0,
            true,
            Some(24.0),
            "#654321".into(),
        )
        .unwrap();
        let theme: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(theme["ui"]["colorScheme"], "auto");
        assert_eq!(theme["ui"]["effectiveColorScheme"], "dark");
        assert_eq!(theme["font"]["size"], 15.0);
        assert_eq!(theme["candidate"]["selectedLabelColor"]["red"], 0x12);
        assert_eq!(theme["panel"]["cornerRadius"], 24.0);
        assert_eq!(theme["panel"]["background"]["red"], 0x65);
        assert!(resolve_candidate_panel_preview(
            UiColorSchemeDto::Light,
            "invalid".into(),
            20.0,
            false,
            None,
            String::new(),
        )
        .is_err());
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(resolve_candidate_panel_preview(
                UiColorSchemeDto::Light,
                "#123456".into(),
                invalid,
                false,
                None,
                String::new(),
            )
            .is_err());
        }
    }

    fn config() -> BridgeConfig {
        // All tests share the process-wide Core and keep its directories alive.
        static TEMP: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        let temp = TEMP.get_or_init(|| tempfile::tempdir().unwrap());
        let root = temp.path().join("user");
        BridgeConfig {
            data_dir: temp.path().join("state").to_string_lossy().into_owned(),
            cache_dir: temp.path().join("cache").to_string_lossy().into_owned(),
            resource_dir: temp.path().join("resources").to_string_lossy().into_owned(),
            app_version: "test".into(),
            platform: if cfg!(target_os = "android") {
                BridgePlatform::Android
            } else if cfg!(target_os = "ios") {
                BridgePlatform::Ios
            } else if cfg!(target_os = "windows") {
                BridgePlatform::Windows
            } else if cfg!(target_os = "macos") {
                BridgePlatform::MacOs
            } else {
                BridgePlatform::Linux
            },
            user_root_override: Some(root.to_string_lossy().into_owned()),
        }
    }

    #[test]
    fn init_core_reuses_same_config() {
        let first = init_core(config()).unwrap();
        complete_onboarding().unwrap();
        let second = init_core(config()).unwrap();
        assert_eq!(second.user_root, first.user_root);
        assert_eq!(second.platform, first.platform);
        assert_eq!(second.app_version, first.app_version);
        assert!(onboarding().unwrap().completed);

        let mut updated_version = config();
        updated_version.app_version = "new-engine".into();
        let existing = init_core(updated_version).unwrap();
        assert_eq!(existing.app_version, first.app_version);

        let mut equivalent_root = config();
        equivalent_root
            .user_root_override
            .as_mut()
            .unwrap()
            .push('/');
        let existing = init_core(equivalent_root).unwrap();
        assert_eq!(existing.user_root, first.user_root);
    }

    #[test]
    fn init_core_rejects_changed_config() {
        init_core(config()).unwrap();
        for field in [
            "data_dir",
            "cache_dir",
            "resource_dir",
            "platform",
            "user_root",
        ] {
            let mut changed = config();
            match field {
                "data_dir" => changed.data_dir.push_str("-other"),
                "cache_dir" => changed.cache_dir.push_str("-other"),
                "resource_dir" => changed.resource_dir.push_str("-other"),
                "platform" => {
                    changed.platform = if changed.platform == BridgePlatform::Android {
                        BridgePlatform::Ios
                    } else {
                        BridgePlatform::Android
                    };
                }
                "user_root" => changed
                    .user_root_override
                    .as_mut()
                    .unwrap()
                    .push_str("-other"),
                _ => unreachable!(),
            }
            assert!(
                matches!(init_core(changed), Err(error) if error.contains("already initialized")),
                "changing {field} must require a process restart"
            );
        }
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    #[tokio::test]
    async fn android_settings_change_one_field_preserves_other_fields() {
        init_core(config()).unwrap();
        let before = get_android_ime_input_settings().await.unwrap();
        let mut edited = before.clone();
        edited.haptics_enabled = !edited.haptics_enabled;
        set_android_ime_input_settings(edited.clone())
            .await
            .unwrap();
        let after = get_android_ime_input_settings().await.unwrap();
        assert_eq!(after, edited);
    }
}
