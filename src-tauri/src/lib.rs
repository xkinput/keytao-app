use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[cfg(target_os = "android")]
use jni::{
    objects::{JObject, JString},
    sys::{jboolean, jint, jlong, jstring},
    JNIEnv,
};

#[cfg(any(target_os = "linux", target_os = "android"))]
use std::sync::Mutex;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use keytao_core;

#[cfg(target_os = "linux")]
mod rime;

// Linux protocol frontends live in the keytao-ime daemon. The GUI only deploys
// assets and starts the daemon when needed.

#[derive(Serialize, Deserialize, Clone)]
struct ReleaseCache {
    etag: String,
    cached_at: u64,
    release: ReleaseInfo,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DownloadUrls {
    pub macos: Option<String>,
    pub windows: Option<String>,
    pub linux: Option<String>,
    pub android: Option<String>,
    pub ios: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PlatformRelease {
    pub version: String,
    pub download_urls: DownloadUrls,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub name: String,
    pub published_at: String,
    pub body: String,
    pub github: Option<PlatformRelease>,
    pub gitee: Option<PlatformRelease>,
}

#[derive(Serialize, Clone)]
pub struct InstallProgress {
    pub stage: String,
    pub percent: u32,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct FileItem {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Serialize, Clone)]
pub struct VerifyEntry {
    pub path: String,
    pub ok: bool,
    pub note: String,
}

#[derive(Serialize, Clone)]
pub struct InstallResult {
    pub merged_schemas: Vec<String>,
    pub logs: Vec<String>,
    pub verify: Vec<VerifyEntry>,
}

const API_BASE: &str = "https://keytao.rea.ink";
const DEBUG_LOG_RETENTION_DAYS: i64 = 3;
const DEBUG_LOG_MAX_LINES: usize = 20_000;
const IME_RELOAD_STAMP_FILE: &str = "keytao-ime.reload";
#[cfg(target_os = "ios")]
const IOS_APP_GROUP_IDENTIFIER: &str = "group.ink.rea.keytao-app";

fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn non_empty_string(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn default_user_data_dir_string() -> Option<String> {
    keytao_core::default_user_data_dir().map(path_string)
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "linux")))]
#[tauri::command]
fn rime_get_data_dir() -> Option<String> {
    default_user_data_dir_string()
}

#[cfg(target_os = "ios")]
#[tauri::command]
fn rime_get_data_dir<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Option<String> {
    ios_keytao_root(&app).ok().map(path_string)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn reload_stamp_path() -> Option<PathBuf> {
    keytao_core::default_user_data_dir().map(|dir| dir.join(IME_RELOAD_STAMP_FILE))
}

#[cfg(target_os = "ios")]
fn ios_app_group_container() -> Option<PathBuf> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSFileManager, NSString};

    autoreleasepool(|_| {
        let manager = NSFileManager::defaultManager();
        let group = NSString::from_str(IOS_APP_GROUP_IDENTIFIER);
        let url = manager.containerURLForSecurityApplicationGroupIdentifier(&group)?;
        url.to_file_path()
    })
}

#[cfg(target_os = "ios")]
fn ios_keytao_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    if let Ok(override_dir) = std::env::var("KEYTAO_IOS_USER_DATA_DIR") {
        if !override_dir.trim().is_empty() {
            return Ok(PathBuf::from(override_dir));
        }
    }

    if let Some(container) = ios_app_group_container() {
        return Ok(container.join("keytao"));
    }

    app.path()
        .app_data_dir()
        .map(|dir| dir.join("keytao"))
        .map_err(|e| format!("Cannot determine iOS KeyTao data directory: {e}"))
}

#[cfg(target_os = "ios")]
fn ios_reload_stamp_path(root: &Path) -> PathBuf {
    root.join(IME_RELOAD_STAMP_FILE)
}

#[cfg(target_os = "ios")]
fn write_ios_reload_stamp(root: &Path) -> Result<PathBuf, String> {
    let stamp = ios_reload_stamp_path(root);
    if let Some(parent) = stamp.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 iOS 输入法目录失败: {e}"))?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::fs::write(&stamp, format!("{now}\n"))
        .map_err(|e| format!("写入 iOS 输入法重载标记失败 {}: {e}", stamp.display()))?;
    Ok(stamp)
}

#[cfg(target_os = "ios")]
fn ios_app_shared_data_dir<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    user_root: &Path,
) -> Option<String> {
    let mut candidates = vec![
        user_root.to_path_buf(),
        user_root.join("rime-data"),
        user_root.join("shared"),
    ];

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.extend([
            resource_dir.join("rime-data"),
            resource_dir.join("runtime").join("rime-data"),
            resource_dir.join("SharedSupport"),
        ]);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.extend([
                exe_dir.join("rime-data"),
                exe_dir.join("runtime").join("rime-data"),
                exe_dir.join("..").join("rime-data"),
            ]);
        }
    }

    candidates
        .into_iter()
        .find(|dir| dir.join("default.yaml").is_file())
        .map(|dir| dir.to_string_lossy().into_owned())
}

