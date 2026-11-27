use crate::{Core, CoreError, ime_host::ImeHost, scheme_files::write_file_atomic};
use keytao_core::{RuntimeDirs, runtime_log::AppOperation};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) const INSTALLATION_LOCK: &str = ".keytao-rime-installation.lock";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RimeSyncSettings {
    pub installation_id: String,
    pub sync_dir: String,
    pub effective_sync_dir: String,
    pub default_sync_dir: String,
    pub package_sync_dir: String,
    pub last_sync_at: u64,
    pub last_sync_error: Option<String>,
}

pub fn ensure_installation(core: &Core, root: &Path) -> Result<(), CoreError> {
    let has_id = |mapping: &Mapping| match mapping.get(Value::String("installation_id".into())) {
        None | Some(Value::Null) => false,
        Some(Value::String(id)) => !id.trim().is_empty(),
        Some(_) => true,
    };
    if has_id(&read_document(root).map_err(CoreError::Other)?.2) {
        return Ok(());
    }
    fs::create_dir_all(root)?;
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(CoreError::Other(
            "Rime user root must be a directory".into(),
        ));
    }

    #[cfg(target_os = "android")]
    let lock_path = {
        // Shared storage may reject both locking and no-replace rename.
        let data_dir = &core.env().data_dir;
        fs::create_dir_all(data_dir)?;
        data_dir.join(".rime-installation.lock")
    };
    #[cfg(not(target_os = "android"))]
    let lock_path = root.join(INSTALLATION_LOCK);
    let _lock = crate::package_fs::file_lease(&lock_path, true)?;
    let (path, text, mut document) = read_document(root).map_err(CoreError::Other)?;
    if has_id(&document) {
        return Ok(());
    }

    let installation_id = new_installation_id().map_err(CoreError::Other)?;
    if let Some(text) = text {
        document.insert("installation_id".into(), installation_id.into());
        let yaml =
            update_document(&text, &document, &["installation_id"]).map_err(CoreError::Other)?;
        write_file_atomic(&path, yaml.as_bytes())?;
        return Ok(());
    }
    let version = match core.env().app_version.trim() {
        "" => env!("CARGO_PKG_VERSION"),
        version => version,
    };
    for (key, value) in [
        ("distribution_name", "KeyTao".into()),
        ("distribution_code_name", "keytao".into()),
        ("distribution_version", version.into()),
        ("installation_id", installation_id.clone()),
        ("install_time", time::OffsetDateTime::now_utc().to_string()),
    ] {
        document.insert(key.into(), value.into());
    }
    if let Some(version) = keytao_core::librime_runtime_version() {
        document.insert("rime_version".into(), version.into());
    }
    let yaml =
        serde_yaml::to_string(&document).map_err(|error| CoreError::Other(error.to_string()))?;
    #[cfg(target_os = "android")]
    write_file_atomic(&path, yaml.as_bytes())?;
    #[cfg(not(target_os = "android"))]
    {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        file.write_all(yaml.as_bytes())?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
                // A frontend may have created its own installation file.
                let (path, text, mut document) = read_document(root).map_err(CoreError::Other)?;
                if let Some(text) = text
                    && !has_id(&document)
                {
                    document.insert("installation_id".into(), installation_id.into());
                    let yaml = update_document(&text, &document, &["installation_id"])
                        .map_err(CoreError::Other)?;
                    write_file_atomic(&path, yaml.as_bytes())?;
                }
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    Ok(())
}

pub fn new_installation_id() -> Result<String, String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..],
    ))
}

fn read_document(root: &Path) -> Result<(PathBuf, Option<String>, Mapping), String> {
    let path = crate::packages::safe_path(root, "installation.yaml")?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(format!("Read installation.yaml: {error}")),
    };
    let value: Value = serde_yaml::from_str(text.as_deref().unwrap_or_default())
        .map_err(|error| format!("Read installation.yaml: {error}"))?;
    let mapping = match value {
        Value::Null => Mapping::new(),
        Value::Mapping(mapping) => mapping,
        _ => return Err("installation.yaml must contain a mapping".into()),
    };
    Ok((path, text, mapping))
}

fn settings(root: &Path, mapping: &Mapping) -> Result<RimeSyncSettings, String> {
    let string = |key: &str| match mapping.get(Value::String(key.into())) {
        Some(Value::String(value)) => Ok(value.clone()),
        None | Some(Value::Null) => Ok(String::new()),
        _ => Err(format!("installation.yaml {key} must be a string")),
    };
    let installation_id = string("installation_id")?;
    let sync_dir = string("sync_dir")?;
    let effective = if sync_dir.is_empty() {
        root.join("sync")
    } else {
        PathBuf::from(&sync_dir)
    };
    Ok(RimeSyncSettings {
        installation_id,
        sync_dir,
        effective_sync_dir: effective.to_string_lossy().into_owned(),
        default_sync_dir: String::new(),
        package_sync_dir: String::new(),
        last_sync_at: 0,
        last_sync_error: None,
    })
}

pub fn read_settings(root: &Path) -> Result<RimeSyncSettings, String> {
    settings(root, &read_document(root)?.2)
}

