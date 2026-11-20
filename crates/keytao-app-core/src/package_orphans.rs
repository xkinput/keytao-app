use crate::{
    Core, CoreError, Platform, package_activation,
    package_store::PackageStore,
    packages::{safe_path, sha256_reader, staging_relative},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrphanFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize)]
pub struct KeptBackup {
    pub path: String,
    pub backup: String,
}

#[derive(Serialize)]
pub struct OrphanReport {
    pub files: Vec<OrphanFile>,
    pub kept_backups: Vec<KeptBackup>,
}

pub fn report(core: &Core, root: &Path) -> Result<OrphanReport, CoreError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(CoreError::Other(
                "Rime directory must be a directory, not a symlink".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OrphanReport {
                files: Vec::new(),
                kept_backups: Vec::new(),
            });
        }
        Err(error) => return Err(error.into()),
    }
    let mut owned = BTreeSet::new();
    owned.extend(crate::addon_store::report_paths(core, root)?);
    owned.extend(crate::managed_custom::generated_paths(root).map_err(CoreError::Other)?);
    let store = PackageStore::new(core);
    for key in core.state().installed_packages.keys() {
        if let Some(meta) = store.read_meta(key)? {
            owned.extend(meta.manifest.into_keys());
        }
    }
    let mut kept_backups = Vec::new();
    let mut seen_backups = BTreeSet::new();
    let active = package_activation::read(core, root)?;
    for active in active.iter().chain(active.iter().flat_map(|record| record.activations.iter().flat_map(|records| records.values()))) {
        owned.extend(
            active
                .owned
                .iter()
                .chain(&active.generated)
                .map(|file| file.path.clone()),
        );
        for backup in &active.kept_backups {
            if !seen_backups.insert((&backup.path, &backup.backup)) {
                continue;
            }
            kept_backups.push(KeptBackup {
                path: backup.path.clone(),
                backup: safe_path(&store.root(), &backup.backup)
                    .map_err(CoreError::Other)?
                    .to_string_lossy()
                    .into_owned(),
            });
        }
    }
    // Receipts also cover add-on variants whose files differ from built-in allowlists.
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".keytao-rime-user-")
            && name.ends_with(".lua")
            && entry.file_type()?.is_file()
        {
            kept_backups.push(KeptBackup {
                path: "rime.lua".into(),
                backup: root.join(&name).to_string_lossy().into_owned(),
            });
        }
        if name.starts_with(".keytao-")
            && name.ends_with(".json")
            && name != keytao_core::ACTIVE_SCHEME_MANIFEST
        {
            let path = safe_path(root, &name).map_err(CoreError::Other)?;
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&fs::read(path)?) else {
                continue;
            };
            if let Some(files) = value["files"].as_array() {
                for file in files {
                    if let Some(path) = file["path"]
                        .as_str()
                        .filter(|p| safe_path(root, p).is_ok())
                    {
                        owned.insert(path.into());
                    }
                }
            }
            if let Some(path) = value["grammar"]["path"]
                .as_str()
                .filter(|p| safe_path(root, p).is_ok())
            {
                owned.insert(path.into());
            }
        }
    }
    let isolated = keytao_core::RuntimeDirs::isolation_enabled(root);
    // The entire legacy build is retained for rollback when isolation is enabled.
    if !isolated {
        let dirs = keytao_core::RuntimeDirs::flat(root, root);
        let build = staging_relative(root, &dirs);
        let mut artifacts = BTreeSet::from([format!("{build}/default.yaml")]);
        let schemas = crate::managed_custom::active_schema_ids(root).map_err(CoreError::Other)?;
        artifacts.extend(crate::package_artifacts::owned_artifact_paths(root, &schemas)?);
        for path in &owned {
            if let Some(id) = path.strip_suffix(".schema.yaml") {
                artifacts.insert(format!("{build}/{id}.schema.yaml"));
            }
            if let Some(id) = path.strip_suffix(".dict.yaml") {
                for extension in ["table", "prism", "reverse"] {
                    artifacts.insert(format!("{build}/{id}.{extension}.bin"));
                    artifacts.insert(format!(
                        "{build}/{}.{extension}.bin",
                        id.rsplit('/').next().unwrap()
                    ));
                }
            }
        }
        owned.extend(artifacts);
    }
    let mut files = Vec::new();
    collect(root, root, isolated, &owned, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(OrphanReport {
        files,
        kept_backups,
    })
}