fn file_signature(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            format!("{}:{}", metadata.len(), modified)
        }
        Err(_) => "missing".into(),
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn reload_stamp_status() -> (Option<String>, Option<String>) {
    let Some(path) = reload_stamp_path() else {
        return (None, None);
    };
    let signature = file_signature(&path);
    (Some(path_string(path)), Some(signature))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImeUiSettings {
    pub color_scheme: keytao_theme::UiColorScheme,
    pub effective_color_scheme: keytao_theme::EffectiveColorScheme,
    pub orientation: keytao_theme::PanelOrientation,
    pub accent_color: String,
    pub theme_path: Option<String>,
    pub theme_exists: bool,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub message: String,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn ime_theme_path() -> Result<PathBuf, String> {
    keytao_theme::default_user_theme_path().ok_or("Cannot determine keytao data directory".into())
}

fn ime_ui_settings_from_paths(
    theme_path: PathBuf,
    reload_stamp_path: Option<PathBuf>,
    message: String,
) -> Result<ImeUiSettings, String> {
    let theme = keytao_theme::ThemeResolver::new(None, Some(theme_path.clone())).current();
    let (reload_stamp_path, reload_stamp_signature) = reload_stamp_path
        .map(|path| {
            let signature = file_signature(&path);
            (Some(path_string(path)), Some(signature))
        })
        .unwrap_or((None, None));
    let accent_color = theme
        .ui
        .accent_color
        .unwrap_or(theme.candidate.selected_label_color);
    Ok(ImeUiSettings {
        color_scheme: theme.ui.color_scheme,
        effective_color_scheme: theme.ui.effective_color_scheme,
        orientation: theme.panel.orientation,
        accent_color: color_to_hex(accent_color),
        theme_exists: theme_path.is_file(),
        theme_path: Some(path_string(theme_path)),
        reload_stamp_path,
        reload_stamp_signature,
        message,
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn ime_ui_settings_with_message(message: String) -> Result<ImeUiSettings, String> {
    let theme_path = ime_theme_path()?;
    let reload_stamp_path = reload_stamp_path();
    ime_ui_settings_from_paths(theme_path, reload_stamp_path, message)
}

fn write_ime_ui_settings_to_path(
    theme_path: &Path,
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
) -> Result<(), String> {
    if let Some(parent) = theme_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建主题目录失败: {e}"))?;
    }
    let accent_color = normalize_hex_color(&accent_color)?;

    let mut root = if theme_path.is_file() {
        let content = std::fs::read_to_string(&theme_path)
            .map_err(|e| format!("读取主题配置失败 {}: {e}", theme_path.display()))?;
        serde_yaml::from_str::<serde_yaml::Value>(&content)
            .map_err(|e| format!("主题配置无法解析 {}: {e}", theme_path.display()))?
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };

    if !matches!(root, serde_yaml::Value::Mapping(_)) {
        root = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    let mapping = root
        .as_mapping_mut()
        .ok_or("主题配置根节点必须是 YAML mapping")?;
    mapping
        .entry(serde_yaml::Value::String("version".into()))
        .or_insert_with(|| {
            serde_yaml::Value::Number(serde_yaml::Number::from(keytao_theme::THEME_SCHEMA_VERSION))
        });

    let ui_mapping = yaml_child_mapping(mapping, "ui", "主题 UI 配置必须是 YAML mapping")?;
    let color_scheme = match color_scheme {
        keytao_theme::UiColorScheme::Auto => "auto",
        keytao_theme::UiColorScheme::Light => "light",
        keytao_theme::UiColorScheme::Dark => "dark",
    };
    ui_mapping.insert(
        serde_yaml::Value::String("colorScheme".into()),
        serde_yaml::Value::String(color_scheme.into()),
    );
    ui_mapping.insert(
        serde_yaml::Value::String("accentColor".into()),
        serde_yaml::Value::String(accent_color),
    );

    let panel_mapping = yaml_child_mapping(mapping, "panel", "主题面板配置必须是 YAML mapping")?;
    let orientation = match orientation {
        keytao_theme::PanelOrientation::Horizontal => "horizontal",
        keytao_theme::PanelOrientation::Vertical => "vertical",
    };
    panel_mapping.insert(
        serde_yaml::Value::String("orientation".into()),
        serde_yaml::Value::String(orientation.into()),
    );

    let content = serde_yaml::to_string(&root).map_err(|e| format!("序列化主题配置失败: {e}"))?;
    std::fs::write(&theme_path, content)
        .map_err(|e| format!("写入主题配置失败 {}: {e}", theme_path.display()))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_ime_ui_settings(
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
) -> Result<(), String> {
    let theme_path = ime_theme_path()?;
    write_ime_ui_settings_to_path(&theme_path, color_scheme, orientation, accent_color)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn write_ime_ui_color_scheme(color_scheme: keytao_theme::UiColorScheme) -> Result<(), String> {
    let current = ime_ui_settings_with_message(String::new())?;
    write_ime_ui_settings(color_scheme, current.orientation, current.accent_color)
}

fn yaml_child_mapping<'a>(
    mapping: &'a mut serde_yaml::Mapping,
    key: &str,
    error: &str,
) -> Result<&'a mut serde_yaml::Mapping, String> {
    let value = mapping
        .entry(serde_yaml::Value::String(key.into()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    if !matches!(value, serde_yaml::Value::Mapping(_)) {
        *value = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    value.as_mapping_mut().ok_or_else(|| error.to_string())
}

fn color_to_hex(color: keytao_theme::RgbaColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
}

fn normalize_hex_color(value: &str) -> Result<String, String> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("主题色必须是 #RRGGBB 格式".into());
    }
    Ok(format!("#{}", hex.to_ascii_uppercase()))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn shared_data_status(packaged: Option<String>) -> (Option<String>, String) {
    if let Some(path) = packaged {
        (Some(path), "packaged".into())
    } else {
        (
            non_empty_string(keytao_core::default_shared_data_dir()),
            "default_fallback".into(),
        )
    }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct ManagedImeHelper(Mutex<Option<std::process::Child>>);

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
fn stop_managed_ime_helper(app: &tauri::AppHandle) {
    let state = app.state::<ManagedImeHelper>();
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
fn refresh_managed_ime_helper(app: &tauri::AppHandle) -> Option<u32> {
    let state = app.state::<ManagedImeHelper>();
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
fn linux_ime_status_with_message(app: &tauri::AppHandle, message: String) -> LinuxImeStatus {
    let (_, command) = resolve_keytao_ime_command(app);
    let managed_pid = refresh_managed_ime_helper(app);
    let process_entries = keytao_ime_process_entries();
    let kde_native_processes = process_entries.iter().filter(|p| p.kde_native).count();
    let fallback_processes = process_entries.iter().filter(|p| !p.kde_native).count();
    let processes = process_entries.into_iter().map(|p| p.line).collect();
    let (shared_data_dir, shared_data_source) = shared_data_status(linux_app_shared_data_dir(app));
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
fn linux_runtime_dirs(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
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
fn linux_app_shared_data_dir(app: &tauri::AppHandle) -> Option<String> {
    linux_runtime_dirs(app)
        .into_iter()
        .map(|dir| dir.join("rime-data"))
        .find(|dir| dir.join("default.yaml").is_file())
        .map(|dir| dir.to_string_lossy().into_owned())
}

#[cfg(target_os = "linux")]
fn linux_runtime_lib_dirs(app: &tauri::AppHandle) -> Vec<PathBuf> {
    linux_runtime_dirs(app)
        .into_iter()
        .map(|dir| dir.join("lib"))
        .filter(|dir| dir.is_dir())
        .collect()
}

#[cfg(target_os = "linux")]
fn configure_linux_runtime_env(app: &tauri::AppHandle, command: &mut std::process::Command) {
    if let Some(shared) = linux_app_shared_data_dir(app) {
        command.env("KEYTAO_RIME_SHARED_DATA_DIR", &shared);
        command.env("RIME_SHARED_DATA_DIR", shared);
    }

    let mut lib_dirs = linux_runtime_lib_dirs(app);
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
fn keytao_ime_command(
    app: &tauri::AppHandle,
    program: impl AsRef<std::ffi::OsStr>,
) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    configure_linux_runtime_env(app, &mut command);
    command
}

#[cfg(target_os = "linux")]
fn resolve_keytao_ime_command(app: &tauri::AppHandle) -> (std::process::Command, String) {
    if let Ok(path) = std::env::var("KEYTAO_IME_BIN") {
        return (keytao_ime_command(app, &path), path);
    }

    if let Ok(current_exe) = std::env::current_exe() {
        let sibling = current_exe.with_file_name("keytao-ime");
        if sibling.is_file() {
            let display = sibling.display().to_string();
            return (keytao_ime_command(app, &sibling), display);
        }
    }

    if let Ok(resource_dir) = app.path().resource_dir() {
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
                return (keytao_ime_command(app, candidate), display);
            }
        }
    }

    (
        keytao_ime_command(app, "keytao-ime"),
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn write_keytao_ime_reload_stamp() -> Result<(), String> {
    let dir =
        keytao_core::default_user_data_dir().ok_or("Cannot determine keytao data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let stamp = dir.join(IME_RELOAD_STAMP_FILE);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::fs::write(&stamp, format!("{now}\n"))
        .map_err(|e| format!("写入 keytao-ime 重载标记失败 {}: {e}", stamp.display()))
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

#[cfg(target_os = "windows")]
const WINDOWS_TEXT_SERVICE_CLSID: &str = "{4A5C6D7E-8F90-1A2B-3C4D-5E6F7A8B9C0D}";

#[cfg(target_os = "windows")]
const WINDOWS_TEXT_SERVICE_GUID: windows::core::GUID = windows::core::GUID {
    data1: 0x4A5C6D7E,
    data2: 0x8F90,
    data3: 0x1A2B,
    data4: [0x3C, 0x4D, 0x5E, 0x6F, 0x7A, 0x8B, 0x9C, 0x0D],
};

#[cfg(target_os = "windows")]
const WINDOWS_PROFILE_GUID: windows::core::GUID = windows::core::GUID {
    data1: 0x1B2C3D4E,
    data2: 0x5F60,
    data3: 0x7A8B,
    data4: [0x9C, 0x0D, 0x1E, 0x2F, 0x3A, 0x4B, 0x5C, 0x6D],
};

#[cfg(target_os = "windows")]
const WINDOWS_LANGID_CHINESE_SIMPLIFIED: u16 = 0x0804;

#[cfg(target_os = "windows")]
struct WindowsComApartment(bool);

#[cfg(target_os = "windows")]
impl Drop for WindowsComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_init_com_apartment() -> Result<WindowsComApartment, String> {
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED},
    };

    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_ok() {
        Ok(WindowsComApartment(true))
    } else if hr == RPC_E_CHANGED_MODE {
        Ok(WindowsComApartment(false))
    } else {
        Err(format!("initialize COM apartment: {}", hr.message()))
    }
}

#[cfg(target_os = "windows")]
fn windows_tsf_profile_enabled() -> Result<bool, String> {
    use windows::{
        core::{IUnknown, Interface},
        Win32::{
            System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            UI::{
                Input::KeyboardAndMouse::HKL,
                TextServices::{
                    CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr,
                    ITfInputProcessorProfiles, TF_INPUTPROCESSORPROFILE, TF_IPP_FLAG_ENABLED,
                    TF_PROFILETYPE_INPUTPROCESSOR,
                },
            },
        },
    };

    let _com = windows_init_com_apartment()?;
    unsafe {
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(|e| format!("open TSF input processor profiles: {e}"))?;
        let profile_mgr: ITfInputProcessorProfileMgr = profiles
            .cast()
            .map_err(|e| format!("open modern TSF profile manager: {e}"))?;
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        profile_mgr
            .GetProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                WINDOWS_LANGID_CHINESE_SIMPLIFIED,
                &WINDOWS_TEXT_SERVICE_GUID,
                &WINDOWS_PROFILE_GUID,
                HKL::default(),
                &mut profile,
            )
            .map_err(|e| format!("query KeyTao TSF profile: {e}"))?;
        Ok(profile.dwFlags & TF_IPP_FLAG_ENABLED != 0)
    }
}

#[cfg(target_os = "windows")]
fn windows_ime_runtime_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.extend([
            resource_dir.join("current"),
            resource_dir.join("x64"),
            resource_dir.join("x86"),
            resource_dir
                .join("keytao-windows-ime-runtime")
                .join("current"),
            resource_dir.join("keytao-windows-ime-runtime").join("x64"),
            resource_dir.join("keytao-windows-ime-runtime").join("x86"),
            resource_dir
                .join("target")
                .join("keytao-windows-ime-runtime")
                .join("current"),
            resource_dir
                .join("target")
                .join("keytao-windows-ime-runtime")
                .join("x64"),
            resource_dir
                .join("resources")
                .join("keytao-windows-ime-runtime")
                .join("current"),
            resource_dir
                .join("resources")
                .join("keytao-windows-ime-runtime")
                .join("x64"),
            resource_dir
                .join("_up_")
                .join("target")
                .join("keytao-windows-ime-runtime")
                .join("current"),
            resource_dir
                .join("_up_")
                .join("target")
                .join("keytao-windows-ime-runtime")
                .join("x64"),
        ]);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.extend([
                dir.join("current"),
                dir.join("x64"),
                dir.join("x86"),
                dir.join("keytao-windows-ime-runtime").join("current"),
                dir.join("keytao-windows-ime-runtime").join("x64"),
                dir.join("keytao-windows-ime-runtime").join("x86"),
                dir.join("resources").join("current"),
                dir.join("resources").join("x64"),
                dir.join("resources")
                    .join("keytao-windows-ime-runtime")
                    .join("current"),
                dir.join("resources")
                    .join("keytao-windows-ime-runtime")
                    .join("x64"),
                dir.join("_up_")
                    .join("target")
                    .join("keytao-windows-ime-runtime")
                    .join("current"),
                dir.join("_up_")
                    .join("target")
                    .join("keytao-windows-ime-runtime")
                    .join("x64"),
                dir.join("resources")
                    .join("_up_")
                    .join("target")
                    .join("keytao-windows-ime-runtime")
                    .join("current"),
                dir.join("resources")
                    .join("_up_")
                    .join("target")
                    .join("keytao-windows-ime-runtime")
                    .join("x64"),
            ]);
        }
    }

    candidates
        .into_iter()
        .find(|dir| dir.join("keytao_windows_ime.dll").is_file())
}

#[cfg(target_os = "windows")]
fn windows_normal_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
}

#[cfg(target_os = "windows")]
fn windows_app_shared_data_dir(app: &tauri::AppHandle) -> Option<String> {
    windows_ime_runtime_dir(app)
        .map(|dir| dir.join("rime-data"))
        .filter(|dir| dir.join("default.yaml").is_file())
        .map(|dir| windows_normal_path(&dir))
}

#[cfg(target_os = "macos")]
fn macos_app_shared_data_dir(app: &tauri::AppHandle) -> Option<String> {
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.extend([
            resource_dir.join("rime-data"),
            resource_dir.join("SharedSupport"),
            resource_dir
                .join("KeyTao.app")
                .join("Contents")
                .join("Resources")
                .join("rime-data"),
            resource_dir
                .join("KeyTao.app")
                .join("Contents")
                .join("SharedSupport"),
        ]);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                let resources_dir = contents_dir.join("Resources");
                candidates.extend([
                    resources_dir.join("rime-data"),
                    resources_dir.join("SharedSupport"),
                    resources_dir
                        .join("KeyTao.app")
                        .join("Contents")
                        .join("Resources")
                        .join("rime-data"),
                    resources_dir
                        .join("KeyTao.app")
                        .join("Contents")
                        .join("SharedSupport"),
                ]);
            }
        }
    }

    candidates.extend([
        PathBuf::from("/Library/Input Methods/KeyTao.app/Contents/Resources/rime-data"),
        PathBuf::from("/Library/Input Methods/KeyTao.app/Contents/SharedSupport"),
    ]);

    candidates
        .into_iter()
        .find(|dir| dir.join("default.yaml").is_file())
        .map(|dir| dir.to_string_lossy().into_owned())
}

#[cfg(target_os = "windows")]
fn windows_registered_ime_path() -> Option<String> {
    let key = format!(r"HKCR\CLSID\{}\InprocServer32", WINDOWS_TEXT_SERVICE_CLSID);
    let output = std::process::Command::new("reg")
        .args(["query", &key, "/ve"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        let marker = "REG_SZ";
        let (_, value) = trimmed.split_once(marker)?;
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

#[cfg(target_os = "windows")]
fn windows_ime_status_with_message(app: &tauri::AppHandle, message: String) -> WindowsImeStatus {
    let runtime_dir = windows_ime_runtime_dir(app);
    let dll_path = runtime_dir
        .as_ref()
        .map(|dir| dir.join("keytao_windows_ime.dll"));
    let registered_path = windows_registered_ime_path();
    let packaged = dll_path.as_ref().is_some_and(|path| path.is_file())
        && runtime_dir
            .as_ref()
            .is_some_and(|dir| dir.join("rime.dll").is_file())
        && runtime_dir
            .as_ref()
            .is_some_and(|dir| dir.join("rime-data").join("default.yaml").is_file())
        && runtime_dir
            .as_ref()
            .is_some_and(|dir| dir.join("default-theme.yaml").is_file());
    let registered_dll = match (&dll_path, &registered_path) {
        (Some(dll), Some(path)) => {
            let lhs = std::fs::canonicalize(dll).unwrap_or_else(|_| dll.clone());
            let rhs = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            lhs.to_string_lossy()
                .eq_ignore_ascii_case(&rhs.to_string_lossy())
        }
        _ => false,
    };
    let (profile_enabled, profile_status) = match windows_tsf_profile_enabled() {
        Ok(true) => (true, "TSF profile 已启用".to_string()),
        Ok(false) => (false, "TSF profile 已存在但未启用".to_string()),
        Err(e) => (false, format!("TSF profile 不可用：{e}")),
    };
    let registered = registered_dll && profile_enabled;
    let (shared_data_dir, shared_data_source) =
        shared_data_status(windows_app_shared_data_dir(app));
    let (reload_stamp_path, reload_stamp_signature) = reload_stamp_status();

    WindowsImeStatus {
        supported: true,
        packaged,
        registered,
        registered_dll,
        profile_enabled,
        runtime_dir: runtime_dir.map(|p| windows_normal_path(&p)),
        dll_path: dll_path.map(|p| windows_normal_path(&p)),
        registered_path,
        profile_status,
        user_data_dir: default_user_data_dir_string(),
        shared_data_dir,
        shared_data_source,
        reload_stamp_path,
        reload_stamp_signature,
        message,
    }
}

#[cfg(target_os = "windows")]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
fn run_regsvr32_elevated(dll_path: &Path, unregister: bool) -> Result<(), String> {
    let dll_path = std::fs::canonicalize(dll_path).unwrap_or_else(|_| dll_path.to_path_buf());
    let dll_dir = dll_path
        .parent()
        .ok_or("invalid keytao_windows_ime.dll path")?;
    let temp_dir = std::env::temp_dir();
    let stamp = format!(
        "keytao-ime-register-{}-{}",
        std::process::id(),
        OffsetDateTime::now_utc().unix_timestamp_nanos()
    );
    let script_path = temp_dir.join(format!("{stamp}.ps1"));
    let result_path = temp_dir.join(format!("{stamp}.txt"));
    let proc_name = if unregister {
        "DllUnregisterServer"
    } else {
        "DllRegisterServer"
    };
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$dir = {dir}
$dll = {dll}
$procName = {proc_name}
$resultFile = {result}
$source = @"
using System;
using System.Runtime.InteropServices;
public static class KeyTaoNativeRegister {{
  [DllImport("kernel32", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool SetDllDirectory(string lpPathName);
  [DllImport("kernel32", SetLastError=true, CharSet=CharSet.Unicode)] public static extern IntPtr LoadLibrary(string lpFileName);
  [DllImport("kernel32", SetLastError=true, CharSet=CharSet.Ansi)] public static extern IntPtr GetProcAddress(IntPtr hModule, string procName);
  [DllImport("kernel32", SetLastError=true)] public static extern bool FreeLibrary(IntPtr hModule);
  [UnmanagedFunctionPointer(CallingConvention.StdCall)] public delegate int RegisterDelegate();
}}
"@
try {{
  Add-Type -TypeDefinition $source
  [KeyTaoNativeRegister]::SetDllDirectory($dir) | Out-Null
  $module = [KeyTaoNativeRegister]::LoadLibrary($dll)
  if ($module -eq [IntPtr]::Zero) {{
    $code = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    "LoadLibrary failed: $code" | Set-Content -Encoding UTF8 -Path $resultFile
    exit 3
  }}
  $proc = [KeyTaoNativeRegister]::GetProcAddress($module, $procName)
  if ($proc -eq [IntPtr]::Zero) {{
    $code = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    "GetProcAddress $procName failed: $code" | Set-Content -Encoding UTF8 -Path $resultFile
    [KeyTaoNativeRegister]::FreeLibrary($module) | Out-Null
    exit 4
  }}
  $delegate = [Runtime.InteropServices.Marshal]::GetDelegateForFunctionPointer($proc, [KeyTaoNativeRegister+RegisterDelegate])
  $hr = $delegate.Invoke()
  [KeyTaoNativeRegister]::FreeLibrary($module) | Out-Null
  ("{{0}} HRESULT=0x{{1:X8}}" -f $procName, ($hr -band 0xffffffff)) | Set-Content -Encoding UTF8 -Path $resultFile
  if ($hr -eq 0) {{ exit 0 }}
  exit 5
}} catch {{
  ("PowerShell registration failed: " + $_.Exception.Message) | Set-Content -Encoding UTF8 -Path $resultFile
  exit 9
}}
"#,
        dir = powershell_quote(&windows_normal_path(dll_dir)),
        dll = powershell_quote(&windows_normal_path(&dll_path)),
        proc_name = powershell_quote(proc_name),
        result = powershell_quote(&windows_normal_path(&result_path)),
    );
    std::fs::write(&script_path, script).map_err(|e| format!("write registration script: {e}"))?;

    let elevated_script = format!(
        "$p = Start-Process -FilePath powershell.exe -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', {}) -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
        powershell_quote(&windows_normal_path(&script_path))
    );
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &elevated_script,
        ])
        .output()
        .map_err(|e| format!("start regsvr32: {e}"))?;
    let detail = std::fs::read_to_string(&result_path)
        .unwrap_or_default()
        .trim()
        .to_string();
    let _ = std::fs::remove_file(&script_path);
    let _ = std::fs::remove_file(&result_path);
    if output.status.success() {
        Ok(())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let process_detail = [detail.as_str(), stdout.trim(), stderr.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Err(format!(
            "TSF registration failed with exit code {}{}{}",
            output.status.code().unwrap_or(-1),
            if process_detail.is_empty() { "" } else { ": " },
            process_detail
        ))
    }
}

#[tauri::command]
#[cfg(target_os = "windows")]
fn windows_ime_status(app: AppHandle) -> WindowsImeStatus {
    windows_ime_status_with_message(&app, "已刷新 KeyTao Windows IME 状态".into())
}

#[tauri::command]
#[cfg(target_os = "windows")]
fn windows_register_ime(app: AppHandle) -> Result<WindowsImeStatus, String> {
    let runtime_dir =
        windows_ime_runtime_dir(&app).ok_or("安装包中未找到 keytao_windows_ime.dll")?;
    let dll = runtime_dir.join("keytao_windows_ime.dll");
    run_regsvr32_elevated(&dll, false)?;
    let status = windows_ime_status_with_message(&app, "已注册 KeyTao Windows IME".into());
    if status.registered {
        Ok(status)
    } else {
        Err("regsvr32 已结束，但 TSF 注册表中仍未找到 KeyTao；请确认 UAC 已同意，并查看是否有安全软件拦截注册表写入".into())
    }
}

#[tauri::command]
#[cfg(target_os = "windows")]
fn windows_unregister_ime(app: AppHandle) -> Result<WindowsImeStatus, String> {
    let dll = windows_ime_runtime_dir(&app)
        .map(|dir| dir.join("keytao_windows_ime.dll"))
        .or_else(|| windows_registered_ime_path().map(PathBuf::from))
        .ok_or("未找到可卸载的 KeyTao Windows IME DLL")?;
    run_regsvr32_elevated(&dll, true)?;
    Ok(windows_ime_status_with_message(
        &app,
        "已卸载 KeyTao Windows IME".into(),
    ))
}

#[tauri::command]
#[cfg(target_os = "windows")]
fn windows_restart_ime(app: AppHandle) -> Result<WindowsImeStatus, String> {
    let runtime_dir =
        windows_ime_runtime_dir(&app).ok_or("安装包中未找到 keytao_windows_ime.dll")?;
    let dll = runtime_dir.join("keytao_windows_ime.dll");
    let _ = run_regsvr32_elevated(&dll, true);
    run_regsvr32_elevated(&dll, false)?;
    let status = windows_ime_status_with_message(&app, "已重新注册 KeyTao Windows IME".into());
    if status.registered {
        Ok(status)
    } else {
        Err("regsvr32 已结束，但 TSF 注册表中仍未找到 KeyTao；请确认 UAC 已同意，并查看是否有安全软件拦截注册表写入".into())
    }
}

#[cfg(target_os = "linux")]
fn launch_keytao_ime(app: &tauri::AppHandle, restart: bool) -> Result<LinuxImeStatus, String> {
    cleanup_kde_legacy_ime_files();
    let (mut ime_cmd, ime_display) = resolve_keytao_ime_command(app);

    if restart {
        stop_managed_ime_helper(app);
        stop_external_keytao_ime_processes(false);
    } else if is_fallback_keytao_ime_running() {
        let message = if is_same_keytao_ime_running(&ime_display) {
            format!("keytao-ime XIM+IBUS 已在运行：{ime_display}")
        } else {
            format!("已有其他 keytao-ime XIM+IBUS 进程在运行，当前 app 期望使用：{ime_display}")
        };
        return Ok(linux_ime_status_with_message(app, message));
    }

    let child = ime_cmd
        .spawn()
        .map_err(|e| format!("启动 keytao-ime 失败（{ime_display}）：{e}"))?;
    let pid = child.id();
    if let Ok(mut slot) = app.state::<ManagedImeHelper>().0.lock() {
        *slot = Some(child);
    }
    tracing::info!("keytao-ime spawned from {ime_display} pid={pid}");

    std::thread::sleep(std::time::Duration::from_millis(150));
    Ok(linux_ime_status_with_message(
        app,
        format!("已启动 keytao-ime pid={pid}"),
    ))
}

#[cfg(target_os = "linux")]
fn desktop_exec_value(command: &str) -> String {
    if command.contains(char::is_whitespace) {
        format!(
            "\"{}\"",
            command
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('$', "\\$")
                .replace('`', "\\`")
        )
    } else {
        command.to_string()
    }
}

#[cfg(target_os = "linux")]
fn ensure_kde_virtual_keyboard_desktop(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("无法确定 HOME 目录")?;
    let applications_dir = std::path::Path::new(&home)
        .join(".local")
        .join("share")
        .join("applications");
    std::fs::create_dir_all(&applications_dir)
        .map_err(|e| format!("创建 KDE desktop 目录失败: {e}"))?;

    let (_, ime_display) = resolve_keytao_ime_command(app);
    let desktop_file = applications_dir.join("keytao-wayland-launcher.desktop");
    let content = format!(
        "[Desktop Entry]\n\
         Name=KeyTao Input Method (Wayland)\n\
         Name[zh_CN]=键道输入法 (Wayland)\n\
         GenericName=Input Method\n\
         GenericName[zh_CN]=输入法\n\
         Comment=KeyTao Chinese Input Method Engine (KDE Virtual Keyboard)\n\
         Comment[zh_CN]=键道中文输入法引擎（KDE 虚拟键盘）\n\
         Exec={}\n\
         Icon=input-keyboard\n\
         Terminal=false\n\
         Type=Application\n\
         Categories=System;Utility;\n\
         StartupNotify=false\n\
         NoDisplay=true\n\
         OnlyShowIn=KDE\n\
         X-KDE-StartupNotify=false\n\
         X-KDE-Wayland-VirtualKeyboard=true\n",
        desktop_exec_value(&ime_display)
    );
    std::fs::write(&desktop_file, content)
        .map_err(|e| format!("写入 KDE desktop 文件失败 {}: {e}", desktop_file.display()))?;
    Ok(desktop_file)
}

#[cfg(target_os = "linux")]
fn run_first_available_command(commands: &[&str], args: &[&str]) -> Result<(), String> {
    let mut errors = Vec::new();
    for command in commands {
        match std::process::Command::new(command).args(args).output() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => errors.push(format!(
                "{command}: {} {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(e) => errors.push(format!("{command}: {e}")),
        }
    }
    Err(errors.join("; "))
}

#[cfg(target_os = "linux")]
fn configure_kde_virtual_keyboard(app: &tauri::AppHandle) -> Result<Vec<String>, String> {
    if !is_kde_session() {
        return Err("当前不是 KDE 会话，无法配置 KDE 虚拟键盘输入法".into());
    }

    let desktop_file = ensure_kde_virtual_keyboard_desktop(app)?;
    run_first_available_command(
        &["kwriteconfig6", "kwriteconfig5"],
        &[
            "--file",
            "kwinrc",
            "--group",
            "Wayland",
            "--key",
            "InputMethod",
            "keytao-wayland-launcher.desktop",
        ],
    )
    .map_err(|e| format!("写入 KDE 输入法配置失败: {e}"))?;

    let _ = run_first_available_command(
        &["qdbus6", "qdbus"],
        &["org.kde.KWin", "/KWin", "reconfigure"],
    );

    Ok(vec![
        format!("已写入 {}", desktop_file.display()),
        "已设置 KWin Wayland/InputMethod=keytao-wayland-launcher.desktop".into(),
    ])
}

fn build_client<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<reqwest::Client, String> {
    let version = app.package_info().version.to_string();
    reqwest::Client::builder()
        .user_agent(format!("keytao-app/{version}"))
        .connect_timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())
}

fn parse_download_urls(obj: &serde_json::Value) -> DownloadUrls {
    let urls = &obj["downloadUrls"];
    DownloadUrls {
        macos: urls["macos"].as_str().map(|s| s.to_string()),
        windows: urls["windows"].as_str().map(|s| s.to_string()),
        linux: urls["linux"].as_str().map(|s| s.to_string()),
        android: urls["android"].as_str().map(|s| s.to_string()),
        ios: urls["ios"].as_str().map(|s| s.to_string()),
    }
}

#[tauri::command]
async fn fetch_latest_release(app: AppHandle) -> Result<ReleaseInfo, String> {
    let t0 = std::time::Instant::now();
    tracing::info!("[fetch_latest_release] start");
    let cache_path = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())
        .map(|d| d.join("release_cache.json"))
        .ok();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Check disk cache (5 min TTL, no ETag for our proxy API)
    let cached: Option<ReleaseCache> = cache_path.as_ref().and_then(|p| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str::<ReleaseCache>(&s).ok())
    });
    if let Some(ref c) = cached {
        if now.saturating_sub(c.cached_at) < 300 {
            tracing::info!(
                "[fetch_latest_release] cache hit, {}ms",
                t0.elapsed().as_millis()
            );
            return Ok(c.release.clone());
        }
    }

    let client = build_client(&app)?;
    let url = format!("{API_BASE}/api/install/latest-release");

    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?;

    if !response.status().is_success() {
        tracing::warn!(
            "[fetch_latest_release] HTTP {} after {}ms",
            response.status(),
            t0.elapsed().as_millis()
        );
        if let Some(c) = cached {
            return Ok(c.release);
        }
        return Err(format!("获取版本信息失败，HTTP {}", response.status()));
    }

    let data: serde_json::Value = response.json().await.map_err(|e| {
        tracing::warn!(
            "[fetch_latest_release] body parse failed after {}ms: {e}",
            t0.elapsed().as_millis()
        );
        format!("解析响应失败: {e}")
    })?;

    let github_version = data["github"]["version"].as_str().unwrap_or("").to_string();
    let github = if !github_version.is_empty() {
        Some(PlatformRelease {
            version: github_version.clone(),
            download_urls: parse_download_urls(&data["github"]),
        })
    } else {
        None
    };

    let gitee_version = data["gitee"]["version"].as_str().unwrap_or("").to_string();
    let gitee = if !gitee_version.is_empty() {
        Some(PlatformRelease {
            version: gitee_version,
            download_urls: parse_download_urls(&data["gitee"]),
        })
    } else {
        None
    };

    let version = data["version"].as_str().unwrap_or("unknown").to_string();
    let name = data["name"].as_str().unwrap_or("").to_string();
    let published_at = data["publishedAt"].as_str().unwrap_or("").to_string();
    let body = data["body"].as_str().unwrap_or("").to_string();

    let info = ReleaseInfo {
        version,
        name,
        published_at,
        body,
        github,
        gitee,
    };

    if let Some(path) = cache_path {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let cache = ReleaseCache {
            etag: String::new(),
            cached_at: now,
            release: info.clone(),
        };
        serde_json::to_string(&cache)
            .ok()
            .and_then(|s| std::fs::write(&path, s).ok());
    }

    tracing::info!(
        "[fetch_latest_release] done in {}ms",
        t0.elapsed().as_millis()
    );
    Ok(info)
}

fn rime_default_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        keytao_core::default_user_data_dir()
    }
    #[cfg(target_os = "windows")]
    {
        dirs::config_dir().map(|c| c.join("Rime"))
    }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir()?;
        let fcitx5 = home.join(".local/share/fcitx5/rime");
        let ibus = home.join(".config/ibus/rime");
        if fcitx5.exists() {
            Some(fcitx5)
        } else if ibus.exists() {
            Some(ibus)
        } else {
            Some(fcitx5)
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

#[tauri::command]
async fn select_directory(
    #[allow(unused_variables)] app: AppHandle,
    im_type: Option<String>,
) -> Result<Option<String>, String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        use tauri_plugin_dialog::{DialogExt, FilePath};
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut builder = app.dialog().file();
        let resolved_default = im_type
            .as_deref()
            .and_then(|im| {
                let home = dirs::home_dir()?;
                match im {
                    "fcitx5" => Some(home.join(".local/share/fcitx5/rime")),
                    "ibus" => Some(home.join(".config/ibus/rime")),
                    _ => None,
                }
            })
            .or_else(rime_default_path);
        if let Some(default) = resolved_default {
            builder = builder.set_directory(default);
        }
        builder.pick_folder(move |folder: Option<FilePath>| {
            let _ = tx.send(folder);
        });
        let result = rx.await.map_err(|e| e.to_string())?;
        Ok(result.map(|p| p.to_string()))
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        Err("Not supported on this platform".into())
    }
}

#[tauri::command]
fn list_dir(path: String) -> Result<Vec<FileItem>, String> {
    let entries = std::fs::read_dir(&path).map_err(|e| format!("读取目录失败: {e}"))?;
    let mut items: Vec<FileItem> = entries
        .filter_map(|e| e.ok())
        .map(|e| FileItem {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
        })
        .collect();
    items.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(items)
}

#[tauri::command]
fn read_local_schemas(path: String) -> Vec<String> {
    let base = std::path::Path::new(&path);
    let content = std::fs::read_to_string(base.join("default.custom.yaml"))
        .or_else(|_| std::fs::read_to_string(base.join("default-custom.yaml")))
        .unwrap_or_default();
    parse_schema_list(&content)
}

fn is_default_custom(filename: &str) -> bool {
    filename == "default.custom.yaml" || filename == "default-custom.yaml"
}

fn parse_schema_list(content: &str) -> Vec<String> {
    let mut schemas = Vec::new();
    let mut in_list = false;
    for line in content.lines() {
        let t = line.trim();
        if t.contains("schema_list:") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(rest) = t.strip_prefix("- schema:") {
                let s = rest.trim().to_string();
                if !s.is_empty() {
                    schemas.push(s);
                }
            } else if !t.is_empty() && !t.starts_with('#') && !t.starts_with('-') {
                in_list = false;
            }
        }
    }
    schemas
}

// Returns (merged_rime_lua, renames) where renames is [(old_module, new_module)].
// Conflicting user modules are renamed to "<name>_user" to avoid overwrite by zip's lua files.
fn merge_rime_lua(
    local_content: &str,
    zip_content: &str,
    zip_lua_filenames: &std::collections::HashSet<String>,
) -> (String, Vec<(String, String)>) {
    keytao_core::merge_rime_lua_content(Some(local_content), zip_content, zip_lua_filenames)
}

fn merge_default_custom(existing: Option<&str>, zip_content: &str) -> (String, Vec<String>) {
    keytao_core::merge_default_custom_content(existing, zip_content).unwrap_or_else(|_| {
        let user: Vec<String> = existing
            .map(|c| {
                parse_schema_list(c)
                    .into_iter()
                    .filter(|s| !s.starts_with("keytao"))
                    .collect()
            })
            .unwrap_or_default();
        (zip_content.to_string(), user)
    })
}

/// After extraction, verify key files were written correctly.
/// - default.custom.yaml / rime.lua: read back and compare byte-for-byte with expected content
/// - dict / schema / lua files: just check existence
fn verify_install(
    dest: &std::path::Path,
    expected_dc: Option<&str>,
    expected_rl: Option<&str>,
    zip_bytes: &[u8],
) -> Vec<VerifyEntry> {
    let mut entries: Vec<VerifyEntry> = Vec::new();

    // Verify default.custom.yaml content matches what we wrote
    if let Some(expected) = expected_dc {
        let path = dest.join("default.custom.yaml");
        let label = "default.custom.yaml".to_string();
        match std::fs::read_to_string(&path) {
            Ok(actual) if actual == expected => entries.push(VerifyEntry {
                path: label,
                ok: true,
                note: "内容一致".into(),
            }),
            Ok(_) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: "内容与写入时不符，可能被其他程序修改或写入不完整".into(),
            }),
            Err(e) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: format!("读取失败: {e}"),
            }),
        }
    }

    // Verify rime.lua content matches what we wrote
    if let Some(expected) = expected_rl {
        let path = dest.join("rime.lua");
        let label = "rime.lua".to_string();
        match std::fs::read_to_string(&path) {
            Ok(actual) if actual == expected => entries.push(VerifyEntry {
                path: label,
                ok: true,
                note: "内容一致".into(),
            }),
            Ok(_) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: "内容与写入时不符，可能被其他程序修改或写入不完整".into(),
            }),
            Err(e) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: format!("读取失败: {e}"),
            }),
        }
    }

    // Check that every non-empty zip entry (excluding the two merge-handled files) was written to disk
    if let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)) {
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                let raw = file.name().to_string();
                let relative = raw.trim_end_matches('/').to_string();
                if relative.is_empty() || file.is_dir() {
                    continue;
                }
                let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();
                // Only spot-check key file types (schemas, dicts, lua, opencc)
                let is_key = filename.ends_with(".schema.yaml")
                    || filename.ends_with(".dict.yaml")
                    || (filename.ends_with(".lua") && !relative.contains('/'))
                    || relative.starts_with("lua/")
                    || relative.starts_with("opencc/");
                if !is_key {
                    continue;
                }
                // Skip merge-handled files (already verified above)
                if is_default_custom(&filename) || filename == "rime.lua" {
                    continue;
                }
                let on_disk = dest.join(&relative);
                if on_disk.exists() {
                    entries.push(VerifyEntry {
                        path: relative,
                        ok: true,
                        note: "文件存在".into(),
                    });
                } else {
                    entries.push(VerifyEntry {
                        path: relative,
                        ok: false,
                        note: "文件不存在".into(),
                    });
                }
            }
        }
    }
    entries
}