pub fn ensure_and_read_ui_settings(core: &Core, root: &Path) -> Result<RimeSyncSettings, String> {
    let ensure_error = match ensure_installation(core, root) {
        Ok(()) => None,
        Err(error) => {
            tracing::warn!(%error, "Could not ensure Rime installation metadata");
            Some(error.to_string())
        }
    };
    let mut settings = read_ui_settings(root)?;
    if settings.installation_id.trim().is_empty()
        && settings.last_sync_error.is_none()
        && let Some(error) = ensure_error
    {
        settings.last_sync_error = Some(error);
    }
    Ok(settings)
}

pub fn read_ui_settings(root: &Path) -> Result<RimeSyncSettings, String> {
    let mut settings = read_settings(root)?;
    let (runtime, sync_dir) = match package_sync_dir(root, &settings, rename_sync_dir) {
        Ok(resolved) => resolved,
        Err(error) => {
            settings.last_sync_error = Some(error);
            return Ok(settings);
        }
    };
    let default_dir = if settings.sync_dir.is_empty() && !sync_dir.as_os_str().is_empty() {
        sync_dir.clone()
    } else if runtime.active_package_key.is_some() {
        runtime.user_data_dir.join("sync")
    } else {
        root.join("sync")
    };
    settings.default_sync_dir = default_dir.to_string_lossy().into_owned();
    settings.package_sync_dir = sync_dir.to_string_lossy().into_owned();
    match keytao_core::read_user_data_sync_result(&runtime.user_data_dir) {
        Ok(Some(result)) if result.finished_at != 0 => {
            settings.last_sync_at = result.finished_at;
            if !result.ok {
                settings.last_sync_error = Some(
                    result
                        .error
                        .unwrap_or_else(|| "Rime user data sync failed".into()),
                );
            }
        }
        Err(error) => settings.last_sync_error = Some(error),
        _ => {}
    }
    Ok(settings)
}

fn validate_installation_id(value: &str) -> Result<&str, String> {
    let value = value.trim();
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || stem.strip_prefix("COM").is_some_and(is_device_digit)
        || stem.strip_prefix("LPT").is_some_and(is_device_digit);
    if value.is_empty()
        || value.contains("..")
        || value.ends_with('.')
        || value
            .chars()
            .any(|ch| ch.is_control() || "/\\<>:\"|?*".contains(ch))
        || reserved
    {
        return Err(
            "Invalid installation_id: use a non-empty folder name without path separators or '..'"
                .into(),
        );
    }
    Ok(value)
}

fn is_device_digit(value: &str) -> bool {
    matches!(
        value,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    )
}

fn validate_sync_dir(value: &str) -> Result<&str, String> {
    if !value.is_empty() && (!Path::new(value).is_absolute() || value.chars().any(char::is_control))
    {
        return Err("sync_dir must be an absolute path".into());
    }
    Ok(value)
}

pub fn validate_user_sync_dir(path: &Path) -> Result<(), String> {
    use std::io::Write;

    validate_sync_dir(path.to_str().ok_or("Rime sync path is not valid UTF-8")?)?;
    if !path.parent().is_some_and(Path::is_dir) {
        return Err("Rime sync path must have an existing parent directory".into());
    }
    if !path.is_dir()
        && let Err(error) = fs::create_dir(path)
        && error.kind() != ErrorKind::AlreadyExists
    {
        return Err(format!("Create Rime sync directory: {error}"));
    }
    let mut probe = tempfile::NamedTempFile::new_in(path)
        .map_err(|error| format!("Rime sync directory is not writable: {error}"))?;
    probe
        .write_all(b"\n")
        .and_then(|_| probe.close())
        .map_err(|error| format!("Rime sync directory is not writable: {error}"))
}

fn inline_comment(line: &str) -> &str {
    line.match_indices('#')
        .find(|(offset, _)| {
            line[..*offset]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
                && serde_yaml::from_str::<Value>(&line[..*offset]).is_ok()
        })
        .map(|(offset, _)| &line[offset..])
        .unwrap_or_default()
}

// Patch librime's usual block mapping; unusual YAML falls back to a semantic rewrite.
fn update_document(text: &str, mapping: &Mapping, keys: &[&str]) -> Result<String, String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines = Vec::new();
    let mut replaced = [false; 2];
    let mut in_replaced_value = false;
    for line in text.lines() {
        if in_replaced_value
            && line.starts_with(char::is_whitespace)
            && !line.trim_start().starts_with('#')
        {
            continue;
        }
        if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            in_replaced_value = false;
        }
        let key = line.split_once(':').map(|(key, _)| key);
        let Some(index) = keys.iter().position(|candidate| key == Some(*candidate)) else {
            lines.push(line.to_owned());
            continue;
        };
        replaced[index] = true;
        in_replaced_value = true;
        let key = key.unwrap();
        if let Some(value) = mapping.get(Value::String(key.into())) {
            let scalar = serde_json::to_string(value.as_str().unwrap())
                .map_err(|error| error.to_string())?;
            let comment = inline_comment(line);
            let suffix = if comment.is_empty() {
                String::new()
            } else {
                format!(" {comment}")
            };
            lines.push(format!("{key}: {scalar}{suffix}"));
        } else if !inline_comment(line).is_empty() {
            lines.push(inline_comment(line).to_owned());
        }
    }
    for (index, key) in keys.iter().enumerate() {
        if !replaced[index] {
            if let Some(value) = mapping.get(Value::String((*key).into())) {
                let scalar = serde_json::to_string(value.as_str().unwrap())
                    .map_err(|error| error.to_string())?;
                lines.push(format!("{key}: {scalar}"));
            }
        }
    }
    let patched = format!("{}{newline}", lines.join(newline));
    if serde_yaml::from_str::<Value>(&patched).ok() == Some(Value::Mapping(mapping.clone())) {
        Ok(patched)
    } else {
        serde_yaml::to_string(mapping).map_err(|error| error.to_string())
    }
}

