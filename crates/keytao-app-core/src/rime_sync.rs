use crate::{ime_host::ImeHost, scheme_files::write_file_atomic};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RimeSyncSettings {
    pub installation_id: String,
    pub sync_dir: String,
    pub effective_sync_dir: String,
}

fn read_document(root: &Path) -> Result<(PathBuf, String, Mapping), String> {
    let path = crate::packages::safe_path(root, "installation.yaml")?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("Read installation.yaml: {error}")),
    };
    let value: Value =
        serde_yaml::from_str(&text).map_err(|error| format!("Read installation.yaml: {error}"))?;
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
        None => Ok(String::new()),
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
    })
}

pub fn read_settings(root: &Path) -> Result<RimeSyncSettings, String> {
    settings(root, &read_document(root)?.2)
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
fn update_document(text: &str, mapping: &Mapping) -> Result<String, String> {
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
        let Some(index) = ["installation_id", "sync_dir"]
            .iter()
            .position(|candidate| key == Some(*candidate))
        else {
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
    for (index, key) in ["installation_id", "sync_dir"].iter().enumerate() {
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
    let (path, text, mut mapping) = read_document(root)?;
    mapping.insert("installation_id".into(), installation_id.into());
    if sync_dir.is_empty() {
        mapping.remove(Value::String("sync_dir".into()));
    } else {
        mapping.insert("sync_dir".into(), sync_dir.into());
    }
    let text = update_document(&text, &mapping)?;
    std::fs::create_dir_all(root)
        .map_err(|error| format!("Create Rime user directory: {error}"))?;
    write_file_atomic(&path, text.as_bytes()).map_err(|error| error.to_string())?;
    settings(root, &mapping)
}

pub async fn sync_user_data(host: impl ImeHost) -> Result<(), String> {
    let root = host.user_root()?;
    let settings = read_settings(&root)?;
    if validate_installation_id(&settings.installation_id)? != settings.installation_id {
        return Err("installation_id must be trimmed".into());
    }
    validate_sync_dir(&settings.sync_dir)?;
    let request_root = prepare_sync_root(&root, &settings)?;
    let generation = keytao_core::request_user_data_sync(&request_root)?;
    if let Err(error) = host.write_reload_stamp(&root) {
        if keytao_core::cancel_user_data_sync(&request_root, generation).unwrap_or(true) {
            return Err(error);
        }
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
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
        if let Some(process_id) = process_id {
            if !process_alive(process_id) {
                return Err("IME exited before completing user data sync".into());
            }
        } else if tokio::time::Instant::now() >= deadline
            && keytao_core::cancel_user_data_sync(&request_root, generation)?
        {
            return Err(
                "IME did not start syncing within 20 seconds; make sure it is running".into(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn prepare_sync_root(root: &Path, settings: &RimeSyncSettings) -> Result<PathBuf, String> {
    let runtime = keytao_core::RuntimeDirs::resolve(root, root)?;
    let Some(key) = runtime.active_package_key else {
        return if keytao_core::RuntimeDirs::needs_migration(root) {
            Ok(runtime.user_data_dir)
        } else {
            Err("没有使用中的方案包".into())
        };
    };
    if !runtime.user_data_dir.is_dir() {
        return Err(format!("Rime runtime is not available for {key}"));
    }
    // Librime backs up configs beside userdb snapshots, so packages need separate sync folders.
    let sync_dir = Path::new(&settings.effective_sync_dir).join("keytao-packages").join(key);
    save_settings(
        &runtime.user_data_dir,
        &settings.installation_id,
        sync_dir.to_str().ok_or("Rime sync path is not valid UTF-8")?,
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

    #[test]
    fn isolated_sync_prepares_settings_only_for_the_selected_runtime() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let settings = save_settings(root, "device", "").unwrap();
        let legacy = std::fs::read(root.join("installation.yaml")).unwrap();
        let store = root.join("scheme-packages");
        for key in ["one", "two"] {
            std::fs::create_dir_all(store.join("cloud").join(key).join("files")).unwrap();
            std::fs::create_dir_all(root.join("runtime").join(key)).unwrap();
        }
        std::fs::write(root.join(keytao_core::RuntimeDirs::MIGRATION_MARKER), r#"{"version":1,"runtime_package":"one"}"#).unwrap();
        for key in ["one", "two"] {
            std::fs::write(store.join("active-manifest.json"), format!(r#"{{"active_package":"{key}","runtime_migration_version":1}}"#)).unwrap();
            let runtime = prepare_sync_root(root, &settings).unwrap();
            assert_eq!(runtime, root.join("runtime").join(key));
            let prepared = read_settings(&runtime).unwrap();
            assert_eq!(prepared.installation_id, "device");
            assert_eq!(Path::new(&prepared.effective_sync_dir), root.join("sync/keytao-packages").join(key));
        }
        assert_eq!(std::fs::read(root.join("installation.yaml")).unwrap(), legacy);
        assert!(!root.join("sync").exists());
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