/// Writes `content` to `path`, forcibly overwriting even read-only files.
/// On Linux, triggers a polkit (pkexec) root-auth dialog if the file is root-owned.
/// Returns a tag for logging: "" (normal), " [forced]", or " [root]".
fn write_file_force(path: &std::path::Path, content: &[u8]) -> Result<&'static str, String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).ok();
    }
    if std::fs::write(path, content).is_ok() {
        return Ok("");
    }
    // Try chmod before falling back to root — works when we own the file but it's read-only
    #[cfg(unix)]
    if path.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
        if std::fs::write(path, content).is_ok() {
            return Ok(" [forced]");
        }
    }
    write_file_privileged_fallback(path, content)
}

#[cfg(target_os = "linux")]
fn write_file_privileged_fallback(
    path: &std::path::Path,
    content: &[u8],
) -> Result<&'static str, String> {
    let tmp = std::env::temp_dir().join("keytao_privileged_write");
    std::fs::write(&tmp, content).map_err(|e| format!("临时文件写入失败: {e}"))?;
    let result = std::process::Command::new("pkexec")
        .arg("cp")
        .arg("--")
        .arg(&tmp)
        .arg(path)
        .output();
    let _ = std::fs::remove_file(&tmp);
    match result {
        Ok(o) if o.status.success() => Ok(" [root]"),
        Ok(o) => Err(format!(
            "需要 root 权限写入 {}，认证失败或被取消: {}",
            path.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("无法启动 pkexec（请确认系统已安装 polkit）: {e}")),
    }
}

#[cfg(not(target_os = "linux"))]
fn write_file_privileged_fallback(
    path: &std::path::Path,
    _content: &[u8],
) -> Result<&'static str, String> {
    Err(format!("写入失败（权限不足）：{}", path.display()))
}

