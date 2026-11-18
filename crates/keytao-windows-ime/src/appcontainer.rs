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
const LIBRIME_VERSION_FILE: &str = "build/keytao-librime-version.txt";

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
    pub(crate) fn mark_loaded(&self) -> std::result::Result<(), String> {
        std::fs::write(self.user_dir.join(SEED_MARKER), &self.generation)
            .map_err(|error| format!("record sandbox schema deployment: {error}"))
    }
}

pub(crate) fn prepare_data(
    librime_version: &str,
    shared_data_dir: &str,
) -> std::result::Result<SandboxData, String> {
    let desktop = local_app_data(KF_FLAG_NO_PACKAGE_REDIRECTION | KF_FLAG_DONT_VERIFY)
        .map_err(|error| format!("find public schema directory: {error}"))?;
    let private = local_app_data(KF_FLAG_FORCE_PACKAGE_REDIRECTION)
        .map_err(|error| format!("find AppContainer private directory: {error}"))?;
    if private == desktop {
        return Err("AppContainer data directory was not redirected".into());
    }
    let private = private.join("KeyTao");
    // Release every process-local session's mapped build files before replacing them.
    keytao_core::reinitialize(
        private.to_string_lossy().into_owned(),
        shared_data_dir.to_owned(),
    )?;
    prepare_at(
        &desktop.join("KeyTao/appcontainer-schemas"),
        &private,
        librime_version,
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

pub(crate) fn public_asset(path: &Path) -> bool {
    if path == Path::new(keytao_core::ACTIVE_SCHEME_MANIFEST)
        || path == Path::new(".keytao-current-schema")
        || path == Path::new(keytao_core::SCHEMA_SWITCH_REQUEST_FILE)
        || path == Path::new("build/.keytao-skipped-schemas.json")
    {
        return true;
    }
    if let Ok(relative) = path.strip_prefix(".keytao-package-lua") {
        let parts: Vec<_> = relative.components().collect();
        return parts.iter().all(|part| {
            matches!(part, Component::Normal(name)
            if !name.to_string_lossy().starts_with('.')
                && !name.to_string_lossy().to_ascii_lowercase().ends_with(".userdb")
                && !name.to_string_lossy().eq_ignore_ascii_case("log"))
        }) && (parts.len() < 2 || parts[1].as_os_str() == "lua");
    }
    !path.components().any(|part| {
        let part = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        part.starts_with('.') || part.ends_with(".userdb") || part == "log"
    }) && !matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("user.yaml" | "installation.yaml")
    )
}

pub(crate) fn fallback_directories(
    sandboxed: bool,
) -> std::result::Result<(PathBuf, PathBuf, PathBuf, Option<String>), String> {
    let desktop = local_app_data(KF_FLAG_NO_PACKAGE_REDIRECTION | KF_FLAG_DONT_VERIFY)
        .map_err(|error| format!("find public schema directory: {error}"))?;
    let temporary = if sandboxed {
        let private = local_app_data(KF_FLAG_FORCE_PACKAGE_REDIRECTION)
            .map_err(|error| format!("find AppContainer temporary directory: {error}"))?;
        if private == desktop {
            return Err("AppContainer temporary directory was not redirected".into());
        }
        private.join("Temp")
    } else {
        std::env::temp_dir()
    };
    let public = desktop.join("KeyTao/appcontainer-schemas");
    let path = public.join("current.txt");
    let signature = keytao_core::ReloadStamp::signature_at(&path);
    let generation = std::fs::read_to_string(&path)
        .map_err(|error| format!("read public schema generation: {error}"))?;
    let generation = generation.trim();
    if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid public schema generation".into());
    }
    Ok((temporary, public.join(generation), path, signature))
}

pub(crate) fn validate_prebuilt(
    root: &Path,
    librime_version: &str,
) -> std::result::Result<(), String> {
    for path in [
        root.to_owned(),
        root.join("build"),
        root.join(LIBRIME_VERSION_FILE),
    ] {
        let kind = std::fs::symlink_metadata(&path)
            .map_err(|_| "需要重新部署".to_string())?
            .file_type();
        if kind.is_symlink() {
            return Err("symbolic link in prebuilt schema data".into());
        }
    }
    let version = std::fs::read_to_string(root.join(LIBRIME_VERSION_FILE))
        .map_err(|_| "需要重新部署".to_string())?;
    if librime_version.is_empty() || version.trim() != librime_version {
        return Err("需要重新部署".into());
    }
    let state = keytao_core::schema_install_state(root);
    if !state.installed
        || !state.schemas.iter().all(|schema| {
            root.join("build")
                .join(format!("{schema}.schema.yaml"))
                .is_file()
        })
    {
        return Err("需要重新部署".into());
    }
    Ok(())
}

