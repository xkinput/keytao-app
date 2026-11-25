use crate::{
    Core, CoreError, package_activation, package_store::PackageStore, packages::safe_path,
};
use keytao_core::{ReloadStamp, RuntimeDirs, isolated_deploy};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(crate) const MIGRATIONS_DIR: &str = "migrations";
pub(crate) const BACKUP_DIR: &str = "legacy-backup";
const BACKUP_COMPLETE: &str = "runtime/.keytao-backup-complete";

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
struct MigrationMarker {
    version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backup_dir: Option<String>,
    #[serde(default)]
    notified: bool,
}

pub async fn ensure_before_write(core: &Core, root: &Path) -> Result<(), CoreError> {
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    migrate_if_needed(core, root, &shared).map(|_| ())
}

pub fn start_app(core: &Core, root: &Path) -> Result<(), CoreError> {
    let shared = crate::deploy::deploy_shared_data_dir(core, root);
    migrate_if_needed(core, root, &shared).map(|_| ())
}

pub fn migrate_if_needed(core: &Core, root: &Path, _bundled: &Path) -> Result<bool, CoreError> {
    if let Err(error) = crate::rime_sync::ensure_installation(core, root) {
        tracing::warn!(%error, "Rime installation metadata creation will retry");
    }
    if RuntimeDirs::migration_complete(root) {
        return Ok(false);
    }
    if !RuntimeDirs::needs_migration(root) {
        if fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir()) {
            if let Ok(root) = checked_root(root) {
                let _ = isolated_deploy::with_migration_lock(&root, || {
                    if !RuntimeDirs::migration_complete(&root) && !RuntimeDirs::needs_migration(&root)
                    {
                        initialize_fresh_isolated_marker(core, &root)
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                });
            }
        }
        return Ok(false);
    }
    let root = checked_root(root)?;
    isolated_deploy::with_migration_lock(&root, || {
        if !RuntimeDirs::needs_migration(&root) {
            return Ok(false);
        }
        backup_and_reset_to_isolated(core, &root).map_err(|error| error.to_string())?;
        Ok(true)
    })
    .map_err(|error| {
        tracing::error!(%error, "Package directory backup migration failed");
        CoreError::Other(format!("方案目录迁移失败：{error}"))
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn migrate_default_if_needed(root: &Path, bundled: &Path) -> Result<bool, CoreError> {
    if !RuntimeDirs::needs_migration(root) {
        return Ok(false);
    }
    struct Sink;
    impl crate::EventSink for Sink {
        fn emit(&self, _: crate::CoreEvent) {}
    }
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| CoreError::Other("Cannot determine app data directory".into()))?
        .join("ink.rea.keytao-app");
    let core = Core::new(
        crate::AppEnv {
            cache_dir: data_dir.join("cache"),
            data_dir,
            resource_dir: bundled.to_owned(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            #[cfg(target_os = "linux")]
            platform: crate::Platform::Linux,
            #[cfg(target_os = "macos")]
            platform: crate::Platform::MacOs,
            #[cfg(target_os = "windows")]
            platform: crate::Platform::Windows,
        },
        std::sync::Arc::new(Sink),
    )?;
    migrate_if_needed(&core, root, bundled)
}

pub fn pending_backup_notice(root: &Path) -> Result<Option<String>, CoreError> {
    if !root.try_exists()? {
        return Ok(None);
    }
    let path = marker_path(root)?;
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Ok(marker) = serde_json::from_slice::<MigrationMarker>(&bytes) else {
        return Ok(None);
    };
    if marker.version != RuntimeDirs::MIGRATION_VERSION || marker.notified {
        return Ok(None);
    }
    Ok(marker.backup_dir.filter(|dir| !dir.trim().is_empty()))
}

pub fn dismiss_backup_notice(root: &Path) -> Result<(), CoreError> {
    if !root.try_exists()? {
        return Ok(());
    }
    let root = checked_root(root)?;
    isolated_deploy::with_migration_lock(&root, || {
        let path = marker_path(&root).map_err(|error| error.to_string())?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        let Ok(mut marker) = serde_json::from_slice::<MigrationMarker>(&bytes) else {
            return Ok(());
        };
        if !marker.notified {
            marker.notified = true;
            let serialized = serde_json::to_vec(&marker).map_err(|error| error.to_string())?;
            crate::scheme_files::write_file_atomic(&path, &serialized)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })
    .map_err(CoreError::Other)
}

fn backup_and_reset_to_isolated(core: &Core, root: &Path) -> Result<(), CoreError> {
    let _ = package_activation::recover(core);
    let mut sync_paths = Vec::new();
    if let Ok(sync_dir) = crate::runtime_materialize::legacy_sync_dir(root) {
        sync_paths.push(sync_dir);
    }
    if let Ok(lexical) = lexical_sync_path(root) {
        sync_paths.push(lexical);
    }
    sync_paths.sort();
    sync_paths.dedup();

    let version_dir = sanitize_version(&core.env().app_version);
    let backup_dir = safe_path(root, &format!("{MIGRATIONS_DIR}/{version_dir}"))
        .map_err(CoreError::Other)?;
    fs::create_dir_all(&backup_dir)?;

    move_root_entries(root, root, &backup_dir, &sync_paths)?;

    let store_root = PackageStore::new(core).root();
    let root_store = root.join("scheme-packages");
    let same_store = store_root == root_store
        || (store_root.canonicalize().ok().is_some()
            && store_root.canonicalize().ok() == root_store.canonicalize().ok());
    if !same_store && fs::symlink_metadata(&store_root).is_ok_and(|meta| meta.is_dir()) {
        let target_name = if backup_dir.join("scheme-packages").exists() {
            "app-scheme-packages"
        } else {
            "scheme-packages"
        };
        move_store_entries(&store_root, &backup_dir.join(target_name))?;
    }

    core.update_state(|state| {
        state.installed_packages.clear();
        state.active_scheme = None;
        state.last_deploy = None;
        state.dismissed_scheme_updates.clear();
    })?;

    fs::create_dir_all(root.join("runtime/.unselected"))?;
    fs::create_dir_all(&root_store)?;
    let empty_manifest = serde_json::to_vec(&serde_json::json!({
        "active_package": serde_json::Value::Null,
        "runtime_migration_version": RuntimeDirs::MIGRATION_VERSION,
    }))?;
    crate::scheme_files::write_file_atomic(
        &root_store.join("active-manifest.json"),
        &empty_manifest,
    )?;
    if !same_store {
        fs::create_dir_all(&store_root)?;
        crate::scheme_files::write_file_atomic(
            &store_root.join("active-manifest.json"),
            &empty_manifest,
        )?;
    }

    safe_path(root, keytao_core::RELOAD_STAMP_FILE_NAME).map_err(CoreError::Other)?;
    ReloadStamp::write(root).map_err(CoreError::Other)?;

    let marker = MigrationMarker {
        version: RuntimeDirs::MIGRATION_VERSION,
        backup_dir: Some(backup_dir.to_string_lossy().into_owned()),
        notified: false,
    };
    let marker_file = marker_path(root)?;
    if let Some(parent) = marker_file.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::scheme_files::write_file_atomic(&marker_file, &serde_json::to_vec(&marker)?)?;
    Ok(())
}

fn sanitize_version(raw: &str) -> String {
    let trimmed = raw.trim();
    let candidate = if trimmed.is_empty() {
        env!("CARGO_PKG_VERSION")
    } else {
        trimmed
    };
    let sanitized: String = candidate
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '+') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let clean = sanitized.trim_matches('.').trim_matches('_');
    if clean.is_empty() {
        format!("v{}", RuntimeDirs::MIGRATION_VERSION)
    } else {
        clean.to_owned()
    }
}

fn move_root_entries(
    root: &Path,
    source: &Path,
    destination: &Path,
    sync_dirs: &[PathBuf],
) -> Result<(), CoreError> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if sync_dirs.iter().any(|dir| from.starts_with(dir)) {
            continue;
        }
        if root == source {
            if preserved_during_migration(&name_str) {
                continue;
            }
            if name_str == "runtime" && entry.file_type()?.is_dir() {
                move_runtime_entries(&from, &destination.join("runtime"))?;
                continue;
            }
        }
        if sync_dirs.iter().any(|dir| dir.starts_with(&from)) && entry.file_type()?.is_dir() {
            let to = destination.join(&name);
            fs::create_dir_all(&to)?;
            move_root_entries(root, &from, &to, sync_dirs)?;
            continue;
        }
        let to = non_colliding_destination(&destination.join(&name))?;
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        move_path(&from, &to)?;
    }
    Ok(())
}

fn move_runtime_entries(runtime_dir: &Path, destination: &Path) -> Result<(), CoreError> {
    for entry in fs::read_dir(runtime_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy() == ".keytao-migration.lock" {
            continue;
        }
        fs::create_dir_all(destination)?;
        let to = non_colliding_destination(&destination.join(&name))?;
        move_path(&entry.path(), &to)?;
    }
    Ok(())
}

fn move_store_entries(store_root: &Path, destination: &Path) -> Result<(), CoreError> {
    let entries: Vec<_> = fs::read_dir(store_root)?.collect::<Result<_, _>>()?;
    if entries.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    for entry in entries {
        let to = non_colliding_destination(&destination.join(entry.file_name()))?;
        move_path(&entry.path(), &to)?;
    }
    Ok(())
}

fn non_colliding_destination(preferred: &Path) -> Result<PathBuf, CoreError> {
    if !preferred.try_exists()? {
        return Ok(preferred.to_owned());
    }
    let parent = preferred
        .parent()
        .ok_or_else(|| CoreError::Other("Invalid backup destination".into()))?;
    let name = preferred
        .file_name()
        .ok_or_else(|| CoreError::Other("Invalid backup filename".into()))?
        .to_string_lossy();
    let mut index = 1usize;
    loop {
        let candidate = parent.join(format!("{name}.{index}"));
        if !candidate.try_exists()? {
            return Ok(candidate);
        }
        index += 1;
    }
}

pub(crate) fn move_path(from: &Path, to: &Path) -> Result<(), CoreError> {
    match crate::package_fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_recursive(from, to, false)?;
            let meta = fs::symlink_metadata(from)?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                crate::package_fs::remove_dir_all(from)?;
            } else {
                fs::remove_file(from)?;
            }
            Ok(())
        }
    }
}

