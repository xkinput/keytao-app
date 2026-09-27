use crate::{api::types::*, frb_generated::StreamSink, host::BridgeHost};
use keytao_app_core::{AppEnv, Core, CoreEvent, EventSink, Platform};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

static STATE: OnceLock<BridgeState> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct BridgeState {
    pub core: Arc<Core>,
    pub host: BridgeHost,
    pub events: Arc<BridgeEvents>,
    pub install_lock: tokio::sync::Mutex<()>,
}

pub(crate) fn state() -> Result<&'static BridgeState, String> {
    STATE
        .get()
        .ok_or_else(|| "Core is not initialized; call init_core first".into())
}

pub(crate) fn initialize(config: BridgeConfig) -> Result<BridgeInfo, String> {
    let _guard = INIT_LOCK
        .lock()
        .map_err(|_| "Core initialization lock poisoned")?;
    if STATE.get().is_some() {
        return Err("Core is already initialized; restart the process to change config".into());
    }
    let root = BridgeHost::resolve_root(config.user_root_override.as_deref())?;
    let platform = match config.platform {
        BridgePlatform::Windows => Platform::Windows,
        BridgePlatform::MacOs => Platform::MacOs,
        BridgePlatform::Linux => Platform::Linux,
        BridgePlatform::Android => Platform::Android,
        BridgePlatform::Ios => Platform::Ios,
    };
    let events = Arc::new(BridgeEvents::default());
    let core = Core::new(
        AppEnv {
            data_dir: absolute_dir(&config.data_dir)?,
            cache_dir: absolute_dir(&config.cache_dir)?,
            resource_dir: absolute_dir(&config.resource_dir)?,
            app_version: config.app_version.clone(),
            platform,
        },
        events.clone(),
    )
    .map_err(|error| error.to_string())?;
    core.initialize_onboarding(Some(root.clone()))
        .map_err(|error| error.to_string())?;
    let info = BridgeInfo {
        platform: config.platform,
        app_version: config.app_version,
        user_root: root.to_string_lossy().into_owned(),
    };
    let host = BridgeHost::new(core.clone(), root);
    STATE
        .set(BridgeState {
            core,
            host,
            events,
            install_lock: tokio::sync::Mutex::new(()),
        })
        .map_err(|_| "Core is already initialized")?;
    Ok(info)
}

pub(crate) fn absolute_dir(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("Bridge directories must be non-empty absolute paths".into());
    }
    Ok(path)
}

#[derive(Default)]
pub(crate) struct BridgeEvents(Mutex<Option<StreamSink<BridgeEvent>>>);

impl BridgeEvents {
    pub fn register(&self, sink: StreamSink<BridgeEvent>) -> Result<(), String> {
        let mut current = self.0.lock().map_err(|_| "Event stream lock poisoned")?;
        sink.add(BridgeEvent {
            kind: BridgeEventKind::Ready,
            install_progress: None,
            deploy_progress: None,
            windows_ime_status: None,
        })
        .map_err(|error| error.to_string())?;
        *current = Some(sink);
        Ok(())
    }
}

impl EventSink for BridgeEvents {
    fn emit(&self, event: CoreEvent) {
        let mut current = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(sink) = current.as_ref() {
            if sink.add(event.into()).is_err() {
                *current = None;
            }
        }
    }
}

impl From<CoreEvent> for BridgeEvent {
    fn from(event: CoreEvent) -> Self {
        let mut dto = Self {
            kind: BridgeEventKind::WindowsImeAction,
            install_progress: None,
            deploy_progress: None,
            windows_ime_status: None,
        };
        match event {
            CoreEvent::InstallProgress(progress) => {
                dto.kind = BridgeEventKind::InstallProgress;
                dto.install_progress = Some(InstallProgressDto {
                    stage: progress.stage,
                    percent: progress.percent,
                    message: progress.message,
                });
            }
            CoreEvent::DeployProgress(message) => {
                dto.kind = BridgeEventKind::DeployProgress;
                dto.deploy_progress = Some(message);
            }
            CoreEvent::WindowsImeStatus(status) => {
                dto.kind = BridgeEventKind::WindowsImeStatus;
                dto.windows_ime_status = Some(WindowsImeStatusDto {
                    supported: status.supported,
                    packaged: status.packaged,
                    registered: status.registered,
                    registered_dll: status.registered_dll,
                    profile_enabled: status.profile_enabled,
                    registration_busy: status.registration_busy,
                    registration_state: status.registration_state,
                    registration_error: status.registration_error,
                    runtime_dir: status.runtime_dir,
                    dll_path: status.dll_path,
                    registered_path: status.registered_path,
                    profile_status: status.profile_status,
                    user_data_dir: status.user_data_dir,
                    shared_data_dir: status.shared_data_dir,
                    shared_data_source: status.shared_data_source,
                    reload_stamp_path: status.reload_stamp_path,
                    reload_stamp_signature: status.reload_stamp_signature,
                    message: status.message,
                });
            }
            CoreEvent::WindowsImeAction(()) => {}
        }
        dto
    }
}
