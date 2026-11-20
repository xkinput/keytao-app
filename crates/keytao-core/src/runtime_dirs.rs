use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const PREVIOUS_BUILD: &str = "previous-build";

fn read_regular_file(path: &Path) -> Option<Vec<u8>> {
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

    pub fn isolation_enabled(control_root: &Path) -> bool {
        match std::env::var("KEYTAO_RUNTIME_ISOLATION").as_deref() {
            Ok("1" | "true") => true,
            Ok("0" | "false") => false,
            _ => read_regular_file(&control_root.join(Self::SETTINGS_FILE))
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|settings| settings["runtime_isolation"].as_bool())
                .unwrap_or(false),
        }
    }

    pub fn resolve(control_root: &Path, shared: &Path) -> Self {
        if Self::isolation_enabled(control_root) {
            for store in Self::package_stores(control_root) {
                if let Some(dirs) = manifest_package_key(&store)
                    .and_then(|key| Self::isolated(control_root, shared, Some(&store), &key))
                {
                    return dirs;
                }
            }
        }
        Self::flat(control_root, shared)
    }

    pub fn active_package_key(control_root: &Path) -> Option<String> {
        Self::package_stores(control_root).find_map(|store| manifest_package_key(&store))
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
        if key.starts_with("builtin/") {
            return Self::isolated(control_root, shared, None, key);
        }
        Self::package_stores(control_root)
            .find_map(|store| Self::isolated(control_root, shared, Some(&store), key))
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
        Self::isolated(control_root, shared, Some(&store), key)
    }

    fn package_stores(control_root: &Path) -> impl Iterator<Item = PathBuf> {
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
    ) -> Option<Self> {
        let user_data_dir = Self::isolated_user_data_dir(control_root, key)?;
        let staging_dir = runtime_build_dir(&user_data_dir);
        let runtime_prebuilt = user_data_dir.join("prebuilt");
        if [&staging_dir, &runtime_prebuilt]
            .into_iter()
            .any(|path| fs::symlink_metadata(path).is_ok_and(|entry| !entry.is_dir()))
        {
            return None;
        }
        let shared_data_dir = if key.starts_with("builtin/") {
            // Preserve the caller's bundled search order, including macOS Squirrel SharedSupport.
            if (shared == control_root || shared.as_os_str().is_empty())
                && !shared.join("default.yaml").is_file()
            {
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
            let store = store?;
            let namespace = if key.starts_with("local-") {
                "local"
            } else {
                "cloud"
            };
            [namespace, "addons"].into_iter().find_map(|namespace| {
                let mut path = store.to_path_buf();
                for part in [namespace, key, "files"] {
                    path.push(part);
                    if !fs::symlink_metadata(&path).is_ok_and(|entry| entry.is_dir()) {
                        return None;
                    }
                }
                Some(path)
            })?
        };
        let package_build = shared_data_dir.join("build");
        let prebuilt_data_dir = if package_build.is_dir() {
            package_build
        } else {
            runtime_prebuilt
        };
        Some(Self {
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

fn manifest_package_key(store: &Path) -> Option<String> {
    let bytes = read_regular_file(&store.join("active-manifest.json"))?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let key = manifest["active_package"].as_str()?;
    Some(key.to_owned())
}

fn runtime_build_dir(user_dir: &Path) -> PathBuf {
    let build = user_dir.join("build");
    let previous = user_dir.join(PREVIOUS_BUILD);
    // Two renames may leave only the preserved build if the deployer exits between them.
    if fs::symlink_metadata(&build).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && fs::symlink_metadata(&previous).is_ok_and(|entry| entry.is_dir())
    {
        return previous;
    }
    build
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
                RuntimeDirs::resolve(root, shared),
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
        let dirs = RuntimeDirs::resolve(root, root);
        assert_eq!(dirs.user_data_dir, root);
        assert_eq!(dirs.shared_data_dir, root);
        assert_eq!(dirs.staging_dir, Path::new("sample/keytao/build"));
        assert_eq!(dirs.prebuilt_data_dir, Path::new("sample/keytao/prebuilt"));
        assert_eq!(dirs.active_package_key, None);
    }
}
