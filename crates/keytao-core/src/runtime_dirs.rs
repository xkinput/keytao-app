use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

pub(crate) const PREVIOUS_BUILD: &str = "previous-build";
pub(crate) const BUILD_GENERATION_MARKER: &str = ".keytao-build-current";
pub(crate) const BUILD_GENERATION_PREFIX: &str = ".keytao-build-";
static LOGGED_RESOLVE_FAILURES: LazyLock<Mutex<HashSet<(PathBuf, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub(crate) fn read_regular_file(path: &Path) -> Option<Vec<u8>> {
    fs::symlink_metadata(path).ok()?.is_file().then_some(())?;
    fs::read(path).ok()
}

/// librime directories resolved from the frontend's unchanged control root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeDirs {
    pub user_data_dir: PathBuf,
    pub shared_data_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub prebuilt_data_dir: PathBuf,
    pub active_package_key: Option<String>,
}

impl RuntimeDirs {
    pub const SETTINGS_FILE: &str = "keytao-runtime-settings.json";
    pub const MANIFEST_FILE: &str = ".keytao-runtime.json";
    pub const MIGRATION_MARKER: &str = "runtime/.keytao-migration.json";
    pub const PUBLIC_PACKAGE_FILE: &str = ".keytao-public-package";

    pub fn isolation_enabled(control_root: &Path) -> bool {
        let environment_enabled = match std::env::var("KEYTAO_RUNTIME_ISOLATION").as_deref() {
            Ok("0" | "false") => return false,
            Ok("1" | "true") => true,
            _ => false,
        };
        read_regular_file(&control_root.join(Self::SETTINGS_FILE))
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|settings| settings["runtime_isolation"].as_bool())
            .unwrap_or(environment_enabled)
    }

    pub fn resolve(control_root: &Path, shared: &Path) -> Result<Self, String> {
        if Self::isolation_enabled(control_root) {
            if let Some(key) = Self::active_package_key(control_root) {
                return Self::resolve_package(control_root, shared, &key).map_err(|reason| {
                    let error = format!(
                        "cannot resolve isolated runtime for active package {key:?}: {reason}"
                    );
                    if crate::lock_ignore_poison(&LOGGED_RESOLVE_FAILURES)
                        .insert((control_root.to_path_buf(), key.clone()))
                    {
                        crate::rt_log!(
                            crate::Level::Info,
                            "error",
                            "runtime_resolve_failed",
                            root = control_root.to_string_lossy(),
                            error = &error
                        );
                        eprintln!("KeyTao {}: {error}", control_root.display());
                    }
                    error
                });
            }
        }
        Ok(Self::flat(control_root, shared))
    }

    pub fn active_package_key(control_root: &Path) -> Option<String> {
        let migration = Self::migration_marker(control_root);
        let version = migration.as_ref().and_then(|marker| marker["version"].as_u64());
        Self::package_stores(control_root)
            .find_map(|store| manifest_package_key(&store, version))
            .or_else(|| {
                let marker = migration?;
                marker["runtime_package"].as_str().map(str::to_owned)
            })
    }

    pub(crate) fn migration_version(control_root: &Path) -> Option<u64> {
        Self::migration_marker(control_root)?["version"].as_u64()
    }

    pub fn migration_complete(control_root: &Path) -> bool {
        Self::migration_version(control_root).is_some()
    }

    fn migration_marker(control_root: &Path) -> Option<serde_json::Value> {
        let path = control_root.join(Self::MIGRATION_MARKER);
        fs::symlink_metadata(path.parent()?).ok()?.is_dir().then_some(())?;
        let marker: serde_json::Value = serde_json::from_slice(&read_regular_file(&path)?).ok()?;
        (marker["version"].as_u64()? == 1).then_some(marker)
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    pub(crate) fn flat_active_package_key(control_root: &Path) -> Option<String> {
        let local = control_root.join(crate::ACTIVE_SCHEME_MANIFEST);
        Self::package_stores(control_root)
            .map(|store| store.join("active-manifest.json"))
            .chain([local])
            .find_map(|path| {
                let manifest: serde_json::Value =
                    serde_json::from_slice(&read_regular_file(&path)?).ok()?;
                ["scheme_key", "key", "active_package"]
                    .iter()
                    .filter_map(|field| manifest[*field].as_str())
                    .find(|key| !key.is_empty())
                    .map(str::to_owned)
            })
    }

    pub fn for_package(control_root: &Path, shared: &Path, key: &str) -> Option<Self> {
        if !Self::isolation_enabled(control_root) {
            return None;
        }
        Self::resolve_package(control_root, shared, key).ok()
    }

    fn resolve_package(control_root: &Path, shared: &Path, key: &str) -> Result<Self, String> {
        if key.starts_with("builtin/") {
            return Self::isolated(control_root, shared, None, key);
        }
        Self::isolated_user_data_dir(control_root, key)
            .ok_or("invalid package key or runtime directory")?;
        let mut stores = Self::package_stores(control_root);
        let first = stores.next().ok_or("missing package store")?;
        let result = Self::isolated(control_root, shared, Some(&first), key);
        if result.is_err() && !package_exists(&first, key) {
            for store in stores {
                if package_exists(&store, key) {
                    return Self::isolated(control_root, shared, Some(&store), key);
                }
            }
        }
        result
    }

    pub fn for_package_in_store(
        control_root: &Path,
        shared: &Path,
        data_root: &Path,
        key: &str,
    ) -> Option<Self> {
        if !Self::isolation_enabled(control_root) {
            return None;
        }
        let store = crate::package_store::root(data_root);
        fs::symlink_metadata(&store).ok()?.is_dir().then_some(())?;
        Self::isolated(control_root, shared, Some(&store), key).ok()
    }

    pub(crate) fn package_stores(control_root: &Path) -> impl Iterator<Item = PathBuf> {
        let app_root = (crate::default_user_data_dir().as_deref() == Some(control_root))
            .then(|| dirs::data_local_dir().map(|root| root.join("ink.rea.keytao-app")))
            .flatten();
        std::iter::once(crate::package_store::root(control_root))
            .chain(app_root.map(|root| crate::package_store::root(&root)))
            .filter(|store| fs::symlink_metadata(store).is_ok_and(|entry| entry.is_dir()))
    }

    pub(crate) fn package_shared(&self) -> Option<&Path> {
        self.active_package_key
            .as_ref()
            .map(|_| self.shared_data_dir.as_path())
    }

    pub fn isolated_user_data_dir(control_root: &Path, key: &str) -> Option<PathBuf> {
        let name = key.strip_prefix("builtin/").unwrap_or(key);
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return None;
        }
        let mut path = control_root.to_path_buf();
        for part in std::iter::once("runtime").chain(key.split('/')) {
            path.push(part);
            if fs::symlink_metadata(&path).is_ok_and(|entry| !entry.is_dir()) {
                return None;
            }
        }
        Some(path)
    }

    fn isolated(
        control_root: &Path,
        shared: &Path,
        store: Option<&Path>,
        key: &str,
    ) -> Result<Self, String> {
        let user_data_dir = Self::isolated_user_data_dir(control_root, key)
            .ok_or("invalid package key or runtime directory")?;
        let staging_dir = runtime_build_dir(&user_data_dir)?;
        let runtime_prebuilt = user_data_dir.join("prebuilt");
        if [&staging_dir, &runtime_prebuilt]
            .into_iter()
            .any(|path| fs::symlink_metadata(path).is_ok_and(|entry| !entry.is_dir()))
        {
            return Err("runtime build or prebuilt directory is invalid".into());
        }
        let shared_data_dir = if key.starts_with("builtin/") {
            // The control root can still contain the legacy layout kept for rollback.
            if shared == control_root || shared.as_os_str().is_empty() {
                let bundled = control_root.join("rime-data");
                if bundled.join("default.yaml").is_file() {
                    bundled
                } else {
                    PathBuf::from(crate::default_shared_data_dir())
                }
            } else {
                shared.to_path_buf()
            }
        } else {
            package_files_dir(store.ok_or("missing package store")?, key)?
        };
        let package_build = shared_data_dir.join("build");
        let prebuilt_data_dir = if package_build.is_dir() {
            package_build
        } else {
            runtime_prebuilt
        };
        Ok(Self {
            staging_dir,
            user_data_dir,
            shared_data_dir,
            prebuilt_data_dir,
            active_package_key: Some(key.to_owned()),
        })
    }

    pub fn flat(control_root: &Path, shared: &Path) -> Self {
        Self {
            user_data_dir: control_root.to_path_buf(),
            shared_data_dir: shared.to_path_buf(),
            staging_dir: control_root.join("build"),
            prebuilt_data_dir: shared.join(if control_root == shared {
                "prebuilt"
            } else {
                "build"
            }),
            active_package_key: None,
        }
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    pub(crate) fn with_rime_traits<T>(
        &self,
        action: impl FnOnce(librime_sys::RimeTraits) -> T,
    ) -> T {
        use std::ffi::CString;
        let user = CString::new(self.user_data_dir.to_string_lossy().as_bytes())
            .expect("valid user data dir");
        let shared = CString::new(self.shared_data_dir.to_string_lossy().as_bytes())
            .expect("valid shared data dir");
        let staging =
            CString::new(self.staging_dir.to_string_lossy().as_bytes()).expect("valid staging dir");
        let prebuilt = CString::new(self.prebuilt_data_dir.to_string_lossy().as_bytes())
            .expect("valid prebuilt data dir");
        librime_sys::rime_struct!(traits: librime_sys::RimeTraits);
        traits.user_data_dir = user.as_ptr();
        traits.shared_data_dir = shared.as_ptr();
        traits.staging_dir = staging.as_ptr();
        traits.prebuilt_data_dir = prebuilt.as_ptr();
        // Keep directory strings alive until librime has consumed the traits.
        action(traits)
    }
}

