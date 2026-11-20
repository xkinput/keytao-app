use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const SUBCOMMAND: &str = "--deploy-staged";
pub const DIRECTORY_ENV: &str = "KEYTAO_WINDOWS_DEPLOY_STAGE";
const PREFIX: &str = ".keytao-ime-deploy-";

pub struct Stage(PathBuf);

impl Stage {
    pub fn create(source: &Path) -> Result<Self, String> {
        let dirs = keytao_core::RuntimeDirs::resolve(source, source);
        let source = dirs.user_data_dir.as_path();
        let staging = dirs
            .staging_dir
            .strip_prefix(source)
            .map_err(|error| error.to_string())?;
        let parent = source.parent().ok_or("invalid Rime directory")?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = parent.join(format!("{PREFIX}{}-{nonce}", std::process::id()));
        fs::create_dir(&path).map_err(|error| format!("create deployment stage: {error}"))?;
        let stage = Self(path);
        copy_inputs(source, source, &stage.0, staging)?;
        Ok(stage)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn directory_name(&self) -> &std::ffi::OsStr {
        self.0.file_name().unwrap()
    }

    pub fn install(
        &self,
        root: &Path,
        mut reload: impl FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        let dirs = keytao_core::RuntimeDirs::resolve(root, root);
        let current = dirs.staging_dir;
        let staging = current
            .strip_prefix(&dirs.user_data_dir)
            .map_err(|error| error.to_string())?;
        let incoming = keytao_core::RuntimeDirs::flat(&self.0, &self.0).staging_dir;
        let previous = self.0.join("previous-build");
        fs::rename(&current, &previous)
            .map_err(|error| format!("preserve previous deployment: {error}"))?;
        let installed = fs::rename(&incoming, &current)
            .map_err(|error| format!("install deployment: {error}"))
            .and_then(|()| move_preserved_files(&previous, &previous, &current, staging))
            .and_then(|()| reload());
        if let Err(error) = installed {
            if current.exists() {
                fs::rename(&current, &incoming)
                    .map_err(|rollback| format!("{error}; preserve rejected build: {rollback}"))?;
                move_preserved_files(&incoming, &incoming, &previous, staging)
                    .map_err(|rollback| format!("{error}; restore build files: {rollback}"))?;
            }
            fs::rename(&previous, &current)
                .map_err(|rollback| format!("{error}; restore previous deployment: {rollback}"))?;
            return match reload() {
                Ok(()) => Err(error),
                Err(recovery) => Err(format!("{error}; reload previous deployment: {recovery}")),
            };
        }
        let _ = fs::remove_dir_all(previous);
        Ok(())
    }
}

fn move_preserved_files(
    base: &Path,
    source: &Path,
    target: &Path,
    staging: &Path,
) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let relative = path.strip_prefix(base).map_err(|error| error.to_string())?;
        if relative == Path::new(".keytao-deploying")
            || relative == Path::new(".keytao-deploy-failed")
        {
            continue;
        }
        let relative = staging.join(relative);
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let destination = target.join(entry.file_name());
        if kind.is_dir() && deployment_input(&relative, true, staging) {
            fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
            move_preserved_files(base, &path, &destination, staging)?;
        } else if !deployment_input(&relative, kind.is_dir(), staging) {
            fs::rename(&path, &destination)
                .map_err(|error| format!("preserve build file: {error}"))?;
        }
    }
    Ok(())
}