fn collect(
    root: &Path,
    directory: &Path,
    isolated: bool,
    owned: &BTreeSet<String>,
    files: &mut Vec<OrphanFile>,
) -> Result<(), CoreError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        // Never traverse links, including links to user dictionaries outside this root.
        if metadata.file_type().is_symlink() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if (isolated
            && matches!(
                relative.as_str(),
                "build" | "runtime" | "scheme-packages" | keytao_core::RuntimeDirs::SETTINGS_FILE
            ))
            || package_activation::user_file(&relative)
            || matches!(
                relative.as_str(),
                "installation.yaml"
                    | "rime.lua"
                    | "keytao-ime.reload"
                    | "keytao-librime-upgrade.lock"
                    | keytao_core::english_mode::SETTINGS_FILE
                    | "theme.yaml"
                    | "keyboard.yaml"
                    | "log"
            )
        {
            continue;
        }
        if metadata.is_dir() {
            collect(root, &path, isolated, owned, files)?;
        } else if metadata.is_file()
            && !owned.contains(&relative)
            && !generated(&relative, &path)?
        {
            files.push(OrphanFile {
                path: relative,
                sha256: sha256_reader(fs::File::open(path)?)?,
                size: metadata.len(),
            });
        }
    }
    Ok(())
}

fn generated(relative: &str, path: &Path) -> Result<bool, CoreError> {
    if matches!(relative, "default.custom.yaml" | "default-custom.yaml") {
        return Ok(true);
    }
    if !relative.ends_with(".yaml") && !relative.ends_with(".lua") {
        return Ok(false);
    }
    let bytes = fs::read(path)?;
    Ok(crate::managed_custom::is_generated(relative, &bytes))
}

/// Moves only selections still present in a fresh report with the displayed fingerprint.
pub fn trash_selected(
    core: &Core,
    root: &Path,
    selected: &[OrphanFile],
) -> Result<Vec<String>, CoreError> {
    trash_with(core, root, selected, None)
}

fn trash_with(
    core: &Core,
    root: &Path,
    selected: &[OrphanFile],
    trash_override: Option<&Path>,
) -> Result<Vec<String>, CoreError> {
    let available = report(core, root)?.files;
    let mut seen = BTreeSet::new();
    for file in selected {
        safe_path(root, &file.path).map_err(CoreError::Other)?;
        if !seen.insert(&file.path) || !available.contains(file) {
            return Err(CoreError::Other(format!(
                "File changed or is no longer an orphan: {}",
                file.path
            )));
        }
    }
    let mut moved = Vec::new();
    for file in selected {
        let path = safe_path(root, &file.path).map_err(CoreError::Other)?;
        if sha256_reader(fs::File::open(&path)?)? != file.sha256 {
            return Err(CoreError::Other(format!(
                "File changed before moving: {}",
                file.path
            )));
        }
        if let Err(error) = move_to_trash(core, &path, trash_override) {
            return Err(CoreError::Other(format!(
                "Could not move {}; already moved: {}: {error}",
                file.path,
                moved.join(", ")
            )));
        }
        moved.push(file.path.clone());
    }
    Ok(moved)
}