pub(crate) fn copy_repair_backup(from: &Path, to: &Path) -> Result<(), CoreError> {
    copy_recursive(from, to, true)
}

fn copy_recursive(from: &Path, to: &Path, reject_special_files: bool) -> Result<(), CoreError> {
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()), reject_special_files)?;
        }
    } else if meta.is_file() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to)?;
    } else if reject_special_files {
        return Err(CoreError::Other(format!(
            "无法跨文件系统备份符号链接或特殊文件：{}",
            from.display()
        )));
    }
    Ok(())
}

fn initialize_fresh_isolated_marker(core: &Core, root: &Path) -> Result<(), CoreError> {
    for store in [root.join("scheme-packages"), PackageStore::new(core).root()] {
        let manifest_path = store.join("active-manifest.json");
        if let Ok(bytes) = fs::read(&manifest_path) {
            if let Ok(mut manifest) =
                serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)
            {
                if manifest.get("runtime_migration_version").is_none() {
                    manifest.insert(
                        "runtime_migration_version".into(),
                        RuntimeDirs::MIGRATION_VERSION.into(),
                    );
                    let _ = crate::scheme_files::write_file_atomic(
                        &manifest_path,
                        &serde_json::to_vec(&manifest)?,
                    );
                }
            }
        }
    }
    let marker = MigrationMarker {
        version: RuntimeDirs::MIGRATION_VERSION,
        backup_dir: None,
        notified: true,
    };
    let marker_file = marker_path(root)?;
    if let Some(parent) = marker_file.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::scheme_files::write_file_atomic(&marker_file, &serde_json::to_vec(&marker)?)?;
    Ok(())
}