impl Drop for Stage {
    fn drop(&mut self) {
        // A failed rollback keeps the only preserved build available for recovery.
        if !self.0.join("previous-build").exists() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

pub fn directory_from_environment(source: &Path) -> Result<PathBuf, String> {
    let name = std::env::var(DIRECTORY_ENV).map_err(|error| error.to_string())?;
    checked_directory(source, &name)
}

fn checked_directory(source: &Path, name: &str) -> Result<PathBuf, String> {
    let suffix = name
        .strip_prefix(PREFIX)
        .ok_or("invalid deployment stage")?;
    if suffix.is_empty()
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
    {
        return Err("invalid deployment stage".into());
    }
    let dirs = keytao_core::RuntimeDirs::resolve(source, source);
    let parent = dirs
        .user_data_dir
        .parent()
        .ok_or("invalid Rime directory")?;
    let path = parent.join(name);
    if !fs::symlink_metadata(&path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        || path
            .canonicalize()
            .map_err(|error| error.to_string())?
            .parent()
            != Some(
                parent
                    .canonicalize()
                    .map_err(|error| error.to_string())?
                    .as_path(),
            )
    {
        return Err("deployment stage is outside the Rime directory parent".into());
    }
    Ok(path)
}

fn copy_inputs(base: &Path, source: &Path, target: &Path, staging: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let relative = path.strip_prefix(base).map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if !deployment_input(relative, kind.is_dir(), staging) {
            continue;
        }
        if kind.is_symlink() {
            return Err(format!(
                "symbolic link in deployment input: {}",
                relative.display()
            ));
        }
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_inputs(base, &path, &destination, staging)?;
        } else if kind.is_file() {
            fs::copy(&path, &destination).map_err(|error| error.to_string())?;
            fs::OpenOptions::new()
                .write(true)
                .open(&destination)
                .and_then(|file| file.set_modified(entry.metadata()?.modified()?))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn deployment_input(path: &Path, directory: bool, staging: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if path.components().any(|part| {
        let Component::Normal(name) = part else {
            return true;
        };
        let name = name.to_string_lossy();
        name.ends_with(".userdb") || matches!(name.as_ref(), "log" | "sync")
    }) {
        return false;
    }
    if directory {
        return !name.starts_with('.') || name == ".keytao-package-lua";
    }
    if name.starts_with('.') {
        if let Ok(relative) = path.strip_prefix(staging) {
            return [
                ".keytao-deployed",
                ".keytao-desktop-inputs.json",
                ".keytao-schema-inputs.json",
                ".keytao-windows-build-signature.json",
                ".keytao-skipped-schemas.json",
            ]
            .iter()
            .any(|name| relative == Path::new(name));
        }
        return [
            ".keytao-active-scheme.json",
            ".keytao-current-schema",
            ".keytao-schema-switches.json",
            ".keytao-windows-build-repair-v1",
            "rime-data/.bundled-version",
        ]
        .iter()
        .any(|name| path == Path::new(name));
    }
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("yaml" | "lua" | "txt" | "bin" | "ocd2" | "gram")
    ) || (path.starts_with("opencc")
        && path
            .extension()
            .is_some_and(|extension| extension == "json"))
        || path == Path::new("lua/rime_ice/lunar.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Stage {
        let root = std::env::temp_dir().join(format!(
            "keytao-staged-deploy-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Stage(root)
    }

    #[test]
    fn snapshot_only_copies_deployment_inputs() {
        let source = fixture();
        for (path, value) in [
            ("schema.schema.yaml", "schema"),
            ("build/schema.table.bin", "dictionary"),
            ("build/.keytao-deployed", "ready"),
            (".keytao-package-lua/package/lua/init.lua", "return {}"),
            ("opencc/config.json", "{}"),
            ("app-state.json", "private"),
            ("auth-tokens.json", "secret"),
            ("schema.userdb/words.txt", "learned"),
            ("log/engine.txt", "private"),
            ("build/.keytao-deploying", "busy"),
        ] {
            let path = source.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        }
        let stage = Stage::create(source.path()).unwrap();
        for path in [
            "schema.schema.yaml",
            "build/schema.table.bin",
            "build/.keytao-deployed",
            ".keytao-package-lua/package/lua/init.lua",
            "opencc/config.json",
        ] {
            assert!(stage.path().join(path).is_file(), "{path}");
        }
        for path in [
            "app-state.json",
            "auth-tokens.json",
            "schema.userdb",
            "log",
            "build/.keytao-deploying",
        ] {
            assert!(!stage.path().join(path).exists(), "{path}");
        }
        let name = stage.directory_name().to_str().unwrap();
        assert_eq!(
            checked_directory(source.path(), name).unwrap(),
            stage.path()
        );
        assert!(checked_directory(source.path(), "../outside").is_err());
    }

    #[test]
    fn rejected_build_restores_old_artifacts_without_touching_user_data() {
        let source = fixture();
        fs::create_dir(source.path().join("build")).unwrap();
        fs::write(source.path().join("build/schema.table.bin"), "old").unwrap();
        fs::write(source.path().join("build/custom.data"), "keep").unwrap();
        fs::write(source.path().join("user.yaml"), "selected").unwrap();
        let stage = Stage::create(source.path()).unwrap();
        fs::write(stage.path().join("build/schema.table.bin"), "new").unwrap();
        let mut reloads = 0;
        assert!(stage
            .install(source.path(), || {
                reloads += 1;
                if reloads == 1 {
                    Err("rejected".into())
                } else {
                    Ok(())
                }
            })
            .is_err());
        assert_eq!(reloads, 2);
        assert_eq!(
            fs::read(source.path().join("build/schema.table.bin")).unwrap(),
            b"old"
        );
        assert_eq!(
            fs::read(source.path().join("user.yaml")).unwrap(),
            b"selected"
        );
        assert_eq!(
            fs::read(source.path().join("build/custom.data")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn successful_install_preserves_other_files_and_removes_its_backup() {
        let source = fixture();
        fs::create_dir(source.path().join("build")).unwrap();
        fs::write(source.path().join("build/schema.table.bin"), "old").unwrap();
        fs::write(source.path().join("build/custom.data"), "keep").unwrap();
        let stage = Stage::create(source.path()).unwrap();
        fs::write(source.path().join("build/.keytao-deploying"), "failed").unwrap();
        fs::write(source.path().join("build/.keytao-deploy-failed"), "failed").unwrap();
        fs::write(stage.path().join("build/schema.table.bin"), "new").unwrap();
        stage.install(source.path(), || Ok(())).unwrap();
        assert_eq!(
            fs::read(source.path().join("build/schema.table.bin")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(source.path().join("build/custom.data")).unwrap(),
            b"keep"
        );
        assert!(!stage.path().join("previous-build").exists());
        assert!(!source.path().join("build/.keytao-deploying").exists());
        assert!(!source.path().join("build/.keytao-deploy-failed").exists());
    }
}
