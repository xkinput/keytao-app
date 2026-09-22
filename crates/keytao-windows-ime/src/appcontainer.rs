//! AppContainer hosts get a private writable Rime cache, seeded exclusively
//! from the app's published public packages. The desktop user dictionary is
//! neither read nor granted to sandboxed applications.

use std::path::{Component, Path, PathBuf};
use windows::{
    core::Result,
    Win32::{
        Foundation::{CloseHandle, HANDLE, RPC_E_CHANGED_MODE},
        Security::{GetTokenInformation, TokenIsAppContainer, TOKEN_QUERY},
        System::{
            Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
        UI::Shell::{
            FOLDERID_LocalAppData, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY,
            KF_FLAG_FORCE_PACKAGE_REDIRECTION, KF_FLAG_NO_PACKAGE_REDIRECTION, KNOWN_FOLDER_FLAG,
        },
    },
};

const SEED_MARKER: &str = ".keytao-public-seed.version";
const SEED_FILES: &str = ".keytao-public-seed.files";

pub(crate) fn is_appcontainer() -> Result<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut flag = 0u32;
        let mut returned = 0u32;
        let result = GetTokenInformation(
            token,
            TokenIsAppContainer,
            Some((&mut flag as *mut u32).cast()),
            std::mem::size_of::<u32>() as u32,
            &mut returned,
        );
        let _ = CloseHandle(token);
        result?;
        Ok(flag != 0)
    }
}

fn local_app_data(flags: KNOWN_FOLDER_FLAG) -> Result<PathBuf> {
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
        if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
            initialized.ok()?;
        }
        let result = SHGetKnownFolderPath(&FOLDERID_LocalAppData, flags, HANDLE::default());
        let path = result.and_then(|value| {
            let text = value.to_string();
            CoTaskMemFree(Some(value.0.cast()));
            Ok(PathBuf::from(text?))
        });
        if initialized.is_ok() {
            CoUninitialize();
        }
        path
    }
}

pub(crate) struct SandboxData {
    pub(crate) user_dir: PathBuf,
    pub(crate) reload_stamp_path: PathBuf,
    pub(crate) reload_stamp_signature: Option<String>,
    generation: String,
    pub(crate) changed: bool,
}

impl SandboxData {
    pub(crate) fn mark_deployed(&self) -> std::result::Result<(), String> {
        keytao_core::mark_windows_rime_build_repair_complete(&self.user_dir)?;
        std::fs::write(self.user_dir.join(SEED_MARKER), &self.generation)
            .map_err(|error| format!("record sandbox schema deployment: {error}"))
    }
}

pub(crate) fn prepare_data() -> std::result::Result<SandboxData, String> {
    let desktop = local_app_data(KF_FLAG_NO_PACKAGE_REDIRECTION | KF_FLAG_DONT_VERIFY)
        .map_err(|error| format!("find public schema directory: {error}"))?;
    let private = local_app_data(KF_FLAG_FORCE_PACKAGE_REDIRECTION)
        .map_err(|error| format!("find AppContainer private directory: {error}"))?;
    if private == desktop {
        return Err("AppContainer data directory was not redirected".into());
    }
    prepare_at(
        &desktop.join("KeyTao/appcontainer-schemas"),
        &private.join("KeyTao"),
    )
}

fn checked_relative(text: &str) -> std::result::Result<PathBuf, String> {
    let path = PathBuf::from(text);
    if text.is_empty()
        || text.contains(':')
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("invalid relative path in public schema snapshot".into());
    }
    Ok(path)
}

fn public_asset(path: &Path) -> bool {
    !path.components().any(|part| {
        let part = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        part.starts_with('.') || part.ends_with(".userdb") || part == "log"
    }) && !matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("user.yaml" | "installation.yaml")
    )
}

