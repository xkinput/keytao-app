use keytao_core::runtime_log::AppOperation;
use std::time::Duration;

pub enum AppOperationName {
    ConnectEvents,
    StartPackageMigration,
    Onboarding,
    RefreshState,
    ReadState,
    RefreshOnResume,
    RefreshLiveSchemaSwitches,
    PreviewCandidates,
    LoadSystemFont,
    OpenWebsite,
}

#[flutter_rust_bridge::frb(opaque)]
pub struct AppOperationSpan {
    _operation: AppOperation,
}

#[flutter_rust_bridge::frb(sync)]
pub fn start_app_operation(operation: AppOperationName) -> Option<AppOperationSpan> {
    let name = match operation {
        AppOperationName::ConnectEvents => "connect_events",
        AppOperationName::StartPackageMigration => "start_package_migration",
        AppOperationName::Onboarding => "onboarding",
        AppOperationName::RefreshState => "refresh_state",
        AppOperationName::ReadState => "read_state",
        AppOperationName::RefreshOnResume => "refresh_on_resume",
        AppOperationName::RefreshLiveSchemaSwitches => "refresh_live_schema_switches",
        AppOperationName::PreviewCandidates => "preview_candidates",
        AppOperationName::LoadSystemFont => "load_system_font",
        AppOperationName::OpenWebsite => "open_website",
    };
    AppOperation::start(name).map(|operation| AppOperationSpan {
        _operation: operation,
    })
}

#[flutter_rust_bridge::frb(sync)]
pub fn record_bootstrap_timings(rust_lib_init_us: u32, bridge_config_us: u32, init_core_us: u32) {
    for (name, micros) in [
        ("rust_lib_init", rust_lib_init_us),
        ("bridge_config", bridge_config_us),
        ("init_core", init_core_us),
    ] {
        AppOperation::record_timing(name, Duration::from_micros(u64::from(micros)));
    }
}
