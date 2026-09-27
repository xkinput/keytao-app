use crate::{Core, ime_host::ImeHost, ime_paths::*};
use serde::Serialize;
use std::{collections::HashSet, path::PathBuf, sync::Mutex};

#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct ManagedImeHelper(Mutex<Option<std::process::Child>>);

#[cfg(target_os = "linux")]
#[derive(Serialize, Clone)]
pub struct LinuxImeStatus {
    pub supported: bool,
    pub kde_session: bool,
    pub kde_configured: bool,
    pub running: bool,
    pub managed_pid: Option<u32>,
    pub daemon_owner_pid: Option<u32>,
    pub command: String,
    pub processes: Vec<String>,
    pub kde_native_processes: usize,
    pub fallback_processes: usize,
    pub user_data_dir: Option<String>,
    pub shared_data_dir: Option<String>,
    pub shared_data_source: String,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub message: String,
}

#[cfg(target_os = "linux")]
#[derive(Clone)]
struct KeytaoImeProcess {
    pid: u32,
    line: String,
    kde_native: bool,
}

#[cfg(target_os = "linux")]
pub fn stop_managed_ime_helper(host: &impl ImeHost) {
    let state = host.managed_helper();
    let Ok(mut child_slot) = state.0.lock() else {
        tracing::warn!("failed to lock managed IME helper state during shutdown");
        return;
    };
    if let Some(mut child) = child_slot.take() {
        let pid = child.id();
        if let Err(e) = child.kill() {
            tracing::warn!("failed to kill managed keytao-ime pid={pid}: {e}");
        }
        let _ = child.wait();
        tracing::info!("managed keytao-ime pid={pid} stopped");
    }
}

#[cfg(target_os = "linux")]
fn refresh_managed_ime_helper(host: &impl ImeHost) -> Option<u32> {
    let state = host.managed_helper();
    let Ok(mut child_slot) = state.0.lock() else {
        tracing::warn!("failed to lock managed IME helper state");
        return None;
    };

    let Some(child) = child_slot.as_mut() else {
        return None;
    };

    match child.try_wait() {
        Ok(Some(status)) => {
            tracing::info!("managed keytao-ime exited with {status}");
            *child_slot = None;
            None
        }
        Ok(None) => Some(child.id()),
        Err(e) => {
            tracing::warn!("failed to inspect managed keytao-ime: {e}");
            Some(child.id())
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_target_triple() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "unknown-linux-gnu"
    }
}

#[cfg(target_os = "linux")]
fn is_kde_session() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    desktop.split(':').any(|s| s == "kde")
}

#[cfg(target_os = "linux")]
fn cleanup_kde_legacy_ime_files() {
    if !is_kde_session() {
        return;
    }

    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let desktop_file = std::path::Path::new(&home)
        .join(".config")
        .join("autostart")
        .join("keytao-ime.desktop");
    if desktop_file.exists() {
        match std::fs::remove_file(&desktop_file) {
            Ok(()) => tracing::info!(
                "Removed legacy KDE autostart {} to avoid conflicting IME daemons",
                desktop_file.display()
            ),
            Err(e) => tracing::warn!(
                "Cannot remove legacy KDE autostart {}: {e}",
                desktop_file.display()
            ),
        }
    }
}

