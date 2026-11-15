//! The non-Windows entry point intentionally has no native engine dependencies.

#[cfg(any(windows, test))]
use keytao_ime_ipc::engine_config;

#[cfg(windows)]
mod server;

#[cfg(any(windows, test))]
mod authorization;

#[cfg(any(windows, test))]
mod handshake;

#[cfg(windows)]
fn main() {
    if let Err(error) = server::run() {
        eprintln!("KeyTao IME server: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}
