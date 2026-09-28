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
    keytao_app_core::install::finish_windows_scheme_install(
        dest,
        package_schema_ids,
        package_dictionary_ids,
        zip_bytes,
    )
}