#[cfg(target_os = "linux")]
fn kde_input_method_config() -> Option<String> {
    for command in ["kreadconfig6", "kreadconfig5"] {
        let output = std::process::Command::new(command)
            .args([
                "--file",
                "kwinrc",
                "--group",
                "Wayland",
                "--key",
                "InputMethod",
            ])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }

    let home = std::env::var_os("HOME")?;
    let kwinrc = std::path::Path::new(&home).join(".config").join("kwinrc");
    let content = std::fs::read_to_string(kwinrc).ok()?;
    let mut in_wayland_group = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_wayland_group = trimmed == "[Wayland]";
            continue;
        }
        if in_wayland_group {
            if let Some(value) = trimmed.strip_prefix("InputMethod=") {
                let value = value.trim().to_string();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn is_kde_input_method_configured() -> bool {
    kde_input_method_config()
        .as_deref()
        .is_some_and(|value| value == "keytao-wayland-launcher.desktop")
}

#[cfg(target_os = "linux")]
fn linux_ime_status_with_message(
    core: &Core,
    host: &impl ImeHost,
    message: String,
) -> LinuxImeStatus {
    let (_, command) = resolve_keytao_ime_command(core);
    let managed_pid = refresh_managed_ime_helper(host);
    let process_entries = keytao_ime_process_entries();
    let kde_native_processes = process_entries.iter().filter(|p| p.kde_native).count();
    let fallback_processes = process_entries.iter().filter(|p| !p.kde_native).count();
    let processes = process_entries.into_iter().map(|p| p.line).collect();
    let (shared_data_dir, shared_data_source) = shared_data_status(linux_app_shared_data_dir(core));
    let (reload_stamp_path, reload_stamp_signature) = reload_stamp_status();
    LinuxImeStatus {
        supported: true,
        kde_session: is_kde_session(),
        kde_configured: is_kde_input_method_configured(),
        running: kde_native_processes + fallback_processes > 0,
        managed_pid,
        daemon_owner_pid: linux_ime_daemon_owner_pid(),
        command,
        processes,
        kde_native_processes,
        fallback_processes,
        user_data_dir: default_user_data_dir_string(),
        shared_data_dir,
        shared_data_source,
        reload_stamp_path,
        reload_stamp_signature,
        message,
    }
}

#[cfg(target_os = "linux")]
fn linux_ime_daemon_owner_pid() -> Option<u32> {
    let output = std::process::Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetConnectionUnixProcessID",
            "s",
            "org.xkinput.keytao.ime.Daemon",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find_map(|part| part.parse::<u32>().ok())
}

#[cfg(target_os = "linux")]
fn linux_runtime_dirs(core: &Core) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    {
        let resource_dir = core.env().resource_dir.clone();
        candidates.extend([
            resource_dir.join("runtime"),
            resource_dir.join("resources").join("runtime"),
        ]);
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(bin_dir) = current_exe.parent() {
            candidates.extend([
                bin_dir.join("runtime"),
                bin_dir.join("resources").join("runtime"),
                bin_dir.join("..").join("runtime"),
                bin_dir
                    .join("..")
                    .join("lib")
                    .join("keytao-app")
                    .join("runtime"),
                bin_dir
                    .join("..")
                    .join("lib")
                    .join("keytao-app")
                    .join("resources")
                    .join("runtime"),
            ]);
        }
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[cfg(target_os = "linux")]
pub fn linux_app_shared_data_dir(core: &Core) -> Option<String> {
    linux_runtime_dirs(core)
        .into_iter()
        .map(|dir| dir.join("rime-data"))
        .find(|dir| dir.join("default.yaml").is_file())
        .map(|dir| dir.to_string_lossy().into_owned())
}

#[cfg(target_os = "linux")]
fn linux_runtime_lib_dirs(core: &Core) -> Vec<PathBuf> {
    linux_runtime_dirs(core)
        .into_iter()
        .map(|dir| dir.join("lib"))
        .filter(|dir| dir.is_dir())
        .collect()
}

#[cfg(target_os = "linux")]
fn configure_linux_runtime_env(core: &Core, command: &mut std::process::Command) {
    if let Some(shared) = linux_app_shared_data_dir(core) {
        command.env("KEYTAO_RIME_SHARED_DATA_DIR", &shared);
        command.env("RIME_SHARED_DATA_DIR", shared);
    }

    let mut lib_dirs = linux_runtime_lib_dirs(core);
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        lib_dirs.extend(std::env::split_paths(&existing));
    }
    if !lib_dirs.is_empty() {
        if let Ok(joined) = std::env::join_paths(lib_dirs) {
            command.env("LD_LIBRARY_PATH", joined);
        }
    }
}

#[cfg(target_os = "linux")]
fn keytao_ime_command(core: &Core, program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    configure_linux_runtime_env(core, &mut command);
    command
}

#[cfg(target_os = "linux")]
fn resolve_keytao_ime_command(core: &Core) -> (std::process::Command, String) {
    if let Ok(path) = std::env::var("KEYTAO_IME_BIN") {
        return (keytao_ime_command(core, &path), path);
    }

    if let Ok(current_exe) = std::env::current_exe() {
        let sibling = current_exe.with_file_name("keytao-ime");
        if sibling.is_file() {
            let display = sibling.display().to_string();
            return (keytao_ime_command(core, &sibling), display);
        }
    }

    {
        let resource_dir = core.env().resource_dir.clone();
        let target = linux_target_triple();
        for candidate in [
            resource_dir.join("keytao-ime"),
            resource_dir.join(format!("keytao-ime-{target}")),
            resource_dir
                .join("binaries")
                .join(format!("keytao-ime-{target}")),
        ] {
            if candidate.is_file() {
                let display = candidate.display().to_string();
                return (keytao_ime_command(core, candidate), display);
            }
        }
    }

    (
        keytao_ime_command(core, "keytao-ime"),
        "keytao-ime".to_string(),
    )
}

#[cfg(target_os = "linux")]
fn is_same_keytao_ime_running(expected: &str) -> bool {
    if expected == "keytao-ime" {
        return is_any_keytao_ime_running();
    }

    keytao_ime_process_lines()
        .into_iter()
        .any(|line| line.contains(expected))
}

#[cfg(target_os = "linux")]
fn is_any_keytao_ime_running() -> bool {
    !keytao_ime_process_lines().is_empty()
}

#[cfg(target_os = "linux")]
fn is_fallback_keytao_ime_running() -> bool {
    keytao_ime_process_entries()
        .into_iter()
        .any(|process| !process.kde_native)
}

#[cfg(target_os = "linux")]
fn keytao_ime_process_lines() -> Vec<String> {
    keytao_ime_process_entries()
        .into_iter()
        .map(|process| process.line)
        .collect()
}

#[cfg(target_os = "linux")]
fn keytao_ime_process_entries() -> Vec<KeytaoImeProcess> {
    let output = std::process::Command::new("pgrep")
        .args(["-af", "keytao-ime"])
        .output()
        .ok();

    let current_pid = std::process::id();
    output
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let (pid_text, _) = line.split_once(' ')?;
                    let pid = pid_text.parse::<u32>().ok()?;
                    if pid == current_pid || !is_keytao_ime_executable(pid) {
                        return None;
                    }
                    Some(KeytaoImeProcess {
                        pid,
                        line: line.to_owned(),
                        kde_native: process_has_wayland_socket(pid),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn is_keytao_ime_executable(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_os_string()))
        .and_then(|name| name.into_string().ok())
        .is_some_and(|name| name.starts_with("keytao-ime"))
}

#[cfg(target_os = "linux")]
fn process_has_wayland_socket(pid: u32) -> bool {
    let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    environ
        .split(|byte| *byte == 0)
        .any(|item| item.starts_with(b"WAYLAND_SOCKET="))
}

#[cfg(target_os = "linux")]
fn stop_external_keytao_ime_processes(include_kde_native: bool) {
    let processes: Vec<KeytaoImeProcess> = keytao_ime_process_entries()
        .into_iter()
        .filter(|process| include_kde_native || !process.kde_native)
        .collect();
    if processes.is_empty() {
        return;
    }

    for process in &processes {
        match std::process::Command::new("kill")
            .args(["-TERM", &process.pid.to_string()])
            .output()
        {
            Ok(output) if output.status.success() => {
                tracing::info!("requested keytao-ime pid={} to stop", process.pid);
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    "kill -TERM keytao-ime pid={} returned {}: {}",
                    process.pid,
                    output.status,
                    stderr.trim()
                );
            }
            Err(e) => tracing::warn!("failed to run kill for keytao-ime pid={}: {e}", process.pid),
        }
    }

    let requested_pids: Vec<u32> = processes.iter().map(|process| process.pid).collect();
    for _ in 0..20 {
        if requested_pids.iter().all(|pid| !process_exists(*pid)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    for pid in requested_pids
        .into_iter()
        .filter(|pid| process_exists(*pid))
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output();
        tracing::warn!("force-killed lingering keytao-ime pid={pid}");
    }
}

#[cfg(target_os = "linux")]
fn process_exists(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
#[cfg(target_os = "linux")]
pub fn launch_keytao_ime(
    core: &Core,
    host: &impl ImeHost,
    restart: bool,
) -> Result<LinuxImeStatus, String> {
    cleanup_kde_legacy_ime_files();
    let (mut ime_cmd, ime_display) = resolve_keytao_ime_command(core);

    if restart {
        stop_managed_ime_helper(host);
        stop_external_keytao_ime_processes(false);
    } else if is_fallback_keytao_ime_running() {
        let message = if is_same_keytao_ime_running(&ime_display) {
            format!("keytao-ime XIM+IBUS 已在运行：{ime_display}")
        } else {
            format!("已有其他 keytao-ime XIM+IBUS 进程在运行，当前 app 期望使用：{ime_display}")
        };
        return Ok(linux_ime_status_with_message(core, host, message));
    }

    let child = ime_cmd
        .spawn()
        .map_err(|e| format!("启动 keytao-ime 失败（{ime_display}）：{e}"))?;
    let pid = child.id();
    if let Ok(mut slot) = host.managed_helper().0.lock() {
        *slot = Some(child);
    }
    tracing::info!("keytao-ime spawned from {ime_display} pid={pid}");

    std::thread::sleep(std::time::Duration::from_millis(150));
    Ok(linux_ime_status_with_message(
        core,
        host,
        format!("已启动 keytao-ime pid={pid}"),
    ))
}
#[cfg(target_os = "linux")]
pub fn linux_ime_status(core: &Core, host: &impl ImeHost) -> LinuxImeStatus {
    linux_ime_status_with_message(core, host, "已刷新 keytao-ime 状态".into())
}