#[tauri::command]
async fn download_to_temp<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    url: String,
) -> Result<String, String> {
    let emit = |stage: &str, percent: u32, message: &str| {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: stage.to_string(),
                percent,
                message: message.to_string(),
            },
        );
    };

    emit("downloading", 0, "正在下载...");

    let client = build_client(&app)?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("下载失败，HTTP {}", response.status()));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded = 0u64;
    let mut bytes: Vec<u8> = if total_size > 0 {
        Vec::with_capacity(total_size as usize)
    } else {
        Vec::new()
    };

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
        downloaded += chunk.len() as u64;
        bytes.extend_from_slice(&chunk);
        if total_size > 0 {
            let percent = (downloaded * 60 / total_size) as u32;
            emit(
                "downloading",
                percent,
                &format!(
                    "正在下载... {:.1}MB / {:.1}MB",
                    downloaded as f64 / 1_048_576.0,
                    total_size as f64 / 1_048_576.0
                ),
            );
        }
    }

    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    let temp_path = cache_dir.join("keytao_download.zip");
    std::fs::write(&temp_path, &bytes).map_err(|e| format!("保存临时文件失败: {e}"))?;

    emit("downloading", 60, "下载完成，准备解压...");
    Ok(temp_path.to_string_lossy().into_owned())
}

#[tauri::command]
async fn smart_install<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    zip_path: String,
    dest_path: String,
) -> Result<InstallResult, String> {
    let emit = |stage: &str, percent: u32, message: &str| {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: stage.to_string(),
                percent,
                message: message.to_string(),
            },
        );
    };

    emit("extracting", 61, "正在解压...");

    let zip_bytes = std::fs::read(&zip_path).map_err(|e| e.to_string())?;
    let dest = PathBuf::from(&dest_path);

    // First pass: collect zip metadata and merge candidates
    let (
        merged_dc_path,
        merged_dc_content,
        merged_schemas,
        merged_rime_lua_path,
        merged_rime_lua_content,
        renamed_lua_files,
    ) = {
        use std::collections::HashSet;
        use std::io::Read;

        let cursor = std::io::Cursor::new(&zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败: {e}"))?;

        let mut zip_dc_path: Option<String> = None;
        let mut zip_dc_content: Option<String> = None;
        let mut zip_rime_lua_path: Option<String> = None;
        let mut zip_rime_lua_content: Option<String> = None;
        let mut zip_lua_filenames: HashSet<String> = HashSet::new();

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let raw = file.name().to_string();
            let relative = raw.trim_end_matches('/').to_string();
            if relative.is_empty() || file.is_dir() {
                continue;
            }
            let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();

            if is_default_custom(&filename) && zip_dc_path.is_none() {
                let mut buf = String::new();
                file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                zip_dc_path = Some(relative);
                zip_dc_content = Some(buf);
            } else if filename == "rime.lua"
                && !relative.contains('/')
                && zip_rime_lua_path.is_none()
            {
                let mut buf = String::new();
                file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                zip_rime_lua_path = Some(relative);
                zip_rime_lua_content = Some(buf);
            } else if relative.starts_with("lua/") && !relative[4..].contains('/') {
                zip_lua_filenames.insert(filename);
            }
        }

        // Merge default.custom.yaml
        let (dc_path, dc_content, schemas) =
            if let (Some(path), Some(content)) = (zip_dc_path, zip_dc_content) {
                let existing = std::fs::read_to_string(dest.join("default.custom.yaml"))
                    .ok()
                    .or_else(|| std::fs::read_to_string(dest.join("default-custom.yaml")).ok());
                let (merged, user) = merge_default_custom(existing.as_deref(), &content);
                (Some(path), Some(merged), user)
            } else {
                (None, None, Vec::new())
            };

        // Merge rime.lua
        let (rl_path, rl_content, renamed) =
            if let (Some(path), Some(zip_rl)) = (zip_rime_lua_path, zip_rime_lua_content) {
                if let Ok(local_rl) = std::fs::read_to_string(dest.join("rime.lua")) {
                    let (merged, renames) = merge_rime_lua(&local_rl, &zip_rl, &zip_lua_filenames);
                    // Read local lua files that need renaming before zip overwrites them
                    let renamed_contents: Vec<(String, Vec<u8>)> = renames
                        .iter()
                        .filter_map(|(old, new)| {
                            let local_file = dest.join("lua").join(format!("{}.lua", old));
                            std::fs::read(&local_file)
                                .ok()
                                .map(|bytes| (new.clone(), bytes))
                        })
                        .collect();
                    (Some(path), Some(merged), renamed_contents)
                } else {
                    (Some(path), Some(zip_rl), Vec::new())
                }
            } else {
                (None, None, Vec::new())
            };

        (dc_path, dc_content, schemas, rl_path, rl_content, renamed)
    };

    // Second pass: smart extraction
    let cursor = std::io::Cursor::new(&zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败: {e}"))?;
    let total = archive.len();
    let mut logs: Vec<String> = Vec::new();

    for i in 0..total {
        let (relative, is_dir, content) = {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let raw = file.name().to_string();
            let relative = raw.trim_end_matches('/').to_string();
            if relative.is_empty() {
                continue;
            }
            let is_dir = file.is_dir();
            let mut buf = Vec::new();
            if !is_dir {
                use std::io::Read;
                file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            }
            (relative, is_dir, buf)
        };

        if is_dir {
            if let Err(e) = std::fs::create_dir_all(dest.join(&relative)) {
                logs.push(format!("[WARN] mkdir {relative}: {e}"));
            }
        } else if Some(&relative) == merged_dc_path.as_ref() {
            if let Some(ref mc) = merged_dc_content {
                let out = dest.join(&relative);
                match write_file_force(&out, mc.as_bytes()) {
                    Ok(tag) => logs.push(format!("[MERGED]{tag} {relative}")),
                    Err(e) => {
                        logs.push(format!("[ERROR] {relative}: {e}"));
                        return Err(e);
                    }
                }
            }
        } else if Some(&relative) == merged_rime_lua_path.as_ref() {
            if let Some(ref mc) = merged_rime_lua_content {
                let out = dest.join(&relative);
                match write_file_force(&out, mc.as_bytes()) {
                    Ok(tag) => logs.push(format!("[MERGED]{tag} {relative}")),
                    Err(e) => {
                        logs.push(format!("[ERROR] {relative}: {e}"));
                        return Err(e);
                    }
                }
            }
        } else {
            let out = dest.join(&relative);
            match write_file_force(&out, &content) {
                Ok(tag) => logs.push(format!("[OK]{tag} {relative}")),
                Err(e) => {
                    logs.push(format!("[ERROR] {relative}: {e}"));
                    return Err(e);
                }
            }
        }

        let percent = 61 + ((i + 1) * 39 / total) as u32;
        let fname = relative.rsplit('/').next().unwrap_or(&relative);
        emit(
            "extracting",
            percent,
            &format!("正在安装... {}/{}: {}", i + 1, total, fname),
        );
    }

    // Write renamed user lua files (saved before zip overwrote them)
    for (new_module, bytes) in &renamed_lua_files {
        let out = dest.join("lua").join(format!("{}.lua", new_module));
        match write_file_force(&out, bytes) {
            Ok(tag) => logs.push(format!("[RENAMED]{tag} lua/{new_module}.lua")),
            Err(e) => {
                logs.push(format!("[ERROR] rename lua/{new_module}.lua: {e}"));
                return Err(e);
            }
        }
    }

    std::fs::remove_file(&zip_path).ok();
    emit("done", 100, "安装完成！");

    let verify = verify_install(
        &dest,
        merged_dc_content.as_deref(),
        merged_rime_lua_content.as_deref(),
        &zip_bytes,
    );

    Ok(InstallResult {
        merged_schemas,
        logs,
        verify,
    })
}

// ─── Android plugin ──────────────────────────────────────────────────────────

#[cfg(target_os = "android")]
struct ScopedStorageHandle<R: tauri::Runtime>(tauri::plugin::PluginHandle<R>);

fn scoped_storage_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("scopedStorage")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin("ink.rea.keytao_app", "ScopedStoragePlugin")?;
                _app.manage(ScopedStorageHandle(handle));
            }
            Ok(())
        })
        .build()
}

#[cfg(target_os = "android")]
fn optional_jni_path(env: &mut JNIEnv<'_>, value: JString<'_>) -> Option<PathBuf> {
    if value.is_null() {
        return None;
    }
    let value = env.get_string(&value).ok()?;
    let value = value.to_string_lossy();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

#[cfg(target_os = "android")]
fn optional_jni_effective_color_scheme(
    env: &mut JNIEnv<'_>,
    value: JString<'_>,
) -> Option<keytao_theme::EffectiveColorScheme> {
    if value.is_null() {
        return None;
    }
    let value = env.get_string(&value).ok()?;
    match value.to_string_lossy().trim().to_ascii_lowercase().as_str() {
        "dark" | "night" => Some(keytao_theme::EffectiveColorScheme::Dark),
        "light" | "day" => Some(keytao_theme::EffectiveColorScheme::Light),
        _ => None,
    }
}

#[cfg(target_os = "android")]
fn jni_string(env: &mut JNIEnv<'_>, value: &str) -> jstring {
    env.new_string(value)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[cfg(target_os = "android")]
static ANDROID_IME_RUNTIME: Mutex<Option<keytao_core::ImeRuntime>> = Mutex::new(None);

#[cfg(target_os = "android")]
static ANDROID_IME_USER_THEME_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(target_os = "android")]
struct AndroidThemeResolverState {
    default_theme_path: Option<PathBuf>,
    user_theme_path: Option<PathBuf>,
    system_scheme: keytao_theme::EffectiveColorScheme,
    resolver: keytao_theme::ThemeResolver,
}

#[cfg(target_os = "android")]
impl AndroidThemeResolverState {
    fn new(
        default_theme_path: Option<PathBuf>,
        user_theme_path: Option<PathBuf>,
        system_scheme: keytao_theme::EffectiveColorScheme,
    ) -> Self {
        Self {
            resolver: keytao_theme::ThemeResolver::with_system_scheme(
                default_theme_path.clone(),
                user_theme_path.clone(),
                Some(system_scheme),
            ),
            default_theme_path,
            user_theme_path,
            system_scheme,
        }
    }

    fn matches(
        &self,
        default_theme_path: &Option<PathBuf>,
        user_theme_path: &Option<PathBuf>,
        system_scheme: keytao_theme::EffectiveColorScheme,
    ) -> bool {
        self.default_theme_path == *default_theme_path
            && self.user_theme_path == *user_theme_path
            && self.system_scheme == system_scheme
    }
}

#[cfg(target_os = "android")]
static ANDROID_IME_SYSTEM_COLOR_SCHEME: Mutex<keytao_theme::EffectiveColorScheme> =
    Mutex::new(keytao_theme::EffectiveColorScheme::Light);

#[cfg(target_os = "android")]
static ANDROID_IME_THEME_RESOLVER: Mutex<Option<AndroidThemeResolverState>> = Mutex::new(None);

#[cfg(target_os = "android")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AndroidImeStateJson {
    preedit: String,
    cursor: usize,
    candidates: Vec<keytao_core::Candidate>,
    all_candidates: Vec<keytao_core::Candidate>,
    highlighted_candidate_index: usize,
    page_size: usize,
    page: usize,
    is_last_page: bool,
    committed: String,
    select_keys: String,
    ascii_mode: bool,
    schema_name: String,
    accepted: bool,
    candidate_panel: keytao_theme::CandidatePanelModel,
    mode_hint: keytao_theme::ModeHintModel,
}

