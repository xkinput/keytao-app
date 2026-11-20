#[cfg(any(target_os = "android", test))]
use crate::scheme_types::VerifyEntry;
use crate::{
    Core, CoreError, legacy_flat::install::smart_install_archive, scheme_types::InstallResult,
};
use serde::Deserialize;
#[cfg(target_os = "android")]
use std::path::Path;

#[cfg(test)]
pub(crate) use crate::legacy_flat::install::{
    collect_rime_package_build_id, smart_install_files, write_package_file,
};
#[cfg(target_os = "windows")]
pub use crate::legacy_flat::install::{WindowsInstallFinish, finish_windows_scheme_install};

pub(crate) fn is_default_custom(filename: &str) -> bool {
    filename == "default.custom.yaml" || filename == "default-custom.yaml"
}

pub fn is_protected_user_file(relative: &str) -> bool {
    !relative.contains('/')
        && !is_default_custom(relative)
        && (relative.ends_with(".user.dict.yaml")
            || relative.ends_with(".user.yaml")
            || relative == "user.yaml"
            || relative == "user_dict.txt"
            || relative == "keytao.user.remove.txt"
            || relative == keytao_core::ACTIVE_SCHEME_MANIFEST
            || relative.ends_with("_user.txt")
            || (relative.starts_with(".keytao-sync-base-") && relative.ends_with(".json")))
}

#[derive(serde::Serialize)]
pub struct ProtectedFileInstall {
    pub protected: bool,
    pub write: bool,
    pub backup_name: Option<String>,
    pub log: Option<String>,
}

pub fn prepare_protected_file_install(
    relative: &str,
    existing: Option<&[u8]>,
) -> ProtectedFileInstall {
    let mut decision = ProtectedFileInstall {
        protected: is_protected_user_file(relative),
        write: true,
        backup_name: None,
        log: None,
    };
    if !decision.protected {
        return decision;
    }
    if relative.starts_with(".keytao-sync-base-") || relative == "keytao.user.remove.txt" {
        decision.write = false;
        decision.log = Some(format!("[KEPT] {relative}"));
        return decision;
    }
    let Some(existing) = existing else {
        return decision;
    };
    let validity = std::str::from_utf8(existing)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            if relative.ends_with(".yaml") {
                let text = text.strip_prefix('\u{feff}').unwrap_or(text);
                // Match Rime: only the first document counts (dictionary entries follow
                // the header's document end) and duplicate keys are not errors.
                serde_yaml::Deserializer::from_str(text)
                    .next()
                    .map(serde::de::IgnoredAny::deserialize)
                    .transpose()
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            } else {
                Ok(())
            }
        });
    match validity {
        Ok(()) => {
            decision.write = false;
            decision.log = Some(format!("[KEPT] {relative}"));
        }
        Err(error) => {
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let backup_name = format!("{relative}.broken-{seconds}");
            decision.log = Some(format!(
                "[BROKEN] {relative}: {error} (backup: {backup_name})"
            ));
            decision.backup_name = Some(backup_name);
        }
    }
    decision
}

pub fn prepare_rime_lua_install(
    package_content: Option<&str>,
    addon_lines: &[&str],
) -> Option<String> {
    keytao_core::generate_rime_lua_content(package_content, addon_lines)
}

pub fn merge_default_custom(existing: Option<&str>, zip_content: &str) -> (String, Vec<String>) {
    keytao_core::merge_default_custom_content(existing, zip_content)
        .unwrap_or_else(|_| (zip_content.to_string(), Vec::new()))
}

pub(crate) fn merge_local_default_custom(
    existing: Option<&str>,
    package: &str,
) -> Result<(String, Vec<String>), String> {
    let Some(existing) = existing else {
        return Ok(merge_default_custom(None, package));
    };
    let mut incoming: serde_yaml::Value =
        serde_yaml::from_str(package).map_err(|error| format!("方案配置无效：{error}"))?;
    let current: serde_yaml::Value = serde_yaml::from_str(existing)
        .map_err(|error| format!("default.custom.yaml 无效：{error}"))?;
    let patch = incoming
        .get_mut("patch")
        .and_then(serde_yaml::Value::as_mapping_mut)
        .ok_or("方案配置缺少 patch")?;
    // A local package chooses schemas without resetting the user's other settings.
    if let Some(current_patch) = current.get("patch").and_then(serde_yaml::Value::as_mapping) {
        for (key, value) in current_patch {
            if key.as_str() != Some("schema_list") {
                patch.insert(key.clone(), value.clone());
            }
        }
    }
    let incoming = serde_yaml::to_string(&incoming).map_err(|error| error.to_string())?;
    Ok(merge_default_custom(Some(existing), &incoming))
}

pub async fn smart_install(
    core: &Core,
    zip_path: String,
    dest_path: String,
    scheme_key: Option<&str>,
    #[cfg(target_os = "windows")] finish_windows: WindowsInstallFinish,
) -> Result<InstallResult, CoreError> {
    smart_install_archive(
        core,
        zip_path,
        dest_path,
        scheme_key,
        #[cfg(target_os = "windows")]
        finish_windows,
    )
}

#[cfg(target_os = "android")]
pub async fn prepare_android_install(
    core: &Core,
    root: &Path,
    url: String,
) -> Result<String, CoreError> {
    std::fs::create_dir_all(root)
        .map_err(|e| CoreError::Other(format!("创建 Android 输入法目录失败: {e}")))?;
    crate::download::download_to_temp(core, url).await
}

#[cfg(any(target_os = "android", test))]
pub fn finish_android_install(
    core: &Core,
    result: &serde_json::Value,
    scheme_key: Option<&str>,
) -> Result<InstallResult, CoreError> {
    if result["coreInstall"].as_bool() == Some(true) {
        let archive = result["packageZipPath"]
            .as_str()
            .ok_or_else(|| CoreError::Other("Android installation is missing its archive".into()))?;
        let root = result["packageRoot"]
            .as_str()
            .ok_or_else(|| CoreError::Other("Android installation is missing its root".into()))?;
        if !std::path::Path::new(root).is_absolute() {
            return Err(CoreError::Other("Android installation requires an absolute root".into()));
        }
        return smart_install_archive(
            core,
            archive.into(),
            root.into(),
            scheme_key,
            #[cfg(target_os = "windows")]
            finish_windows_scheme_install,
        );
    }
    // SAF exports are outside the live root and do not create an activation.
    Ok(install_result_from_value(result))
}

#[cfg(any(target_os = "android", test))]
fn install_result_from_value(result: &serde_json::Value) -> InstallResult {
    let merged_schemas = result["mergedSchemas"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let logs = result["logs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let verify = result["verify"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    Some(VerifyEntry {
                        path: v["path"].as_str()?.to_string(),
                        ok: v["ok"].as_bool().unwrap_or(false),
                        note: v["note"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    InstallResult {
        merged_schemas,
        logs,
        verify,
    }
}
