use std::path::{Path, PathBuf};

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
    pub fn resolve(control_root: &Path, shared: &Path) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_layout_matches_platform_golden_paths() {
        for (root, shared, staging, prebuilt) in [
            (
                "/sample/Library/keytao",
                "/sample/Resources/rime-data",
                "/sample/Library/keytao/build",
                "/sample/Resources/rime-data/build",
            ),
            (
                "C:/sample/keytao",
                "C:/sample/rime-data",
                "C:/sample/keytao/build",
                "C:/sample/rime-data/build",
            ),
            (
                "/sample/.local/share/keytao",
                "/sample/share/rime-data",
                "/sample/.local/share/keytao/build",
                "/sample/share/rime-data/build",
            ),
            (
                "/sample/android/files/keytao",
                "/sample/android/files/rime-data",
                "/sample/android/files/keytao/build",
                "/sample/android/files/rime-data/build",
            ),
            (
                "/sample/ios/group/keytao",
                "/sample/ios/rime-data",
                "/sample/ios/group/keytao/build",
                "/sample/ios/rime-data/build",
            ),
        ] {
            assert_eq!(
                RuntimeDirs::resolve(Path::new(root), Path::new(shared)),
                RuntimeDirs {
                    user_data_dir: root.into(),
                    shared_data_dir: shared.into(),
                    staging_dir: staging.into(),
                    prebuilt_data_dir: prebuilt.into(),
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