pub fn save_settings(
    root: &Path,
    installation_id: &str,
    sync_dir: &str,
) -> Result<RimeSyncSettings, String> {
    let installation_id = validate_installation_id(installation_id)?;
    let sync_dir = validate_sync_dir(sync_dir)?;
    update_installation(root, &["installation_id", "sync_dir"], |mapping| {
        mapping.insert("installation_id".into(), installation_id.into());
        if sync_dir.is_empty() {
            mapping.remove(Value::String("sync_dir".into()));
        } else {
            mapping.insert("sync_dir".into(), sync_dir.into());
        }
        Ok(true)
    })
}

fn update_installation(
    root: &Path,
    keys: &[&str],
    update: impl FnOnce(&mut Mapping) -> Result<bool, String>,
) -> Result<RimeSyncSettings, String> {
    fs::create_dir_all(root).map_err(|error| format!("Create Rime user directory: {error}"))?;
    #[cfg(not(target_os = "android"))]
    let _lock = crate::package_fs::file_lease(&root.join(INSTALLATION_LOCK), true)
        .map_err(|error| format!("Lock installation.yaml: {error}"))?;
    let (path, text, mut mapping) = read_document(root)?;
    if update(&mut mapping)? {
        let text = update_document(text.as_deref().unwrap_or_default(), &mapping, keys)?;
        write_file_atomic(&path, text.as_bytes()).map_err(|error| error.to_string())?;
    }
    settings(root, &mapping)
}

pub async fn sync_user_data(host: impl ImeHost) -> Result<(), String> {
    let _operation = AppOperation::start("sync_rime_user_data");
    sync_user_data_with_timeout(host, Duration::from_secs(120)).await
}

async fn sync_user_data_with_timeout(
    host: impl ImeHost,
    completion_timeout: Duration,
) -> Result<(), String> {
    let root = host.user_root()?;
    let settings = read_settings(&root)?;
    if validate_installation_id(&settings.installation_id)? != settings.installation_id {
        return Err("installation_id must be trimmed".into());
    }
    validate_sync_dir(&settings.sync_dir)?;
    let request_root = prepare_sync_root(&root, &settings, rename_sync_dir)?;
    let generation = keytao_core::request_user_data_sync(&request_root)?;
    if let Err(error) = host.write_reload_stamp(&root) {
        if keytao_core::cancel_user_data_sync(&request_root, generation).unwrap_or(true) {
            return Err(error);
        }
    }
    let started_at = tokio::time::Instant::now();
    let start_deadline = started_at + Duration::from_secs(20);
    let completion_deadline = started_at + completion_timeout;
    let mut process_id = None;
    loop {
        match keytao_core::read_user_data_sync_result(&request_root) {
            Ok(Some(result)) if result.generation == generation => {
                if result.finished_at != 0 {
                    return if result.ok {
                        Ok(())
                    } else {
                        Err(result
                            .error
                            .unwrap_or_else(|| "Rime user data sync failed".into()))
                    };
                }
                process_id = Some(result.process_id);
            }
            Err(error) if process_id.is_none() => {
                if keytao_core::cancel_user_data_sync(&request_root, generation).unwrap_or(true) {
                    return Err(error);
                }
            }
            _ => {}
        }
        let now = tokio::time::Instant::now();
        if let Some(process_id) = process_id {
            if !process_alive(process_id) {
                return Err("IME exited before completing user data sync".into());
            }
        } else if now >= start_deadline
            && keytao_core::cancel_user_data_sync(&request_root, generation)?
        {
            return Err(
                "IME did not start syncing within 20 seconds; make sure it is running".into(),
            );
        }
        if now >= completion_deadline {
            return Err(
                "Timed out waiting for IME to complete Rime user data sync; \
                 check that the sync directory is available"
                    .into(),
            );
        }
        tokio::time::sleep_until((now + Duration::from_millis(100)).min(completion_deadline)).await;
    }
}

