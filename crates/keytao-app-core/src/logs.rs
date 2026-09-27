use crate::{
    Core, CoreError, ime_host::ImeHost, ime_paths::path_string, scheme_files::write_file_atomic,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const DEBUG_LOG_RETENTION_DAYS: i64 = 3;
const DEBUG_LOG_MAX_LINES: usize = 20_000;

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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeLogLevel {
    Info,
    Verbose,
}

#[derive(Serialize, Deserialize)]
struct RuntimeLogConfig {
    enabled: bool,
    level: RuntimeLogLevel,
}

#[derive(Serialize)]
struct RuntimeLogFileInfo {
    name: String,
    size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogSettings {
    #[serde(flatten)]
    config: RuntimeLogConfig,
    log_dir: String,
    files: Vec<RuntimeLogFileInfo>,
    file_count: usize,
    total_bytes: u64,
}

fn runtime_log_directory(root: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = root;
        ime_state_log_dir().ok_or("Cannot determine runtime log directory".into())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(root.join("log"))
    }
}

fn is_runtime_log_name(name: &str) -> bool {
    let Some((tag, suffix)) = name
        .strip_prefix("keytao-")
        .and_then(|n| n.split_once(".log"))
    else {
        return false;
    };
    // The existing Linux daemon log is not a structured runtime log.
    !tag.is_empty()
        && tag != "ime"
        && tag.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        && (suffix.is_empty()
            || suffix.strip_prefix('.').is_some_and(|rotation| {
                !rotation.is_empty() && rotation.bytes().all(|c| c.is_ascii_digit())
            }))
}

fn runtime_log_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    // Surface permission failures instead of reporting a misleading empty list.
    match std::fs::read_dir(dir) {
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Cannot read runtime log directory: {e}")),
    }
    let mut paths = collect_dir_log_paths(dir)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_runtime_log_name)
                && std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
        })
        .collect::<Vec<_>>();
    // Read older rotations first; a cross-process timestamp merge is deferred.
    paths.sort_by_cached_key(|path| {
        (
            std::fs::metadata(path).and_then(|m| m.modified()).ok(),
            path.clone(),
        )
    });
    Ok(paths)
}

pub async fn get_runtime_log_settings(
    _core: &Core,
    host: &impl ImeHost,
) -> Result<RuntimeLogSettings, CoreError> {
    let result: Result<_, String> = async {
        let root = host.user_root()?;
        let config = match std::fs::read(root.join("runtime-log.json")) {
            Ok(bytes) => serde_json::from_slice::<RuntimeLogConfig>(&bytes)
                .map_err(|e| format!("Cannot parse runtime log settings: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RuntimeLogConfig {
                enabled: true,
                level: RuntimeLogLevel::Info,
            },
            Err(e) => return Err(format!("Cannot read runtime log settings: {e}")),
        };
        let dir = runtime_log_directory(&root)?;
        let mut files = Vec::new();
        for path in runtime_log_paths(&dir)? {
            match path.metadata() {
                Ok(metadata) => files.push(RuntimeLogFileInfo {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    size: metadata.len(),
                }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(format!("Cannot inspect runtime log: {e}")),
            }
        }
        Ok(RuntimeLogSettings {
            config,
            log_dir: path_string(dir),
            file_count: files.len(),
            total_bytes: files.iter().map(|file| file.size).sum(),
            files,
        })
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn set_runtime_log_settings(
    _core: &Core,
    host: &impl ImeHost,
    enabled: bool,
    level: RuntimeLogLevel,
) -> Result<(), CoreError> {
    let result: Result<_, String> = async {
        let root = host.user_root()?;
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let bytes =
            serde_json::to_vec(&RuntimeLogConfig { enabled, level }).map_err(|e| e.to_string())?;
        write_file_atomic(&root.join("runtime-log.json"), &bytes).map_err(|e| e.to_string())
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn read_runtime_log(
    _core: &Core,
    host: &impl ImeHost,
    max_lines: Option<usize>,
) -> Result<DebugLogFile, CoreError> {
    let result: Result<_, String> = async {
        let dir = runtime_log_directory(&host.user_root()?)?;
        let paths = runtime_log_paths(&dir)?;
        if paths.is_empty() {
            return Ok(DebugLogFile {
                lines: Vec::new(),
                truncated: false,
            });
        }
        // Runtime logs are size-bounded. Do not prune, rewrite, or time-filter them.
        let mut logs = read_log_paths(paths, "", OffsetDateTime::UNIX_EPOCH, None);
        let limit = max_lines
            .unwrap_or(DEBUG_LOG_MAX_LINES)
            .min(DEBUG_LOG_MAX_LINES);
        if logs.lines.len() > limit {
            logs.lines.drain(..logs.lines.len() - limit);
            logs.truncated = true;
        }
        Ok(logs)
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn clear_runtime_log(_core: &Core, host: &impl ImeHost) -> Result<(), CoreError> {
    let result: Result<_, String> = async {
        let dir = runtime_log_directory(&host.user_root()?)?;
        for path in runtime_log_paths(&dir)? {
            match std::fs::remove_file(path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(format!("Cannot clear runtime log: {e}")),
            }
        }
        Ok(())
    }
    .await;
    result.map_err(CoreError::Other)
}

pub enum RuntimeLogShare {
    #[cfg(target_os = "android")]
    Android,
    #[cfg(target_os = "ios")]
    Ios(Vec<PathBuf>),
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    Directory(String),
}

pub async fn share_runtime_log<F, Fut>(
    _core: &Core,
    host: &impl ImeHost,
    share: F,
) -> Result<serde_json::Value, CoreError>
where
    F: FnOnce(RuntimeLogShare) -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, String>>,
{
    let result: Result<_, String> = async {
        #[cfg(target_os = "android")]
        let request = {
            let _ = host;
            RuntimeLogShare::Android
        };
        #[cfg(target_os = "ios")]
        let request = {
            let dir = runtime_log_directory(&host.user_root()?)?;
            let paths = runtime_log_paths(&dir)?;
            if paths.is_empty() {
                return Err("No runtime logs to share".into());
            }
            RuntimeLogShare::Ios(paths)
        };
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let request =
            RuntimeLogShare::Directory(path_string(runtime_log_directory(&host.user_root()?)?));
        share(request).await
    }
    .await;
    result.map_err(CoreError::Other)
}

pub async fn read_debug_logs(_core: &Core) -> Result<DebugLogs, CoreError> {
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(DEBUG_LOG_RETENTION_DAYS);
    let ime = read_ime_logs(cutoff);
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

/// Where the keytao-ime daemon keeps its log, mirroring the daemon's own
/// choice: the log is derived from what the user types, so on Linux it lives in
/// the per-user state directory instead of a world-readable `/tmp`.  The other
/// platforms simply have no such directory.
fn ime_state_log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))?;
    Some(base.join("keytao").join("log"))
}

/// Read the keytao-ime daemon log; `/tmp` is only consulted for logs older
/// releases left behind.
fn read_ime_logs(cutoff: OffsetDateTime) -> DebugLogFile {
    const PREFIX: &str = "keytao-ime.log";
    let mut paths = ime_state_log_dir()
        .map(|dir| collect_dir_log_paths(&dir))
        .unwrap_or_default();
    paths.extend(collect_tmp_log_paths(PREFIX));
    read_log_paths(paths, "No keytao-ime.log found", cutoff, Some(PREFIX))
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
