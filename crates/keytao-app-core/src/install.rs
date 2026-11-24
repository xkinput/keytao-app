use crate::scheme_types::VerifyEntry;
use crate::{Core, CoreError, scheme_types::InstallResult};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::Path,
};

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

pub async fn install_live_scheme(
    core: &Core,
    zip_path: String,
    root: &Path,
    scheme_key: &str,
    version: Option<&str>,
) -> Result<InstallResult, CoreError> {
    crate::deploy::ensure_not_using_libjd(core, root)?;
    crate::package_migration::ensure_before_write(core, root).await?;
    crate::packages::store_cloud_archive_reader(
        core,
        scheme_key,
        version,
        "official",
        None,
        BufReader::new(File::open(&zip_path)?),
        root,
        None,
    )?;
    emit_progress(core, "done", 100, "安装完成！".into());
    Ok(InstallResult {
        merged_schemas: Vec::new(),
        logs: Vec::new(),
        verify: Vec::new(),
    })
}

fn emit_progress(core: &Core, stage: &str, percent: u32, message: String) {
    core.emit(crate::CoreEvent::InstallProgress(
        crate::events::InstallProgress {
            stage: stage.into(),
            percent,
            message,
        },
    ));
}

/// Export for another Rime installation without changing the KeyTao package store.
pub fn export_scheme(
    core: &Core,
    zip_path: String,
    root: &Path,
) -> Result<InstallResult, CoreError> {
    let mut archive = zip::ZipArchive::new(BufReader::new(File::open(zip_path)?))
        .map_err(|error| CoreError::Other(error.to_string()))?;
    if archive.len() > 10_000 {
        return Err(CoreError::Other("方案包文件过多".into()));
    }
    let entries =
        crate::packages::checked_archive_paths(&mut archive, root).map_err(CoreError::Other)?;
    let mut files = Vec::new();
    let mut remaining = 512 * 1024 * 1024u64;
    let mut merged_schemas = Vec::new();
    for relative in entries {
        let mut entry = archive
            .by_name(&relative)
            .map_err(|error| CoreError::Other(error.to_string()))?;
        let expected = entry.size();
        if expected > remaining {
            return Err(CoreError::Other("方案包解压后过大".into()));
        }
        let mut content = Vec::new();
        entry
            .by_ref()
            .take(expected + 1)
            .read_to_end(&mut content)?;
        if content.len() as u64 != expected {
            return Err(CoreError::Other("方案包文件大小不符".into()));
        }
        remaining -= expected;
        if is_default_custom(&relative) {
            let existing = ["default.custom.yaml", "default-custom.yaml"]
                .into_iter()
                .map(|name| crate::packages::safe_path(root, name).map_err(CoreError::Other))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .find(|path| path.is_file())
                .map(fs::read_to_string)
                .transpose()?;
            let text = std::str::from_utf8(&content)
                .map_err(|error| CoreError::Other(error.to_string()))?;
            let (merged, schemas) = merge_default_custom(existing.as_deref(), text);
            content = merged.into_bytes();
            merged_schemas = schemas;
        } else if crate::package_activation::user_file(&relative)
            && !is_protected_user_file(&relative)
        {
            continue;
        }
        files.push((relative, content));
    }
    let mut logs = Vec::new();
    let mut verify = Vec::new();
    let total = files.len();
    for (index, (relative, content)) in files.into_iter().enumerate() {
        write_export_file(root, &relative, &content, &mut logs).map_err(CoreError::Other)?;
        if is_default_custom(&relative) || relative.ends_with(".yaml") || relative.ends_with(".lua")
        {
            let path = crate::packages::safe_path(root, &relative).map_err(CoreError::Other)?;
            let ok = path.is_file();
            verify.push(VerifyEntry {
                path: relative.clone(),
                ok,
                note: if ok {
                    "文件存在"
                } else {
                    "文件不存在"
                }
                .into(),
            });
        }
        emit_progress(
            core,
            "extracting",
            61 + ((index + 1) * 39 / total) as u32,
            format!("正在导出 {relative}"),
        );
    }
    emit_progress(core, "done", 100, "导出完成".into());
    Ok(InstallResult {
        merged_schemas,
        logs,
        verify,
    })
}

pub(crate) fn write_export_file(
    root: &Path,
    relative: &str,
    content: &[u8],
    logs: &mut Vec<String>,
) -> Result<(), String> {
    let path = crate::packages::safe_path(root, relative)?;
    let existing = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(crate::package_fs::context("读取文件", &path, error).to_string()),
    };
    let decision = prepare_protected_file_install(relative, existing.as_deref());
    if let Some(backup_name) = &decision.backup_name {
        use std::io::Write;
        let backup = root.join(backup_name);
        let result = File::create_new(backup).and_then(|mut file| {
            file.write_all(existing.as_deref().unwrap_or_default())?;
            file.sync_all()
        });
        if let Err(error) = result {
            logs.push(format!(
                "{}; backup failed: {error}; left untouched",
                decision.log.as_deref().unwrap_or_default()
            ));
            return Ok(());
        }
    }
    logs.extend(decision.log);
    if decision.write && existing.as_deref() != Some(content) {
        fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
        crate::scheme_files::write_file_atomic(&path, content)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
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
