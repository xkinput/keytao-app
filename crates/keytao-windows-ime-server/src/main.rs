//! The non-Windows entry point intentionally has no native engine dependencies.

// A GUI-subsystem exe never gets a console, however it is launched.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
use keytao_ime_ipc::engine_config;

#[cfg(windows)]
mod server;

#[cfg(any(windows, test))]
mod authorization;

#[cfg(any(windows, test))]
mod handshake;

#[cfg(any(windows, test))]
mod pipe_io;

#[cfg(windows)]
fn main() {
    if std::env::args_os().skip(1).any(|arg| arg == "--deploy") {
        let code = keytao_windows_ime::engine_backend::InProcEngine::deploy_server(|| {
            keytao_core::deploy_child::run_deploy_child_main_with_post_deploy(
                keytao_app_core::windows_public_schemas::deploy_and_publish,
            )
        })
        .unwrap_or_else(|error| {
            eprintln!("KeyTao deploy child: {error}");
            println!("{}", serde_json::json!({"ok": false, "error": error}));
            1
        });
        std::process::exit(code);
    }
    if let Err(error) = server::run() {
        eprintln!("KeyTao IME server: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}
