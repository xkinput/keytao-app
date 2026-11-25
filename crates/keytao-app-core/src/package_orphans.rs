use crate::{
    Core, CoreError, Platform, package_activation,
    package_fs::is_link,
    package_store::PackageStore,
    packages::{safe_path, sha256_reader},
    runtime_materialize,
};
use keytao_core::{RuntimeDirs, isolated_deploy::with_isolated_package_lock};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrphanFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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
    let mut owned = BTreeSet::new();
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
    owned.extend(crate::addon_store::read_report_paths(core, root)?);
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
    let sync_dir = crate::runtime_materialize::legacy_sync_dir(root)?;
    let flat_live = keytao_core::RuntimeDirs::needs_migration(root);
    let mut files = Vec::new();
    collect(root, root, &sync_dir, &owned, flat_live, &mut files)?;
    for (key, runtime) in orphan_runtimes(core, root)? {
        let manifest = match runtime_materialize::orphan_manifest(&runtime, &key) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%key, %error, "Could not verify orphan runtime ownership");
                continue;
            }
        };
        let protected = |relative: &str| {
            relative.split('/').any(package_activation::user_file)
                || relative.split('/').any(|part| part.ends_with(".leveldb"))
        };
        for (relative, expected) in manifest.owned.iter().chain(&manifest.generated) {
            if manifest.user_owned.contains_key(relative) || protected(&relative.to_ascii_lowercase()) {
                continue;
            }
            let Ok(path) = safe_path(&runtime, relative) else {
                continue;
            };
            if path.starts_with(&sync_dir) {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file() || is_link(&metadata) {
                continue;
            }
            let sha256 = sha256_reader(fs::File::open(&path)?)?;
            if &sha256 == expected {
                files.push(OrphanFile {
                    path: format!("runtime/{key}/{relative}"),
                    sha256,
                    size: metadata.len(),
                });
            }
        }
    }
    for backup in crate::package_migration::completed_backups(root)? {
        files.push(OrphanFile {
            path: backup.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"),
            sha256: backup_fingerprint(&backup)?,
            size: 0,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(OrphanReport {
        files,
        kept_backups,
    })
}

fn orphan_runtimes(core: &Core, root: &Path) -> Result<Vec<(String, PathBuf)>, CoreError> {
    let runtime = root.join("runtime");
    if !RuntimeDirs::migration_complete(root)
        || !fs::symlink_metadata(&runtime).is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata))
    {
        return Ok(Vec::new());
    }
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    let mut installed = runtime_materialize::isolated_package_keys(core, root, &shared)?;
    let data_dir = &core.env().data_dir;
    safe_path(data_dir, "app-state.json").map_err(CoreError::Other)?;
    installed.extend(crate::AppState::load(data_dir)?.installed_packages.into_keys());
    installed.extend(RuntimeDirs::active_package_key(root));
    let package_roots = [root, data_dir.as_path()];
    for package_root in package_roots {
        let path = safe_path(package_root, "scheme-packages/active-manifest.json")
            .map_err(CoreError::Other)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !is_link(&metadata) => {
                let data: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
                match data.get("active_package") {
                    Some(serde_json::Value::String(key)) => {
                        installed.insert(key.clone());
                    }
                    Some(serde_json::Value::Null) => {}
                    None if data["root"].is_string() && data["manifest"].is_object() => {
                        let Some(key) = data["scheme_key"].as_str().filter(|key| !key.is_empty())
                        else {
                            return Ok(Vec::new());
                        };
                        installed.insert(key.to_owned());
                    }
                    _ => return Ok(Vec::new()),
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            _ => return Ok(Vec::new()),
        }
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(runtime)? {
        let entry = entry?;
        let Ok(key) = entry.file_name().into_string() else {
            continue;
        };
        if key == "builtin" || installed.contains(&key)
            || !fs::symlink_metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata))
            || RuntimeDirs::isolated_user_data_dir(root, &key).as_deref() != Some(entry.path().as_path())
        {
            continue;
        }
        if package_roots.iter().any(|root| {
            ["cloud", "local", "addons"].iter().any(|namespace| {
                safe_path(root, &format!("scheme-packages/{namespace}/{key}")).map_or(true, |path| {
                    fs::symlink_metadata(path).err().is_none_or(|error| error.kind() != ErrorKind::NotFound)
                })
            })
        }) {
            continue;
        }
        candidates.push((key, entry.path()));
    }
    Ok(candidates)
}

