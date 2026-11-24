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
const PACKAGE_CACHE_POINTER: &str = "current.txt";
#[cfg(test)]
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

pub(crate) fn public_schema_root() -> Result<PathBuf> {
    local_app_data(KF_FLAG_NO_PACKAGE_REDIRECTION | KF_FLAG_DONT_VERIFY)
        .map(|desktop| desktop.join("KeyTao/appcontainer-schemas"))
}

pub(crate) struct SandboxData {
    pub(crate) user_dir: PathBuf,
    cache_dir: PathBuf,
    pub(crate) reload_stamp_path: PathBuf,
    pub(crate) reload_stamp_signature: Option<String>,
    generation: String,
    active_package_key: Option<String>,
    pub(crate) changed: bool,
}

impl SandboxData {
    pub(crate) fn mark_loaded(&self) -> std::result::Result<(), String> {
        if self.active_package_key.is_some() {
            let path = self.cache_dir.with_file_name(PACKAGE_CACHE_POINTER);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(self.generation.as_str()) {
                keytao_core::write_file_atomic(&path, self.generation.as_bytes())
                    .map_err(|error| format!("record package schema cache: {error}"))?;
            }
        }
        if self.changed {
            std::fs::write(
                self.cache_dir.join(SEED_MARKER),
                seed_generation(&self.generation, self.active_package_key.as_deref()),
            )
            .map_err(|error| format!("record sandbox schema deployment: {error}"))?;
        }
        Ok(())
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
    prepare_at_with(
        &desktop.join("KeyTao/appcontainer-schemas"),
        &private,
        librime_version,
        |user_dir, isolated| {
            if isolated {
                keytao_core::ImeRuntime::with_dirs(user_dir, shared_data_dir).shutdown()
            } else {
                // Release every process-local session's mapped build files before replacing them.
                keytao_core::reinitialize(
                    user_dir.to_string_lossy().into_owned(),
                    shared_data_dir.to_owned(),
                )
            }
        },
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
    public_asset_at(path, None)
}

fn public_asset_at(path: &Path, staging: Option<&Path>) -> bool {
    if path == Path::new(keytao_core::ACTIVE_SCHEME_MANIFEST)
        || path == Path::new(".keytao-current-schema")
        || path == Path::new(keytao_core::SCHEMA_SWITCH_REQUEST_FILE)
        || (path
            .file_name()
            .is_some_and(|name| name == ".keytao-skipped-schemas.json")
            && path.parent().is_some_and(|parent| {
                let root = Path::new("");
                staging.map_or_else(
                    || parent == keytao_core::RuntimeDirs::flat(root, root).staging_dir,
                    |staging| parent == staging,
                )
            }))
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
    let dirs = keytao_core::RuntimeDirs::flat(root, root);
    let version_file = dirs.staging_dir.join("keytao-librime-version.txt");
    for path in [
        dirs.user_data_dir.clone(),
        dirs.staging_dir.clone(),
        version_file.clone(),
    ] {
        let kind = std::fs::symlink_metadata(&path)
            .map_err(|_| "需要重新部署".to_string())?
            .file_type();
        if kind.is_symlink() {
            return Err("symbolic link in prebuilt schema data".into());
        }
    }
    let version = std::fs::read_to_string(version_file).map_err(|_| "需要重新部署".to_string())?;
    if librime_version.is_empty() || version.trim() != librime_version {
        return Err("需要重新部署".into());
    }
    let state = keytao_core::schema_install_state(root);
    if !state.installed
        || !state.schemas.iter().all(|schema| {
            dirs.staging_dir
                .join(format!("{schema}.schema.yaml"))
                .is_file()
        })
    {
        return Err("需要重新部署".into());
    }
    Ok(())
}

pub(crate) fn public_package_key(root: &Path) -> std::result::Result<Option<String>, String> {
    let path = root.join(keytao_core::RuntimeDirs::PUBLIC_PACKAGE_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.is_file() => {}
        _ => return Err("invalid public package metadata".into()),
    }
    let key = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    if keytao_core::RuntimeDirs::isolated_user_data_dir(root, &key).is_none() {
        return Err("invalid public package key".into());
    }
    Ok(Some(key))
}

#[cfg(test)]
pub(crate) fn cache_directory(root: &Path, generation: &str, librime_version: &str) -> PathBuf {
    cache_directory_for_package(root, generation, librime_version, None)
}

pub(crate) fn cache_directory_for_package(
    root: &Path,
    generation: &str,
    librime_version: &str,
    active_package_key: Option<&str>,
) -> PathBuf {
    let version: String = librime_version
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut path = root.join(format!("{}-{version}", env!("CARGO_PKG_VERSION")));
    if let Some(key) = active_package_key {
        let key: String = key.bytes().map(|byte| format!("{byte:02x}")).collect();
        path.push(format!("package-{key}"));
    }
    path.join(generation)
}

fn seed_generation(generation: &str, active_package_key: Option<&str>) -> String {
    match active_package_key {
        Some(key) => format!("{generation}\n{key}"),
        None => generation.to_owned(),
    }
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
    if let Ok(dirs) = keytao_core::RuntimeDirs::resolve(root, root) {
        prune_legacy_cache_at(root, &dirs.staging_dir);
    }
}

fn prune_legacy_cache_at(root: &Path, staging_dir: &Path) {
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
    let safe_path = |relative: &Path| {
        let mut path = root.to_owned();
        let safe = relative.components().all(|part| {
            let Component::Normal(name) = part else {
                return false;
            };
            path.push(name);
            std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| !metadata.file_type().is_symlink())
        });
        safe.then_some(path)
    };
    let build = staging_dir.strip_prefix(root).ok().filter(|relative| {
        relative.file_name().is_some() && relative.parent().and_then(safe_path).is_some()
    });
    if let Some(relative) = build.filter(|_| {
        std::fs::symlink_metadata(staging_dir)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    }) {
        if let Ok(entries) = std::fs::read_dir(staging_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if [".schema.yaml", ".table.bin", ".prism.bin", ".reverse.bin"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                    || name == "keytao-librime-version.txt"
                {
                    files.push(relative.join(&*name).to_string_lossy().into_owned());
                }
            }
        }
    }
    for name in files {
        let Ok(relative) = checked_relative(&name) else {
            continue;
        };
        if !public_asset_at(&relative, build) {
            continue;
        }
        if let Some(path) = safe_path(&relative).filter(|path| path.is_file()) {
            let _ = std::fs::remove_file(path);
        }
    }
    if build.is_some() {
        let _ = std::fs::remove_dir(staging_dir);
    }
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

#[cfg(test)]
fn prepare_at(
    public_root: &Path,
    private: &Path,
    librime_version: &str,
) -> std::result::Result<SandboxData, String> {
    prepare_at_with(public_root, private, librime_version, |_, _| Ok(()))
}

fn prepare_at_with(
    public_root: &Path,
    private: &Path,
    librime_version: &str,
    before_copy: impl FnOnce(&Path, bool) -> std::result::Result<(), String>,
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
    let source_root = public_root.join(&generation);
    let active_package_key = public_package_key(&source_root)?;
    let cache = active_package_key.as_ref().map(|key| {
        cache_directory_for_package(
            &private.join("tsf-cache"),
            &generation,
            librime_version,
            Some(key),
        )
    });
    let private = cache.as_deref().unwrap_or(private);
    let user_dir = sandbox_user_dir(private)?;
    validate_prebuilt(&source_root, librime_version)?;
    let seed_generation = seed_generation(&generation, active_package_key.as_deref());
    let changed = std::fs::read_to_string(private.join(SEED_MARKER))
        .ok()
        .as_deref()
        != Some(seed_generation.as_str())
        || validate_prebuilt(&user_dir, librime_version).is_err()
        || !keytao_core::schema_build_is_available(&user_dir)
        || keytao_core::windows_rime_build_repair_required(&user_dir);
    if active_package_key.is_none() || changed {
        before_copy(&user_dir, active_package_key.is_some())?;
    }
    let result = SandboxData {
        user_dir,
        cache_dir: private.to_owned(),
        reload_stamp_path,
        reload_stamp_signature,
        generation,
        active_package_key,
        changed,
    };
    if !changed {
        prune_legacy_caches(private);
        return Ok(result);
    }
    // First deployments write only into the package's own private directory.
    let user_dir = &result.user_dir;
    std::fs::create_dir_all(user_dir)
        .map_err(|error| format!("create sandbox schema cache: {error}"))?;
    let selection = user_dir.join("user.yaml");
    if result.active_package_key.is_some() && !selection.exists() {
        restore_cached_schema_selection(private, &selection)?;
    }
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
    copy_assets(&source_root, &source_root, user_dir, &mut copied)?;
    keytao_core::rebase_active_scheme_manifest(user_dir)?;
    let dirs = keytao_core::RuntimeDirs::flat(user_dir, user_dir);
    if let Ok(previous) = std::fs::read_to_string(private.join(SEED_FILES)) {
        for name in previous
            .lines()
            .filter(|name| !copied.iter().any(|current| current == name))
        {
            let relative = checked_relative(name)?;
            if public_asset(&relative) {
                let path = user_dir.join(&relative);
                if path.is_file() {
                    std::fs::remove_file(path).map_err(|error| error.to_string())?;
                }
                // Prebuilt caches upgraded from the source-only manifest also have compiled files.
                if relative.components().count() == 1
                    && relative.to_string_lossy().ends_with(".schema.yaml")
                {
                    let compiled = dirs.staging_dir.join(&relative);
                    if compiled.is_file() {
                        std::fs::remove_file(compiled).map_err(|error| error.to_string())?;
                    }
                }
            }
        }
    }
    keytao_core::write_file_atomic(&private.join(SEED_FILES), copied.join("\n").as_bytes())
        .map_err(|error| error.to_string())?;
    keytao_core::complete_deployment(user_dir).map_err(|error| format!("需要重新部署: {error}"))?;
    keytao_core::mark_windows_rime_build_repair_complete(user_dir)?;
    prune_legacy_caches(private);
    Ok(result)
}

fn sandbox_user_dir(cache: &Path) -> std::result::Result<PathBuf, String> {
    match std::fs::symlink_metadata(cache) {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(cache.to_owned()),
        _ => return Err("invalid sandbox schema cache directory".into()),
    }
    // Keep any previously used runtime in place, including its live user dictionaries.
    let dirs = keytao_core::RuntimeDirs::resolve(cache, cache)?;
    if dirs.user_data_dir.is_dir() {
        Ok(dirs.user_data_dir)
    } else {
        Ok(cache.to_owned())
    }
}

fn restore_cached_schema_selection(
    cache: &Path,
    selection: &Path,
) -> std::result::Result<(), String> {
    use std::io::{ErrorKind, Write};

    let previous = cache.with_file_name(PACKAGE_CACHE_POINTER);
    let Ok(previous) = std::fs::read_to_string(previous) else {
        return Ok(());
    };
    if previous.len() != 64 || !previous.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid package schema cache generation".into());
    }
    let previous = sandbox_user_dir(&cache.with_file_name(previous))?.join("user.yaml");
    let content = match std::fs::read(previous) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let result =
        tempfile::NamedTempFile::new_in(selection.parent().unwrap()).and_then(|mut file| {
            file.write_all(&content)?;
            file.as_file().sync_all()?;
            file.persist_noclobber(selection)
                .map_err(|error| error.error)
        });
    match result {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("preserve sandbox schema selection: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &str = "1.0.0";

    #[test]
    fn switching_back_reuses_the_package_cache_and_its_schema_selection() {
        let root = tempfile::tempdir().unwrap();
        let public = root.path().join("public");
        let private = root.path().join("private");
        let first = "a".repeat(64);
        let second = "b".repeat(64);
        let updated_first = "c".repeat(64);
        for (generation, key) in [
            (&first, "first"),
            (&second, "second"),
            (&updated_first, "first"),
        ] {
            let source = public.join(generation);
            seed(&source);
            std::fs::write(
                source.join(keytao_core::RuntimeDirs::PUBLIC_PACKAGE_FILE),
                key,
            )
            .unwrap();
        }
        let pointer = public.join("current.txt");
        std::fs::write(&pointer, &first).unwrap();
        let first_cache = prepare_at(&public, &private, VERSION).unwrap();
        assert!(first_cache.user_dir.join(LIBRIME_VERSION_FILE).is_file());
        first_cache.mark_loaded().unwrap();
        let selection = first_cache.user_dir.join("user.yaml");
        std::fs::write(&selection, "var: {previously_selected_schema: first}").unwrap();

        std::fs::write(&pointer, &second).unwrap();
        let second_cache = prepare_at(&public, &private, VERSION).unwrap();
        assert_ne!(first_cache.user_dir, second_cache.user_dir);
        assert!(!second_cache.user_dir.join("user.yaml").exists());

        std::fs::write(&pointer, &first).unwrap();
        let cached = prepare_at(&public, &private, VERSION).unwrap();
        assert!(!cached.changed);
        assert_eq!(cached.user_dir, first_cache.user_dir);
        assert_eq!(
            std::fs::read_to_string(selection).unwrap(),
            "var: {previously_selected_schema: first}"
        );

        std::fs::write(&pointer, &updated_first).unwrap();
        let rebuilt = prepare_at(&public, &private, VERSION).unwrap();
        assert!(rebuilt.changed);
        assert_eq!(
            std::fs::read_to_string(rebuilt.user_dir.join("user.yaml")).unwrap(),
            "var: {previously_selected_schema: first}"
        );
    }

    #[test]
    fn nested_cache_keeps_learning_and_passes_selection_to_the_next_generation() {
        let root = tempfile::tempdir().unwrap();
        let public = root.path().join("public");
        let private = root.path().join("private");
        let first = "a".repeat(64);
        let second = "b".repeat(64);
        for generation in [&first, &second] {
            let source = public.join(generation);
            seed(&source);
            std::fs::write(
                source.join(keytao_core::RuntimeDirs::PUBLIC_PACKAGE_FILE),
                "keytao",
            )
            .unwrap();
        }

        let cache = cache_directory_for_package(
            &private.join("tsf-cache"),
            &first,
            VERSION,
            Some("keytao"),
        );
        let nested = cache.join("runtime/.unselected");
        seed(&nested);
        let selection = "var: {previously_selected_schema: keytao}";
        std::fs::write(nested.join("user.yaml"), selection).unwrap();
        std::fs::write(nested.join("installation.yaml"), "installation_id: sandbox").unwrap();
        std::fs::create_dir(nested.join("keytao.userdb")).unwrap();
        let learning = nested.join("keytao.userdb/LOG");
        std::fs::write(&learning, b"learned words").unwrap();
        keytao_core::complete_deployment(&nested).unwrap();
        keytao_core::mark_windows_rime_build_repair_complete(&cache).unwrap();
        std::fs::write(
            cache.join(SEED_MARKER),
            seed_generation(&first, Some("keytao")),
        )
        .unwrap();

        let pointer = public.join("current.txt");
        std::fs::write(&pointer, &first).unwrap();
        let prepared = prepare_at(&public, &private, VERSION).unwrap();
        assert_eq!(prepared.user_dir, nested);
        assert!(prepared.changed);
        prepared.mark_loaded().unwrap();
        assert!(!prepare_at(&public, &private, VERSION).unwrap().changed);
        assert_eq!(std::fs::read(&learning).unwrap(), b"learned words");
        assert_eq!(
            std::fs::read_to_string(nested.join("installation.yaml")).unwrap(),
            "installation_id: sandbox"
        );

        std::fs::write(pointer, &second).unwrap();
        let updated = prepare_at(&public, &private, VERSION).unwrap();
        assert_eq!(
            std::fs::read_to_string(updated.user_dir.join("user.yaml")).unwrap(),
            selection
        );
        assert!(!updated.user_dir.join("keytao.userdb").exists());
    }

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
    fn cache_identity_includes_package_key_and_preserves_flat_paths() {
        let root = Path::new("cache");
        let generation = "1".repeat(64);
        let legacy = root
            .join(format!("{}-312e302e30", env!("CARGO_PKG_VERSION")))
            .join(&generation);
        assert_eq!(
            cache_directory_for_package(root, &generation, VERSION, None),
            legacy
        );
        let first = cache_directory_for_package(root, &generation, VERSION, Some("cloud/a"));
        let second = cache_directory_for_package(root, &generation, VERSION, Some("cloud/b"));
        assert_ne!(first, second);
        assert_ne!(first, legacy);
        assert!(first.starts_with(root));
        assert_eq!(seed_generation(&generation, None), generation);
        assert_ne!(
            seed_generation(&generation, Some("cloud/a")),
            seed_generation(&generation, Some("cloud/b")),
        );
    }

    #[test]
    fn legacy_cache_prunes_compiled_files_from_resolved_staging() {
        let root = tempfile::tempdir().unwrap();
        let flat = root.path().join("build");
        let staging = root.path().join("runtime/keytao/build");
        for dir in [&flat, &staging] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("obsolete.table.bin"), b"compiled dictionary").unwrap();
        }

        prune_legacy_cache_at(root.path(), &staging);

        assert!(!staging.exists());
        assert!(flat.join("obsolete.table.bin").is_file());
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