#[cfg(target_os = "android")]
fn android_state_json(state: keytao_core::ImeState, accepted: bool) -> String {
    let theme = android_current_theme();
    let mut ui_capabilities = keytao_theme::UiCapabilities::full_custom();
    ui_capabilities.supports_vertical = false;
    let candidate_panel = theme.candidate_panel_model(
        keytao_theme::CandidatePanelInput {
            preedit: state.preedit.clone(),
            candidates: state
                .candidates
                .iter()
                .map(|candidate| keytao_theme::ThemeCandidate {
                    text: candidate.text.clone(),
                    comment: candidate.comment.clone(),
                })
                .collect(),
            highlighted_candidate_index: state.highlighted_candidate_index,
            page: state.page,
            is_last_page: state.is_last_page,
            select_keys: state.select_keys.clone(),
        },
        &ui_capabilities,
    );
    let mode_hint = theme.mode_hint_model(state.ascii_mode);
    let value = AndroidImeStateJson {
        preedit: state.preedit,
        cursor: state.cursor,
        candidates: state.candidates,
        all_candidates: state.all_candidates,
        highlighted_candidate_index: state.highlighted_candidate_index,
        page_size: state.page_size,
        page: state.page,
        is_last_page: state.is_last_page,
        committed: state.committed.unwrap_or_default(),
        select_keys: state.select_keys.unwrap_or_default(),
        ascii_mode: state.ascii_mode,
        schema_name: state.schema_name,
        accepted,
        candidate_panel,
        mode_hint,
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".into())
}

#[cfg(target_os = "android")]
fn android_candidates_json(candidates: Vec<keytao_core::Candidate>) -> String {
    serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".into())
}

#[cfg(target_os = "android")]
fn android_current_theme() -> keytao_theme::ResolvedImeTheme {
    let user_path = ANDROID_IME_USER_THEME_PATH
        .lock()
        .ok()
        .and_then(|path| path.clone())
        .filter(|path| path.is_file());
    let system_scheme = ANDROID_IME_SYSTEM_COLOR_SCHEME
        .lock()
        .map(|scheme| *scheme)
        .unwrap_or(keytao_theme::EffectiveColorScheme::Light);
    android_cached_theme(None, user_path, system_scheme)
}

#[cfg(target_os = "android")]
fn android_cached_theme(
    default_theme_path: Option<PathBuf>,
    user_theme_path: Option<PathBuf>,
    system_scheme: keytao_theme::EffectiveColorScheme,
) -> keytao_theme::ResolvedImeTheme {
    let Ok(mut slot) = ANDROID_IME_THEME_RESOLVER.lock() else {
        return keytao_theme::resolve_theme_from_paths_with_system_scheme(
            default_theme_path.as_deref(),
            user_theme_path.as_deref(),
            system_scheme,
        );
    };
    let needs_resolver = slot
        .as_ref()
        .map(|state| !state.matches(&default_theme_path, &user_theme_path, system_scheme))
        .unwrap_or(true);
    if needs_resolver {
        *slot = Some(AndroidThemeResolverState::new(
            default_theme_path,
            user_theme_path,
            system_scheme,
        ));
    }
    slot.as_ref()
        .map(|state| state.resolver.current())
        .unwrap_or_default()
}

#[cfg(target_os = "android")]
fn android_result_json(result: keytao_core::KeyProcessResult) -> String {
    android_state_json(result.state, result.accepted)
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AndroidImeStatus {
    pub package_name: String,
    pub service_name: String,
    pub input_method_id: Option<String>,
    pub default_input_method: Option<String>,
    pub enabled: bool,
    pub selected: bool,
    pub can_show_picker: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AndroidStoragePermissionStatus {
    pub path: String,
    pub granted: bool,
    pub writable: bool,
    pub requires_manage_all_files: bool,
    pub can_open_settings: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AndroidImeInputSettings {
    pub haptics_enabled: bool,
    pub haptic_intensity: u8,
    pub config_path: Option<String>,
    pub reload_stamp_path: Option<String>,
    pub message: String,
}

#[cfg(target_os = "android")]
fn android_session<'a>(session: jlong) -> Option<&'a keytao_core::ImeRuntimeSession> {
    if session == 0 {
        return None;
    }
    Some(unsafe { &*(session as *mut keytao_core::ImeRuntimeSession) })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeResolveThemeJson(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    default_theme_path: JString<'_>,
    user_theme_path: JString<'_>,
    system_color_scheme: JString<'_>,
) -> jstring {
    let default_path = optional_jni_path(&mut env, default_theme_path);
    let user_path = optional_jni_path(&mut env, user_theme_path);
    let system_scheme = optional_jni_effective_color_scheme(&mut env, system_color_scheme)
        .unwrap_or(keytao_theme::EffectiveColorScheme::Light);
    if let Ok(mut current) = ANDROID_IME_SYSTEM_COLOR_SCHEME.lock() {
        *current = system_scheme;
    }
    let theme = android_cached_theme(default_path, user_path, system_scheme);
    match keytao_theme::resolved_theme_json(&theme) {
        Ok(json) => jni_string(&mut env, &json),
        Err(error) => jni_string(&mut env, &format!(r#"{{"error":"{error}"}}"#)),
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeDefaultKeyboardYaml(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jstring {
    jni_string(&mut env, keytao_theme::default_keyboard_yaml())
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeResolveKeyboardJson(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    default_keyboard_path: JString<'_>,
    user_keyboard_path: JString<'_>,
) -> jstring {
    let default_path = optional_jni_path(&mut env, default_keyboard_path);
    let user_path = optional_jni_path(&mut env, user_keyboard_path);
    let keyboard =
        keytao_theme::resolve_keyboard_from_paths(default_path.as_deref(), user_path.as_deref());
    match keytao_theme::resolved_keyboard_json(&keyboard) {
        Ok(json) => jni_string(&mut env, &json),
        Err(error) => jni_string(&mut env, &format!(r#"{{"error":"{error}"}}"#)),
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeEngineAvailable(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jboolean {
    1
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeInit(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    user_dir: JString<'_>,
    shared_dir: JString<'_>,
) -> jboolean {
    let Some(user_dir) = optional_jni_path(&mut env, user_dir) else {
        return 0;
    };
    let user_theme_path = user_dir.join("theme.yaml");
    let shared_dir = optional_jni_path(&mut env, shared_dir)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(keytao_core::default_shared_data_dir);

    let runtime = keytao_core::ImeRuntime::with_dirs(user_dir, shared_dir);
    if runtime.init().is_err() {
        return 0;
    }
    if let Ok(mut theme_path) = ANDROID_IME_USER_THEME_PATH.lock() {
        *theme_path = Some(user_theme_path);
    }
    if let Ok(mut theme_resolver) = ANDROID_IME_THEME_RESOLVER.lock() {
        *theme_resolver = None;
    }
    let Ok(mut slot) = ANDROID_IME_RUNTIME.lock() else {
        return 0;
    };
    *slot = Some(runtime);
    1
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeReload(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jboolean {
    let Ok(slot) = ANDROID_IME_RUNTIME.lock() else {
        return 0;
    };
    let Some(runtime) = slot.as_ref().cloned() else {
        return 0;
    };
    drop(slot);
    if runtime.reload().is_ok() {
        1
    } else {
        0
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeCreateSession(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
) -> jlong {
    let Ok(slot) = ANDROID_IME_RUNTIME.lock() else {
        return 0;
    };
    let Some(runtime) = slot.as_ref().cloned() else {
        return 0;
    };
    drop(slot);
    match runtime.create_session() {
        Ok(session) => Box::into_raw(Box::new(session)) as jlong,
        Err(_) => 0,
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeDestroySession(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) {
    if session == 0 {
        return;
    }
    unsafe {
        drop(Box::from_raw(
            session as *mut keytao_core::ImeRuntimeSession,
        ));
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSessionState(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_state_json(session.state(), false))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeProcessKey(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    keyval: jint,
    modifiers: jint,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    let Some(result) = session.process_key_result(keyval as u32, modifiers as u32) else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_result_json(result))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSelectCandidate(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    index: jint,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.select_candidate(index.max(0) as usize) else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_state_json(state, true))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSelectCandidateGlobal(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    index: jint,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.select_candidate_global(index.max(0) as usize) else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_state_json(state, true))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeAllCandidates(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    limit: jint,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    let Some(candidates) = session.all_candidates_limited(limit.max(0) as usize) else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_candidates_json(candidates))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeChangePage(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    backward: jboolean,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.change_page(backward != 0) else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_state_json(state, true))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeReset(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.reset() else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_state_json(state, true))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeGetAsciiMode(
    _env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
) -> jboolean {
    let Some(session) = android_session(session) else {
        return 0;
    };
    if session.is_ascii_mode() {
        1
    } else {
        0
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_ink_rea_keytao_1app_KeytaoNativeBridge_nativeSetAsciiMode(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    session: jlong,
    enabled: jboolean,
) -> jstring {
    let Some(session) = android_session(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.set_ascii_mode(enabled != 0) else {
        return std::ptr::null_mut();
    };
    jni_string(&mut env, &android_state_json(state, true))
}

#[tauri::command]
async fn android_ime_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<AndroidImeStatus, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("imeStatus", ())
            .map_err(|e| e.to_string())?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_storage_permission_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<AndroidStoragePermissionStatus, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("storagePermissionStatus", ())
            .map_err(|e| e.to_string())?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_open_storage_permission_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("openStoragePermissionSettings", ())
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_open_input_method_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("openInputMethodSettings", ())
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_show_input_method_picker<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("showInputMethodPicker", ())
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Not Android".into())
    }
}

#[cfg(target_os = "android")]
fn android_keytao_root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    let result: serde_json::Value = app
        .state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin("keytaoRoot", ())
        .map_err(|e| e.to_string())?;
    let path = result
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or("Android KeyTao data directory is unavailable")?;
    Ok(PathBuf::from(path))
}

#[cfg(target_os = "android")]
fn android_reload_stamp_path(root: &Path) -> PathBuf {
    root.join(IME_RELOAD_STAMP_FILE)
}

#[cfg(target_os = "android")]
fn write_android_reload_stamp(root: &Path) -> Result<PathBuf, String> {
    let stamp = android_reload_stamp_path(root);
    if let Some(parent) = stamp.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 Android 输入法目录失败: {e}"))?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::fs::write(&stamp, now.to_string())
        .map_err(|e| format!("写入 Android 输入法重载标记失败 {}: {e}", stamp.display()))?;
    Ok(stamp)
}

#[cfg(target_os = "android")]
fn android_ime_config_path(root: &Path) -> PathBuf {
    root.join("android_ime.json")
}

#[cfg(target_os = "android")]
fn read_android_ime_config(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

#[cfg(target_os = "android")]
fn android_ime_haptics_settings_from_config(
    root: &Path,
    message: String,
) -> AndroidImeInputSettings {
    let path = android_ime_config_path(root);
    let config = read_android_ime_config(&path);
    let haptics = config.get("haptics").and_then(|value| value.as_object());
    let haptics_enabled = haptics
        .and_then(|value| value.get("enabled"))
        .and_then(|value| value.as_bool())
        .or_else(|| {
            config
                .get("hapticsEnabled")
                .and_then(|value| value.as_bool())
        })
        .unwrap_or(true);
    let haptic_intensity = haptics
        .and_then(|value| value.get("intensity"))
        .and_then(|value| value.as_u64())
        .or_else(|| {
            config
                .get("hapticIntensity")
                .and_then(|value| value.as_u64())
        })
        .unwrap_or(42)
        .clamp(1, 100) as u8;

    AndroidImeInputSettings {
        haptics_enabled,
        haptic_intensity,
        config_path: Some(path_string(path)),
        reload_stamp_path: Some(path_string(android_reload_stamp_path(root))),
        message,
    }
}

#[tauri::command]
async fn get_android_ime_input_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<AndroidImeInputSettings, String> {
    #[cfg(target_os = "android")]
    {
        let root = android_keytao_root(&app)?;
        Ok(android_ime_haptics_settings_from_config(
            &root,
            "已读取 Android 输入反馈配置".into(),
        ))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("Android IME input settings are only available on Android".into())
    }
}

#[tauri::command]
async fn set_android_ime_input_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    haptics_enabled: bool,
    haptic_intensity: u8,
) -> Result<AndroidImeInputSettings, String> {
    #[cfg(target_os = "android")]
    {
        let root = android_keytao_root(&app)?;
        let path = android_ime_config_path(&root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建 Android 输入法配置目录失败: {e}"))?;
        }
        let mut config = read_android_ime_config(&path);
        let mut haptics = config
            .get("haptics")
            .and_then(|value| value.as_object())
            .cloned()
            .unwrap_or_default();
        haptics.insert("enabled".into(), serde_json::Value::Bool(haptics_enabled));
        haptics.insert(
            "intensity".into(),
            serde_json::Value::from(haptic_intensity.clamp(1, 100)),
        );
        config.insert("haptics".into(), serde_json::Value::Object(haptics));

        let content = serde_json::to_string_pretty(&serde_json::Value::Object(config))
            .map_err(|e| format!("序列化 Android 输入法配置失败: {e}"))?;
        std::fs::write(&path, format!("{content}\n"))
            .map_err(|e| format!("写入 Android 输入法配置失败 {}: {e}", path.display()))?;

        let message = match write_android_reload_stamp(&root) {
            Ok(stamp) => format!(
                "已保存 Android 输入反馈配置并通知输入法重载：{}",
                stamp.display()
            ),
            Err(e) => format!("已保存 Android 输入反馈配置，但输入法重载通知失败：{e}"),
        };
        Ok(android_ime_haptics_settings_from_config(&root, message))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let _ = haptics_enabled;
        let _ = haptic_intensity;
        Err("Android IME input settings are only available on Android".into())
    }
}

#[tauri::command]
async fn android_keytao_data_dir<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        android_keytao_root(&app).map(|path| Some(path_string(path)))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(None)
    }
}

#[tauri::command]
async fn android_open_app<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    package_name: String,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "openApp",
                serde_json::json!({ "packageName": package_name }),
            )
            .map(|_: serde_json::Value| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_pick_directory<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        app.state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("pickDirectory", ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_list_files<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    tree_uri: String,
) -> Result<Vec<FileItem>, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin("listFiles", serde_json::json!({ "treeUri": tree_uri }))
            .map_err(|e| e.to_string())?;

        let files = result["files"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| FileItem {
                        name: v["name"].as_str().unwrap_or("").to_string(),
                        is_dir: v["isDir"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(files)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[tauri::command]
async fn android_read_local_schemas<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    tree_uri: String,
) -> Result<LocalSchemaInfo, String> {
    #[cfg(target_os = "android")]
    {
        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "readLocalSchemas",
                serde_json::json!({ "treeUri": tree_uri }),
            )
            .map_err(|e| e.to_string())?;

        let schemas = result["schemas"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let version = result["version"].as_str().map(String::from);

        let installed = result["installed"].as_bool().unwrap_or(false);

        Ok(LocalSchemaInfo {
            installed,
            version,
            schemas,
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[cfg(target_os = "android")]
fn install_result_from_value(result: &serde_json::Value) -> InstallResult {
    let merged_schemas = result["mergedSchemas"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let logs = result["logs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let verify = result["verify"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    Some(VerifyEntry {
                        path: v["path"].as_str()?.to_string(),
                        ok: v["ok"].as_bool().unwrap_or(false),
                        note: v["note"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    InstallResult {
        merged_schemas,
        logs,
        verify,
    }
}

#[tauri::command]
async fn android_smart_extract<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    zip_path: String,
    tree_uri: String,
) -> Result<InstallResult, String> {
    #[cfg(target_os = "android")]
    {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: "extracting".into(),
                percent: 61,
                message: "正在解压...".into(),
            },
        );

        let app2 = app.clone();
        let on_progress: tauri::ipc::Channel<serde_json::Value> =
            tauri::ipc::Channel::new(move |body: tauri::ipc::InvokeResponseBody| {
                let data: serde_json::Value = match body {
                    tauri::ipc::InvokeResponseBody::Json(s) => {
                        serde_json::from_str(&s).unwrap_or_default()
                    }
                    tauri::ipc::InvokeResponseBody::Raw(b) => {
                        serde_json::from_slice(&b).unwrap_or_default()
                    }
                };
                if let (Some(stage), Some(percent), Some(message)) = (
                    data.get("stage").and_then(|v| v.as_str()).map(String::from),
                    data.get("percent")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    data.get("message")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                ) {
                    let _ = app2.emit(
                        "install-progress",
                        InstallProgress {
                            stage,
                            percent,
                            message,
                        },
                    );
                }
                Ok(())
            });

        let _private_result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "smartExtractZipToPrivate",
                serde_json::json!({ "zipPath": zip_path.clone() }),
            )
            .map_err(|e| e.to_string())?;

        let result: serde_json::Value = app
            .state::<ScopedStorageHandle<R>>()
            .0
            .run_mobile_plugin(
                "smartExtractZip",
                serde_json::json!({ "zipPath": zip_path, "treeUri": tree_uri, "onProgress": on_progress }),
            )
            .map_err(|e| e.to_string())?;

        let _ = app.emit(
            "install-progress",
            InstallProgress {
                stage: "done".into(),
                percent: 100,
                message: "安装完成！".into(),
            },
        );

        Ok(install_result_from_value(&result))
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Not Android".into())
    }
}

#[derive(Serialize)]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
}

#[tauri::command]
async fn check_app_update(app: AppHandle) -> Result<AppUpdateInfo, String> {
    let t0 = std::time::Instant::now();
    tracing::info!("[check_app_update] start");
    let current = app.package_info().version.to_string();
    let client = build_client(&app)?;
    let resp = client
        .get("https://api.github.com/repos/xkinput/keytao-app/releases/latest")
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(
                "[check_app_update] failed after {}ms: {e}",
                t0.elapsed().as_millis()
            );
            e.to_string()
        })?;
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    tracing::info!("[check_app_update] done in {}ms", t0.elapsed().as_millis());
    let latest_tag = json["tag_name"].as_str().unwrap_or("").to_string();
    let latest = latest_tag.trim_start_matches('v').to_string();
    let release_url = json["html_url"].as_str().unwrap_or("").to_string();
    let has_update = !latest.is_empty() && latest != current;
    Ok(AppUpdateInfo {
        current_version: current,
        latest_version: latest,
        has_update,
        release_url,
    })
}

// ─── macOS system IME management ─────────────────────────────────────────────

#[derive(Serialize)]
pub struct MacosImeStatus {
    pub installed: bool,
    pub app_path: Option<String>,
    pub user_data_dir: Option<String>,
    pub shared_data_dir: Option<String>,
    pub shared_data_source: String,
    pub reload_stamp_path: Option<String>,
    pub reload_stamp_signature: Option<String>,
    pub log_dir: Option<String>,
    pub message: String,
}

#[cfg(target_os = "macos")]
fn macos_ime_app_path() -> PathBuf {
    PathBuf::from("/Library/Input Methods/KeyTao.app")
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn macos_install_script_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("KEYTAO_MACOS_IME_INSTALL_SCRIPT").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_dir = manifest_dir.parent()?;
    let path = workspace_dir.join("crates/keytao-macos-ime/install.sh");
    path.is_file().then_some(path)
}

#[cfg(target_os = "macos")]
fn macos_ime_status_inner(app: &AppHandle) -> MacosImeStatus {
    let path = macos_ime_app_path();
    let installed = path.join("Contents/MacOS/KeyTaoIME").is_file();
    let (shared_data_dir, shared_data_source) = shared_data_status(macos_app_shared_data_dir(app));
    let (reload_stamp_path, reload_stamp_signature) = reload_stamp_status();
    MacosImeStatus {
        installed,
        app_path: Some(path.to_string_lossy().into_owned()),
        user_data_dir: default_user_data_dir_string(),
        shared_data_dir,
        shared_data_source,
        reload_stamp_path,
        reload_stamp_signature,
        log_dir: keytao_core::default_user_data_dir().map(|dir| path_string(dir.join("log"))),
        message: if installed {
            "KeyTao macOS input method is installed".into()
        } else {
            "KeyTao macOS input method is not installed".into()
        },
    }
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn command_output_tail(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output).trim().to_string();
    let count = text.chars().count();
    if count <= 4000 {
        text
    } else {
        text.chars().skip(count - 4000).collect()
    }
}

/// Check whether the KeyTao.app input method is installed.
#[tauri::command]
fn macos_ime_status(app: AppHandle) -> MacosImeStatus {
    #[cfg(target_os = "macos")]
    {
        macos_ime_status_inner(&app)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        MacosImeStatus {
            installed: false,
            app_path: None,
            user_data_dir: None,
            shared_data_dir: None,
            shared_data_source: "unsupported".into(),
            reload_stamp_path: None,
            reload_stamp_signature: None,
            log_dir: None,
            message: "macOS only".into(),
        }
    }
}

/// Run the repository install script for local macOS IME testing.
#[tauri::command]
#[cfg(target_os = "macos")]
async fn macos_install_ime(_app: AppHandle) -> Result<MacosImeStatus, String> {
    #[cfg(not(debug_assertions))]
    {
        return Err("macOS IME install script is only available in development builds".into());
    }

    #[cfg(debug_assertions)]
    {
        let script = macos_install_script_path().ok_or(
        "macOS IME install script not found; build and install target/keytao-macos-pkg/KeyTao.pkg instead",
    )?;
        let workspace_dir = script
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .ok_or("Cannot determine workspace directory for install script")?;
        let build_dir = workspace_dir.join("target/keytao-macos-ime");

        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("/bin/bash")
                .arg(&script)
                .arg("--release")
                .current_dir(&workspace_dir)
                .env("KEYTAO_MACOS_BUILD_DIR", build_dir)
                .output()
        })
        .await
        .map_err(|e| format!("install task failed: {e}"))?
        .map_err(|e| format!("run install script: {e}"))?;

        if !output.status.success() {
            let stdout = command_output_tail(&output.stdout);
            let stderr = command_output_tail(&output.stderr);
            return Err(format!(
                "install script failed with status {}\nstdout:\n{}\nstderr:\n{}",
                output.status, stdout, stderr
            ));
        }

        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.keyboard?InputSources")
            .output()
            .map_err(|e| format!("open System Settings: {e}"))?;

        Ok(macos_ime_status_inner(&_app))
    }
}

#[tauri::command]
#[cfg(not(target_os = "macos"))]
async fn macos_install_ime(_app: AppHandle) -> Result<MacosImeStatus, String> {
    Err("macOS only".into())
}

/// Remove KeyTao.app from /Library/Input Methods/.
#[tauri::command]
async fn macos_uninstall_ime() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        #[cfg(not(debug_assertions))]
        {
            return Err(
                "macOS IME uninstall script is only available in development builds".into(),
            );
        }

        #[cfg(debug_assertions)]
        {
            let dst = macos_ime_app_path();
            if dst.exists() {
                let output = std::process::Command::new("sudo")
                    .args(["rm", "-rf"])
                    .arg(&dst)
                    .output()
                    .map_err(|e| format!("run sudo rm: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "remove failed with status {}\n{}",
                        output.status,
                        command_output_tail(&output.stderr)
                    ));
                }
            }
            Ok(())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("macOS only".into())
    }
}

// ─── Local schema info ───────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct LocalSchemaInfo {
    pub installed: bool,
    pub version: Option<String>,
    pub schemas: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct ComponentVersions {
    pub app_version: String,
    pub tauri_version: String,
    pub librime_version: Option<String>,
    pub opencc_version: Option<String>,
    pub data_dir: Option<String>,
}

fn command_version(command: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let value = text.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn non_empty_version(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn env_version(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty_version)
}

fn nix_store_package_version(package: &str) -> Option<String> {
    let entries = std::fs::read_dir("/nix/store").ok()?;
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.ends_with(".drv"))
        .filter_map(|name| {
            let marker = format!("-{package}-");
            let version = name.split_once(&marker)?.1.to_string();
            Some(version)
        })
        .max()
}

fn version_from_pkg_config_content(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() == "Version" {
            non_empty_version(value)
        } else {
            None
        }
    })
}

fn pkg_config_file_version(lib_dir: &Path, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let path = lib_dir.join("pkgconfig").join(name);
        let content = std::fs::read_to_string(path).ok()?;
        version_from_pkg_config_content(&content)
    })
}

fn metadata_file_version(path: &Path, keys: &[&str]) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        keys.iter()
            .any(|candidate| key.trim() == *candidate)
            .then(|| non_empty_version(value))
            .flatten()
    })
}

fn rime_filename_version(name: &str) -> Option<String> {
    let version = name
        .strip_prefix("librime.")
        .and_then(|value| value.strip_suffix(".dylib"))
        .or_else(|| name.strip_prefix("librime.so."))?;
    version
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_digit())
        .and_then(|_| non_empty_version(version))
}

fn rime_lib_dir_version(path: &Path) -> Option<String> {
    if path.is_file() {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(rime_filename_version);
    }

    metadata_file_version(
        &path.join("librime-release.txt"),
        &["version", "librime_version"],
    )
    .or_else(|| pkg_config_file_version(path, &["rime.pc", "librime.pc"]))
    .or_else(|| {
        std::fs::read_dir(path)
            .ok()?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| rime_filename_version(&name))
            .max()
    })
}

fn opencc_dir_version(path: &Path) -> Option<String> {
    if path.is_file() {
        return std::fs::read_to_string(path)
            .ok()
            .and_then(|content| version_from_pkg_config_content(&content));
    }

    metadata_file_version(
        &path.join("opencc-release.txt"),
        &["version", "opencc_version"],
    )
    .or_else(|| pkg_config_file_version(path, &["opencc.pc", "libopencc.pc", "OpenCC.pc"]))
    .or_else(|| {
        pkg_config_file_version(
            &path.join("lib"),
            &["opencc.pc", "libopencc.pc", "OpenCC.pc"],
        )
    })
}

fn bundled_runtime_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.extend([
            resource_dir.clone(),
            resource_dir.join("Frameworks"),
            resource_dir.join("rime-data"),
            resource_dir.join("runtime"),
            resource_dir.join("runtime/lib"),
            resource_dir.join("runtime/rime-data"),
            resource_dir.join("keytao-windows-ime-runtime/current"),
            resource_dir.join("keytao-windows-ime-runtime/current/bin"),
            resource_dir.join("keytao-windows-ime-runtime/current/lib"),
            resource_dir.join("keytao-windows-ime-runtime/current/rime-data"),
        ]);
        if let Some(contents) = resource_dir.parent() {
            candidates.push(contents.join("Frameworks"));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.extend([
                exe_dir.to_path_buf(),
                exe_dir.join("Frameworks"),
                exe_dir.join("runtime"),
                exe_dir.join("runtime/lib"),
                exe_dir.join("runtime/rime-data"),
            ]);
            if let Some(contents) = exe_dir.parent() {
                candidates.push(contents.join("Frameworks"));
            }
        }
    }

    candidates
}

fn bundled_librime_version(app: &AppHandle) -> Option<String> {
    bundled_runtime_candidates(app)
        .into_iter()
        .find_map(|dir| rime_lib_dir_version(&dir))
}

fn runtime_librime_version() -> Option<String> {
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    {
        keytao_core::librime_runtime_version().and_then(non_empty_version)
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        None
    }
}

fn librime_version(app: &AppHandle) -> Option<String> {
    runtime_librime_version()
        .or_else(|| option_env!("RIME_VERSION").and_then(non_empty_version))
        .or_else(|| env_version("RIME_VERSION"))
        .or_else(|| {
            env_version("RIME_LIB_DIR").and_then(|path| rime_lib_dir_version(Path::new(&path)))
        })
        .or_else(|| bundled_librime_version(app))
        .or_else(|| command_version("pkg-config", &["--modversion", "rime"]))
        .or_else(|| command_version("pkg-config", &["--modversion", "librime"]))
        .or_else(|| nix_store_package_version("librime"))
}

fn opencc_version(app: &AppHandle) -> Option<String> {
    option_env!("OPENCC_VERSION")
        .and_then(non_empty_version)
        .or_else(|| env_version("OPENCC_VERSION"))
        .or_else(|| {
            env_version("OPENCC_LIB_DIR").and_then(|path| opencc_dir_version(Path::new(&path)))
        })
        .or_else(|| {
            bundled_runtime_candidates(app)
                .into_iter()
                .find_map(|dir| opencc_dir_version(&dir))
        })
        .or_else(|| {
            ["opencc", "libopencc", "OpenCC"]
                .iter()
                .find_map(|name| command_version("pkg-config", &["--modversion", name]))
        })
        .or_else(|| command_version("opencc", &["--version"]))
        .or_else(|| nix_store_package_version("opencc"))
}

#[tauri::command]
fn get_component_versions(app: AppHandle) -> ComponentVersions {
    #[cfg(target_os = "android")]
    let data_dir = android_keytao_root(&app).ok().map(path_string);
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let data_dir = keytao_core::default_user_data_dir().map(|p| p.to_string_lossy().into_owned());
    #[cfg(target_os = "ios")]
    let data_dir = ios_keytao_root(&app).ok().map(path_string);

    ComponentVersions {
        app_version: app.package_info().version.to_string(),
        tauri_version: tauri::VERSION.to_string(),
        librime_version: librime_version(&app),
        opencc_version: opencc_version(&app),
        data_dir,
    }
}

#[tauri::command]
fn check_local_schema<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: Option<String>,
) -> LocalSchemaInfo {
    let dir: Option<PathBuf> = path.map(PathBuf::from).or_else(|| {
        #[cfg(target_os = "android")]
        {
            return android_keytao_root(&app).ok();
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            return keytao_core::default_user_data_dir();
        }
        #[cfg(target_os = "ios")]
        {
            return ios_keytao_root(&app).ok();
        }
    });

    let Some(dir) = dir else {
        return LocalSchemaInfo {
            installed: false,
            version: None,
            schemas: vec![],
        };
    };

    let installed = dir.join("keytao.schema.yaml").exists()
        || dir.join("build").join("keytao.table.bin").exists();

    let version = std::fs::read_to_string(dir.join("version.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let schemas = read_local_schemas(dir.to_string_lossy().into_owned());

    LocalSchemaInfo {
        installed,
        version,
        schemas,
    }
}

// ─── Deploy librime to default keytao data dir ────────────────────────────────

#[derive(Serialize, Clone)]
pub struct DeployResult {
    pub success: bool,
    pub message: String,
}

#[tauri::command]
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
async fn rime_deploy_default(app: AppHandle) -> Result<DeployResult, String> {
    let dest =
        keytao_core::default_user_data_dir().ok_or("Cannot determine keytao data directory")?;
    let user = dest.to_string_lossy().into_owned();
    #[cfg(target_os = "windows")]
    let shared =
        windows_app_shared_data_dir(&app).unwrap_or_else(keytao_core::default_shared_data_dir);
    #[cfg(target_os = "macos")]
    let shared =
        macos_app_shared_data_dir(&app).unwrap_or_else(keytao_core::default_shared_data_dir);
    #[cfg(target_os = "linux")]
    let shared =
        linux_app_shared_data_dir(&app).unwrap_or_else(keytao_core::default_shared_data_dir);

    let _ = app.emit("deploy-progress", "正在部署 librime...");

    match tokio::task::spawn_blocking(move || keytao_core::deploy(user, shared)).await {
        Ok(Ok(())) => {
            let _ = app.emit("deploy-progress", "部署完成");
            // Refresh the test-input Rime session so the new schemas take effect immediately.
            #[cfg(target_os = "linux")]
            if let Ok(engine) = keytao_core::Engine::new() {
                let state: tauri::State<rime::RimeEngine> = app.state();
                *state.engine.lock().unwrap() = Some(engine);
            }
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            match write_keytao_ime_reload_stamp() {
                Ok(()) => {
                    let _ = app.emit("deploy-progress", "已通知系统输入法重载");
                    tracing::info!("IME reload stamp written after deploy");
                }
                Err(e) => {
                    let _ = app.emit("deploy-progress", format!("系统输入法重载通知失败：{e}"));
                    tracing::warn!("IME reload stamp failed after deploy: {e}");
                }
            }
            Ok(DeployResult {
                success: true,
                message: "部署成功".into(),
            })
        }
        Ok(Err(e)) => Err(format!("部署失败: {e}")),
        Err(e) => Err(format!("任务错误: {e}")),
    }
}

#[tauri::command]
#[cfg(target_os = "android")]
async fn rime_deploy_default<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<DeployResult, String> {
    let result: serde_json::Value = app
        .state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin("writeImeReloadStamp", ())
        .map_err(|e| e.to_string())?;
    let path = result["path"]
        .as_str()
        .unwrap_or("/storage/emulated/0/keytao/keytao-ime.reload");
    Ok(DeployResult {
        success: true,
        message: format!("已通知 Android 输入法重载：{path}"),
    })
}

#[tauri::command]
#[cfg(target_os = "ios")]
async fn rime_deploy_default(app: AppHandle) -> Result<DeployResult, String> {
    let dest = ios_keytao_root(&app)?;
    let user = dest.to_string_lossy().into_owned();
    let shared =
        ios_app_shared_data_dir(&app, &dest).unwrap_or_else(keytao_core::default_shared_data_dir);

    let _ = app.emit("deploy-progress", "正在部署 librime...");

    match tokio::task::spawn_blocking(move || keytao_core::deploy(user, shared)).await {
        Ok(Ok(())) => {
            let _ = app.emit("deploy-progress", "部署完成");
            match write_ios_reload_stamp(&dest) {
                Ok(stamp) => {
                    let message = format!("已通知 iOS 输入法重载：{}", stamp.display());
                    let _ = app.emit("deploy-progress", &message);
                    Ok(DeployResult {
                        success: true,
                        message,
                    })
                }
                Err(e) => {
                    let message = format!("部署完成，但 iOS 输入法重载通知失败：{e}");
                    let _ = app.emit("deploy-progress", &message);
                    Ok(DeployResult {
                        success: true,
                        message,
                    })
                }
            }
        }
        Ok(Err(e)) => Err(format!("部署失败: {e}")),
        Err(e) => Err(format!("任务错误: {e}")),
    }
}

#[tauri::command]
#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
)))]
async fn rime_deploy_default(_app: AppHandle) -> Result<DeployResult, String> {
    Err("librime deployment is not supported on this platform yet".into())
}

#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn get_ime_ui_settings() -> Result<ImeUiSettings, String> {
    ime_ui_settings_with_message("已读取输入法 UI 配置".into())
}

#[tauri::command]
#[cfg(target_os = "android")]
fn get_ime_ui_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ImeUiSettings, String> {
    let root = android_keytao_root(&app)?;
    ime_ui_settings_from_paths(
        root.join("theme.yaml"),
        Some(android_reload_stamp_path(&root)),
        "已读取 Android 输入法 UI 配置".into(),
    )
}

#[tauri::command]
#[cfg(target_os = "ios")]
fn get_ime_ui_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ImeUiSettings, String> {
    let root = ios_keytao_root(&app)?;
    ime_ui_settings_from_paths(
        root.join("theme.yaml"),
        Some(ios_reload_stamp_path(&root)),
        "已读取 iOS 输入法 UI 配置".into(),
    )
}

#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn set_ime_ui_color_scheme(
    color_scheme: keytao_theme::UiColorScheme,
) -> Result<ImeUiSettings, String> {
    write_ime_ui_color_scheme(color_scheme)?;
    let reload_message = match write_keytao_ime_reload_stamp() {
        Ok(()) => "已保存输入法 UI 配置并通知系统输入法重载".to_string(),
        Err(e) => format!("已保存输入法 UI 配置，但系统输入法重载通知失败：{e}"),
    };
    ime_ui_settings_with_message(reload_message)
}

#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn set_ime_ui_settings(
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
) -> Result<ImeUiSettings, String> {
    write_ime_ui_settings(color_scheme, orientation, accent_color)?;
    let reload_message = match write_keytao_ime_reload_stamp() {
        Ok(()) => "已保存输入法 UI 配置并通知系统输入法重载".to_string(),
        Err(e) => format!("已保存输入法 UI 配置，但系统输入法重载通知失败：{e}"),
    };
    ime_ui_settings_with_message(reload_message)
}

#[tauri::command]
#[cfg(target_os = "android")]
fn set_ime_ui_color_scheme<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    color_scheme: keytao_theme::UiColorScheme,
) -> Result<ImeUiSettings, String> {
    let root = android_keytao_root(&app)?;
    let theme_path = root.join("theme.yaml");
    let current = ime_ui_settings_from_paths(
        theme_path.clone(),
        Some(android_reload_stamp_path(&root)),
        String::new(),
    )?;
    write_ime_ui_settings_to_path(
        &theme_path,
        color_scheme,
        current.orientation,
        current.accent_color,
    )?;
    let reload_message = match write_android_reload_stamp(&root) {
        Ok(path) => format!(
            "已保存 Android 输入法 UI 配置并通知输入法重载：{}",
            path.display()
        ),
        Err(e) => format!("已保存 Android 输入法 UI 配置，但输入法重载通知失败：{e}"),
    };
    ime_ui_settings_from_paths(
        theme_path,
        Some(android_reload_stamp_path(&root)),
        reload_message,
    )
}

#[tauri::command]
#[cfg(target_os = "ios")]
fn set_ime_ui_color_scheme(
    app: tauri::AppHandle,
    color_scheme: keytao_theme::UiColorScheme,
) -> Result<ImeUiSettings, String> {
    let root = ios_keytao_root(&app)?;
    let theme_path = root.join("theme.yaml");
    let current = ime_ui_settings_from_paths(
        theme_path.clone(),
        Some(ios_reload_stamp_path(&root)),
        String::new(),
    )?;
    write_ime_ui_settings_to_path(
        &theme_path,
        color_scheme,
        current.orientation,
        current.accent_color,
    )?;
    let reload_message = match write_ios_reload_stamp(&root) {
        Ok(path) => format!(
            "已保存 iOS 输入法 UI 配置并通知输入法重载：{}",
            path.display()
        ),
        Err(e) => format!("已保存 iOS 输入法 UI 配置，但输入法重载通知失败：{e}"),
    };
    ime_ui_settings_from_paths(
        theme_path,
        Some(ios_reload_stamp_path(&root)),
        reload_message,
    )
}

#[tauri::command]
#[cfg(target_os = "android")]
fn set_ime_ui_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
) -> Result<ImeUiSettings, String> {
    let root = android_keytao_root(&app)?;
    let theme_path = root.join("theme.yaml");
    write_ime_ui_settings_to_path(&theme_path, color_scheme, orientation, accent_color)?;
    let reload_message = match write_android_reload_stamp(&root) {
        Ok(path) => format!(
            "已保存 Android 输入法 UI 配置并通知输入法重载：{}",
            path.display()
        ),
        Err(e) => format!("已保存 Android 输入法 UI 配置，但输入法重载通知失败：{e}"),
    };
    ime_ui_settings_from_paths(
        theme_path,
        Some(android_reload_stamp_path(&root)),
        reload_message,
    )
}

#[tauri::command]
#[cfg(target_os = "ios")]
fn set_ime_ui_settings(
    app: tauri::AppHandle,
    color_scheme: keytao_theme::UiColorScheme,
    orientation: keytao_theme::PanelOrientation,
    accent_color: String,
) -> Result<ImeUiSettings, String> {
    let root = ios_keytao_root(&app)?;
    let theme_path = root.join("theme.yaml");
    write_ime_ui_settings_to_path(&theme_path, color_scheme, orientation, accent_color)?;
    let reload_message = match write_ios_reload_stamp(&root) {
        Ok(path) => format!(
            "已保存 iOS 输入法 UI 配置并通知输入法重载：{}",
            path.display()
        ),
        Err(e) => format!("已保存 iOS 输入法 UI 配置，但输入法重载通知失败：{e}"),
    };
    ime_ui_settings_from_paths(
        theme_path,
        Some(ios_reload_stamp_path(&root)),
        reload_message,
    )
}

#[cfg(target_os = "windows")]
#[derive(Serialize, Clone)]
pub struct WindowsImeStatus {
    pub supported: bool,
    pub packaged: bool,
    pub registered: bool,
    pub registered_dll: bool,
    pub profile_enabled: bool,
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

// ─── Install schemas to default keytao data dir ───────────────────────────────

/// Download the given zip URL and smart-install it to `~/Library/keytao`
/// (or the platform-equivalent). Returns the same InstallResult as smart_install.
#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn rime_install_to_default(app: AppHandle, url: String) -> Result<InstallResult, String> {
    let dest =
        keytao_core::default_user_data_dir().ok_or("Cannot determine keytao data directory")?;
    let dest_str = dest.to_string_lossy().into_owned();
    std::fs::create_dir_all(&dest).map_err(|e| format!("创建目录失败: {e}"))?;
    let temp = download_to_temp(app.clone(), url).await?;
    smart_install(app, temp, dest_str).await
}

#[tauri::command]
#[cfg(target_os = "android")]
async fn rime_install_to_default<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    url: String,
) -> Result<InstallResult, String> {
    let permission_value: serde_json::Value = app
        .state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin("storagePermissionStatus", ())
        .map_err(|e| e.to_string())?;
    let permission: AndroidStoragePermissionStatus =
        serde_json::from_value(permission_value).map_err(|e| e.to_string())?;
    if !permission.granted {
        return Err(permission.message);
    }

    let root = android_keytao_root(&app)?;
    std::fs::create_dir_all(&root).map_err(|e| format!("创建 Android 输入法目录失败: {e}"))?;
    let temp = download_to_temp(app.clone(), url).await?;
    let result: serde_json::Value = app
        .state::<ScopedStorageHandle<R>>()
        .0
        .run_mobile_plugin(
            "smartExtractZipToPrivate",
            serde_json::json!({ "zipPath": temp }),
        )
        .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "install-progress",
        InstallProgress {
            stage: "done".into(),
            percent: 100,
            message: "安装完成！".into(),
        },
    );
    Ok(install_result_from_value(&result))
}

