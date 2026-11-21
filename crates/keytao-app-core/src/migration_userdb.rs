use crate::CoreError;
use keytao_core::{RuntimeDirs, with_userdb_migration};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

pub(crate) fn legacy_dictionary_names(root: &Path) -> Result<BTreeSet<String>, CoreError> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let filename = entry.file_name();
        let Some(name) = filename.to_str().and_then(|s| s.strip_suffix(".userdb")) else {
            continue;
        };
        let is_directory = entry.file_type()?.is_dir();
        if is_directory && name.strip_prefix('.').is_some_and(valid_name) {
            continue;
        }
        if !valid_name(name) || !is_directory {
            return Err(CoreError::Other(format!(
                "Invalid legacy user dictionary directory: {}",
                entry.path().display()
            )));
        }
        names.insert(name.to_owned());
    }
    Ok(names)
}

pub(crate) fn migrate(
    root: &Path,
    targets: &[RuntimeDirs],
    sync_dir: &Path,
) -> Result<(), CoreError> {
    let names = legacy_dictionary_names(root)?;
    if names.is_empty() {
        return Ok(());
    }
    let system_temp = fs::canonicalize(std::env::temp_dir())?;
    let private_temp = if system_temp.starts_with(sync_dir) {
        fs::canonicalize(root.join("runtime"))?
    } else {
        system_temp
    };
    if private_temp.starts_with(sync_dir) {
        return Err(CoreError::Other(
            "No private user dictionary snapshot directory".into(),
        ));
    }
    let temporary = tempfile::Builder::new()
        .prefix("keytao-userdb-migration-")
        .tempdir_in(private_temp)?;
    let source = temporary.path().join("source");
    let sync = source.join("sync");
    fs::create_dir(&source)?;
    fs::write(
        source.join("installation.yaml"),
        b"installation_id: keytao-migration\n",
    )?;
    with_userdb_migration(|migration| {
        let mut runtime_names = Vec::new();
        for (index, target) in targets.iter().enumerate() {
            let private = temporary.path().join(index.to_string());
            let (dirs, schemas) =
                stage_schema_configs(target, &private).map_err(|error| error.to_string())?;
            runtime_names.push(migration.dictionary_names(&dirs, &schemas)?);
        }
        let mut snapshots = Vec::new();
        for name in &names {
            let filename = format!("{name}.userdb");
            let path = root.join(&filename);
            let canonical = fs::canonicalize(&path).map_err(|error| error.to_string())?;
            if canonical.starts_with(sync_dir) || sync_dir.starts_with(&canonical) {
                return Err(format!("Sync directory overlaps user dictionary {name}"));
            }
            stage_closed_database(&path, &source.join(filename)).map_err(|error| {
                format!(
                    "Could not stage user dictionary {name}; close the IME before retrying: {error}"
                )
            })?;
            snapshots.push((name, migration.backup(&source, &sync, name)?));
        }
        for (target, names) in targets.iter().zip(runtime_names) {
            for (name, snapshot) in &snapshots {
                if names.contains(*name) {
                    let destination = target.user_data_dir.join(format!("{name}.userdb"));
                    if check_restore_destination(&target.user_data_dir, &destination)
                        .map_err(|error| error.to_string())?
                    {
                        migration.restore(&target.user_data_dir, name, snapshot)?;
                    } else {
                        let stage = tempfile::Builder::new()
                            .prefix(".keytao-import-")
                            .tempdir_in(&target.user_data_dir)
                            .map_err(|error| error.to_string())?;
                        migration.restore(stage.path(), name, snapshot)?;
                        fs::rename(stage.path().join(format!("{name}.userdb")), destination)
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        Ok(())
    })
    .map_err(CoreError::Other)
}

fn stage_schema_configs(
    target: &RuntimeDirs,
    private: &Path,
) -> io::Result<(RuntimeDirs, Vec<String>)> {
    fs::create_dir(private)?;
    let mut schemas = BTreeSet::new();
    for directory in [&target.shared_data_dir, &target.user_data_dir] {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let filename = entry.file_name();
            let Some(name) = filename.to_str() else {
                continue;
            };
            if entry.file_type()?.is_file() {
                if let Some(id) = name
                    .strip_suffix(".schema.yaml")
                    .filter(|id| valid_name(id))
                {
                    schemas.insert(id.to_owned());
                }
                if directory == &target.user_data_dir
                    && name.ends_with(".yaml")
                    && !name.ends_with(".dict.yaml")
                    && !matches!(name, "user.yaml" | "installation.yaml")
                {
                    fs::copy(entry.path(), private.join(name))?;
                }
            }
        }
    }
    let mut dirs = RuntimeDirs::flat(private, &target.shared_data_dir);
    dirs.prebuilt_data_dir = private.join("prebuilt");
    Ok((dirs, schemas.into_iter().collect()))
}

fn stage_closed_database(source: &Path, destination: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(source)?.is_dir() {
        return Err(io::Error::other("Invalid legacy LevelDB directory"));
    }
    let lock_path = source.join("LOCK");
    if !fs::symlink_metadata(&lock_path)?.is_file() {
        return Err(io::Error::other("Invalid LevelDB lock file"));
    }
    let _lock = try_database_lock(&lock_path)?;
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == "LOCK" {
            continue;
        }
        if !entry.file_type()?.is_file() {
            return Err(io::Error::other("Invalid file in legacy LevelDB"));
        }
        fs::copy(entry.path(), destination.join(entry.file_name()))?;
    }
    verify_current_manifest(destination)
}

#[cfg(unix)]
fn try_database_lock(path: &Path) -> io::Result<File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    // Use the same record lock as LevelDB, rather than the unrelated flock(2) lock.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(windows)]
fn try_database_lock(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // LevelDB holds LOCK open for read and write with FILE_SHARE_READ.
    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
}

fn check_restore_destination(target: &Path, destination: &Path) -> io::Result<bool> {
    if !fs::symlink_metadata(target)?.is_dir() {
        return Err(io::Error::other(
            "Invalid runtime user dictionary destination",
        ));
    }
    match fs::symlink_metadata(target.join(".temp.userdb")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => {
            return Err(io::Error::other(
                "Runtime contains a user dictionary restore directory",
            ));
        }
    }
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Ok(metadata) if metadata.is_dir() => {
            for entry in fs::read_dir(destination)? {
                if !entry?.file_type()?.is_file() {
                    return Err(io::Error::other("Invalid file in runtime LevelDB"));
                }
            }
            verify_current_manifest(destination).map(|()| true)
        }
        Ok(_) => Err(io::Error::other(
            "Invalid runtime user dictionary directory",
        )),
        Err(error) => Err(error),
    }
}

fn verify_current_manifest(database: &Path) -> io::Result<()> {
    let current = fs::read_to_string(database.join("CURRENT"))?;
    let name = current.trim_end_matches(['\r', '\n']);
    let valid = name.strip_prefix("MANIFEST-").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    });
    if !valid || !fs::symlink_metadata(database.join(name))?.is_file() {
        return Err(io::Error::other("Invalid LevelDB manifest"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn dictionary_discovery_skips_temp_directories_and_rejects_invalid_entries() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        for name in [".temp.userdb", ".backup.userdb", "learned.userdb"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        assert_eq!(
            legacy_dictionary_names(root).unwrap(),
            BTreeSet::from(["learned".into()])
        );

        for name in [".userdb", "..temp.userdb"] {
            let invalid = root.join(name);
            fs::create_dir(&invalid).unwrap();
            assert!(legacy_dictionary_names(root).is_err(), "{name}");
            fs::remove_dir(invalid).unwrap();
        }
        fs::write(root.join(".file.userdb"), b"invalid").unwrap();
        assert!(legacy_dictionary_names(root).is_err());
    }

    #[test]
    fn userdb_snapshots_use_private_sync_and_only_matching_runtimes() {
        if crate::run_librime_test_in_child(
            "migration_userdb::tests::userdb_snapshots_use_private_sync_and_only_matching_runtimes",
        ) {
            return;
        }
        let fixture = tempfile::tempdir().unwrap();
        let legacy = fixture.path().join("legacy");
        let sync = fixture.path().join("external-sync");
        let shared = fixture.path().join("shared");
        for dir in [&legacy, &sync, &shared] {
            fs::create_dir(dir).unwrap();
        }
        fs::write(sync.join("untouched"), "external sync fixture").unwrap();
        fs::write(
            legacy.join("installation.yaml"),
            serde_yaml::to_string(&serde_json::json!({
                "installation_id": "real-installation",
                "sync_dir": sync,
            }))
            .unwrap(),
        )
        .unwrap();
        let seed = fixture.path().join("learned.userdb.txt");
        fs::write(
            &seed,
            concat!(
                "# Rime user dictionary\n#@/db_name\tlearned.userdb\n",
                "#@/db_type\tuserdb\n#@/tick\t42\n",
                "ni hao \t你好\tc=7 d=1.5 t=42\n",
            ),
        )
        .unwrap();
        with_userdb_migration(|migration| migration.restore(&legacy, "learned", &seed)).unwrap();
        let before = tree_bytes(&legacy);
        let sync_before = tree_bytes(&sync);
        let targets = [
            ("one", "learned.extra", None),
            ("two", "second", Some("learned")),
            ("unrelated", "unrelated", None),
        ]
        .map(|(id, dictionary, explicit)| {
            let user = fixture.path().join(id);
            let source = shared.join(id);
            fs::create_dir(&user).unwrap();
            fs::create_dir(&source).unwrap();
            fs::write(
                source.join(format!("{id}.schema.yaml")),
                format!("schema: {{schema_id: {id}}}\ntranslator: {{dictionary: {dictionary}}}\n"),
            )
            .unwrap();
            if let Some(name) = explicit {
                fs::write(
                    user.join(format!("{id}.custom.yaml")),
                    format!("patch:\n  translator/user_dict: {name}\n"),
                )
                .unwrap();
            }
            RuntimeDirs::flat(&user, &source)
        });

        migrate(&legacy, &targets, &sync).unwrap();

        assert_eq!(tree_bytes(&legacy), before);
        assert_eq!(tree_bytes(&sync), sync_before);
        assert!(!targets[2].user_data_dir.join("learned.userdb").exists());
        for dirs in &targets[..2] {
            let verify = tempfile::tempdir_in(fixture.path()).unwrap();
            stage_closed_database(
                &dirs.user_data_dir.join("learned.userdb"),
                &verify.path().join("learned.userdb"),
            )
            .unwrap();
            fs::write(
                verify.path().join("installation.yaml"),
                "installation_id: keytao-migration\n",
            )
            .unwrap();
            let backup = with_userdb_migration(|migration| {
                migration.backup(verify.path(), &verify.path().join("sync"), "learned")
            })
            .unwrap();
            let contents = fs::read_to_string(backup).unwrap();
            assert!(contents.contains("#@/tick\t42"));
            assert!(contents.contains("ni hao \t你好\tc=7 d=1.5 t=42"));
        }
    }

    fn tree_bytes(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            let name = PathBuf::from(path.file_name().unwrap());
            if path.is_dir() {
                files.extend(
                    tree_bytes(&path)
                        .into_iter()
                        .map(|(relative, bytes)| (name.join(relative), bytes)),
                );
            } else {
                files.push((name, fs::read(path).unwrap()));
            }
        }
        files.sort();
        files
    }
}