pub(crate) fn prune_orphan_runtime_builds(core: &Core, root: &Path) -> Result<(), CoreError> {
    if !fs::symlink_metadata(root).is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata))
        || !RuntimeDirs::migration_complete(root)
    {
        return Ok(());
    }
    with_isolated_package_lock(root, || {
        for (key, _) in orphan_runtimes(core, root).map_err(|error| error.to_string())? {
            if let Err(error) = prune_runtime_build(root, &key) {
                tracing::warn!(%error, %key, "Could not prune orphan runtime build");
            }
        }
        Ok(())
    }).map_err(CoreError::Other)
}

pub(crate) fn prune_runtime_build(root: &Path, key: &str) -> Result<(), CoreError> {
    let Some(runtime) = RuntimeDirs::isolated_user_data_dir(root, key) else {
        return Ok(());
    };
    let build = safe_path(root, &format!("runtime/{key}/build")).map_err(CoreError::Other)?;
    if !fs::symlink_metadata(&build).is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata))
        || build.starts_with(runtime_materialize::legacy_sync_dir(root)?)
        || runtime_materialize::orphan_manifest(&runtime, key)?.is_none()
    {
        return Ok(());
    }
    let _lease = runtime_materialize::lock_runtime(&runtime, false)?;
    if runtime_materialize::orphan_manifest(&runtime, key)?.is_none() {
        return Ok(());
    }
    for entry in fs::read_dir(&build)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || is_link(&metadata) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                continue;
            }
        }
        if keytao_core::is_rime_compiled_output(&path) {
            fs::remove_file(&path)?;
            tracing::info!(path = %path.display(), "Removed orphan runtime build artifact");
        }
    }
    if fs::read_dir(&build)?.next().is_none() {
        fs::remove_dir(&build)?;
        tracing::info!(path = %build.display(), "Removed empty orphan runtime build directory");
    }
    Ok(())
}

fn backup_fingerprint(path: &Path) -> Result<String, CoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(CoreError::Other("Legacy backup is no longer a directory".into()));
    }
    let identity = format!("{:?}:{:?}", metadata.created().ok(), metadata.modified()?);
    Ok(sha256_reader(identity.as_bytes())?)
}

fn collect(
    root: &Path,
    directory: &Path,
    sync_dir: &Path,
    owned: &BTreeSet<String>,
    flat_live: bool,
    files: &mut Vec<OrphanFile>,
) -> Result<(), CoreError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let name_lower = name.to_ascii_lowercase();
        if matches!(name_lower.as_str(), "app-state.json" | "sync")
            || name_lower.contains(".userdb")
            || name_lower.ends_with(".leveldb")
            || path.starts_with(sync_dir)
        {
            continue;
        }
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
        let relative_lower = relative.to_ascii_lowercase();
        if (directory == root
            && (relative_lower == "build"
                || crate::package_migration::control_entry(&relative_lower)))
            || (flat_live && flat_rime_input(&relative_lower))
            || package_activation::user_file(&relative_lower)
            || matches!(
                relative_lower.as_str(),
                "installation.yaml"
                    | "rime.lua"
                    | "keytao-ime.reload"
                    | "keytao-librime-upgrade.lock"
                    | ".keytao-package-deploy.lock"
                    | keytao_core::english_mode::SETTINGS_FILE
                    | "theme.yaml"
                    | "keyboard.yaml"
                    | "log"
            )
        {
            continue;
        }
        if metadata.is_dir() {
            collect(root, &path, sync_dir, owned, flat_live, files)?;
        } else if metadata.is_file()
            && !owned.contains(&relative)
            && !generated(&relative_lower, &path)?
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

