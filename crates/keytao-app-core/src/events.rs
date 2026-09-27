use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String,
    pub percent: u32,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct WindowsImeStatus {
    pub supported: bool,
    pub packaged: bool,
    pub registered: bool,
    pub registered_dll: bool,
    pub profile_enabled: bool,
    pub registration_busy: bool,
    pub registration_state: String,
    pub registration_error: Option<String>,
    pub runtime_dir: Option<String>,
    pub dll_path: Option<String>,
    pub registered_path: Option<String>,
    pub profile_status: String,
    pub user_data_dir: Option<String>,
    pub shared_data_dir: Option<String>,
    pub shared_data_source: String,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub message: String,
}

pub enum CoreEvent {
    InstallProgress(InstallProgress),
    DeployProgress(String),
    WindowsImeStatus(WindowsImeStatus),
    WindowsImeAction(()),
}

impl CoreEvent {
    pub fn tauri_name(&self) -> &'static str {
        match self {
            Self::InstallProgress(_) => "install-progress",
            Self::DeployProgress(_) => "deploy-progress",
            Self::WindowsImeStatus(_) => "windows-ime-status",
            Self::WindowsImeAction(()) => "windows-ime-action",
        }
    }
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: CoreEvent);
}