fn preserved_during_migration(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        MIGRATIONS_DIR
            | "log"
            | "logs"
            | "sync"
            | "rime-data"
            | "resources"
            | "app-state"
            | "shared"
            | "trash"
            | "cache"
            | "installation.yaml"
            | "theme.yaml"
            | "keyboard.yaml"
            | "keyboard-layout.yaml"
            | "libjd"
            | "runtime-log.json"
            | "android_ime.json"
            | "ios_ime.json"
            | "keytao-librime-upgrade.lock"
            | "keytao-librime.version"
            | "keytao-ime-engine.json"
            | ".rime-data.lock"
            | ".rime-data.staging"
            | ".rime-data.previous"
            | "auth-tokens.json"
            | keytao_core::RELOAD_STAMP_FILE_NAME
            | keytao_core::english_mode::SETTINGS_FILE
            | crate::account::USER_DICTIONARY
            | "keytao.user.remove.txt"
            | ".keytao-migration.lock"
            | ".keytao-package-deploy.lock"
    ) || lower.starts_with("app-state.json")
}

pub(crate) fn control_entry(name: &str) -> bool {
    matches!(
        name,
        "runtime" | "scheme-packages" | BACKUP_DIR | MIGRATIONS_DIR | "keytao-runtime-settings.json"
    ) || preserved_during_migration(name)
        || (name.starts_with(".keytao-")
            && name != ".keytao-package-lua"
            && !name.starts_with(".keytao-rime-user-"))
}

