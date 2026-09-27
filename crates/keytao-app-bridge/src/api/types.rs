#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgePlatform {
    Windows,
    MacOs,
    Linux,
    Android,
    Ios,
}

pub struct BridgeConfig {
    pub data_dir: String,
    pub cache_dir: String,
    pub resource_dir: String,
    pub app_version: String,
    pub platform: BridgePlatform,
    /// An absolute path. When set, all bridge hosts use this root exclusively.
    pub user_root_override: Option<String>,
}

pub struct BridgeInfo {
    pub platform: BridgePlatform,
    pub app_version: String,
    pub user_root: String,
}

pub struct LocalSchemaDto {
    pub installed: bool,
    pub deployed: bool,
    pub version: Option<String>,
    pub schemas: Vec<String>,
}

pub struct OnboardingDto {
    pub completed: bool,
}

pub struct SchemeReleaseDto {
    pub scheme: String,
    pub source_type: Option<String>,
    pub label: String,
    pub version: String,
    pub name: String,
    pub published_at: Option<String>,
    pub download_url: String,
    pub asset_name: String,
}

pub struct VerifyEntryDto {
    pub path: String,
    pub ok: bool,
    pub note: String,
}

pub struct InstallResultDto {
    pub merged_schemas: Vec<String>,
    pub logs: Vec<String>,
    pub verify: Vec<VerifyEntryDto>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BridgeEventKind {
    /// Subscription handshake; subsequent events come from Core's EventSink.
    Ready,
    InstallProgress,
    DeployProgress,
    WindowsImeStatus,
    WindowsImeAction,
}

pub struct InstallProgressDto {
    pub stage: String,
    pub percent: u32,
    pub message: String,
}

pub struct WindowsImeStatusDto {
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

/// Exactly one payload is present for progress/status events; action/ready have none.
/// Plain structs avoid requiring any Dart code-generation packages.
pub struct BridgeEvent {
    pub kind: BridgeEventKind,
    pub install_progress: Option<InstallProgressDto>,
    pub deploy_progress: Option<String>,
    pub windows_ime_status: Option<WindowsImeStatusDto>,
}