pub(crate) fn cache_directory(root: &Path, generation: &str, librime_version: &str) -> PathBuf {
    let version: String = librime_version
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    root.join(format!("{}-{version}", env!("CARGO_PKG_VERSION")))
        .join(generation)
}

pub(crate) fn prune_cache(root: &Path, active: Option<&Path>) {
    if active == Some(root)
        || !std::fs::symlink_metadata(root)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !public_asset(Path::new(&name))
            && name != SEED_MARKER
            && name != SEED_FILES
            && name != ".keytao-no-learning.version"
        {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && !kind.is_symlink() {
            prune_cache(&path, active);
        } else if kind.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
    // Windows may retain a mapped dictionary; retry cleanup on the next load.
    let _ = std::fs::remove_dir(root);
}

fn prune_legacy_cache(root: &Path) {
    if !std::fs::symlink_metadata(root)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return;
    }
    let mut files: Vec<String> = std::fs::read_to_string(root.join(SEED_FILES))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let build = root.join("build");
    if std::fs::symlink_metadata(&build)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        if let Ok(entries) = std::fs::read_dir(&build) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if [".schema.yaml", ".table.bin", ".prism.bin", ".reverse.bin"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                    || name == "keytao-librime-version.txt"
                {
                    files.push(format!("build/{name}"));
                }
            }
        }
    }
    for name in files {
        let Ok(relative) = checked_relative(&name) else {
            continue;
        };
        if !public_asset(&relative) {
            continue;
        }
        let mut path = root.to_owned();
        let safe = relative.components().all(|part| {
            path.push(part.as_os_str());
            std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| !metadata.file_type().is_symlink())
        });
        if safe && path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
    let _ = std::fs::remove_dir(build);
    let _ = std::fs::remove_dir(root);
}