fn lexical_sync_path(root: &Path) -> Result<PathBuf, CoreError> {
    let configured = crate::rime_sync::read_settings(root).map_err(CoreError::Other)?;
    let configured_path = if configured.sync_dir.is_empty() {
        root.join("sync")
    } else {
        root.join(configured.sync_dir)
    };
    Ok(std::path::absolute(configured_path)?
        .components()
        .fold(PathBuf::new(), |mut path, part| {
            if part == std::path::Component::ParentDir {
                path.pop();
            } else {
                path.push(part);
            }
            path
        }))
}

fn is_backup_timestamp(timestamp: &str) -> bool {
    timestamp.len() == 26
        && timestamp
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 => byte == b'T',
                15 => byte == b'.',
                25 => byte == b'Z',
                _ => byte.is_ascii_digit(),
            })
}

pub(crate) fn completed_backups(root: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let complete = safe_path(root, BACKUP_COMPLETE).map_err(CoreError::Other)?;
    if !complete.is_file() {
        return Ok(Vec::new());
    }
    let parent = safe_path(root, BACKUP_DIR).map_err(CoreError::Other)?;
    let mut backups = Vec::new();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(backups),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry.file_name().to_str().is_some_and(is_backup_timestamp)
        {
            backups.push(entry.path());
        }
    }
    backups.sort();
    Ok(backups)
}

fn checked_root(root: &Path) -> Result<PathBuf, CoreError> {
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(CoreError::Other("迁移目录必须是普通目录".into()));
    }
    Ok(std::path::absolute(root)?)
}