fn prepare_at(public_root: &Path, private: &Path) -> std::result::Result<SandboxData, String> {
    let reload_stamp_path = public_root.join("current.txt");
    // Capture before reading the generation we deploy. Publishing a newer
    // snapshot during deployment must still trigger another reload afterwards.
    // A concurrent publication here can only cause one harmless extra reload.
    let reload_stamp_signature = keytao_core::ReloadStamp::signature_at(&reload_stamp_path);
    let generation = std::fs::read_to_string(&reload_stamp_path).map_err(|error| {
        format!("public schemas are unavailable; install a scheme in KeyTao: {error}")
    })?;
    let generation = generation.trim().to_owned();
    if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid public schema generation".into());
    }
    let changed = std::fs::read_to_string(private.join(SEED_MARKER))
        .ok()
        .as_deref()
        != Some(&generation);
    let result = SandboxData {
        user_dir: private.to_owned(),
        reload_stamp_path,
        reload_stamp_signature,
        generation,
        changed,
    };
    if !changed {
        return Ok(result);
    }
    let source = public_root.join(&result.generation);
    if std::fs::symlink_metadata(&source)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("symbolic link at public schema snapshot root".into());
    }
    // First deployments write only into the package's own private directory.
    std::fs::create_dir_all(private)
        .map_err(|error| format!("create sandbox schema cache: {error}"))?;
    let mut copied = Vec::new();
    fn copy_assets(
        base: &Path,
        dir: &Path,
        dest: &Path,
        copied: &mut Vec<String>,
    ) -> std::result::Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                return Err("symbolic link in public schema snapshot".into());
            }
            let path = entry.path();
            let relative = path.strip_prefix(base).map_err(|error| error.to_string())?;
            if !public_asset(relative) {
                continue;
            }
            let relative = checked_relative(&relative.to_string_lossy())?;
            if kind.is_dir() {
                copy_assets(base, &path, dest, copied)?;
            } else if kind.is_file() {
                let output = dest.join(&relative);
                std::fs::create_dir_all(output.parent().unwrap())
                    .map_err(|error| error.to_string())?;
                let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                if std::fs::read(&output).ok().as_deref() != Some(bytes.as_slice()) {
                    std::fs::write(&output, bytes).map_err(|error| error.to_string())?;
                }
                copied.push(relative.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    copy_assets(&source, &source, private, &mut copied)?;
    // Remove only files previously copied from public packages. Private userdb
    // and host configuration files never enter this manifest.
    if let Ok(previous) = std::fs::read_to_string(private.join(SEED_FILES)) {
        for name in previous
            .lines()
            .filter(|name| !copied.iter().any(|current| current == name))
        {
            let relative = checked_relative(name)?;
            if public_asset(&relative) {
                let path = private.join(&relative);
                if path.is_file() {
                    std::fs::remove_file(path).map_err(|error| error.to_string())?;
                }
                if relative.components().count() == 1
                    && relative.to_string_lossy().ends_with(".schema.yaml")
                {
                    let compiled = private.join("build").join(&relative);
                    if compiled.is_file() {
                        std::fs::remove_file(compiled).map_err(|error| error.to_string())?;
                    }
                }
            }
        }
    }
    std::fs::write(private.join(SEED_FILES), copied.join("\n"))
        .map_err(|error| error.to_string())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_seed_can_be_retried_and_newer_publication_is_not_acknowledged() {
        let root = std::env::temp_dir().join(format!(
            "keytao-sandbox-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let public = root.join("public");
        let private = root.join("private");
        assert!(prepare_at(&public, &private).is_err());
        let first = "a".repeat(64);
        std::fs::create_dir_all(public.join(&first)).unwrap();
        std::fs::write(public.join(&first).join("keytao.schema.yaml"), b"first").unwrap();
        std::fs::write(public.join("current.txt"), &first).unwrap();
        let prepared = prepare_at(&public, &private).unwrap();
        assert!(prepared.changed);
        // Preparing source files is not proof that Rime deployment succeeded.
        assert!(prepare_at(&public, &private).unwrap().changed);
        assert!(!private.join(SEED_MARKER).exists());
        let second = "b".repeat(64);
        std::fs::write(public.join("current.txt"), &second).unwrap();
        assert_ne!(
            prepared.reload_stamp_signature,
            keytao_core::ReloadStamp::signature_at(&prepared.reload_stamp_path)
        );
        assert_eq!(prepared.generation, first);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sandbox_seed_updates_only_public_files_and_keeps_private_learning() {
        let root = std::env::temp_dir().join(format!(
            "keytao-sandbox-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let public = root.join("public");
        let private = root.join("private");
        let first = "1".repeat(64);
        let second = "2".repeat(64);
        std::fs::create_dir_all(public.join(&first)).unwrap();
        std::fs::create_dir_all(private.join("keytao.userdb")).unwrap();
        std::fs::write(private.join("keytao.userdb/LOG"), b"private words").unwrap();
        std::fs::write(
            public.join(&first).join("keytao.schema.yaml"),
            b"public schema",
        )
        .unwrap();
        std::fs::write(public.join(&first).join("addon.schema.yaml"), b"addon").unwrap();
        std::fs::write(public.join(&first).join("user.yaml"), b"must not copy").unwrap();
        std::fs::write(public.join("current.txt"), &first).unwrap();
        assert!(prepare_at(&public, &private).unwrap().changed);
        assert!(!private.join("user.yaml").exists());
        assert!(private.join("addon.schema.yaml").is_file());
        std::fs::create_dir_all(private.join("build")).unwrap();
        std::fs::write(private.join("build/addon.schema.yaml"), b"compiled addon").unwrap();
        std::fs::create_dir_all(public.join(&second)).unwrap();
        std::fs::write(
            public.join(&second).join("keytao.schema.yaml"),
            b"updated schema",
        )
        .unwrap();
        std::fs::write(public.join("current.txt"), &second).unwrap();
        prepare_at(&public, &private).unwrap();
        assert!(!private.join("addon.schema.yaml").exists());
        assert!(!private.join("build/addon.schema.yaml").exists());
        assert_eq!(
            std::fs::read(private.join("keytao.userdb/LOG")).unwrap(),
            b"private words"
        );
        assert_eq!(
            std::fs::read(private.join("keytao.schema.yaml")).unwrap(),
            b"updated schema"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
