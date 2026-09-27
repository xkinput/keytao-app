use keytao_app_core::{ime_host::ImeHost, scheme_host::SchemeHost, Core};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct BridgeHost {
    core: Arc<Core>,
    // Resolve once, using the core's real desktop root unless explicitly overridden.
    root: PathBuf,
    #[cfg(target_os = "linux")]
    helper: Arc<keytao_app_core::ime_status::linux::ManagedImeHelper>,
}

impl BridgeHost {
    pub(crate) fn resolve_root(override_root: Option<&str>) -> Result<PathBuf, String> {
        if let Some(root) = override_root {
            return crate::runtime::absolute_dir(root);
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        return Err(
            "Mobile root discovery is unsupported in bridge; set user_root_override".into(),
        );
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        keytao_core::default_user_data_dir()
            .ok_or_else(|| "Cannot determine KeyTao user root; set user_root_override".into())
    }

    pub(crate) fn new(core: Arc<Core>, root: PathBuf) -> Self {
        Self {
            core,
            root,
            #[cfg(target_os = "linux")]
            helper: Default::default(),
        }
    }

    fn stamp(&self) -> Result<Option<PathBuf>, String> {
        keytao_core::ReloadStamp::write(&self.root).map(Some)
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) fn uses_default_root(&self) -> bool {
        keytao_core::default_user_data_dir().as_ref() == Some(&self.root)
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(crate) fn require_default_theme_root(&self) -> Result<(), String> {
        if self.uses_default_root() {
            Ok(())
        } else {
            Err("Desktop theme settings require the default user root".into())
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "ios", test))]
    fn deploy_paths(&self) -> (PathBuf, PathBuf) {
        #[cfg(target_os = "macos")]
        let shared = keytao_app_core::ime_status::macos::macos_app_shared_data_dir(&self.core)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(keytao_core::default_shared_data_dir()));
        #[cfg(not(target_os = "macos"))]
        let shared = {
            let resources = self.core.env().resource_dir.join("rime-data");
            if resources.join("default.yaml").is_file() {
                resources
            } else {
                self.root.clone()
            }
        };
        (self.root.clone(), shared)
    }
}

impl SchemeHost for BridgeHost {
    fn user_root(&self) -> Result<PathBuf, String> {
        Ok(self.root.clone())
    }

    async fn deploy(&self) -> Result<(), String> {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "ios"))]
        {
            // Preserve explicit overrides; the normal macOS path uses Core's
            // default deployment, including system IME reload notifications.
            #[cfg(target_os = "macos")]
            if self.uses_default_root() {
                return keytao_app_core::deploy::rime_deploy_default(&self.core, self.clone())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string());
            }
            let (user, shared) = self.deploy_paths();
            self.core.emit(keytao_app_core::CoreEvent::DeployProgress(
                "正在部署 librime...".into(),
            ));
            tokio::task::spawn_blocking(move || {
                keytao_core::deploy(
                    user.to_string_lossy().into_owned(),
                    shared.to_string_lossy().into_owned(),
                )
            })
            .await
            .map_err(|error| error.to_string())??;
            self.stamp()?;
            self.core.emit(keytao_app_core::CoreEvent::DeployProgress(
                "部署完成".into(),
            ));
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios")))]
        Err("Platform IME deployment is unsupported in bridge".into())
    }

    fn write_reload_stamp(&self, _root: &Path) -> Result<Option<PathBuf>, String> {
        self.stamp()
    }

    #[cfg(target_os = "android")]
    fn copy_addon_assets(&self, _id: &str) -> Result<(), String> {
        Err("Android asset plugin calls are unsupported in bridge".into())
    }
    #[cfg(target_os = "windows")]
    fn publish_english_addon(&self) -> Result<(), String> {
        Err("Windows addon publishing is unsupported in bridge".into())
    }
    #[cfg(target_os = "windows")]
    fn remove_public_schema(&self, _id: &str, _files: &[PathBuf]) -> Result<(), String> {
        Err("Windows public schema removal is unsupported in bridge".into())
    }
    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, _enabled: bool) -> Result<(), String> {
        Err("Windows settings publishing is unsupported in bridge".into())
    }
    #[cfg(target_os = "windows")]
    fn publish_wanxiang_sources(&self, _files: &[(PathBuf, PathBuf)]) -> Result<(), String> {
        Err("Windows source publishing is unsupported in bridge".into())
    }
}

