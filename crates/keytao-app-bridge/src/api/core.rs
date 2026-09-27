use crate::{api::types::*, frb_generated::StreamSink, runtime};
use keytao_app_core::{events::InstallProgress, scheme_host::SchemeHost, CoreEvent};

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
    Ok(LocalSchemaDto {
        installed: local.installed,
        deployed: local.deployed,
        version: local.version,
        schemas: local.schemas,
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
    let state = runtime::state()?;
    // Core's downloader uses a fixed cache filename; serialize installations.
    let _guard = state.install_lock.lock().await;
    let root = state.host.user_root()?;
    state.core.emit(CoreEvent::InstallProgress(InstallProgress {
        stage: "fetching".into(),
        percent: 0,
        message: "正在获取方案版本...".into(),
    }));
    let release = fetch_scheme_release(scheme).await?;
    #[cfg(target_os = "windows")]
    return Err("Windows scheme publishing is unsupported in bridge".into());
    #[cfg(not(target_os = "windows"))]
    {
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let archive =
            keytao_app_core::download::download_to_temp(&state.core, release.download_url)
                .await
                .map_err(|error| error.to_string())?;
        // Never call rime_install_to_default: it bypasses the host on desktop.
        let result = keytao_app_core::install::smart_install(
            &state.core,
            archive.clone(),
            root.to_string_lossy().into_owned(),
        )
        .await;
        // Core removes the archive on success; also remove failed downloads here.
        let _ = std::fs::remove_file(archive);
        let result = result.map_err(|error| error.to_string())?;
        Ok(InstallResultDto {
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
        })
    }
}