fn prune_legacy_caches(private: &Path) {
    let Ok(entries) = std::fs::read_dir(private) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !entry
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        {
            continue;
        }
        if name == "tsf-cache" {
            prune_legacy_caches(&entry.path());
        } else if name.rsplit_once('-').is_some_and(|(version, hex)| {
            version.starts_with(|character: char| character.is_ascii_digit())
                && !hex.is_empty()
                && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            if let Ok(generations) = std::fs::read_dir(entry.path()) {
                for generation in generations.flatten() {
                    let name = generation.file_name();
                    let name = name.to_string_lossy();
                    if name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        prune_legacy_cache(&generation.path());
                    }
                }
            }
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

fn prepare_at(
    public_root: &Path,
    private: &Path,
    librime_version: &str,
) -> std::result::Result<SandboxData, String> {
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
    let source = public_root.join(&generation);
    validate_prebuilt(&source, librime_version)?;
    let changed = std::fs::read_to_string(private.join(SEED_MARKER))
        .ok()
        .as_deref()
        != Some(&generation)
        || validate_prebuilt(private, librime_version).is_err()
        || !keytao_core::schema_install_state(private).deployed
        || keytao_core::windows_rime_build_repair_required(private);
    let result = SandboxData {
        user_dir: private.to_owned(),
        reload_stamp_path,
        reload_stamp_signature,
        generation,
        changed,
    };
    if !changed {
        prune_legacy_caches(private);
        return Ok(result);
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
                    // Never overwrite another host's mapped dictionary in place.
                    keytao_core::write_file_atomic(&output, &bytes)
                        .map_err(|error| error.to_string())?;
                }
                copied.push(relative.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    copy_assets(&source, &source, private, &mut copied)?;
    keytao_core::rebase_active_scheme_manifest(private)?;
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
                // Prebuilt caches upgraded from the source-only manifest also have compiled files.
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
    keytao_core::write_file_atomic(&private.join(SEED_FILES), copied.join("\n").as_bytes())
        .map_err(|error| error.to_string())?;
    keytao_core::complete_deployment(private).map_err(|error| format!("需要重新部署: {error}"))?;
    keytao_core::mark_windows_rime_build_repair_complete(private)?;
    prune_legacy_caches(private);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &str = "1.0.0";

    #[test]
    fn sandbox_lua_snapshots_rebase_and_exclude_private_paths() {
        let root = tempfile::tempdir().unwrap();
        let public = root.path().join("public");
        let private = root.path().join("private");
        let generation = "c".repeat(64);
        let source = public.join(&generation);
        seed(&source);
        let snapshots = source.join(".keytao-package-lua/keytao/lua");
        std::fs::create_dir_all(&snapshots).unwrap();
        std::fs::write(snapshots.join("shared.lua"), "return 'A'").unwrap();
        let marker = r#"{"root":"","packages":{"keytao":{"schemas":["keytao"],"rime_lua":"owner = 'A'\n","lua_files":{"lua/shared.lua":".keytao-package-lua/keytao/lua/shared.lua"}}}}"#;
        std::fs::write(source.join(keytao_core::ACTIVE_SCHEME_MANIFEST), marker).unwrap();
        std::fs::write(source.join(".keytao-current-schema"), "keytao").unwrap();
        std::fs::write(source.join(".private.json"), "private").unwrap();
        std::fs::write(public.join("current.txt"), generation).unwrap();
        prepare_at(&public, &private, VERSION).unwrap();
        let relocated: serde_yaml::Value = serde_yaml::from_slice(
            &std::fs::read(private.join(keytao_core::ACTIVE_SCHEME_MANIFEST)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            relocated["root"].as_str(),
            private.canonicalize().unwrap().to_str()
        );
        assert_eq!(
            std::fs::read_to_string(private.join(".keytao-package-lua/keytao/lua/shared.lua"))
                .unwrap(),
            "return 'A'"
        );
        assert_eq!(
            std::fs::read_to_string(private.join(".keytao-current-schema")).unwrap(),
            "keytao"
        );
        assert!(!private.join(".private.json").exists());
        for path in [
            ".private.json",
            ".keytao-package-lua/keytao/app-state.json",
            ".keytao-package-lua/keytao/lua/../../secret",
            ".keytao-package-lua/keytao/lua/.private.json",
        ] {
            assert!(!public_asset(Path::new(path)), "{path}");
        }
    }

    fn seed(root: &Path) {
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join(LIBRIME_VERSION_FILE), format!("{VERSION}\n")).unwrap();
        std::fs::write(
            root.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao\n",
        )
        .unwrap();
        std::fs::write(root.join("keytao.schema.yaml"), b"public schema").unwrap();
        std::fs::write(root.join("build/keytao.schema.yaml"), b"compiled schema").unwrap();
        std::fs::write(root.join("build/keytao.table.bin"), b"compiled dictionary").unwrap();
    }

    #[test]
    fn sandbox_preserves_skipped_schemas_from_public_prebuilt_data() {
        let root = tempfile::tempdir().unwrap();
        let public = root.path().join("public");
        let private = root.path().join("private");
        let generation = "a".repeat(64);
        let source = public.join(&generation);
        seed(&source);
        std::fs::write(
            source.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao_broken\n    - schema: keytao\n",
        )
        .unwrap();
        std::fs::write(source.join("keytao_broken.schema.yaml"), b"broken schema").unwrap();
        std::fs::write(
            source.join("build/default.yaml"),
            "schema_list:\n  - schema: keytao\n",
        )
        .unwrap();
        let skipped = r#"{"keytao_broken":"方案 keytao_broken 编译失败，已跳过：copy on write failed; incompatible node type: commit"}"#;
        std::fs::write(source.join("build/.keytao-skipped-schemas.json"), skipped).unwrap();
        std::fs::write(source.join("build/.private.json"), b"must not copy").unwrap();
        std::fs::write(source.join(".private.json"), b"must not copy").unwrap();
        std::fs::write(public.join("current.txt"), &generation).unwrap();
        validate_prebuilt(&source, VERSION).unwrap();

        let prepared = prepare_at(&public, &private, VERSION).unwrap();
        assert_eq!(
            std::fs::read_to_string(private.join("build/.keytao-skipped-schemas.json")).unwrap(),
            skipped
        );
        assert!(!private.join("build/.private.json").exists());
        assert!(!private.join(".private.json").exists());
        assert!(!private.join("build/keytao_broken.schema.yaml").exists());
        let state = keytao_core::schema_install_state(&private);
        assert!(state.deployed);
        assert_eq!(state.schemas, ["keytao"]);
        assert_eq!(
            keytao_core::deployed_schema_id_from_dir(&private).as_deref(),
            Some("keytao")
        );
        assert!(keytao_core::deployment_warning(&private)
            .unwrap()
            .contains("keytao_broken"));
        validate_prebuilt(&private, VERSION).unwrap();
        prepared.mark_loaded().unwrap();
        assert!(!prepare_at(&public, &private, VERSION).unwrap().changed);
    }

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
        assert!(prepare_at(&public, &private, VERSION).is_err());
        let first = "a".repeat(64);
        std::fs::create_dir_all(public.join(&first)).unwrap();
        seed(&public.join(&first));
        std::fs::write(public.join(&first).join("keytao.schema.yaml"), b"first").unwrap();
        std::fs::write(public.join("current.txt"), &first).unwrap();
        let prepared = prepare_at(&public, &private, VERSION).unwrap();
        assert!(prepared.changed);
        // Copying prebuilt files is not proof that a runtime loaded them.
        assert!(prepare_at(&public, &private, VERSION).unwrap().changed);
        assert!(!prepared.user_dir.join(SEED_MARKER).exists());
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
    fn sandbox_generations_preserve_active_caches_and_private_learning() {
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
        seed(&public.join(&first));
        std::fs::create_dir_all(private.join("keytao.userdb")).unwrap();
        std::fs::write(private.join("keytao.userdb/LOG"), b"private words").unwrap();
        std::fs::write(
            public.join(&first).join("keytao.schema.yaml"),
            b"public schema",
        )
        .unwrap();
        std::fs::write(public.join(&first).join("addon.schema.yaml"), b"addon").unwrap();
        std::fs::write(public.join(&first).join("user.yaml"), b"must not copy").unwrap();
        let request = br#"{"generation":1,"options":{"mars":false}}"#;
        std::fs::write(
            public
                .join(&first)
                .join(keytao_core::SCHEMA_SWITCH_REQUEST_FILE),
            request,
        )
        .unwrap();
        std::fs::write(private.join(keytao_core::SCHEMA_SWITCH_APPLIED_FILE), b"1").unwrap();
        std::fs::write(
            public
                .join(&first)
                .join(keytao_core::SCHEMA_SWITCH_APPLIED_FILE),
            b"99",
        )
        .unwrap();
        std::fs::write(public.join("current.txt"), &first).unwrap();
        let prepared = prepare_at(&public, &private, VERSION).unwrap();
        let first_cache = prepared.user_dir.clone();
        assert!(prepared.changed);
        prepared.mark_loaded().unwrap();
        assert!(!prepare_at(&public, &private, VERSION).unwrap().changed);
        assert_eq!(
            std::fs::read(first_cache.join("build/keytao.table.bin")).unwrap(),
            b"compiled dictionary"
        );
        std::fs::remove_file(first_cache.join(LIBRIME_VERSION_FILE)).unwrap();
        assert!(prepare_at(&public, &private, VERSION).unwrap().changed);
        assert!(!first_cache.join("user.yaml").exists());
        assert_eq!(
            std::fs::read(first_cache.join(keytao_core::SCHEMA_SWITCH_REQUEST_FILE)).unwrap(),
            request
        );
        assert_eq!(
            std::fs::read(private.join(keytao_core::SCHEMA_SWITCH_APPLIED_FILE)).unwrap(),
            b"1"
        );
        assert!(first_cache.join("addon.schema.yaml").is_file());
        std::fs::write(
            first_cache.join("build/addon.schema.yaml"),
            b"compiled addon",
        )
        .unwrap();
        std::fs::write(first_cache.join("keytao.userdb/learned"), b"host learning").unwrap();
        let active_dictionary =
            std::fs::File::open(first_cache.join("build/keytao.table.bin")).unwrap();
        let legacy_cache = cache_directory(&private.join("tsf-cache"), &first, VERSION);
        seed(&legacy_cache);
        std::fs::create_dir_all(legacy_cache.join("keytao.userdb")).unwrap();
        std::fs::write(legacy_cache.join("keytao.userdb/LOG"), b"legacy learning").unwrap();
        std::fs::write(legacy_cache.join("custom-notes.txt"), b"private notes").unwrap();
        std::fs::create_dir_all(public.join(&second)).unwrap();
        seed(&public.join(&second));
        std::fs::write(
            public.join(&second).join("keytao.schema.yaml"),
            b"updated schema",
        )
        .unwrap();
        std::fs::write(public.join("current.txt"), &second).unwrap();
        let updated = prepare_at(&public, &private, VERSION).unwrap();
        assert_eq!(first_cache, updated.user_dir);
        assert_eq!(updated.user_dir, private);
        assert!(!updated
            .user_dir
            .join(keytao_core::SCHEMA_SWITCH_REQUEST_FILE)
            .exists());
        assert_eq!(
            std::fs::read(private.join(keytao_core::SCHEMA_SWITCH_APPLIED_FILE)).unwrap(),
            b"1"
        );
        assert!(!updated.user_dir.join("addon.schema.yaml").exists());
        assert!(!updated.user_dir.join("build/addon.schema.yaml").exists());
        assert_eq!(
            std::fs::read(updated.user_dir.join("keytao.userdb/learned")).unwrap(),
            b"host learning"
        );
        assert!(!legacy_cache.join("build").exists());
        assert_eq!(
            std::fs::read(legacy_cache.join("keytao.userdb/LOG")).unwrap(),
            b"legacy learning"
        );
        assert_eq!(
            std::fs::read(legacy_cache.join("custom-notes.txt")).unwrap(),
            b"private notes"
        );
        assert_eq!(
            std::fs::read(private.join("keytao.userdb/LOG")).unwrap(),
            b"private words"
        );
        assert_eq!(
            std::fs::read(updated.user_dir.join("keytao.schema.yaml")).unwrap(),
            b"updated schema"
        );
        drop(active_dictionary);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_directory_isolates_generation_and_librime_version() {
        let root = Path::new("cache");
        let first = "1".repeat(64);
        let second = "2".repeat(64);
        let original = cache_directory(root, &first, VERSION);
        assert_ne!(original, cache_directory(root, &second, VERSION));
        assert_ne!(original, cache_directory(root, &first, "other/version"));
        assert_eq!(original, cache_directory(root, &first, VERSION));
        assert!(cache_directory(root, &first, "other/version").starts_with(root));
        assert_eq!(original.file_name().unwrap(), first.as_str());
    }

    #[test]
    fn cache_pruning_does_not_follow_symbolic_links() {
        #[cfg(unix)]
        use std::os::unix::fs::symlink as symlink_dir;
        #[cfg(target_os = "windows")]
        use std::os::windows::fs::symlink_dir;

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        seed(&target);
        let linked = root.path().join("linked");
        match symlink_dir(&target, &linked) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("create cache symlink: {error}"),
        }
        prune_cache(&linked, None);
        prune_legacy_cache(&linked);
        let legacy = root.path().join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        symlink_dir(&target, legacy.join("nested")).unwrap();
        std::fs::write(legacy.join(SEED_FILES), "nested/build/keytao.table.bin").unwrap();
        prune_legacy_cache(&legacy);
        assert_eq!(
            std::fs::read(target.join("build/keytao.table.bin")).unwrap(),
            b"compiled dictionary"
        );
    }

    #[test]
    fn cache_pruning_keeps_active_data_and_retries_mapped_files() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let root = tempfile::tempdir().unwrap();
        let first = cache_directory(root.path(), &"1".repeat(64), VERSION);
        let active = cache_directory(root.path(), &"2".repeat(64), VERSION);
        seed(&first);
        seed(&active);
        let mapped = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(first.join("build/keytao.table.bin"))
            .unwrap();
        prune_cache(root.path(), Some(&active));
        assert!(first.join("build/keytao.table.bin").exists());
        assert!(active.join("build/keytao.table.bin").exists());
        drop(mapped);
        prune_cache(root.path(), Some(&active));
        assert!(!first.exists());
        assert!(active.join("build/keytao.table.bin").exists());
    }

    #[test]
    fn missing_or_mismatched_prebuilt_version_requires_redeployment() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            validate_prebuilt(root.path(), VERSION).unwrap_err(),
            "需要重新部署"
        );
        seed(root.path());
        validate_prebuilt(root.path(), VERSION).unwrap();
        assert_eq!(
            validate_prebuilt(root.path(), "different").unwrap_err(),
            "需要重新部署"
        );
        std::fs::remove_file(root.path().join("build/keytao.schema.yaml")).unwrap();
        assert_eq!(
            validate_prebuilt(root.path(), VERSION).unwrap_err(),
            "需要重新部署"
        );
    }
}
