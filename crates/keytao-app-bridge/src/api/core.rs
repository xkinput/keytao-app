use crate::{api::types::*, frb_generated::StreamSink, host::InstallHost, runtime};
use flutter_rust_bridge::DartFnFuture;
use keytao_app_core::scheme_host::SchemeHost;
#[cfg(not(target_os = "android"))]
use keytao_app_core::{events::InstallProgress, CoreEvent};

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

/// Creates the process-wide Core. Every subsequent call returns an error,
/// including calls with identical config. Restart the process to change roots.
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
    let local =
        keytao_app_core::scheme::check_local_schema(&state.core, Some(state.host.user_root()?))
            .map_err(|error| error.to_string())?;
    let app_state = state.core.state();
    let installed_scheme_version = app_state
        .active_scheme
        .as_ref()
        .and_then(|scheme| app_state.installed_packages.get(scheme))
        .and_then(|package| package.version.clone());
    Ok(LocalSchemaDto {
        installed: local.installed,
        deployed: local.deployed,
        version: local.version,
        schemas: local.schemas,
        installed_scheme: app_state.active_scheme,
        installed_scheme_version,
    })
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

pub fn record_installed_scheme(scheme: String, version: Option<String>) -> Result<(), String> {
    keytao_app_core::scheme::record_installed_scheme(
        &runtime::state()?.core,
        &scheme,
        version.as_deref(),
    )
    .map_err(|error| error.to_string())
}

pub fn list_installed_packages() -> Result<Vec<InstalledPackageDto>, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::packages::list_installed_packages(&state.core)
            .map_err(|error| error.to_string())?,
    )
}

pub async fn import_local_scheme(
    directory_path: String,
    display_name: String,
    scheme_key: Option<String>,
) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::packages::import_local_scheme(
        &state.core,
        std::path::Path::new(&directory_path),
        &display_name,
        scheme_key.as_deref(),
    )
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
    let preview = keytao_app_core::cloud_packages::preview(&state.core, &scheme_key, &state.host.user_root()?)
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&preview).map_err(|error| error.to_string())
}

pub async fn upload_cloud_package(scheme_key: String, expected_sha256: Option<String>) -> Result<String, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let package = keytao_app_core::cloud_packages::upload(&state.core, &scheme_key, &state.host.user_root()?, expected_sha256.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&package).map_err(|error| error.to_string())
}

/// Records only the result returned by Android's deployImeData channel.
pub fn record_android_deploy(success: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = runtime::state()?;
        keytao_app_core::deploy::record_android_result(&state.core, &state.host.user_root()?, success)
            .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "android"))]
    { let _ = success; Err("Android deployment result is only available on Android".into()) }
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
        keytao_app_core::user_words::lookup_user_word(
            &state.core,
            &scheme_key,
            &state.host.user_root()?,
            &word,
            &code,
        )
        .map_err(|error| error.to_string())?,
    )
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

/// Android extraction must return the platform result as JSON and must not
/// re-enter install/deploy bridge calls while the install lock is held.
/// Other platforms ignore the callback. Activation does not deploy the IME.
pub async fn activate_scheme(
    scheme_key: String,
    android_extract: impl Fn(String) -> DartFnFuture<String> + Send + Sync + 'static,
) -> Result<InstallResultDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    #[cfg(not(target_os = "android"))]
    let _ = android_extract;
    let result = keytao_app_core::packages::activate_scheme(
        &state.core,
        &scheme_key,
        &root,
        #[cfg(target_os = "android")]
        move |archive| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
        > {
            let extraction = android_extract(archive);
            Box::pin(async move {
                serde_json::from_str(&extraction.await).map_err(|error| error.to_string())
            })
        },
        #[cfg(target_os = "windows")]
        keytao_app_core::install::finish_windows_scheme_install,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(install_result_dto(result))
}

pub async fn remove_scheme_package(scheme_key: String) -> Result<(), String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    keytao_app_core::packages::remove_scheme_package(&state.core, &scheme_key)
        .map_err(|error| error.to_string())
}