fn marker_path(root: &Path) -> Result<PathBuf, CoreError> {
    safe_path(root, RuntimeDirs::MIGRATION_MARKER).map_err(CoreError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEnv, AppState, CoreEvent, EventSink, Platform, state::InstalledPackage};
    use std::sync::{Arc, Mutex};

    struct Sink;
    impl EventSink for Sink {
        fn emit(&self, _: CoreEvent) {}
    }

    fn fixture(data_dir: &Path, version: &str) -> Core {
        Core {
            self_ref: Default::default(),
            env: AppEnv {
                data_dir: data_dir.to_owned(),
                cache_dir: data_dir.join("cache"),
                resource_dir: data_dir.join("resources"),
                app_version: version.into(),
                platform: Platform::MacOs,
            },
            sink: Arc::new(Sink),
            state: Mutex::new(AppState::default()),
            download_cancel: tokio::sync::watch::channel(0).0,
            app_update_package: tokio::sync::Mutex::new(None),
            recovered_state_file: None,
            pending_package: Mutex::new(()),
            user_words: Mutex::new(crate::user_words::UserWordsState::default()),
            package_word_support: Mutex::default(),
        }
    }

    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn backs_up_legacy_schemes_to_migrations_version_and_clears_packages() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("app-data");
        let root = temp.path().join("rime");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&root).unwrap();

        let core = fixture(&data_dir, "1.2.15-alpha.7");
        core.lock_state().installed_packages.insert(
            "xmjd".into(),
            InstalledPackage {
                version: Some("v1".into()),
            },
        );
        core.lock_state().active_scheme = Some("xmjd".into());

        put(&data_dir, "scheme-packages/cloud/xmjd/files/xmjd6.schema.yaml", b"schema: {}");
        put(&root, "default.custom.yaml", b"patch: {}");
        put(&root, "xmjd6.schema.yaml", b"schema: {}");
        put(&root, "user.yaml", b"var: {previously_selected_schema: xmjd6}");
        put(&root, "rime.lua", b"-- user lua");
        put(&root, "build/xmjd6.table.bin", b"compiled");
        put(&root, "theme.yaml", b"ui: {}");
        let installation =
            b"# Keep settings\ninstallation_id: legacy-device\nsync_dir: /rime-sync\n";
        put(&root, "installation.yaml", installation);
        put(&root, "sync/installation/xmjd6.userdb.txt", b"synced");
        put(&root, crate::account::USER_DICTIONARY, b"words");

        assert!(RuntimeDirs::needs_migration(&root));
        start_app(&core, &root).unwrap();
        assert!(!RuntimeDirs::needs_migration(&root));
        assert!(RuntimeDirs::migration_complete(&root));
        assert_eq!(RuntimeDirs::active_package_key(&root), None);
        assert!(core.state().installed_packages.is_empty());
        assert_eq!(core.state().active_scheme, None);

        // Global files remain untouched in root.
        assert_eq!(fs::read(root.join("theme.yaml")).unwrap(), b"ui: {}");
        assert_eq!(
            fs::read(root.join("installation.yaml")).unwrap(),
            installation
        );
        assert_eq!(
            fs::read(root.join("sync/installation/xmjd6.userdb.txt")).unwrap(),
            b"synced"
        );
        assert_eq!(
            fs::read(root.join(crate::account::USER_DICTIONARY)).unwrap(),
            b"words"
        );

        // Scheme files and store packages moved to migrations/<version>/.
        assert!(!root.join("xmjd6.schema.yaml").exists());
        assert!(!root.join("default.custom.yaml").exists());
        assert!(!root.join("build").exists());
        let backup = root.join("migrations/1.2.15-alpha.7");
        assert!(backup.join("xmjd6.schema.yaml").is_file());
        assert!(backup.join("default.custom.yaml").is_file());
        assert!(backup.join("rime.lua").is_file());
        assert!(backup.join("build/xmjd6.table.bin").is_file());
        assert!(backup.join("scheme-packages/cloud/xmjd/files/xmjd6.schema.yaml").is_file());

        // Notice is returned until dismissed, and never prompts again after dismissal.
        let notice = pending_backup_notice(&root).unwrap();
        assert_eq!(notice.as_deref(), Some(backup.to_string_lossy().as_ref()));
        dismiss_backup_notice(&root).unwrap();
        assert_eq!(pending_backup_notice(&root).unwrap(), None);
    }

    #[test]
    fn fresh_directory_does_not_create_migration_backup_or_notice() {
        let temp = tempfile::tempdir().unwrap();
        let core = fixture(&temp.path().join("data"), "1.2.15");
        let root = temp.path().join("rime");

        assert!(!RuntimeDirs::needs_migration(&root));
        start_app(&core, &root).unwrap();
        assert!(!root.join("migrations").exists());
        assert_eq!(pending_backup_notice(&root).unwrap(), None);
        let path = root.join("installation.yaml");
        let bytes = fs::read(&path).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_slice(&bytes).unwrap();
        assert_eq!(yaml["distribution_name"].as_str(), Some("KeyTao"));
        assert_eq!(yaml["distribution_code_name"].as_str(), Some("keytao"));
        assert_eq!(yaml["distribution_version"].as_str(), Some("1.2.15"));
        assert!(yaml["install_time"].as_str().is_some());
        assert_eq!(
            yaml["rime_version"].as_str(),
            keytao_core::librime_runtime_version().as_deref()
        );
        let settings = crate::rime_sync::read_settings(&root).unwrap();
        let id = settings.installation_id.as_bytes();
        assert_eq!(id.len(), 36);
        assert_eq!(id[14], b'4');
        assert!(b"89ab".contains(&id[19]));
        assert_eq!(Path::new(&settings.effective_sync_dir), root.join("sync"));
        start_app(&core, &root).unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn installation_metadata_leaves_symlinked_root_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let core = fixture(&temp.path().join("data"), "1.2.15");
        let target = temp.path().join("target");
        let root = temp.path().join("rime");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &root).unwrap();

        assert!(crate::rime_sync::ensure_installation(&core, &root).is_err());
        assert!(!target.join("installation.yaml").exists());
    }

    #[test]
    fn older_migration_marker_triggers_backup_to_current_version_directory() {
        let temp = tempfile::tempdir().unwrap();
        let core = fixture(&temp.path().join("data"), "2.0.0");
        let root = temp.path().join("rime");
        put(&root, "runtime/.keytao-migration.json", br#"{"version":0}"#);
        put(&root, "runtime/keytao/keytao.custom.yaml", b"patch: {}");
        put(&root, "scheme-packages/local/demo/files/demo.schema.yaml", b"schema: {}");

        assert!(RuntimeDirs::needs_migration(&root));
        start_app(&core, &root).unwrap();
        assert!(!RuntimeDirs::needs_migration(&root));
        assert!(RuntimeDirs::migration_complete(&root));

        let backup = root.join("migrations/2.0.0");
        assert!(backup.join("runtime/keytao/keytao.custom.yaml").is_file());
        assert!(backup.join("scheme-packages/local/demo/files/demo.schema.yaml").is_file());
        assert!(pending_backup_notice(&root).unwrap().is_some());
    }
}