#[tauri::command]
#[cfg(target_os = "ios")]
async fn rime_install_to_default(app: AppHandle, url: String) -> Result<InstallResult, String> {
    let dest = ios_keytao_root(&app)?;
    std::fs::create_dir_all(&dest).map_err(|e| format!("创建 iOS 输入法目录失败: {e}"))?;
    let dest_str = dest.to_string_lossy().into_owned();
    let temp = download_to_temp(app.clone(), url).await?;
    smart_install(app, temp, dest_str).await
}

#[tauri::command]
#[cfg(target_os = "linux")]
fn linux_ime_status(app: AppHandle) -> LinuxImeStatus {
    linux_ime_status_with_message(&app, "已刷新 keytao-ime 状态".into())
}

#[tauri::command]
#[cfg(target_os = "linux")]
fn linux_start_ime(app: AppHandle) -> Result<LinuxImeStatus, String> {
    launch_keytao_ime(&app, false)
}

#[tauri::command]
#[cfg(target_os = "linux")]
fn linux_restart_ime(app: AppHandle) -> Result<LinuxImeStatus, String> {
    launch_keytao_ime(&app, true)
}

#[tauri::command]
#[cfg(target_os = "linux")]
fn linux_enable_kde_support(app: AppHandle) -> Result<LinuxImeStatus, String> {
    let mut messages = configure_kde_virtual_keyboard(&app)?;
    let mut status = launch_keytao_ime(&app, false)?;
    messages.push(status.message);
    status.message = messages.join("；");
    Ok(status)
}