impl ImeHost for BridgeHost {
    fn user_root(&self) -> Result<PathBuf, String> {
        Ok(self.root.clone())
    }

    fn write_reload_stamp(&self, _root: &Path) -> Result<Option<PathBuf>, String> {
        self.stamp()
    }

    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, _enabled: bool) -> Result<(), String> {
        Err("Windows settings publishing is unsupported in bridge".into())
    }
    #[cfg(target_os = "android")]
    fn deploy_ime_data(&self) -> Result<serde_json::Value, String> {
        Err("Android deployment plugin calls are unsupported in bridge".into())
    }
    #[cfg(target_os = "linux")]
    fn managed_helper(&self) -> &keytao_app_core::ime_status::linux::ManagedImeHelper {
        &self.helper
    }
    #[cfg(target_os = "linux")]
    fn suspend_for_deploy(&self, _user: String, shared: String) -> keytao_core::ImeRuntime {
        // The bridge has no live test-input session to suspend.
        keytao_core::ImeRuntime::with_dirs(self.root.clone(), shared)
    }
    #[cfg(target_os = "linux")]
    fn resume_after_deploy(&self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keytao_app_core::{AppEnv, Platform};

    #[test]
    fn every_host_root_and_reload_stamp_uses_override() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("isolated-rime");
        let resolved = BridgeHost::resolve_root(root.to_str()).unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: temp.path().join("state"),
                cache_dir: temp.path().join("cache"),
                resource_dir: temp.path().join("resources"),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            Arc::new(crate::runtime::BridgeEvents::default()),
        )
        .unwrap();
        let host = BridgeHost::new(core, resolved);
        assert_eq!(SchemeHost::user_root(&host).unwrap(), root);
        assert_eq!(ImeHost::user_root(&host).unwrap(), root);
        assert_eq!(host.deploy_paths().0, root);
        let unrelated = temp.path().join("must-not-be-used");
        let expected = Some(keytao_core::ReloadStamp::path(&root));
        assert_eq!(
            SchemeHost::write_reload_stamp(&host, &unrelated).unwrap(),
            expected
        );
        assert_eq!(
            ImeHost::write_reload_stamp(&host, &unrelated).unwrap(),
            expected
        );
        assert!(expected.unwrap().is_file());
        assert!(!unrelated.exists());
    }

    #[test]
    fn invalid_override_never_falls_back_to_default_root() {
        for root in ["", "relative-rime"] {
            assert!(BridgeHost::resolve_root(Some(root)).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_shared_data_candidates_follow_core_order() {
        let temp = tempfile::tempdir().unwrap();
        let resources = temp.path().join("resources");
        let root = temp.path().join("user");
        let core = Core::new(
            AppEnv {
                data_dir: temp.path().join("state"),
                cache_dir: temp.path().join("cache"),
                resource_dir: resources.clone(),
                app_version: "test".into(),
                platform: Platform::MacOs,
            },
            Arc::new(crate::runtime::BridgeEvents::default()),
        )
        .unwrap();
        let host = BridgeHost::new(core, root.clone());
        let candidates = [
            resources.join("rime-data"),
            resources.join("SharedSupport"),
            resources.join("KeyTao.app/Contents/Resources/rime-data"),
            resources.join("KeyTao.app/Contents/SharedSupport"),
        ];
        for candidate in &candidates {
            std::fs::create_dir_all(candidate).unwrap();
            std::fs::write(candidate.join("default.yaml"), "config_version: '1.0'\n").unwrap();
        }
        for candidate in &candidates {
            assert_eq!(host.deploy_paths(), (root.clone(), candidate.clone()));
            std::fs::remove_file(candidate.join("default.yaml")).unwrap();
        }
        assert_eq!(
            BridgeHost::resolve_root(None).unwrap(),
            keytao_core::default_user_data_dir().unwrap(),
        );
    }
}