fn package_files_dir(store: &Path, key: &str) -> Result<PathBuf, String> {
    let primary = if key.starts_with("local-") {
        "local"
    } else {
        "cloud"
    };
    let invalid = |path: &Path| {
        format!(
            "package files directory is missing, inaccessible or symlinked: {}",
            path.display()
        )
    };
    'namespace: for namespace in [primary, "addons"] {
        let mut package = store.to_path_buf();
        for part in [namespace, key] {
            package.push(part);
            match fs::symlink_metadata(&package) {
                Ok(metadata) if metadata.is_dir() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue 'namespace,
                _ => return Err(invalid(&package)),
            }
        }
        // Match materialization's preference for the primary package if it exists.
        let files = package.join("files");
        return if fs::symlink_metadata(&files).is_ok_and(|metadata| metadata.is_dir()) {
            Ok(files)
        } else {
            Err(invalid(&files))
        };
    }
    Err(invalid(&store.join(primary).join(key).join("files")))
}

fn manifest_package_key(store: &Path, migration_version: Option<u64>) -> Option<String> {
    let bytes = read_regular_file(&store.join("active-manifest.json"))?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if migration_version.is_some() && manifest["runtime_migration_version"].as_u64() != migration_version {
        return None;
    }
    let key = manifest["active_package"].as_str()?;
    Some(key.to_owned())
}