#[derive(Serialize)]
pub struct DebugLogFile {
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Serialize)]
pub struct DebugLogs {
    pub ime: DebugLogFile,
    pub app: DebugLogFile,
    pub macos_ime: Option<DebugLogFile>,
}

#[tauri::command]
async fn read_debug_logs() -> Result<DebugLogs, String> {
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(DEBUG_LOG_RETENTION_DAYS);
    let ime = read_tmp_logs("keytao-ime.log", "No keytao-ime.log found", cutoff);
    let app = read_tmp_logs("keytao-app.log", "No keytao-app.log found", cutoff);
    #[cfg(target_os = "macos")]
    let macos_ime = Some(read_macos_ime_logs(cutoff));
    #[cfg(not(target_os = "macos"))]
    let macos_ime = None;
    Ok(DebugLogs {
        ime,
        app,
        macos_ime,
    })
}

fn read_tmp_logs(prefix: &str, missing_message: &str, cutoff: OffsetDateTime) -> DebugLogFile {
    let paths = collect_tmp_log_paths(prefix);
    read_log_paths(paths, missing_message, cutoff, Some(prefix))
}

#[cfg(target_os = "macos")]
fn read_macos_ime_logs(cutoff: OffsetDateTime) -> DebugLogFile {
    let Some(log_dir) = keytao_core::default_user_data_dir().map(|dir| dir.join("log")) else {
        return DebugLogFile {
            lines: vec!["Cannot determine ~/Library/keytao/log".into()],
            truncated: false,
        };
    };
    let paths = collect_dir_log_paths(&log_dir);
    read_log_paths(
        paths,
        "No macOS librime logs found in ~/Library/keytao/log",
        cutoff,
        None,
    )
}

fn read_log_paths(
    paths: Vec<PathBuf>,
    missing_message: &str,
    cutoff: OffsetDateTime,
    prune_plain_prefix: Option<&str>,
) -> DebugLogFile {
    if paths.is_empty() {
        return DebugLogFile {
            lines: vec![missing_message.to_string()],
            truncated: false,
        };
    }

    let mut lines = VecDeque::with_capacity(DEBUG_LOG_MAX_LINES);
    let mut kept = 0usize;

    for path in paths {
        if prune_plain_prefix
            .is_some_and(|prefix| path.file_name().is_some_and(|name| name == prefix))
        {
            let _ = prune_plain_log_file(&path, cutoff);
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut keep_following = true;
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let display_line = strip_ansi_codes(&line);
            if let Some(timestamp) = parse_log_timestamp(&display_line) {
                keep_following = timestamp >= cutoff;
            }
            if keep_following {
                if lines.len() == DEBUG_LOG_MAX_LINES {
                    lines.pop_front();
                }
                lines.push_back(display_line);
                kept += 1;
            }
        }
    }

    DebugLogFile {
        lines: lines.into_iter().collect(),
        truncated: kept > DEBUG_LOG_MAX_LINES,
    }
}

#[cfg(target_os = "macos")]
fn collect_dir_log_paths(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    paths
}

fn collect_tmp_log_paths(prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/tmp") else {
        return Vec::new();
    };
    let rotated_prefix = format!("{prefix}.");
    let mut seen = HashSet::new();
    let mut paths = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == prefix || (name.starts_with(&rotated_prefix) && !name.ends_with(".tmp")) {
                let path = entry.path();
                let identity = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if seen.insert(identity) {
                    Some((name, path))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    paths.sort_by(|(left, _), (right, _)| left.cmp(right));
    paths.into_iter().map(|(_, path)| path).collect()
}

fn prune_plain_log_file(path: &Path, cutoff: OffsetDateTime) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(());
    }

    let input = std::fs::File::open(path)?;
    let temp_path = path.with_extension("tmp");
    let mut writer = BufWriter::new(std::fs::File::create(&temp_path)?);
    let mut saw_timestamp = false;
    let mut keep_following = true;

    for line in BufReader::new(input).lines().map_while(Result::ok) {
        if let Some(timestamp) = parse_log_timestamp(&strip_ansi_codes(&line)) {
            saw_timestamp = true;
            keep_following = timestamp >= cutoff;
        }
        if keep_following {
            writeln!(writer, "{line}")?;
        }
    }
    writer.flush()?;

    if saw_timestamp {
        std::fs::rename(temp_path, path)?;
    } else {
        let _ = std::fs::remove_file(temp_path);
    }

    Ok(())
}

fn parse_log_timestamp(line: &str) -> Option<OffsetDateTime> {
    let bytes = line.as_bytes();
    for index in 0..bytes.len().saturating_sub(20) {
        if bytes[index].is_ascii_digit()
            && bytes.get(index + 4) == Some(&b'-')
            && bytes.get(index + 7) == Some(&b'-')
            && bytes.get(index + 10) == Some(&b'T')
        {
            let end = line[index..]
                .find(char::is_whitespace)
                .map(|offset| index + offset)
                .unwrap_or(line.len());
            if let Ok(timestamp) = OffsetDateTime::parse(&line[index..end], &Rfc3339) {
                return Some(timestamp);
            }
        }
    }
    None
}

