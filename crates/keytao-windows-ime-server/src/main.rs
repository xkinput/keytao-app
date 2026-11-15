//! The non-Windows entry point intentionally has no native engine dependencies.

#[cfg(all(not(windows), test))]
#[path = "../../keytao-windows-ime/src/engine_config.rs"]
mod engine_config;

#[cfg(windows)]
use keytao_windows_ime::engine_config;

#[cfg(windows)]
mod server;

#[cfg(windows)]
fn main() {
    if let Err(error) = server::run() {
        eprintln!("KeyTao IME server: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}