fn package_sync_dir(
    root: &Path,
    settings: &RimeSyncSettings,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<(RuntimeDirs, PathBuf), String> {
    let runtime = RuntimeDirs::resolve(root, root)?;
    let sync_dir = match runtime.active_package_key.as_deref() {
        Some(key) if settings.sync_dir.is_empty() => {
            migrate_default_sync_dir(root, key, runtime.user_data_dir.join("sync"), rename)
        }
        Some(key) => Path::new(&settings.sync_dir)
            .join("keytao-packages")
            .join(key),
        None if runtime.user_data_dir == root => PathBuf::from(&settings.effective_sync_dir),
        None => PathBuf::new(),
    };
    Ok((runtime, sync_dir))
}

fn migrate_default_sync_dir(
    root: &Path,
    key: &str,
    default_dir: PathBuf,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> PathBuf {
    let relative = format!("sync/keytao-packages/{key}");
    let legacy_dir = root.join(&relative);
    // Preserve user-managed links even when their target is unavailable.
    let mut component = root.to_path_buf();
    for part in relative.split('/') {
        component.push(part);
        if fs::symlink_metadata(&component).is_ok_and(|metadata| metadata.is_symlink()) {
            return std::path::absolute(&legacy_dir).unwrap_or(legacy_dir);
        }
    }
    let migrate = || -> Result<(), String> {
        let destination_error = match fs::symlink_metadata(&default_dir) {
            Ok(_) => return Ok(()),
            Err(error) => error,
        };
        match fs::metadata(&legacy_dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("Inspect previous Rime sync directory: {error}")),
        }
        if destination_error.kind() != ErrorKind::NotFound {
            return Err(format!("Inspect Rime sync directory: {destination_error}"));
        }
        let old_dir = crate::packages::safe_path(root, &relative)?;
        match rename(&old_dir, &default_dir) {
            Ok(()) => Ok(()),
            Err(error)
                if error.kind() == ErrorKind::NotFound
                    && default_dir.is_dir()
                    && !legacy_dir.is_dir() =>
            {
                Ok(())
            }
            Err(error) => Err(format!("Move Rime sync directory: {error}")),
        }
    };
    match migrate() {
        Ok(()) => default_dir,
        Err(error) => {
            tracing::warn!(%error, path = %legacy_dir.display(), "Keeping the previous Rime sync directory");
            // Keep the alias so a user-managed symlink can continue to redirect sync.
            std::path::absolute(&legacy_dir).unwrap_or(legacy_dir)
        }
    }
}

#[cfg(unix)]
fn rename_sync_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // A concurrent sync or directory open can create the destination after the existence check.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_sync_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW},
        core::PCWSTR,
    };

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let (from, to) = (wide(from), wide(to));
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVE_FILE_FLAGS(0),
        )
    }
    .map_err(|error| std::io::Error::from_raw_os_error(error.code().0 & 0xffff))
}

