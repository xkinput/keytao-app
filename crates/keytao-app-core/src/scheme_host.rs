//! Shell callbacks for scheme operations; deployment delegates to the core IME backend.
//! Core controls their ordering, including Wanxiang deployment rollback.
use std::{
    future::Future,
    path::{Path, PathBuf},
};

pub trait SchemeHost: Sync {
    fn user_root(&self) -> Result<PathBuf, String>;
    fn deploy(&self) -> impl Future<Output = Result<(), String>> + Send;
    fn write_reload_stamp(&self, root: &Path) -> Result<Option<PathBuf>, String>;
    #[cfg(target_os = "android")]
    fn copy_addon_assets(&self, id: &str) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn publish_english_addon(&self) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn remove_public_schema(&self, id: &str, files: &[PathBuf]) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String>;
    #[cfg(target_os = "windows")]
    fn publish_wanxiang_sources(&self, files: &[(PathBuf, PathBuf)]) -> Result<(), String>;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    pub(crate) struct AddonHost {
        pub(crate) root: PathBuf,
        pub(crate) core: Arc<crate::Core>,
        pub(crate) deploys: AtomicUsize,
        pub(crate) reloads: AtomicUsize,
        pub(crate) fail_deploys: AtomicUsize,
    }

    impl AddonHost {
        pub(crate) fn new(root: &Path) -> Self {
            struct Sink;
            impl crate::EventSink for Sink {
                fn emit(&self, _: crate::CoreEvent) {}
            }
            Self {
                root: root.to_path_buf(),
                core: crate::Core::new(
                    crate::AppEnv {
                        data_dir: root.join("app-state"),
                        cache_dir: root.join("cache"),
                        resource_dir: root.join("resources"),
                        app_version: "test".into(),
                        platform: crate::Platform::MacOs,
                    },
                    Arc::new(Sink),
                )
                .unwrap(),
                deploys: AtomicUsize::new(0),
                reloads: AtomicUsize::new(0),
                fail_deploys: AtomicUsize::new(0),
            }
        }
    }

    impl SchemeHost for AddonHost {
        fn user_root(&self) -> Result<PathBuf, String> {
            Ok(self.root.clone())
        }
        async fn deploy(&self) -> Result<(), String> {
            self.deploys.fetch_add(1, Ordering::SeqCst);
            if self
                .fail_deploys
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err("fixture deploy failure".into());
            }
            crate::packages::prepare_active_rime_lua(&self.core, &self.root)
                .map_err(|e| e.to_string())?;
            let config =
                std::fs::read(self.root.join("default.custom.yaml")).map_err(|e| e.to_string())?;
            let schemas =
                crate::scheme_files::parse_schema_list(std::str::from_utf8(&config).unwrap());
            std::fs::create_dir_all(self.root.join("build")).map_err(|e| e.to_string())?;
            std::fs::write(self.root.join("build/default.yaml"), config)
                .map_err(|e| e.to_string())?;
            for schema in schemas {
                std::fs::copy(
                    self.root.join(format!("{schema}.schema.yaml")),
                    self.root.join(format!("build/{schema}.schema.yaml")),
                )
                .map_err(|e| e.to_string())?;
                std::fs::write(
                    self.root.join(format!("build/{schema}.table.bin")),
                    b"fixture",
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        fn write_reload_stamp(&self, _: &Path) -> Result<Option<PathBuf>, String> {
            self.reloads.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
        #[cfg(target_os = "android")]
        fn copy_addon_assets(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        #[cfg(target_os = "windows")]
        fn publish_english_addon(&self) -> Result<(), String> {
            Ok(())
        }
        #[cfg(target_os = "windows")]
        fn remove_public_schema(&self, _: &str, _: &[PathBuf]) -> Result<(), String> {
            Ok(())
        }
        #[cfg(target_os = "windows")]
        fn publish_english_settings(&self, _: bool) -> Result<(), String> {
            Ok(())
        }
        #[cfg(target_os = "windows")]
        fn publish_wanxiang_sources(&self, _: &[(PathBuf, PathBuf)]) -> Result<(), String> {
            Ok(())
        }
    }
}
