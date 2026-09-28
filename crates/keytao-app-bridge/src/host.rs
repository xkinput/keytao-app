use keytao_app_core::{ime_host::ImeHost, scheme_host::SchemeHost, Core};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct BridgeHost {
    core: Arc<Core>,
    // Resolve once, using the core's real desktop root unless explicitly overridden.
    root: PathBuf,
    #[cfg(target_os = "windows")]
    pub(crate) actions: keytao_app_core::app_actions::AppActions,
    #[cfg(target_os = "linux")]
    helper: Arc<keytao_app_core::ime_status::linux::ManagedImeHelper>,
}

/// A per-operation host; Core retains ownership of installation and rollback.
pub(crate) struct InstallHost<F> {
    host: BridgeHost,
    android_deploy: Option<F>,
}

impl<F> InstallHost<F> {
    pub(crate) fn new(host: BridgeHost, android_deploy: F) -> Self {
        Self {
            host,
            android_deploy: cfg!(target_os = "android").then_some(android_deploy),
        }
    }
}

impl<F, Fut> SchemeHost for InstallHost<F>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = String> + Send,
{
    fn user_root(&self) -> Result<PathBuf, String> {
        SchemeHost::user_root(&self.host)
    }

    async fn deploy(&self) -> Result<(), String> {
        if let Some(deploy) = &self.android_deploy {
            let error = deploy().await;
            if error.is_empty() {
                Ok(())
            } else {
                Err(error)
            }
        } else {
            SchemeHost::deploy(&self.host).await
        }
    }

    fn write_reload_stamp(&self, root: &Path) -> Result<Option<PathBuf>, String> {
        SchemeHost::write_reload_stamp(&self.host, root)
    }

    /// Dart copies bundled assets through copyAddonSchemaAssets before calling
    /// addon_schema_install; Core then updates the schema list and deploys.
    #[cfg(target_os = "android")]
    fn copy_addon_assets(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn publish_english_addon(&self) -> Result<(), String> {
        self.host.publish_english_addon()
    }
    #[cfg(target_os = "windows")]
    fn remove_public_schema(&self, id: &str, files: &[PathBuf]) -> Result<(), String> {
        self.host.remove_public_schema(id, files)
    }
    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String> {
        SchemeHost::publish_english_settings(&self.host, enabled)
    }
    #[cfg(target_os = "windows")]
    fn publish_wanxiang_sources(&self, files: &[(PathBuf, PathBuf)]) -> Result<(), String> {
        self.host.publish_wanxiang_sources(files)
    }
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
            #[cfg(target_os = "windows")]
            actions: Default::default(),
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

    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    fn deploy_paths(&self) -> (PathBuf, PathBuf) {
        #[cfg(target_os = "macos")]
        let shared = keytao_app_core::ime_status::macos::macos_app_shared_data_dir(&self.core)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(keytao_core::default_shared_data_dir()));
        #[cfg(target_os = "linux")]
        let shared = keytao_app_core::ime_status::linux::linux_app_shared_data_dir(&self.core)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(keytao_core::default_shared_data_dir()));
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
    pub(crate) async fn deploy_default(
        &self,
    ) -> Result<keytao_app_core::deploy::DeployResult, String> {
        #[cfg(target_os = "windows")]
        {
            self.require_default_windows_root()?;
            keytao_app_core::deploy::rime_deploy_default(&self.core, self.clone(), &self.actions)
                .await
                .map_err(|error| error.to_string())
        }
        #[cfg(target_os = "ios")]
        {
            // Core deploys into the App Group override and writes the stamp
            // consumed by the keyboard extension, just like the Tauri host.
            keytao_app_core::deploy::rime_deploy_default(&self.core, self.clone())
                .await
                .map_err(|error| error.to_string())
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            // Core's desktop default path also reloads/starts the system IME.
            // Keep custom roots isolated from that default-root lifecycle.
            if self.uses_default_root() {
                return keytao_app_core::deploy::rime_deploy_default(&self.core, self.clone())
                    .await
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
            Ok(keytao_app_core::deploy::DeployResult {
                success: true,
                message: "部署成功".into(),
            })
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "ios",
            target_os = "windows"
        )))]
        Err("Platform IME deployment is unsupported in bridge".into())
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn require_default_windows_root(&self) -> Result<(), String> {
        if self.uses_default_root() {
            Ok(())
        } else {
            Err("Windows IME deployment requires the default user root".into())
        }
    }
}

impl SchemeHost for BridgeHost {
    fn user_root(&self) -> Result<PathBuf, String> {
        Ok(self.root.clone())
    }

    async fn deploy(&self) -> Result<(), String> {
        self.deploy_default().await.map(|_| ())
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
        keytao_app_core::windows_public_schemas::publish_english_addon(&self.core)
    }
    #[cfg(target_os = "windows")]
    fn remove_public_schema(&self, id: &str, files: &[PathBuf]) -> Result<(), String> {
        keytao_app_core::windows_public_schemas::remove_schema(id, files)
    }
    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String> {
        keytao_app_core::windows_public_schemas::publish_english_settings(enabled)
    }
    #[cfg(target_os = "windows")]
    fn publish_wanxiang_sources(&self, files: &[(PathBuf, PathBuf)]) -> Result<(), String> {
        keytao_app_core::windows_public_schemas::publish_source_files(files)
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
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String> {
        SchemeHost::publish_english_settings(self, enabled)
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

    #[tokio::test]
    async fn install_host_deploy_maps_delegate_error_and_success() {
        let temp = tempfile::tempdir().unwrap();
        let core = Core::new(
            AppEnv {
                data_dir: temp.path().join("state"),
                cache_dir: temp.path().join("cache"),
                resource_dir: temp.path().join("resources"),
                app_version: "test".into(),
                platform: Platform::Android,
            },
            Arc::new(crate::runtime::BridgeEvents::default()),
        )
        .unwrap();
        let host = BridgeHost::new(core, temp.path().join("user"));
        for message in ["platform deploy failed", ""] {
            // Select the Android delegate on host builds without invoking an IME.
            let wrapper = InstallHost {
                host: host.clone(),
                android_deploy: Some(|| std::future::ready(message.to_string())),
            };
            let expected = if message.is_empty() {
                Ok(())
            } else {
                Err(message.to_string())
            };
            assert_eq!(wrapper.deploy().await, expected);
        }
    }

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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_deploy_paths_use_daemon_runtime_data() {
        let temp = tempfile::tempdir().unwrap();
        let resources = temp.path().join("resources");
        let shared = resources.join("runtime/rime-data");
        let root = temp.path().join("user");
        for dir in [&shared, &resources.join("rime-data")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("default.yaml"), "config_version: '1.0'\n").unwrap();
        }
        let core = Core::new(
            AppEnv {
                data_dir: temp.path().join("state"),
                cache_dir: temp.path().join("cache"),
                resource_dir: resources,
                app_version: "test".into(),
                platform: Platform::Linux,
            },
            Arc::new(crate::runtime::BridgeEvents::default()),
        )
        .unwrap();
        assert_eq!(
            keytao_app_core::ime_status::linux::linux_app_shared_data_dir(&core),
            Some(shared.to_string_lossy().into_owned()),
        );
        assert_eq!(
            BridgeHost::new(core, root.clone()).deploy_paths(),
            (root, shared)
        );
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
