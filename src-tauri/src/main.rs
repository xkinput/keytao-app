// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        if desktop.to_lowercase().contains("kde") {
            // Keep the GUI itself off the XIM+IBUS fallback daemon. Otherwise
            // restarting XIM+IBUS invalidates GTK's IBus InputContext inside
            // keytao-app and prints transient UnknownObject warnings.
            std::env::set_var("GTK_IM_MODULE", "wayland");
            std::env::set_var("QT_IM_MODULE", "wayland");
        }
    }
    keytao_app_lib::run()
}