fn move_to_trash(core: &Core, path: &Path, trash_override: Option<&Path>) -> Result<(), CoreError> {
    if let Some(trash) = trash_override {
        return rename_to_trash(path, trash, false);
    }
    if matches!(core.env().platform, Platform::Android | Platform::Ios) {
        return rename_to_trash(path, &core.env().data_dir.join("trash"), true);
    }
    #[cfg(target_os = "windows")]
    {
        return windows_trash(path);
    }
    #[cfg(target_os = "macos")]
    {
        return rename_to_trash(
            path,
            &dirs::home_dir()
                .ok_or_else(|| CoreError::Other("Home directory is unavailable".into()))?
                .join(".Trash"),
            false,
        );
    }
    #[cfg(target_os = "linux")]
    {
        return rename_to_trash(
            path,
            &dirs::data_dir()
                .ok_or_else(|| CoreError::Other("Data directory is unavailable".into()))?
                .join("Trash"),
            true,
        );
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    Err(CoreError::Other(
        "Trash is unavailable on this platform".into(),
    ))
}

fn rename_to_trash(path: &Path, trash: &Path, with_info: bool) -> Result<(), CoreError> {
    let directory = if with_info {
        trash.join("files")
    } else {
        trash.to_owned()
    };
    fs::create_dir_all(&directory)?;
    let original = path.canonicalize()?;
    let basename = path.file_name().unwrap().to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| CoreError::Other(error.to_string()))?
        .as_nanos();
    let mut sequence = 0;
    let destination = loop {
        let candidate = directory.join(format!("{stamp}-{sequence}-{basename}"));
        if !candidate.try_exists()? {
            break candidate;
        }
        sequence += 1;
    };
    let info = if with_info {
        let info_dir = trash.join("info");
        fs::create_dir_all(&info_dir)?;
        let info = info_dir.join(format!(
            "{}.trashinfo",
            destination.file_name().unwrap().to_string_lossy()
        ));
        let encoded = original
            .to_string_lossy()
            .bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                    (byte as char).to_string()
                }
                _ => format!("%{byte:02X}"),
            })
            .collect::<String>();
        let now = time::OffsetDateTime::now_utc();
        fs::write(
            &info,
            format!(
                "[Trash Info]\nPath={encoded}\nDeletionDate={:04}-{:02}-{:02}T{:02}:{:02}:{:02}\n",
                now.year(),
                now.month() as u8,
                now.day(),
                now.hour(),
                now.minute(),
                now.second()
            ),
        )?;
        Some(info)
    } else {
        None
    };
    // Rename fails safely across volumes; never fall back to a copy followed by deletion.
    if let Err(error) = fs::rename(path, destination) {
        if let Some(info) = info {
            let _ = fs::remove_file(info);
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_trash(path: &Path) -> Result<(), CoreError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::Shell::{
                FOF_ALLOWUNDO, FOF_NO_CONNECTED_ELEMENTS, FOF_NOCONFIRMATION, FOF_NOERRORUI,
                FOF_SILENT, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation,
                IShellItem, SHCreateItemFromParsingName,
            },
        },
        core::{HRESULT, PCWSTR},
    };
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // IFileOperation requires an STA; bridge worker threads may already be in an MTA.
    std::thread::spawn(move || unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|error| CoreError::Other(error.to_string()))?;
        let result = (|| -> windows::core::Result<()> {
            let operation: IFileOperation =
                CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            operation.SetOperationFlags(
                FOF_ALLOWUNDO
                    | FOFX_RECYCLEONDELETE
                    | FOF_NOCONFIRMATION
                    | FOF_NO_CONNECTED_ELEMENTS
                    | FOF_NOERRORUI
                    | FOFX_EARLYFAILURE
                    | FOF_SILENT,
            )?;
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None)?;
            operation.DeleteItem(&item, None)?;
            operation.PerformOperations()?;
            if operation.GetAnyOperationsAborted()?.as_bool() {
                return Err(windows::core::Error::from_hresult(HRESULT(
                    0x80004004u32 as i32,
                )));
            }
            Ok(())
        })();
        CoUninitialize();
        result.map_err(|error| CoreError::Other(error.to_string()))
    })
    .join()
    .map_err(|_| CoreError::Other("Recycle Bin operation failed".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, CoreEvent, EventSink};
    use std::{path::PathBuf, sync::Arc};
    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: CoreEvent) {}
    }
    fn setup() -> (tempfile::TempDir, Arc<Core>, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rime");
        fs::create_dir_all(&root).unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: temp.path().join("data"),
                cache_dir: temp.path().join("cache"),
                resource_dir: temp.path().join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            Arc::new(Sink),
        )
        .unwrap();
        (temp, core, root)
    }

    #[test]
    fn isolated_runtime_files_are_not_eligible_for_legacy_orphan_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("runtime/package")).unwrap();
        fs::write(root.join("runtime/package/symbols.yaml"), "package").unwrap();
        fs::write(
            root.join(keytao_core::RuntimeDirs::SETTINGS_FILE),
            r#"{"runtime_isolation":true}"#,
        )
        .unwrap();
        fs::write(root.join("orphan.yaml"), "legacy").unwrap();

        let mut files = Vec::new();
        collect(root, root, true, &BTreeSet::new(), &mut files).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "orphan.yaml");
    }

    #[test]
    fn isolated_orphan_cleanup_preserves_the_legacy_build() {
        let (temp, core, root) = setup();
        let store = keytao_core::package_store::root(&root);
        fs::create_dir_all(store.join("cloud/package/files")).unwrap();
        fs::write(
            store.join("active-manifest.json"),
            br#"{"active_package":"package"}"#,
        )
        .unwrap();
        fs::write(
            root.join(keytao_core::RuntimeDirs::SETTINGS_FILE),
            br#"{"runtime_isolation":true}"#,
        )
        .unwrap();
        fs::create_dir(root.join("build")).unwrap();
        let artifacts = [
            "build/default.yaml",
            "build/legacy.schema.yaml",
            "build/legacy.table.bin",
            "build/.keytao-deployed",
        ];
        for path in artifacts.into_iter().chain(["orphan.yaml"]) {
            fs::write(root.join(path), b"legacy contents").unwrap();
        }

        let orphans = report(&core, &root).unwrap().files;
        assert!(orphans.iter().any(|file| file.path == "orphan.yaml"));
        assert!(orphans
            .iter()
            .all(|file| !artifacts.contains(&file.path.as_str())));
        let path = artifacts[2];
        let selected = OrphanFile {
            path: path.into(),
            sha256: sha256_reader(b"legacy contents".as_slice()).unwrap(),
            size: b"legacy contents".len() as u64,
        };
        assert!(trash_with(&core, &root, &[selected], Some(&temp.path().join("trash"))).is_err());
        assert_eq!(fs::read(root.join(path)).unwrap(), b"legacy contents");
    }

    #[test]
    fn excludes_owned_generated_addon_and_user_files() {
        let (_temp, core, root) = setup();
        fs::create_dir_all(root.join("lua")).unwrap();
        for name in [
            "owned.schema.yaml",
            "generated.custom.yaml",
            "easy_en.dict.yaml",
            "user.yaml",
            "kept-target.yaml",
            "unknown.custom.yaml",
            "kept-target.yaml",
        ] {
            fs::write(root.join(name), "patch: {}\n").unwrap();
        }
        fs::write(
            root.join("legacy.custom.yaml"),
            "__patch: [] # keytao-candidate-page-size v1 0:\n",
        )
        .unwrap();
        fs::write(
            root.join(".keytao-easy-en.json"),
            serde_json::to_vec(&serde_json::json!({
                "format": 1, "version": "fixture", "source": "fixture",
                "archive_sha256": "a".repeat(64),
                "files": [{"path":"easy_en.dict.yaml", "sha256":sha256_reader(b"patch: {}\n".as_slice()).unwrap(), "size":10}],
            })).unwrap(),
        )
        .unwrap();
        fs::write(root.join(".keytao-rime-user-0.lua"), b"user lua").unwrap();
        let active = package_activation::Activation {
            root: root.canonicalize().unwrap().to_string_lossy().into_owned(),
            owned: vec![package_activation::OwnedFile {
                path: "owned.schema.yaml".into(),
                sha256: "unused".into(),
                move_file: false,
            }],
            generated: vec![package_activation::OwnedFile {
                path: "generated.custom.yaml".into(),
                sha256: "unused".into(),
                move_file: false,
            }],
            kept_backups: vec![package_activation::Backup {
                path: "kept-target.yaml".into(),
                backup: "backups/kept-target.yaml".into(),
                sha256: "unused".into(),
            }],
            ..Default::default()
        };
        let store = PackageStore::new(&core);
        fs::create_dir_all(store.root()).unwrap();
        fs::write(
            store.checked_activation_path().unwrap(),
            serde_json::to_vec(&active).unwrap(),
        )
        .unwrap();
        let report = report(&core, &root).unwrap();
        assert_eq!(report.kept_backups.len(), 2);
        assert_eq!(
            report
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["kept-target.yaml", "unknown.custom.yaml"]
        );
    }
    #[test]
    fn excludes_keytao_settings_logs_and_lua_entry_from_report_and_trash() {
        let (temp, core, root) = setup();
        let protected = [
            keytao_core::english_mode::SETTINGS_FILE,
            "theme.yaml",
            "keyboard.yaml",
            "rime.lua",
            "log/a.log",
            "log/nested/debug.yaml",
        ];
        for name in protected {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"user settings\n").unwrap();
        }
        assert!(report(&core, &root).unwrap().files.is_empty());
        for name in protected {
            let path = root.join(name);
            let selected = OrphanFile {
                path: name.into(),
                sha256: sha256_reader(fs::File::open(&path).unwrap()).unwrap(),
                size: fs::metadata(&path).unwrap().len(),
            };
            assert!(
                trash_with(&core, &root, &[selected], Some(&temp.path().join("trash"))).is_err()
            );
            assert_eq!(fs::read(path).unwrap(), b"user settings\n");
        }
    }
    #[test]
    fn selected_move_is_reversible_and_rejects_changed_or_unsafe_files() {
        let (temp, core, root) = setup();
        let source = root.join("old.custom.yaml");
        fs::write(&source, b"user bytes\r\n").unwrap();
        let selected = report(&core, &root).unwrap().files;
        fs::write(&source, b"new bytes").unwrap();
        assert!(trash_with(&core, &root, &selected, Some(&temp.path().join("trash"))).is_err());
        fs::write(&source, b"user bytes\r\n").unwrap();
        let trash = temp.path().join("trash");
        assert_eq!(
            trash_with(&core, &root, &selected, Some(&trash)).unwrap(),
            ["old.custom.yaml"]
        );
        assert!(!source.exists());
        let moved = fs::read_dir(&trash)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(&moved).unwrap(), b"user bytes\r\n");
        fs::rename(moved, &source).unwrap();
        assert_eq!(fs::read(source).unwrap(), b"user bytes\r\n");
        let unsafe_file = OrphanFile {
            path: "../outside".into(),
            sha256: String::new(),
            size: 0,
        };
        assert!(trash_with(&core, &root, &[unsafe_file], Some(&trash)).is_err());
    }
    #[test]
    fn missing_root_has_no_orphans() {
        let (_temp, core, root) = setup();
        assert!(
            report(&core, &root.join("missing"))
                .unwrap()
                .files
                .is_empty()
        );
    }
}