fn package_exists(store: &Path, key: &str) -> bool {
    [if key.starts_with("local-") { "local" } else { "cloud" }, "addons"]
        .into_iter()
        .any(|namespace| {
            let path = store.join(namespace);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => fs::symlink_metadata(path.join(key))
                    .err()
                    .is_none_or(|error| error.kind() != std::io::ErrorKind::NotFound),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                _ => true,
            }
        })
}

pub(crate) fn runtime_build_dir(user_dir: &Path) -> Result<PathBuf, String> {
    let marker = user_dir.join(BUILD_GENERATION_MARKER);
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.is_file() => {
            let name = fs::read_to_string(&marker).map_err(|error| error.to_string())?;
            let valid = name.strip_prefix(BUILD_GENERATION_PREFIX).is_some_and(|suffix| {
                !suffix.is_empty()
                    && suffix.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-')
            });
            let path = user_dir.join(&name);
            if !valid || !fs::symlink_metadata(&path).is_ok_and(|entry| entry.is_dir()) {
                return Err("invalid package build generation".into());
            }
            return Ok(path);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("invalid package build pointer".into()),
    }
    let build = user_dir.join("build");
    let previous = user_dir.join(PREVIOUS_BUILD);
    // Two renames may leave only the preserved build if the deployer exits between them.
    if fs::symlink_metadata(&build).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && fs::symlink_metadata(&previous).is_ok_and(|entry| entry.is_dir())
    {
        return Ok(previous);
    }
    Ok(build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_layout_matches_platform_golden_paths() {
        #[cfg(target_os = "macos")]
        let (root, shared) = ("/sample/Library/keytao", "/sample/Resources/rime-data");
        #[cfg(target_os = "windows")]
        let (root, shared) = ("C:/sample/keytao", "C:/sample/rime-data");
        #[cfg(target_os = "linux")]
        let (root, shared) = ("/sample/.local/share/keytao", "/sample/share/rime-data");
        #[cfg(target_os = "android")]
        let (root, shared) = (
            "/sample/android/files/keytao",
            "/sample/android/files/rime-data",
        );
        #[cfg(target_os = "ios")]
        let (root, shared) = ("/sample/ios/group/keytao", "/sample/ios/rime-data");
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "android",
            target_os = "ios"
        )))]
        let (root, shared) = ("sample/keytao", "sample/rime-data");
        let root = Path::new(root);
        for shared in [Path::new(shared), root] {
            assert_eq!(
                RuntimeDirs::resolve(root, shared).unwrap(),
                RuntimeDirs {
                    user_data_dir: root.into(),
                    shared_data_dir: shared.into(),
                    staging_dir: root.join("build"),
                    prebuilt_data_dir: if root == shared {
                        root.join("prebuilt")
                    } else {
                        shared.join("build")
                    },
                    active_package_key: None,
                }
            );
        }
    }

    #[test]
    fn shared_control_root_keeps_separate_prebuilt_directory() {
        let root = Path::new("sample/keytao");
        let dirs = RuntimeDirs::resolve(root, root).unwrap();
        assert_eq!(dirs.user_data_dir, root);
        assert_eq!(dirs.shared_data_dir, root);
        assert_eq!(dirs.staging_dir, Path::new("sample/keytao/build"));
        assert_eq!(dirs.prebuilt_data_dir, Path::new("sample/keytao/prebuilt"));
        assert_eq!(dirs.active_package_key, None);
    }

    fn active_package_fixture(label: &str) -> (PathBuf, PathBuf) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "keytao-runtime-{label}-{}-{now}",
            std::process::id()
        ));
        let store = crate::package_store::root(&root);
        let package = store.join("cloud/package");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            root.join(RuntimeDirs::SETTINGS_FILE),
            br#"{"runtime_isolation":true}"#,
        )
        .unwrap();
        fs::write(
            store.join("active-manifest.json"),
            br#"{"active_package":"package"}"#,
        )
        .unwrap();
        (root, package.join("files"))
    }

    #[test]
    fn isolated_active_package_rejects_missing_files_directory() {
        let (root, files) = active_package_fixture("missing-files");
        let addon_files = crate::package_store::root(&root).join("addons/package/files");
        fs::create_dir_all(&addon_files).unwrap();
        let missing = RuntimeDirs::resolve(&root, &root);
        fs::create_dir(&files).unwrap();
        let present = RuntimeDirs::resolve(&root, &root);
        fs::remove_dir_all(files.parent().unwrap()).unwrap();
        let addon = RuntimeDirs::resolve(&root, &root);
        fs::remove_dir_all(root).unwrap();

        let error = missing.unwrap_err();
        assert!(error.contains("active package \"package\""));
        assert!(error.contains("package files directory"));
        assert_eq!(present.unwrap().shared_data_dir, files);
        assert_eq!(addon.unwrap().shared_data_dir, addon_files);
    }

    #[cfg(unix)]
    #[test]
    fn isolated_active_package_rejects_symlinked_files_directory() {
        let (root, files) = active_package_fixture("symlink-files");
        let target = root.join("external-files");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(target, files).unwrap();
        let resolved = RuntimeDirs::resolve(&root, &root);
        fs::remove_dir_all(root).unwrap();

        let error = resolved.unwrap_err();
        assert!(error.contains("active package \"package\""));
        assert!(error.contains("symlinked"));
    }
}