fn prepare_sync_root(
    root: &Path,
    settings: &RimeSyncSettings,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<PathBuf, String> {
    let (runtime, sync_dir) = package_sync_dir(root, settings, rename)?;
    let Some(key) = runtime.active_package_key.as_deref() else {
        return if runtime.user_data_dir == root {
            Ok(runtime.user_data_dir)
        } else {
            Err("没有使用中的方案包".into())
        };
    };
    if !runtime.user_data_dir.is_dir() {
        return Err(format!("Rime runtime is not available for {key}"));
    }
    let configured_dir = if sync_dir == runtime.user_data_dir.join("sync") {
        ""
    } else {
        sync_dir
            .to_str()
            .ok_or("Rime sync path is not valid UTF-8")?
    };
    save_settings(
        &runtime.user_data_dir,
        &settings.installation_id,
        configured_dir,
    )?;
    Ok(runtime.user_data_dir)
}

#[cfg(unix)]
fn process_alive(process_id: u32) -> bool {
    let Ok(process_id) = libc::pid_t::try_from(process_id) else {
        return false;
    };
    process_id > 0
        && (unsafe { libc::kill(process_id, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

#[cfg(target_os = "windows")]
fn process_alive(process_id: u32) -> bool {
    use windows::Win32::{
        Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    if process_id == 0 {
        return false;
    }
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) {
            Ok(handle) => handle,
            Err(error) => return error.code() == ERROR_ACCESS_DENIED.to_hresult(),
        };
        let mut exit_code = 0;
        let alive = GetExitCodeProcess(handle, &mut exit_code).is_ok()
            && exit_code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_preserve_other_values_and_comments_and_reset_default() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("installation.yaml");
        let original = "# Rime installation\ndistribution_code_name: keytao # Keep distribution\ndistribution_name: KeyTao\ndistribution_version: 1.2.8\nrime_version: 1.16.0\ninstall_time: \"2026-11-18\"\nupdate_time: 123\ninstallation_id: \"old # id\" # Keep identity comment\n# Keep path comment\nsync_dir: /old\ncustom:\n  nested: [one, two]\n";
        std::fs::write(&path, original).unwrap();
        let custom_path = root
            .path()
            .join("chosen path ")
            .to_string_lossy()
            .into_owned();
        let saved = save_settings(root.path(), "  new-device  ", &custom_path).unwrap();
        assert_eq!(saved.installation_id, "new-device");
        assert_eq!(saved.sync_dir, custom_path);
        assert_eq!(saved.effective_sync_dir, custom_path);
        assert_eq!(read_settings(root.path()).unwrap(), saved);
        let updated = std::fs::read_to_string(&path).unwrap();
        for comment in [
            "# Rime installation",
            "# Keep distribution",
            "# Keep identity comment",
            "# Keep path comment",
        ] {
            assert!(updated.contains(comment), "{comment}");
        }
        let mut expected: Mapping = serde_yaml::from_str(original).unwrap();
        let mut actual: Mapping = serde_yaml::from_str(&updated).unwrap();
        for key in ["installation_id", "sync_dir"] {
            expected.remove(Value::String(key.into()));
            actual.remove(Value::String(key.into()));
        }
        assert_eq!(actual, expected);
        let reset = save_settings(root.path(), "new-device", "").unwrap();
        assert!(reset.sync_dir.is_empty());
        assert_eq!(
            reset.effective_sync_dir,
            root.path().join("sync").to_string_lossy()
        );
        let document: Mapping =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!document.contains_key(Value::String("sync_dir".into())));
    }

    #[test]
    fn settings_reject_invalid_ids_and_paths_without_writing() {
        let root = tempfile::tempdir().unwrap();
        for id in [
            "", "  ", ".", "..", "a..b", "a/b", "a\\b", "a:b", "CON", "lpt1.txt", "COM¹",
            "CON .txt", "CONIN$", "end.", "a\nb",
        ] {
            assert!(save_settings(root.path(), id, "").is_err(), "{id:?}");
        }
        for path in ["relative", "../sync", "~/sync", "a\0b"] {
            assert!(
                save_settings(root.path(), "device", path).is_err(),
                "{path:?}"
            );
        }
        #[cfg(target_os = "windows")]
        for path in ["C:relative", "\\relative", "/relative"] {
            assert!(
                save_settings(root.path(), "device", path).is_err(),
                "{path:?}"
            );
        }
        assert!(!root.path().join("installation.yaml").exists());
    }

    #[test]
    fn settings_keep_unusual_mapping_values_and_reject_malformed_yaml() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("installation.yaml");
        std::fs::write(
            &path,
            "{installation_id: old, arbitrary: {nested: [1, true]}}\n",
        )
        .unwrap();
        save_settings(root.path(), "device", "").unwrap();
        let value: Value = serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value["arbitrary"]["nested"],
            serde_yaml::from_str::<Value>("[1, true]").unwrap()
        );
        for invalid in ["[one, two]", "installation_id: [", "installation_id: 123"] {
            std::fs::write(&path, invalid).unwrap();
            assert!(read_settings(root.path()).is_err());
        }
    }

    fn select_test_package(root: &Path, key: &str) {
        let store = root.join("scheme-packages");
        fs::create_dir_all(store.join("cloud").join(key).join("files")).unwrap();
        fs::create_dir_all(root.join("runtime").join(key)).unwrap();
        fs::write(root.join(RuntimeDirs::MIGRATION_MARKER), r#"{"version":1}"#).unwrap();
        fs::write(
            store.join("active-manifest.json"),
            format!(r#"{{"active_package":"{key}","runtime_migration_version":1}}"#),
        )
        .unwrap();
    }

    #[test]
    fn isolated_sync_prepares_settings_only_for_the_selected_runtime() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let settings = save_settings(root, "device", "").unwrap();
        let legacy = fs::read(root.join("installation.yaml")).unwrap();
        for key in ["one", "two"] {
            select_test_package(root, key);
            let runtime = prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
            assert_eq!(runtime, root.join("runtime").join(key));
            let prepared = read_settings(&runtime).unwrap();
            assert_eq!(prepared.installation_id, "device");
            assert_eq!(
                Path::new(&prepared.effective_sync_dir),
                runtime.join("sync")
            );
            assert!(!read_document(&runtime).unwrap().2.contains_key("sync_dir"));
            let ui = read_ui_settings(root).unwrap();
            assert_eq!(Path::new(&ui.default_sync_dir), runtime.join("sync"));
            assert_eq!(ui.package_sync_dir, ui.default_sync_dir);
            assert!(!runtime.join("sync").exists());
        }
        assert_eq!(fs::read(root.join("installation.yaml")).unwrap(), legacy);
        assert!(!root.join("sync").exists());

        let custom = root.join("shared sync");
        let settings = save_settings(root, "device", custom.to_str().unwrap()).unwrap();
        let runtime = prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
        let expected_path = custom.join("keytao-packages/two");
        assert_eq!(
            Path::new(&read_settings(&runtime).unwrap().sync_dir),
            expected_path
        );
        let ui = read_ui_settings(root).unwrap();
        assert_eq!(Path::new(&ui.default_sync_dir), runtime.join("sync"));
        assert_eq!(Path::new(&ui.package_sync_dir), expected_path);
    }

    #[test]
    fn migrates_old_default_sync_dir_without_replacing_an_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        select_test_package(root, "one");
        let settings = save_settings(root, "device", "").unwrap();
        let old = root.join("sync/keytao-packages/one");
        let new = root.join("runtime/one/sync");
        let snapshot = "device/dict.userdb.txt";
        fs::create_dir_all(old.join("device")).unwrap();
        fs::write(old.join(snapshot), b"old snapshot").unwrap();
        fs::create_dir(&new).unwrap();
        prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
        assert_eq!(fs::read(old.join(snapshot)).unwrap(), b"old snapshot");
        assert!(fs::read_dir(&new).unwrap().next().is_none());

        fs::remove_dir(&new).unwrap();
        prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read(new.join(snapshot)).unwrap(), b"old snapshot");
    }

    #[cfg(unix)]
    #[test]
    fn ui_settings_display_the_resolved_default_sync_directory() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        select_test_package(root, "one");
        save_settings(root, "device", "").unwrap();

        let ui = read_ui_settings(root).unwrap();
        assert_eq!(
            Path::new(&ui.default_sync_dir),
            root.join("runtime/one/sync")
        );
        assert_eq!(ui.default_sync_dir, ui.package_sync_dir);

        let shared = root.join("shared");
        fs::create_dir_all(shared.join("keytao-packages")).unwrap();
        std::os::unix::fs::symlink(&shared, root.join("sync")).unwrap();

        let ui = read_ui_settings(root).unwrap();
        assert_eq!(
            Path::new(&ui.default_sync_dir),
            root.join("sync/keytao-packages/one")
        );
        assert_eq!(ui.default_sync_dir, ui.package_sync_dir);
    }

    #[cfg(unix)]
    #[test]
    fn sync_pins_a_dangling_legacy_sync_root() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        select_test_package(root, "one");
        let settings = save_settings(root, "device", "").unwrap();
        let installation = root.join("installation.yaml");
        let original = fs::read(&installation).unwrap();
        std::os::unix::fs::symlink(root.join("unavailable"), root.join("sync")).unwrap();
        let legacy = root.join("sync/keytao-packages/one");
        let default_dir = root.join("runtime/one/sync");

        let ui = read_ui_settings(root).unwrap();
        assert!(ui.sync_dir.is_empty());
        assert_eq!(Path::new(&ui.package_sync_dir), legacy);
        assert_eq!(ui.default_sync_dir, ui.package_sync_dir);
        assert!(!default_dir.exists());

        let runtime = prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
        assert_eq!(
            Path::new(&read_settings(&runtime).unwrap().sync_dir),
            legacy
        );
        assert!(!default_dir.exists());
        assert_eq!(fs::read(&installation).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn sync_preserves_a_symlinked_legacy_sync_root() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        select_test_package(root, "one");
        let settings = save_settings(root, "device", "").unwrap();
        let shared = root.join("shared");
        let old = shared.join("keytao-packages/one/device");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("dict.userdb.txt"), b"old snapshot").unwrap();
        std::os::unix::fs::symlink(&shared, root.join("sync")).unwrap();

        let runtime = prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
        let legacy = root.join("sync");
        let expected = legacy.join("keytao-packages/one");
        assert!(!read_document(root).unwrap().2.contains_key("sync_dir"));
        assert_eq!(
            Path::new(&read_settings(&runtime).unwrap().sync_dir),
            expected
        );
        assert_eq!(
            Path::new(&read_ui_settings(root).unwrap().package_sync_dir),
            expected
        );
        assert_eq!(
            fs::read(old.join("dict.userdb.txt")).unwrap(),
            b"old snapshot"
        );
        assert!(!runtime.join("sync").exists());

        fs::remove_file(&legacy).unwrap();
        fs::rename(&shared, &legacy).unwrap();
        let ui = read_ui_settings(root).unwrap();
        assert_eq!(Path::new(&ui.package_sync_dir), runtime.join("sync"));
        assert_eq!(
            fs::read(runtime.join("sync/device/dict.userdb.txt")).unwrap(),
            b"old snapshot"
        );
        assert!(!expected.exists());
        prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
        assert!(!read_document(&runtime).unwrap().2.contains_key("sync_dir"));
    }

    #[cfg(unix)]
    #[test]
    fn sync_keeps_the_legacy_directory_when_rename_fails() {
        for errno in [libc::EXDEV, libc::EINVAL] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path();
            select_test_package(root, "one");
            let settings = save_settings(root, "device", "").unwrap();
            let old = root.join("sync/keytao-packages/one");
            let dest = root.join("runtime/one/sync");
            fs::create_dir_all(&old).unwrap();
            fs::write(old.join("snapshot"), b"old snapshot").unwrap();

            let runtime = prepare_sync_root(root, &settings, |from, to| {
                assert_eq!(from, old);
                assert_eq!(to, dest);
                Err(std::io::Error::from_raw_os_error(errno))
            })
            .unwrap();

            assert!(!read_document(root).unwrap().2.contains_key("sync_dir"));
            assert_eq!(Path::new(&read_settings(&runtime).unwrap().sync_dir), old);
            assert_eq!(fs::read(old.join("snapshot")).unwrap(), b"old snapshot");
            assert!(!dest.exists());

            let ui = read_ui_settings(root).unwrap();
            assert_eq!(Path::new(&ui.package_sync_dir), dest);
            assert_eq!(fs::read(dest.join("snapshot")).unwrap(), b"old snapshot");
            assert!(!old.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_legacy_sync_root_takes_precedence_over_existing_default() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        select_test_package(root, "one");
        let settings = save_settings(root, "device", "").unwrap();
        let default_dir = root.join("runtime/one/sync");
        fs::create_dir(&default_dir).unwrap();
        let shared = root.join("shared");
        fs::create_dir_all(shared.join("keytao-packages/one")).unwrap();
        std::os::unix::fs::symlink(&shared, root.join("sync")).unwrap();
        let legacy = root.join("sync/keytao-packages/one");

        let ui = read_ui_settings(root).unwrap();
        assert_eq!(Path::new(&ui.package_sync_dir), legacy);
        let runtime = prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
        assert_eq!(
            Path::new(&read_settings(&runtime).unwrap().sync_dir),
            legacy
        );
    }

    #[cfg(unix)]
    #[test]
    fn sync_pins_symlinked_components_below_the_legacy_sync_root() {
        for component in ["keytao-packages", "keytao-packages/one"] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path();
            select_test_package(root, "one");
            let settings = save_settings(root, "device", "").unwrap();
            let legacy = root.join("sync/keytao-packages/one");
            let link = root.join("sync").join(component);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(root.join("unavailable"), &link).unwrap();

            let ui = read_ui_settings(root).unwrap();
            assert_eq!(Path::new(&ui.package_sync_dir), legacy);
            assert_eq!(ui.default_sync_dir, ui.package_sync_dir);
            let runtime = prepare_sync_root(root, &settings, rename_sync_dir).unwrap();
            assert_eq!(
                Path::new(&read_settings(&runtime).unwrap().sync_dir),
                legacy
            );
            assert!(!runtime.join("sync").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn resetting_sync_path_stays_saved_when_legacy_dir_cannot_move() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        select_test_package(root, "two");
        save_settings(root, "device", root.join("custom").to_str().unwrap()).unwrap();
        let shared = root.join("shared");
        fs::create_dir_all(shared.join("keytao-packages/two")).unwrap();
        std::os::unix::fs::symlink(&shared, root.join("sync")).unwrap();

        save_settings(root, "device", "").unwrap();
        let ui = read_ui_settings(root).unwrap();
        assert!(ui.sync_dir.is_empty());
        assert_eq!(ui.default_sync_dir, ui.package_sync_dir);
        assert_eq!(
            Path::new(&ui.package_sync_dir),
            root.join("sync/keytao-packages/two")
        );
        assert!(!read_document(root).unwrap().2.contains_key("sync_dir"));
    }

    #[test]
    fn ui_settings_keep_saved_fields_when_runtime_resolution_fails() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        select_test_package(root, "one");
        let custom = root.join("custom");
        let saved = save_settings(root, "device", custom.to_str().unwrap()).unwrap();
        fs::remove_dir(root.join("scheme-packages/cloud/one/files")).unwrap();

        let ui = read_ui_settings(root).unwrap();
        assert_eq!(ui.installation_id, saved.installation_id);
        assert_eq!(ui.sync_dir, saved.sync_dir);
        assert!(ui.default_sync_dir.is_empty());
        assert!(ui.package_sync_dir.is_empty());
        assert_eq!(ui.last_sync_at, 0);
        assert!(
            ui.last_sync_error
                .unwrap()
                .contains("package files directory")
        );
    }

    #[test]
    fn ui_settings_keep_sync_paths_when_result_json_is_invalid() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        select_test_package(root, "one");
        save_settings(root, "device", "").unwrap();
        let runtime = root.join("runtime/one");
        fs::write(runtime.join(keytao_core::USER_DATA_SYNC_RESULT_FILE), b"{").unwrap();

        let ui = read_ui_settings(root).unwrap();
        assert_eq!(ui.installation_id, "device");
        assert_eq!(Path::new(&ui.default_sync_dir), runtime.join("sync"));
        assert_eq!(ui.package_sync_dir, ui.default_sync_dir);
        assert_eq!(ui.last_sync_at, 0);
        assert!(ui.last_sync_error.is_some());
    }

    #[test]
    fn fills_missing_and_empty_installation_id_without_changing_other_keys() {
        struct Sink;
        impl crate::EventSink for Sink {
            fn emit(&self, _: crate::CoreEvent) {}
        }

        let temp = tempfile::tempdir().unwrap();
        let core = Core::new(
            crate::AppEnv {
                data_dir: temp.path().join("data"),
                cache_dir: temp.path().join("cache"),
                resource_dir: temp.path().join("resources"),
                app_version: "1.2.15".into(),
                platform: crate::Platform::MacOs,
            },
            std::sync::Arc::new(Sink),
        )
        .unwrap();
        let root = temp.path().join("rime");
        fs::create_dir(&root).unwrap();
        let path = root.join("installation.yaml");
        for id in ["", "installation_id: ''\n", "installation_id:\n"] {
            fs::write(
                &path,
                format!("{id}custom: [1, true]\nsync_dir: /other # keep\n"),
            )
            .unwrap();
            ensure_installation(&core, &root).unwrap();
            let (_, text, mut mapping) = read_document(&root).unwrap();
            let text = text.unwrap();
            let generated = mapping.remove("installation_id").unwrap();
            let bytes = generated.as_str().unwrap().as_bytes();
            assert_eq!(bytes.len(), 36);
            assert_eq!(bytes[14], b'4');
            assert!(b"89ab".contains(&bytes[19]));
            assert_eq!(
                mapping,
                serde_yaml::from_str("custom: [1, true]\nsync_dir: /other").unwrap()
            );
            assert!(text.contains("sync_dir: /other # keep"));
            ensure_installation(&core, &root).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }

        // Opening sync settings when installation.yaml is missing or empty
        // automatically backfills a UUIDv4 installation_id alongside default_sync_dir.
        fs::remove_file(&path).unwrap();
        let ui = ensure_and_read_ui_settings(&core, &root).unwrap();
        let bytes = ui.installation_id.as_bytes();
        assert_eq!(bytes.len(), 36);
        assert_eq!(bytes[14], b'4');
        assert!(b"89ab".contains(&bytes[19]));
        assert_eq!(Path::new(&ui.default_sync_dir), root.join("sync"));
        assert!(ui.last_sync_error.is_none());

        fs::write(&path, "installation_id:\nsync_dir:\n").unwrap();
        let ui2 = ensure_and_read_ui_settings(&core, &root).unwrap();
        assert_eq!(ui2.installation_id.len(), 36);
        assert!(ui2.sync_dir.is_empty());
        assert!(ui2.last_sync_error.is_none());
    }

    #[test]
    fn user_sync_dir_requires_absolute_writable_path_with_existing_parent() {
        let root = tempfile::tempdir().unwrap();
        assert!(validate_user_sync_dir(Path::new("relative")).is_err());
        assert!(validate_user_sync_dir(&root.path().join("missing/sync")).is_err());
        let file = root.path().join("file");
        fs::write(&file, b"keep").unwrap();
        assert!(validate_user_sync_dir(&file).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"keep");
        let dir = root.path().join("sync");
        validate_user_sync_dir(&dir).unwrap();
        assert!(fs::read_dir(&dir).unwrap().next().is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
            if unsafe { libc::geteuid() } != 0 {
                assert!(validate_user_sync_dir(&dir).is_err());
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    mod sync {
        use super::*;
        use std::time::{Duration, Instant};

        struct MockHost {
            root: PathBuf,
            process_id: Option<u32>,
            finish_after: Option<Duration>,
        }

        impl ImeHost for MockHost {
            fn user_root(&self) -> Result<PathBuf, String> {
                Ok(self.root.clone())
            }

            fn write_reload_stamp(&self, root: &Path) -> Result<Option<PathBuf>, String> {
                let Some(process_id) = self.process_id else {
                    return Ok(None);
                };
                let request: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(root.join(keytao_core::USER_DATA_SYNC_REQUEST_FILE)).unwrap(),
                )
                .unwrap();
                let generation = request["generation"].as_u64().unwrap();
                let result_path = root.join(keytao_core::USER_DATA_SYNC_RESULT_FILE);
                let started = keytao_core::UserDataSyncResult {
                    generation,
                    ok: false,
                    error: None,
                    finished_at: 0,
                    process_id,
                };
                write_file_atomic(&result_path, &serde_json::to_vec(&started).unwrap()).unwrap();
                if let Some(delay) = self.finish_after {
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let completed = keytao_core::UserDataSyncResult {
                            ok: true,
                            finished_at: 1,
                            ..started
                        };
                        write_file_atomic(&result_path, &serde_json::to_vec(&completed).unwrap())
                            .unwrap();
                    });
                }
                Ok(None)
            }

            #[cfg(target_os = "windows")]
            fn publish_english_settings(&self, _enabled: bool) -> Result<(), String> {
                unreachable!()
            }
            #[cfg(target_os = "windows")]
            fn publish_schema_switches(&self) -> Result<(), String> {
                unreachable!()
            }
            #[cfg(target_os = "linux")]
            fn managed_helper(&self) -> &crate::ime_status::linux::ManagedImeHelper {
                unreachable!()
            }
            #[cfg(target_os = "linux")]
            fn suspend_for_deploy(
                &self,
                _user: String,
                _shared: String,
            ) -> keytao_core::ImeRuntime {
                unreachable!()
            }
            #[cfg(target_os = "linux")]
            fn resume_after_deploy(&self) -> Result<(), String> {
                unreachable!()
            }
        }

        #[tokio::test]
        async fn sync_waits_for_a_live_ime_past_the_offline_deadline() {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join("build")).unwrap();
            save_settings(root.path(), "device", "").unwrap();
            let started = Instant::now();
            sync_user_data(MockHost {
                root: root.path().to_owned(),
                process_id: Some(std::process::id()),
                finish_after: Some(Duration::from_secs(21)),
            })
            .await
            .unwrap();
            assert!(started.elapsed() >= Duration::from_secs(21));
            let result = keytao_core::read_user_data_sync_result(root.path())
                .unwrap()
                .unwrap();
            assert!(result.ok && result.finished_at != 0);
        }

        #[tokio::test]
        async fn sync_times_out_when_a_live_ime_never_completes() {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join("build")).unwrap();
            save_settings(root.path(), "device", "").unwrap();
            let host = MockHost {
                root: root.path().to_owned(),
                process_id: Some(std::process::id()),
                finish_after: None,
            };
            let error = tokio::time::timeout(
                Duration::from_secs(2),
                sync_user_data_with_timeout(host, Duration::from_millis(20)),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.contains("Timed out waiting for IME to complete"));
            let result = keytao_core::read_user_data_sync_result(root.path())
                .unwrap()
                .unwrap();
            assert_eq!(result.process_id, std::process::id());
            assert_eq!(result.finished_at, 0);
        }

        #[tokio::test]
        async fn sync_reports_a_dead_owner_without_waiting_for_the_offline_deadline() {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join("build")).unwrap();
            save_settings(root.path(), "device", "").unwrap();
            let started = Instant::now();
            let error = sync_user_data(MockHost {
                root: root.path().to_owned(),
                process_id: Some(u32::MAX),
                finish_after: None,
            })
            .await
            .unwrap_err();
            assert!(error.contains("exited"));
            assert!(started.elapsed() < Duration::from_secs(1));
            assert!(!process_alive(0));
        }

        #[tokio::test]
        async fn sync_expires_an_unclaimed_request_when_no_ime_responds() {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join("build")).unwrap();
            save_settings(root.path(), "device", "").unwrap();
            let started = Instant::now();
            let error = sync_user_data(MockHost {
                root: root.path().to_owned(),
                process_id: None,
                finish_after: None,
            })
            .await
            .unwrap_err();
            assert!(error.contains("did not start"));
            assert!(started.elapsed() >= Duration::from_secs(20));
            assert!(started.elapsed() < Duration::from_secs(25));
            let request: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.path().join(keytao_core::USER_DATA_SYNC_REQUEST_FILE)).unwrap(),
            )
            .unwrap();
            assert_eq!(request["expires_at"], 0);
        }
    }
}