/// The persisted account, if a token is stored. The token never leaves Rust.
pub fn account_session() -> Result<Option<AccountUserDto>, String> {
    let state = runtime::state()?.core.state();
    match (state.auth_token, state.user) {
        (Some(_), Some(user)) => Ok(Some(runtime::dto(user)?)),
        _ => Ok(None),
    }
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
                    })
                    .collect(),
                server_deletions: confirmed
                    .server_deletions
                    .into_iter()
                    .map(|entry| keytao_app_core::account::UserDictionaryDeletion {
                        word: entry.word,
                        code: entry.code,
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
            })
            .collect(),
        server_deletions: result
            .server_deletions
            .into_iter()
            .map(|entry| UserDictionaryDeletionDto {
                word: entry.word,
                code: entry.code,
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
    let archive = keytao_app_core::download::download_to_temp(&state.core, url)
        .await
        .map_err(|error| error.to_string())?;
    let result = keytao_app_core::install::smart_install(
        &state.core,
        archive.clone(),
        dir,
        #[cfg(target_os = "windows")]
        keytao_app_core::install::finish_windows_scheme_install,
    )
    .await;
    let _ = std::fs::remove_file(archive);
    let result = result.map_err(|error| error.to_string())?;
    let active = root
        .canonicalize()
        .ok()
        .zip(state.host.user_root()?.canonicalize().ok())
        .is_some_and(|(root, default_root)| root == default_root);
    keytao_app_core::packages::record_package(&state.core, &scheme_key, version.as_deref(), active)
        .map_err(|error| error.to_string())?;
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
    let path = runtime::absolute_dir(&path)?;
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
        // Install into the explicit host directory.
        let result = keytao_app_core::install::smart_install(
            &state.core,
            archive.clone(),
            root.to_string_lossy().into_owned(),
            #[cfg(target_os = "windows")]
            keytao_app_core::install::finish_windows_scheme_install,
        )
        .await;
        // Core removes the archive on success; also remove failed downloads here.
        let _ = std::fs::remove_file(archive);
        let result = result.map_err(|error| error.to_string())?;
        keytao_app_core::packages::record_package(
            &state.core,
            &scheme_key,
            version.as_deref(),
            true,
        )
        .map_err(|error| error.to_string())?;
        Ok(install_result_dto(result))
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

/// Call only after the Android channel confirms storage permission. Dart must
/// serialize prepare -> smartExtractZipToPrivate -> finish as one installation.
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
pub fn finish_android_install(
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
        let active = result["packageRoot"]
            .as_str()
            .and_then(|root| std::path::Path::new(root).canonicalize().ok())
            .zip(state.host.user_root()?.canonicalize().ok())
            .is_some_and(|(root, default_root)| root == default_root);
        let result = keytao_app_core::install::finish_android_install(&state.core, &result)
            .map_err(|error| error.to_string())?;
        keytao_app_core::packages::record_package(
            &state.core,
            &scheme_key,
            version.as_deref(),
            active,
        )
        .map_err(|error| error.to_string())?;
        Ok(install_result_dto(result))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (result_json, scheme_key, version);
        Err("Android installation is only available on Android".into())
    }
}

/// Android deployment belongs to the deployImeData platform channel.
pub async fn deploy_default() -> Result<DeployResultDto, String> {
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "ios"
    ))]
    {
        let state = runtime::state()?;
        let _guard = state.install_lock.lock().await;
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

/// Saves all 23 writable fields; returned path/message fields are read-only.
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
            settings.floating_portrait_enabled,
            settings.floating_portrait_scale,
            settings.floating_landscape_enabled,
            settings.floating_landscape_scale,
        )
        .await
        .map_err(|error| error.to_string())?,
    )
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

pub fn set_ime_ui_settings(
    color_scheme: UiColorSchemeDto,
    orientation: PanelOrientationDto,
    accent_color: String,
    font_size: f32,
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

pub fn get_desktop_english_mode() -> Result<EnglishModeDto, String> {
    runtime::dto(
        keytao_app_core::ime_settings::get_desktop_english_mode(runtime::state()?.host.clone())
            .map_err(|error| error.to_string())?,
    )
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

pub fn addon_schema_status(id: String) -> Result<AddonSchemaStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::addon::addon_schema_status(&state.core, || state.host.user_root(), id)
            .map_err(|error| error.to_string())?,
    )
}

/// Android: await copyAddonSchemaAssets(id) through the platform channel first;
/// this call's host treats copy_addon_assets as already complete. android_deploy
/// must await deployImeData and return "" on success or error text on failure,
/// catching Dart exceptions. Do not re-enter bridge install/deploy calls from
/// the callback: the install lock is held. macOS ignores the callback and uses
/// the existing bridge deploy. InstallProgress continues through core_events.
pub async fn addon_schema_install(
    id: String,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonSchemaStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::addon::addon_schema_install(&state.core, id, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

/// Uses the same per-call deploy contract as addon_schema_install; no asset copy
/// is needed. Core owns schema removal, English-mode reset and progress events.
pub async fn addon_schema_uninstall(
    id: String,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<AddonSchemaStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::addon::addon_schema_uninstall(&state.core, id, &host)
            .await
            .map_err(|error| error.to_string())?,
    )
}

pub fn wanxiang_status() -> Result<WanxiangStatusDto, String> {
    let state = runtime::state()?;
    runtime::dto(
        keytao_app_core::wanxiang::wanxiang_status(&state.core, &state.host.user_root()?)
            .map_err(|error| error.to_string())?,
    )
}

/// Uses addon_schema_install's deploy contract. Core keeps its receipt/rollback
/// flow unchanged and may call android_deploy again after restoring files.
/// The install lock covers the whole operation, including rollback/deployment.
pub async fn manage_wanxiang(
    installed: bool,
    android_deploy: impl Fn() -> DartFnFuture<String> + Send + Sync,
) -> Result<WanxiangStatusDto, String> {
    let state = runtime::state()?;
    let _guard = state.install_lock.lock().await;
    let host = InstallHost::new(state.host.clone(), android_deploy);
    runtime::dto(
        keytao_app_core::wanxiang::manage_wanxiang(&state.core, installed, &host)
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
pub fn linux_start_ime(restart: bool) -> Result<LinuxImeStatusDto, String> {
    #[cfg(target_os = "linux")]
    {
        let state = runtime::state()?;
        runtime::dto(keytao_app_core::ime_status::linux::launch_keytao_ime(
            &state.core,
            &state.host,
            restart,
        )?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = restart;
        Err("Linux IME is only available on Linux".into())
    }
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

#[cfg(all(test, any(target_os = "android", target_os = "ios")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn android_settings_change_one_field_preserves_the_other_22() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("user");
        init_core(BridgeConfig {
            data_dir: temp.path().join("state").to_string_lossy().into_owned(),
            cache_dir: temp.path().join("cache").to_string_lossy().into_owned(),
            resource_dir: temp.path().join("resources").to_string_lossy().into_owned(),
            app_version: "test".into(),
            platform: if cfg!(target_os = "android") {
                BridgePlatform::Android
            } else {
                BridgePlatform::Ios
            },
            user_root_override: Some(root.to_string_lossy().into_owned()),
        })
        .unwrap();
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
