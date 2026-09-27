//! Windows URL actions from the TSF language-bar menu.
//!
//! Keep requests until the webview is ready. Events are only wakeups, so a cold
//! start (or an event arriving before listen()) cannot lose a redeploy request.

use std::sync::Arc;
use keytao_app_core::{Core, CoreEvent};
pub(crate) use keytao_app_core::app_actions::AppActions;
use tauri::{AppHandle, Emitter, Manager};

pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Call before registering any other plugin: the plugin exits secondary
/// processes before they initialize an engine or another application's window.
pub(crate) fn configure(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    let actions = AppActions::default();
    actions.receive_args(std::env::args_os().filter_map(|argument| argument.into_string().ok()));
    builder
        .manage(actions.clone())
        .plugin(tauri_plugin_single_instance::init(
            move |app, args, _cwd| {
                actions.receive_args(args);
                show_main_window(app);
                if let Some(core) = app.try_state::<Arc<Core>>() {
                    core.emit(CoreEvent::WindowsImeAction(()));
                } else {
                    // The single-instance window can receive messages before
                    // app setup manages Core. Preserve the immediate wakeup.
                    let _ = app.emit("windows-ime-action", ());
                }
            },
        ))
}

#[tauri::command]
pub(crate) fn windows_pending_ime_action(actions: tauri::State<AppActions>) -> Option<u32> {
    actions.pending()
}

/// Acknowledge a request whose UI displayed an error before deployment could
/// start (for example no installed schema). Never dismiss a newer request.
#[tauri::command]
pub(crate) fn windows_dismiss_ime_action(actions: tauri::State<AppActions>, id: u32) {
    actions.dismiss(id);
}

#[tauri::command]
pub(crate) async fn windows_redeploy_ime_action(
    app: AppHandle,
    id: u32,
) -> Result<super::DeployResult, String> {
    keytao_app_core::app_actions::windows_redeploy_ime_action(
        &app.state::<Arc<Core>>(), super::ShellImeHost(app.clone()), &app.state::<AppActions>(), id,
    ).await.map_err(|e| e.to_string())
}