fn flat_rime_input(relative_lower: &str) -> bool {
    // Schemas can reference config and vocabulary files with arbitrary names.
    matches!(relative_lower, "build" | "lua" | "opencc" | "prebuilt")
        || matches!(
            Path::new(relative_lower)
                .extension()
                .and_then(|ext| ext.to_str()),
            Some("yaml" | "lua" | "txt" | "bin" | "ocd2" | "gram")
        )
}

fn generated(relative_lower: &str, path: &Path) -> Result<bool, CoreError> {
    if matches!(relative_lower, "default.custom.yaml" | "default-custom.yaml") {
        return Ok(true);
    }
    if !relative_lower.ends_with(".yaml") && !relative_lower.ends_with(".lua") {
        return Ok(false);
    }
    let bytes = fs::read(path)?;
    Ok(crate::managed_custom::is_generated(relative_lower, &bytes))
}

/// Moves only selections still present in a fresh report with the displayed fingerprint.
pub fn trash_selected_with_backups(
    core: &Core,
    root: &Path,
    selected: &[OrphanFile],
    selected_backups: &[KeptBackup],
) -> Result<Vec<String>, CoreError> {
    trash_with(core, root, selected, selected_backups, None)
}

fn trash_with(
    core: &Core,
    root: &Path,
    selected: &[OrphanFile],
    selected_backups: &[KeptBackup],
    trash_override: Option<&Path>,
) -> Result<Vec<String>, CoreError> {
    let runtime_keys: BTreeSet<_> = selected.iter().filter_map(|file| {
        file.path.strip_prefix("runtime/")?.split_once('/').map(|(key, _)| key)
    }).collect();
    if runtime_keys.is_empty() {
        return trash_unlocked(core, root, selected, selected_backups, trash_override);
    }
    if !fs::symlink_metadata(root).is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata)) {
        return Err(CoreError::Other("Invalid runtime cleanup root".into()));
    }
    with_isolated_package_lock(root, || {
        let mut leases = Vec::new();
        for key in runtime_keys {
            let runtime = RuntimeDirs::isolated_user_data_dir(root, key)
                .ok_or_else(|| format!("Invalid runtime package: {key}"))?;
            if runtime_materialize::orphan_manifest(&runtime, key)
                .map_err(|error| error.to_string())?.is_none()
            {
                return Err(format!("Missing runtime ownership: {key}"));
            }
            leases.push(runtime_materialize::lock_runtime(&runtime, false).map_err(|error| error.to_string())?);
        }
        trash_unlocked(core, root, selected, selected_backups, trash_override)
            .map_err(|error| error.to_string())
    }).map_err(CoreError::Other)
}