fn strip_ansi_codes(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }
    output
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(scoped_storage_plugin());

    #[cfg(target_os = "linux")]
    let builder = builder
        .plugin(tauri_plugin_global_shortcut::Builder::default().build())
        .manage(rime::RimeEngine::default());

    #[cfg(target_os = "linux")]
    let builder = builder.manage(ManagedImeHelper::default());

    #[cfg(target_os = "linux")]
    let builder = builder
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            use tauri_plugin_global_shortcut::{
                Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
            };
            let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            let handle = app.handle().clone();
            if let Err(e) =
                app.handle()
                    .global_shortcut()
                    .on_shortcut(shortcut, move |_app, _sc, event| {
                        if event.state() == ShortcutState::Pressed {
                            let pid = rime::get_frontmost_pid();
                            rime::set_injection_target(pid);
                            if let Some(w) = handle.get_webview_window("ime-overlay") {
                                if w.is_visible().unwrap_or(false) {
                                    let _ = w.hide();
                                } else {
                                    let _ = w.center();
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                            }
                        }
                    })
            {
                eprintln!(
                    "Ctrl+Shift+Space global shortcut is already registered or unavailable; \
                     continuing without the overlay hotkey: {e}"
                );
            }
            // Linux tray is handled by keytao-ime daemon now.

            // Ensure the single Linux IME daemon owns Wayland, XIM, and IBus frontends.
            match launch_keytao_ime(app.handle(), false) {
                Ok(status) => tracing::info!("{}", status.message),
                Err(e) => tracing::warn!("{e}"),
            }

            Ok(())
        });

    builder
        .invoke_handler(tauri::generate_handler![
            check_app_update,
            fetch_latest_release,
            get_component_versions,
            select_directory,
            download_to_temp,
            list_dir,
            read_local_schemas,
            smart_install,
            macos_ime_status,
            macos_install_ime,
            macos_uninstall_ime,
            rime_install_to_default,
            android_ime_status,
            android_storage_permission_status,
            android_open_storage_permission_settings,
            android_keytao_data_dir,
            get_android_ime_input_settings,
            set_android_ime_input_settings,
            android_open_input_method_settings,
            android_show_input_method_picker,
            android_open_app,
            android_pick_directory,
            android_list_files,
            android_read_local_schemas,
            android_smart_extract,
            check_local_schema,
            rime_deploy_default,
            get_ime_ui_settings,
            set_ime_ui_color_scheme,
            set_ime_ui_settings,
            read_debug_logs,
            #[cfg(not(any(target_os = "android", target_os = "linux")))]
            rime_get_data_dir,
            #[cfg(target_os = "linux")]
            linux_ime_status,
            #[cfg(target_os = "linux")]
            linux_start_ime,
            #[cfg(target_os = "linux")]
            linux_restart_ime,
            #[cfg(target_os = "linux")]
            linux_enable_kde_support,
            #[cfg(target_os = "windows")]
            windows_ime_status,
            #[cfg(target_os = "windows")]
            windows_register_ime,
            #[cfg(target_os = "windows")]
            windows_unregister_ime,
            #[cfg(target_os = "windows")]
            windows_restart_ime,
            // ── IME engine commands (Linux only for now) ──
            #[cfg(target_os = "linux")]
            rime::rime_setup,
            #[cfg(target_os = "linux")]
            rime::rime_process_key,
            #[cfg(target_os = "linux")]
            rime::rime_select_candidate,
            #[cfg(target_os = "linux")]
            rime::rime_change_page,
            #[cfg(target_os = "linux")]
            rime::rime_reset,
            #[cfg(target_os = "linux")]
            rime::rime_is_ready,
            #[cfg(target_os = "linux")]
            rime::rime_memory_usage,
            #[cfg(target_os = "linux")]
            rime::rime_inject_text,
            #[cfg(target_os = "linux")]
            rime::rime_get_data_dir,
            #[cfg(target_os = "linux")]
            rime::rime_has_schemas,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            #[cfg(target_os = "linux")]
            if matches!(event, tauri::RunEvent::Exit) {
                stop_managed_ime_helper(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // ── parse_rime_lua_requires ───────────────────────────────────────────────

    #[test]
    fn test_rime_filename_version() {
        assert_eq!(
            rime_filename_version("librime.1.17.0.dylib"),
            Some("1.17.0".to_owned())
        );
        assert_eq!(
            rime_filename_version("librime.so.1.17.0"),
            Some("1.17.0".to_owned())
        );
        assert_eq!(rime_filename_version("librime-lua.dylib"), None);
    }

    #[test]
    fn test_rime_lib_dir_version_prefers_pkg_config() {
        let dir =
            std::env::temp_dir().join(format!("keytao-rime-version-test-{}", std::process::id()));
        let pkgconfig_dir = dir.join("pkgconfig");
        std::fs::create_dir_all(&pkgconfig_dir).expect("create pkgconfig dir");
        std::fs::write(
            pkgconfig_dir.join("rime.pc"),
            "Name: Rime\nVersion: 9.8.7\n",
        )
        .expect("write rime.pc");
        std::fs::write(dir.join("librime.1.dylib"), "").expect("write dylib marker");

        assert_eq!(rime_lib_dir_version(&dir), Some("9.8.7".to_owned()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_rime_lib_dir_version_reads_release_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-rime-release-version-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create version dir");
        std::fs::write(
            dir.join("librime-release.txt"),
            "platform=android\nversion=1.17.0\n",
        )
        .expect("write release metadata");
        std::fs::write(dir.join("librime.1.dylib"), "").expect("write dylib marker");

        assert_eq!(rime_lib_dir_version(&dir), Some("1.17.0".to_owned()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_opencc_dir_version_reads_metadata_and_pkg_config() {
        let dir =
            std::env::temp_dir().join(format!("keytao-opencc-version-test-{}", std::process::id()));
        let pkgconfig_dir = dir.join("lib/pkgconfig");
        std::fs::create_dir_all(&pkgconfig_dir).expect("create pkgconfig dir");
        std::fs::write(dir.join("opencc-release.txt"), "version=1.1.9\n")
            .expect("write opencc metadata");
        std::fs::write(
            pkgconfig_dir.join("opencc.pc"),
            "Name: opencc\nVersion: 7.6.5\n",
        )
        .expect("write opencc.pc");

        assert_eq!(opencc_dir_version(&dir), Some("1.1.9".to_owned()));
        std::fs::remove_file(dir.join("opencc-release.txt")).ok();
        assert_eq!(opencc_dir_version(&dir), Some("7.6.5".to_owned()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_parse_requires_basic() {
        let content = "keytao_filter = require(\"keytao_filter\")\nfoo = require('bar')\n";
        let r = keytao_core::parse_rime_lua_requires(content);
        assert_eq!(r, vec!["keytao_filter", "bar"]);
    }

    #[test]
    fn test_parse_requires_skips_single_line_comments() {
        let content = "-- foo = require(\"foo\")\nreal = require(\"real\")\n";
        let r = keytao_core::parse_rime_lua_requires(content);
        assert_eq!(r, vec!["real"]);
    }

    #[test]
    fn test_parse_requires_skips_block_comment_content() {
        let content = "--[[\n  foo = require(\"bar\")\n--]]\nreal = require(\"real\")\n";
        let r = keytao_core::parse_rime_lua_requires(content);
        assert_eq!(r, vec!["real"]);
    }

    // ── merge_rime_lua ────────────────────────────────────────────────────────

    #[test]
    fn test_merge_appends_unique_local_require() {
        let local = "my_mod = require(\"my_mod\")\n";
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(merged.contains("require(\"keytao_filter\")"));
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_skips_require_already_in_zip() {
        let local = "keytao_filter = require(\"keytao_filter\")\n";
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, _) = merge_rime_lua(local, zip, &HashSet::new());
        assert_eq!(merged.matches("require(\"keytao_filter\")").count(), 1);
    }

    #[test]
    fn test_merge_renames_conflicting_module() {
        let local = "my_mod = require(\"my_mod\")\n";
        let zip = "keytao = require(\"keytao\")\n";
        let filenames: HashSet<String> = ["my_mod.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua(local, zip, &filenames);
        assert_eq!(
            renames,
            vec![("my_mod".to_string(), "my_mod_user".to_string())]
        );
        assert!(merged.contains("require(\"my_mod_user\")"));
        assert!(!merged.contains("require(\"my_mod\")"));
    }

    #[test]
    fn test_merge_ignores_block_comment_content() {
        // Reproduces the Android bug: block comment lines such as ``` were
        // appended verbatim to the merged output because the loop did not
        // track --[[ ... --]] state.
        let local = concat!(
            "--[[\n",
            "librime-lua 样例\n",
            "```\n",
            "  engine:\n",
            "    translators:\n",
            "```\n",
            "--]]\n",
            "--[[\n",
            "各例可使用 `require` 引入。\n",
            "```\n",
            "  foo = require(\"bar\")\n",
            "```\n",
            "--]]\n",
            "my_mod = require(\"my_mod\")\n",
        );
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(!merged.contains("librime-lua"), "block comment line leaked");
        assert!(!merged.contains("engine:"), "block comment line leaked");
        assert!(!merged.contains("```"), "block comment backticks leaked");
        assert!(
            !merged.contains("require(\"bar\")"),
            "in-comment require leaked"
        );
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    // ── parse_schema_list ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_schema_list_basic() {
        let content = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        assert_eq!(parse_schema_list(content), vec!["keytao_b", "keytao_bg"]);
    }

    #[test]
    fn test_parse_schema_list_stops_at_non_schema() {
        let content = "patch:\n  schema_list:\n    - schema: foo\n  other_key: val\n";
        assert_eq!(parse_schema_list(content), vec!["foo"]);
    }

    // ── merge_default_custom ──────────────────────────────────────────────────

    #[test]
    fn test_merge_dc_preserves_user_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: another\n";
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert!(merged.contains("- schema: my_schema"));
        assert!(merged.contains("- schema: another"));
        assert!(merged.contains("- schema: keytao_b"));
        assert_eq!(user, vec!["my_schema", "another"]);
    }

    #[test]
    fn test_merge_dc_excludes_user_keytao_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: keytao_b\n";
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n";
        let (merged, user) = merge_default_custom(Some(existing), zip);
        assert_eq!(user, vec!["my_schema"]);
        assert!(merged.contains("- schema: keytao_b"));
        assert!(merged.contains("- schema: keytao_bg"));
    }

    #[test]
    fn test_merge_dc_no_existing_file() {
        let zip = "patch:\n  schema_list:\n    - schema: keytao_b\n";
        let (merged, user) = merge_default_custom(None, zip);
        assert!(user.is_empty());
        assert!(merged.contains("- schema: keytao_b"));
    }

    // ── real keytao rime.lua ──────────────────────────────────────────────────

    const KEYTAO_RIME_LUA: &str = concat!(
        "--[[\n",
        "librime-lua 样例\n",
        "```\n",
        "  engine:\n",
        "    translators:\n",
        "      - lua_translator@lua_function3\n",
        "      - lua_translator@lua_function4\n",
        "    filters:\n",
        "      - lua_filter@lua_function1\n",
        "      - lua_filter@lua_function2\n",
        "```\n",
        "其中各 `lua_function` 为在本文件所定义变量名。\n",
        "--]]\n",
        "\n",
        "--[[\n",
        "本文件的后面是若干个例子，按照由简单到复杂的顺序示例了 librime-lua 的用法。\n",
        "每个例子都被组织在 `lua` 目录下的单独文件中，打开对应文件可看到实现和注解。\n",
        "\n",
        "各例可使用 `require` 引入。\n",
        "```\n",
        "  foo = require(\"bar\")\n",
        "```\n",
        "可认为是载入 `lua/bar.lua` 中的例子，并起名为 `foo`。\n",
        "配方文件中的引用方法为：`...@foo`。\n",
        "--]]\n",
        "\n",
        "date_time_translator = require(\"date_time\")\n",
        "\n",
        "\n",
        "-- single_char_filter: 候选项重排序，使单字优先\n",
        "-- 详见 `lua/single_char.lua`\n",
        "-- single_char_filter = require(\"single_char\")\n",
        "\n",
        "\n",
        "-- keytao_filter: 单字模式 & 630 即 ss 词组提示\n",
        "-- 详见 `lua/keytao_filter.lua`\n",
        "keytao_filter = require(\"keytao_filter\")\n",
        "\n",
        "-- 顶功处理器\n",
        "topup_processor = require(\"for_topup\")\n",
        "\n",
        "-- 声笔笔简码提示 | 顶功提示 | 补全处理\n",
        "hint_filter = require(\"for_hint\")\n",
        "\n",
        "-- number_translator: 将 `=` + 阿拉伯数字 翻译为大小写汉字\n",
        "number_translator = require(\"xnumber\")\n",
        "\n",
        "-- 用 ' 作为次选键\n",
        "smart_2 = require(\"smart_2\")\n",
    );

    #[test]
    fn test_parse_requires_keytao_rime_lua() {
        // Block comment contains `foo = require("bar")` which must NOT be included.
        let requires = keytao_core::parse_rime_lua_requires(KEYTAO_RIME_LUA);
        assert_eq!(
            requires,
            vec![
                "date_time",
                "keytao_filter",
                "for_topup",
                "for_hint",
                "xnumber",
                "smart_2"
            ]
        );
        assert!(
            !requires.contains(&"bar".to_string()),
            "in-comment require must not be parsed"
        );
    }

    #[test]
    fn test_merge_reinstall_no_duplicates() {
        // Installing over an existing identical rime.lua should produce the same file.
        let (merged, renames) = merge_rime_lua(KEYTAO_RIME_LUA, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(renames.is_empty());
        // Every require should appear exactly once.
        for module in &[
            "date_time",
            "keytao_filter",
            "for_topup",
            "for_hint",
            "xnumber",
            "smart_2",
        ] {
            let needle = format!("require(\"{module}\")");
            assert_eq!(
                merged.matches(needle.as_str()).count(),
                1,
                "require(\"{module}\") duplicated after reinstall"
            );
        }
    }

    #[test]
    fn test_merge_user_extra_module_appended() {
        // User has the keytao rime.lua as local, plus one extra module.
        let local = format!("{KEYTAO_RIME_LUA}my_custom = require(\"my_custom\")\n");
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(renames.is_empty());
        assert!(merged.contains("require(\"my_custom\")"));
        // Keytao requires still appear exactly once.
        assert_eq!(merged.matches("require(\"keytao_filter\")").count(), 1);
    }

    #[test]
    fn test_merge_user_extra_module_conflict_renamed() {
        // User has a custom `date_time.lua` that would be overwritten by zip.
        let local = format!("{KEYTAO_RIME_LUA}my_dt = require(\"my_dt\")\n");
        let filenames: HashSet<String> = ["my_dt.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &filenames);
        assert_eq!(
            renames,
            vec![("my_dt".to_string(), "my_dt_user".to_string())]
        );
        assert!(merged.contains("require(\"my_dt_user\")"));
        assert!(!merged.contains("require(\"my_dt\")"));
    }

    // ── zip overwrites local keytao content ──────────────────────────────────

    #[test]
    fn test_merge_zip_is_base_local_keytao_no_duplicates() {
        // Local already has the same keytao rime.lua; merged must equal zip exactly.
        let (merged, renames) = merge_rime_lua(KEYTAO_RIME_LUA, KEYTAO_RIME_LUA, &HashSet::new());
        assert_eq!(merged, KEYTAO_RIME_LUA);
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_old_keytao_missing_module_zip_provides_it() {
        // Local = older keytao rime.lua without smart_2.
        // Zip = new keytao rime.lua with smart_2.
        // smart_2 must appear exactly once in merged output.
        let old_local: String = KEYTAO_RIME_LUA
            .lines()
            .filter(|l| !l.trim_start().starts_with("smart_2"))
            .collect::<Vec<_>>()
            .join("\n");
        let (merged, renames) = merge_rime_lua(&old_local, KEYTAO_RIME_LUA, &HashSet::new());
        assert_eq!(merged.matches("require(\"smart_2\")").count(), 1);
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_user_extra_preserved_zip_overwrites_keytao_no_dups() {
        // Local = keytao rime.lua + user-defined module.
        // Zip = same keytao rime.lua (re-install / upgrade).
        // merged must start with zip content; user module appended once;
        // every keytao module appears exactly once.
        let local = format!("{KEYTAO_RIME_LUA}user_plugin = require(\"user_plugin\")\n");
        let (merged, renames) = merge_rime_lua(&local, KEYTAO_RIME_LUA, &HashSet::new());
        assert!(merged.starts_with(KEYTAO_RIME_LUA));
        assert!(merged.contains("require(\"user_plugin\")"));
        for module in &[
            "date_time",
            "keytao_filter",
            "for_topup",
            "for_hint",
            "xnumber",
            "smart_2",
        ] {
            assert_eq!(
                merged.matches(&format!("require(\"{module}\")")).count(),
                1,
                "require(\"{module}\") must appear exactly once"
            );
        }
        assert!(renames.is_empty());
    }

    #[test]
    fn test_merge_keytao_rime_lua_no_block_comment_leak() {
        // Using actual keytao rime.lua as local; merged result must not contain
        // any content from the --[[ ]] header blocks.
        let local = KEYTAO_RIME_LUA;
        let zip = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, _) = merge_rime_lua(local, zip, &HashSet::new());
        assert!(
            !merged.contains("librime-lua"),
            "block comment header leaked"
        );
        assert!(!merged.contains("engine:"), "block comment content leaked");
        assert!(
            !merged.contains("```"),
            "backticks from block comment leaked"
        );
        assert!(
            !merged.contains("require(\"bar\")"),
            "in-comment require leaked"
        );
    }
}
