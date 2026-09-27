use super::*;
use keytao_app_core::scheme_host::SchemeHost;

pub(super) struct ShellSchemeHost<'a>(pub &'a AppHandle);

impl SchemeHost for ShellSchemeHost<'_> {
    fn user_root(&self) -> Result<PathBuf, String> {
        default_keytao_user_root(self.0)
    }
    async fn deploy(&self) -> Result<(), String> {
        rime_deploy_default(self.0.clone()).await.map(|_| ())
    }
    fn write_reload_stamp(&self, root: &Path) -> Result<Option<PathBuf>, String> {
        write_default_reload_stamp(self.0, root)
    }
    #[cfg(target_os = "android")]
    fn copy_addon_assets(&self, id: &str) -> Result<(), String> {
        install_addon_schema_files(self.0, Path::new(""), id)
    }
    #[cfg(target_os = "windows")]
    fn publish_english_addon(&self) -> Result<(), String> {
        publish_windows_english_addon(self.0)
    }
    #[cfg(target_os = "windows")]
    fn remove_public_schema(&self, id: &str, files: &[PathBuf]) -> Result<(), String> {
        windows_public_schemas::remove_schema(id, files)
    }
    #[cfg(target_os = "windows")]
    fn publish_english_settings(&self, enabled: bool) -> Result<(), String> {
        windows_public_schemas::publish_english_settings(enabled)
    }
    #[cfg(target_os = "windows")]
    fn publish_wanxiang_sources(&self, files: &[(PathBuf, PathBuf)]) -> Result<(), String> {
        windows_public_schemas::publish_source_files(files)
    }
}

#[cfg(target_os = "windows")]
pub(super) fn finish_windows_scheme_install(
    dest: &Path,
    package_schema_ids: &[String],
    package_dictionary_ids: &[String],
    zip_bytes: &[u8],
) -> Result<Vec<String>, String> {
    let mut logs = Vec::new();
    let invalidated = {
        #[cfg(target_os = "windows")]
        let _engine_guard = if !package_schema_ids.is_empty() || !package_dictionary_ids.is_empty()
        {
            Some(WindowsImeEngineInitGuard::acquire()?)
        } else {
            None
        };
        #[cfg(target_os = "windows")]
        if _engine_guard.is_some() {
            keytao_core::clear_windows_rime_build_repair_marker(&dest)?;
            write_keytao_ime_reload_stamp()
                .map_err(|error| format!("通知现有输入法会话暂停加载失败：{error}"))?;
        }
        #[cfg(target_os = "windows")]
        let profile_guard = if _engine_guard.is_some() {
            Some(WindowsImeProfileDeploymentGuard::suspend()?)
        } else {
            None
        };
        let invalidated = keytao_core::invalidate_rime_build_artifacts(
            &dest,
            &package_schema_ids,
            &package_dictionary_ids,
        )?;
        #[cfg(target_os = "windows")]
        if let Some(profile_guard) = profile_guard {
            profile_guard.resume()?;
        }
        invalidated
    };
    logs.extend(
        invalidated
            .into_iter()
            .map(|path| format!("[INVALIDATED] {path}")),
    );
    #[cfg(target_os = "windows")]
    if keytao_core::default_user_data_dir().as_ref() == Some(&dest.to_path_buf()) {
        // Publish the original package, before any user customizations were merged.
        windows_public_schemas::publish_package(&zip_bytes)?;
        logs.push("[WINDOWS] 已更新系统搜索框使用的公共方案".into());
    }

    Ok(logs)
}