fn trash_unlocked(
    core: &Core,
    root: &Path,
    selected: &[OrphanFile],
    selected_backups: &[KeptBackup],
    trash_override: Option<&Path>,
) -> Result<Vec<String>, CoreError> {
    let current = report(core, root)?;
    let available = current.files;
    let available_backups = current.kept_backups;
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
    let store = PackageStore::new(core);
    let store_root = store.root();
    let mut seen_backups = BTreeSet::new();
    for backup in selected_backups {
        if !seen_backups.insert((&backup.path, &backup.backup))
            || !available_backups.contains(backup)
        {
            return Err(CoreError::Other(format!(
                "Backup changed or is no longer available: {}",
                backup.backup
            )));
        }
        validate_kept_backup_path(root, &store_root, backup)?;
    }

    let mut moved = Vec::new();
    let backups = crate::package_migration::completed_backups(root)?;
    for file in selected {
        let path = safe_path(root, &file.path).map_err(CoreError::Other)?;
        let sha256 = if backups.contains(&path) {
            backup_fingerprint(&path)?
        } else {
            sha256_reader(fs::File::open(&path)?)?
        };
        if sha256 != file.sha256 {
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

    let mut removed_store_backups = BTreeSet::new();
    for backup in selected_backups {
        let (target_path, store_relative) = validate_kept_backup_path(root, &store_root, backup)?;
        match fs::symlink_metadata(&target_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(CoreError::Other(format!(
                        "Backup is not a regular file: {}",
                        backup.backup
                    )));
                }
                if let Err(error) = move_to_trash(core, &target_path, trash_override) {
                    return Err(CoreError::Other(format!(
                        "Could not move {}; already moved: {}: {error}",
                        backup.backup,
                        moved.join(", ")
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(relative) = store_relative {
            prune_empty_backup_parents(&store_root.join("backups"), target_path.parent());
            removed_store_backups.insert((backup.path.clone(), relative));
        }
        moved.push(backup.backup.clone());
    }

    if !removed_store_backups.is_empty() {
        remove_activation_kept_backups(core, root, &removed_store_backups)?;
    }

    Ok(moved)
}

fn validate_kept_backup_path(
    root: &Path,
    store_root: &Path,
    backup: &KeptBackup,
) -> Result<( std::path::PathBuf, Option<String>), CoreError> {
    let candidate = Path::new(&backup.backup);
    if let Ok(relative) = candidate.strip_prefix(store_root) {
        let relative_str = relative.to_string_lossy().replace('\\', "/");
        if relative_str.starts_with("backups/") {
            let checked = safe_path(store_root, &relative_str).map_err(CoreError::Other)?;
            return Ok((checked, Some(relative_str)));
        }
    }
    if candidate.parent() == Some(root) {
        if let Some(name) = candidate.file_name().and_then(|n| n.to_str()) {
            if backup.path == "rime.lua"
                && name.starts_with(".keytao-rime-user-")
                && name.ends_with(".lua")
            {
                let checked = safe_path(root, name).map_err(CoreError::Other)?;
                return Ok((checked, None));
            }
        }
    }
    Err(CoreError::Other(format!(
        "Invalid kept backup path: {}",
        backup.backup
    )))
}

fn prune_empty_backup_parents(backups_root: &Path, mut dir: Option<&Path>) {
    while let Some(current) = dir {
        if current == backups_root || !current.starts_with(backups_root) {
            break;
        }
        if fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

fn remove_activation_kept_backups(
    core: &Core,
    root: &Path,
    removed: &BTreeSet<(String, String)>,
) -> Result<(), CoreError> {
    let Some(mut active) = package_activation::read(core, root)? else {
        return Ok(());
    };
    active
        .kept_backups
        .retain(|entry| !removed.contains(&(entry.path.clone(), entry.backup.clone())));
    if let Some(records) = active.activations.as_mut() {
        for record in records.values_mut() {
            record
                .kept_backups
                .retain(|entry| !removed.contains(&(entry.path.clone(), entry.backup.clone())));
        }
    }
    let path = PackageStore::new(core).checked_activation_path()?;
    crate::scheme_files::write_file_atomic(&path, &serde_json::to_vec(&active)?)?;
    Ok(())
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
        fs::write(root.join("orphan.yaml"), "legacy").unwrap();

        let mut files = Vec::new();
        collect(root, root, &root.join("sync"), &BTreeSet::new(), false, &mut files).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "orphan.yaml");
    }

    #[test]
    fn startup_prunes_orphan_runtime_outputs_and_reports_only_proven_package_files() {
        let (temp, core, root) = setup();
        let runtime = root.join("runtime");
        fs::create_dir(&runtime).unwrap();
        fs::write(root.join(RuntimeDirs::MIGRATION_MARKER), r#"{"version":1}"#).unwrap();
        fs::create_dir(root.join("scheme-packages")).unwrap();
        fs::write(root.join("scheme-packages/active-manifest.json"),
            r#"{"active_package":"active","runtime_migration_version":1}"#).unwrap();
        let legacy_store = PackageStore::new(&core);
        fs::create_dir_all(legacy_store.root()).unwrap();
        let legacy_path = legacy_store.active_manifest_path();
        let legacy_manifest = serde_json::json!({
            "root": root.canonicalize().unwrap(),
            "scheme_key": "keytao",
            "manifest": {},
            "packages": {},
            "rime_lua": null,
            "lua_owner": null,
        });
        core.update_state(|state| {
            state.installed_packages.insert("xmjd".into(), Default::default());
        }).unwrap();

        let digest = sha256_reader(b"package source".as_slice()).unwrap();
        for key in ["orphan", "active", "keytao", "xmjd", "unknown"] {
            let path = runtime.join(key);
            fs::create_dir_all(path.join("build")).unwrap();
            let mut header = [0u8; 32];
            header[..15].copy_from_slice(b"Rime::Table/4.0");
            fs::write(path.join("build/scheme.table.bin"), header).unwrap();
            if key != "unknown" {
                fs::write(path.join(RuntimeDirs::MANIFEST_FILE), serde_json::to_vec(&serde_json::json!({
                    "package": key,
                    "owned": {"essay.txt": digest, "edited.txt": digest, "settings.yaml": digest},
                    "generated": {},
                    "user_owned": {"settings.yaml": digest},
                })).unwrap()).unwrap();
                for name in ["essay.txt", "settings.yaml"] {
                    fs::write(path.join(name), b"package source").unwrap();
                }
                fs::write(path.join("edited.txt"), b"user edits").unwrap();
            }
        }
        let orphan = runtime.join("orphan");
        fs::create_dir(orphan.join("orphan.userdb")).unwrap();
        fs::write(orphan.join("orphan.userdb/CURRENT"), b"learned words").unwrap();
        fs::write(orphan.join("build/USER.USERDB.table.bin"), b"protected").unwrap();
        fs::write(orphan.join("build/notes.txt"), b"notes").unwrap();
        fs::write(orphan.join("build/notes.schema.yaml"), b"schema: {schema_id: notes}").unwrap();
        #[cfg(unix)]
        {
            let outside = temp.path().join("outside.bin");
            fs::write(&outside, b"outside").unwrap();
            std::os::unix::fs::symlink(&outside, orphan.join("build/link.table.bin")).unwrap();
        }

        let mut invalid_manifest = legacy_manifest.clone();
        invalid_manifest["active_package"] = 42.into();
        fs::write(&legacy_path, serde_json::to_vec(&invalid_manifest).unwrap()).unwrap();
        crate::package_migration::start_app(&core, &root).unwrap();
        assert!(orphan.join("build/scheme.table.bin").is_file());
        assert!(report(&core, &root).unwrap().files.is_empty());

        fs::write(&legacy_path, serde_json::to_vec(&legacy_manifest).unwrap()).unwrap();
        crate::package_migration::start_app(&core, &root).unwrap();
        assert!(!orphan.join("build/scheme.table.bin").exists());
        for key in ["active", "keytao", "xmjd", "unknown"] {
            assert!(runtime.join(key).join("build/scheme.table.bin").is_file());
        }
        for name in ["orphan.userdb/CURRENT", "build/USER.USERDB.table.bin", "build/notes.txt", "build/notes.schema.yaml", "edited.txt", "settings.yaml"] {
            assert!(orphan.join(name).is_file(), "{name}");
        }
        #[cfg(unix)]
        assert!(orphan.join("build/link.table.bin").is_symlink());
        let selected = report(&core, &root).unwrap().files;
        assert_eq!(selected.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["runtime/orphan/essay.txt"]);
        assert_eq!(trash_with(&core, &root, &selected, &[], Some(&temp.path().join("trash"))).unwrap(),
            ["runtime/orphan/essay.txt"]);
        assert!(!orphan.join("essay.txt").exists());
        assert!(orphan.join("orphan.userdb/CURRENT").is_file());
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
        fs::create_dir(root.join("build")).unwrap();
        let artifacts = [
            "build/default.yaml",
            "build/legacy.schema.yaml",
            "build/legacy.table.bin",
            "build/.keytao-deployed",
        ];
        for path in artifacts.into_iter().chain(["orphan.bak"]) {
            fs::write(root.join(path), b"legacy contents").unwrap();
        }

        let orphans = report(&core, &root).unwrap().files;
        assert!(orphans.iter().any(|file| file.path == "orphan.bak"));
        assert!(orphans
            .iter()
            .all(|file| !artifacts.contains(&file.path.as_str())));
        let path = artifacts[2];
        let selected = OrphanFile {
            path: path.into(),
            sha256: sha256_reader(b"legacy contents".as_slice()).unwrap(),
            size: b"legacy contents".len() as u64,
        };
        assert!(trash_with(&core, &root, &[selected], &[], Some(&temp.path().join("trash"))).is_err());
        assert_eq!(fs::read(root.join(path)).unwrap(), b"legacy contents");
    }

    #[test]
    fn pre_migration_mixed_case_rime_files_are_excluded_from_orphans() {
        let (_temp, core, root) = setup();
        for path in [
            "Legacy.Schema.YAML",
            "Foo.YAML",
            "Words.TXT",
            "Rime.LUA",
            "OpenCC/s2t.json",
            "Dict.Userdb/CURRENT",
            "orphan.bak",
        ] {
            let full = root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, b"legacy").unwrap();
        }

        let orphans = report(&core, &root).unwrap().files;
        assert_eq!(
            orphans.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            ["orphan.bak"]
        );
    }

    #[test]
    fn excludes_owned_generated_addon_and_user_files() {
        let (_temp, core, root) = setup();
        fs::create_dir(root.join("runtime")).unwrap();
        fs::write(
            root.join(keytao_core::RuntimeDirs::MIGRATION_MARKER),
            r#"{"version":1}"#,
        )
        .unwrap();
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
                trash_with(&core, &root, &[selected], &[], Some(&temp.path().join("trash"))).is_err()
            );
            assert_eq!(fs::read(path).unwrap(), b"user settings\n");
        }
    }
    #[test]
    fn selected_move_is_reversible_and_rejects_changed_or_unsafe_files() {
        let (temp, core, root) = setup();
        let source = root.join("old.bak");
        fs::write(&source, b"user bytes\r\n").unwrap();
        let selected = report(&core, &root).unwrap().files;
        fs::write(&source, b"new bytes").unwrap();
        assert!(trash_with(&core, &root, &selected, &[], Some(&temp.path().join("trash"))).is_err());
        fs::write(&source, b"user bytes\r\n").unwrap();
        let trash = temp.path().join("trash");
        assert_eq!(
            trash_with(&core, &root, &selected, &[], Some(&trash)).unwrap(),
            ["old.bak"]
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
        assert!(trash_with(&core, &root, &[unsafe_file], &[], Some(&trash)).is_err());
    }
    #[test]
    fn trashes_kept_backups_and_removes_activation_records() {
        let (temp, core, root) = setup();
        let lua_backup = root.join(".keytao-rime-user-0.lua");
        fs::write(&lua_backup, b"user lua").unwrap();
        let store = PackageStore::new(&core);
        let store_backup = store.root().join("backups/abc123/JD6_userdata/JD6_overrides.tsv");
        fs::create_dir_all(store_backup.parent().unwrap()).unwrap();
        fs::write(&store_backup, b"overrides").unwrap();
        let active = package_activation::Activation {
            root: root.canonicalize().unwrap().to_string_lossy().into_owned(),
            kept_backups: vec![package_activation::Backup {
                path: "JD6_userdata/JD6_overrides.tsv".into(),
                backup: "backups/abc123/JD6_userdata/JD6_overrides.tsv".into(),
                sha256: "abc123".into(),
            }],
            ..Default::default()
        };
        fs::write(
            store.checked_activation_path().unwrap(),
            serde_json::to_vec(&active).unwrap(),
        )
        .unwrap();

        let initial = report(&core, &root).unwrap();
        assert_eq!(initial.kept_backups.len(), 2);
        let trash = temp.path().join("trash");
        let moved = trash_with(&core, &root, &[], &initial.kept_backups, Some(&trash)).unwrap();
        assert_eq!(moved.len(), 2);
        assert!(!lua_backup.exists());
        assert!(!store_backup.exists());
        assert!(report(&core, &root).unwrap().kept_backups.is_empty());
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
