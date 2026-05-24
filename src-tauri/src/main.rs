// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        if desktop.to_lowercase().contains("kde") {
            // Use native Wayland rendering for perfect hover styles, but force IBus
            // for text input because WebKitGTK Wayland IM module is horribly broken on KDE.
            std::env::set_var("GTK_IM_MODULE", "ibus");

            // Fix race condition: GTK initializes IBus immediately when `run()` is called.
            // If the local daemon isn't running yet, GTK fails to connect to D-Bus and never retries.
            // So we spawn the packaged local daemon BEFORE GTK initializes, and give it a moment to start.
            if std::env::var("KWIN_VIRTUAL_KEYBOARD").is_err() {
                let ime_running = std::process::Command::new("pgrep")
                    .arg("-x")
                    .arg("keytao-ime")
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !ime_running {
                    let ime_cmd = std::env::var_os("KEYTAO_IME_BIN")
                        .map(std::path::PathBuf::from)
                        .or_else(|| {
                            std::env::current_exe()
                                .ok()
                                .map(|exe| exe.with_file_name("keytao-ime"))
                                .filter(|path| path.is_file())
                        });
                    match ime_cmd {
                        Some(path) => {
                            println!(
                                "Starting local keytao-ime daemon before GTK initializes: {}",
                                path.display()
                            );
                            if let Err(e) = std::process::Command::new(&path).spawn() {
                                eprintln!(
                                    "Failed to start keytao-ime from {}: {e}",
                                    path.display()
                                );
                            }
                        }
                        None => {
                            println!("Starting local keytao-ime daemon before GTK initializes: keytao-ime");
                            if let Err(e) = std::process::Command::new("keytao-ime").spawn() {
                                eprintln!("Failed to start keytao-ime from PATH: {e}");
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                }
            }
        }
    }
    keytao_app_lib::run()
}
