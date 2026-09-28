use crate::{
    Core, CoreError, CoreEvent, events::InstallProgress, scheme_files::parse_schema_list,
    scheme_types::*,
};
#[cfg(any(target_os = "windows", target_os = "android"))]
use std::path::Path;
use std::{collections::HashSet, path::PathBuf};

fn is_default_custom(filename: &str) -> bool {
    filename == "default.custom.yaml" || filename == "default-custom.yaml"
}

// Returns (merged_rime_lua, renames) where renames is [(old_module, new_module)].
// Conflicting user modules are renamed to "<name>_user" to avoid overwrite by zip's lua files.
pub(crate) fn merge_rime_lua(
    local_content: &str,
    zip_content: &str,
    zip_lua_filenames: &std::collections::HashSet<String>,
) -> (String, Vec<(String, String)>) {
    keytao_core::merge_rime_lua_content(Some(local_content), zip_content, zip_lua_filenames)
}

pub fn merge_default_custom(
    existing: Option<&str>,
    zip_content: &str,
) -> (String, Vec<String>) {
    keytao_core::merge_default_custom_content(existing, zip_content).unwrap_or_else(|_| {
        let user: Vec<String> = existing
            .map(|c| {
                parse_schema_list(c)
                    .into_iter()
                    .filter(|s| {
                        !["keytao", "txjx", "xmjd6", "keydo"]
                            .iter()
                            .any(|prefix| s.starts_with(prefix))
                    })
                    .collect()
            })
            .unwrap_or_default();
        (zip_content.to_string(), user)
    })
}

/// After extraction, verify key files were written correctly.
/// - default.custom.yaml / rime.lua: read back and compare byte-for-byte with expected content
/// - dict / schema / lua files: just check existence
fn verify_install(
    dest: &std::path::Path,
    expected_dc: Option<&str>,
    expected_rl: Option<&str>,
    zip_bytes: &[u8],
) -> Vec<VerifyEntry> {
    let mut entries: Vec<VerifyEntry> = Vec::new();

    // Verify default.custom.yaml content matches what we wrote
    if let Some(expected) = expected_dc {
        let path = dest.join("default.custom.yaml");
        let label = "default.custom.yaml".to_string();
        match std::fs::read_to_string(&path) {
            Ok(actual) if actual == expected => entries.push(VerifyEntry {
                path: label,
                ok: true,
                note: "内容一致".into(),
            }),
            Ok(_) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: "内容与写入时不符，可能被其他程序修改或写入不完整".into(),
            }),
            Err(e) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: format!("读取失败: {e}"),
            }),
        }
    }

    // Verify rime.lua content matches what we wrote
    if let Some(expected) = expected_rl {
        let path = dest.join("rime.lua");
        let label = "rime.lua".to_string();
        match std::fs::read_to_string(&path) {
            Ok(actual) if actual == expected => entries.push(VerifyEntry {
                path: label,
                ok: true,
                note: "内容一致".into(),
            }),
            Ok(_) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: "内容与写入时不符，可能被其他程序修改或写入不完整".into(),
            }),
            Err(e) => entries.push(VerifyEntry {
                path: label,
                ok: false,
                note: format!("读取失败: {e}"),
            }),
        }
    }

    // Check that every non-empty zip entry (excluding the two merge-handled files) was written to disk
    if let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)) {
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                let raw = file.name().to_string();
                let relative = raw.trim_end_matches('/').to_string();
                if relative.is_empty() || file.is_dir() {
                    continue;
                }
                let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();
                // Only spot-check key file types (schemas, dicts, lua, opencc)
                let is_key = filename.ends_with(".schema.yaml")
                    || filename.ends_with(".dict.yaml")
                    || (filename.ends_with(".lua") && !relative.contains('/'))
                    || relative.starts_with("lua/")
                    || relative.starts_with("opencc/");
                if !is_key {
                    continue;
                }
                // Skip merge-handled files (already verified above)
                if is_default_custom(&filename) || filename == "rime.lua" {
                    continue;
                }
                let on_disk = dest.join(&relative);
                if on_disk.exists() {
                    entries.push(VerifyEntry {
                        path: relative,
                        ok: true,
                        note: "文件存在".into(),
                    });
                } else {
                    entries.push(VerifyEntry {
                        path: relative,
                        ok: false,
                        note: "文件不存在".into(),
                    });
                }
            }
        }
    }
    entries
}

pub(crate) fn collect_rime_package_build_id(
    relative: &str,
    schema_ids: &mut HashSet<String>,
    dictionary_ids: &mut HashSet<String>,
) {
    let filename = relative.rsplit('/').next().unwrap_or(relative);
    if let Some(id) = filename.strip_suffix(".schema.yaml") {
        if !id.is_empty() {
            schema_ids.insert(id.to_string());
        }
    } else if let Some(id) = filename.strip_suffix(".dict.yaml") {
        if !id.is_empty() {
            dictionary_ids.insert(id.to_string());
        }
    }
}

fn rime_package_build_ids(zip_bytes: &[u8]) -> Result<(Vec<String>, Vec<String>), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|error| format!("读取方案构建清单失败: {error}"))?;
    let mut schema_ids = HashSet::new();
    let mut dictionary_ids = HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        if !file.is_dir() {
            collect_rime_package_build_id(file.name(), &mut schema_ids, &mut dictionary_ids);
        }
    }
    let mut schema_ids: Vec<String> = schema_ids.into_iter().collect();
    schema_ids.sort();
    let mut dictionary_ids: Vec<String> = dictionary_ids.into_iter().collect();
    dictionary_ids.sort();
    Ok((schema_ids, dictionary_ids))
}

/// Writes `content` to `path`, forcibly overwriting even read-only files.
/// On Linux, triggers a polkit (pkexec) root-auth dialog if the file is root-owned.
/// Returns a tag for logging: "" (normal), " [forced]", or " [root]".
fn write_file_force(path: &std::path::Path, content: &[u8]) -> Result<&'static str, String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).ok();
    }
    if std::fs::write(path, content).is_ok() {
        return Ok("");
    }
    // Try chmod before falling back to root — works when we own the file but it's read-only
    #[cfg(unix)]
    if path.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
        if std::fs::write(path, content).is_ok() {
            return Ok(" [forced]");
        }
    }
    write_file_privileged_fallback(path, content)
}

#[cfg(target_os = "linux")]
fn write_file_privileged_fallback(
    path: &std::path::Path,
    content: &[u8],
) -> Result<&'static str, String> {
    let tmp = std::env::temp_dir().join("keytao_privileged_write");
    std::fs::write(&tmp, content).map_err(|e| format!("临时文件写入失败: {e}"))?;
    let result = std::process::Command::new("pkexec")
        .arg("cp")
        .arg("--")
        .arg(&tmp)
        .arg(path)
        .output();
    let _ = std::fs::remove_file(&tmp);
    match result {
        Ok(o) if o.status.success() => Ok(" [root]"),
        Ok(o) => Err(format!(
            "需要 root 权限写入 {}，认证失败或被取消: {}",
            path.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("无法启动 pkexec（请确认系统已安装 polkit）: {e}")),
    }
}

#[cfg(not(target_os = "linux"))]
fn write_file_privileged_fallback(
    path: &std::path::Path,
    _content: &[u8],
) -> Result<&'static str, String> {
    Err(format!("写入失败（权限不足）：{}", path.display()))
}

#[cfg(target_os = "windows")]
pub type WindowsInstallFinish =
    fn(&Path, &[String], &[String], &[u8]) -> Result<Vec<String>, String>;

#[cfg(target_os = "windows")]
pub fn finish_windows_scheme_install(
    dest: &Path,
    package_schema_ids: &[String],
    package_dictionary_ids: &[String],
    zip_bytes: &[u8],
) -> Result<Vec<String>, String> {
    use crate::{
        ime_paths::write_keytao_ime_reload_stamp,
        ime_status::windows::{WindowsImeEngineInitGuard, WindowsImeProfileDeploymentGuard},
        windows_public_schemas,
    };

    let mut logs = Vec::new();
    let invalidated = {
        let _engine_guard = if !package_schema_ids.is_empty() || !package_dictionary_ids.is_empty()
        {
            Some(WindowsImeEngineInitGuard::acquire()?)
        } else {
            None
        };
        if _engine_guard.is_some() {
            keytao_core::clear_windows_rime_build_repair_marker(&dest)?;
            write_keytao_ime_reload_stamp()
                .map_err(|error| format!("通知现有输入法会话暂停加载失败：{error}"))?;
        }
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
    if keytao_core::default_user_data_dir().as_ref() == Some(&dest.to_path_buf()) {
        // Publish the original package, before any user customizations were merged.
        windows_public_schemas::publish_package(&zip_bytes)?;
        logs.push("[WINDOWS] 已更新系统搜索框使用的公共方案".into());
    }

    Ok(logs)
}

pub async fn smart_install(
    core: &Core,
    zip_path: String,
    dest_path: String,
    #[cfg(target_os = "windows")] finish_windows: WindowsInstallFinish,
) -> Result<InstallResult, CoreError> {
    let result: Result<_, String> = async {
        let emit = |stage: &str, percent: u32, message: &str| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: stage.to_string(),
                percent,
                message: message.to_string(),
            }));
        };

        emit("extracting", 61, "正在解压...");

        let zip_bytes = std::fs::read(&zip_path).map_err(|e| e.to_string())?;
        let dest = PathBuf::from(&dest_path);
        let (package_schema_ids, package_dictionary_ids) = rime_package_build_ids(&zip_bytes)?;

        // First pass: collect zip metadata and merge candidates
        let (
            merged_dc_path,
            merged_dc_content,
            merged_schemas,
            merged_rime_lua_path,
            merged_rime_lua_content,
            renamed_lua_files,
        ) = {
            use std::collections::HashSet;
            use std::io::Read;

            let cursor = std::io::Cursor::new(&zip_bytes);
            let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败: {e}"))?;

            let mut zip_dc_path: Option<String> = None;
            let mut zip_dc_content: Option<String> = None;
            let mut zip_rime_lua_path: Option<String> = None;
            let mut zip_rime_lua_content: Option<String> = None;
            let mut zip_lua_filenames: HashSet<String> = HashSet::new();

            for i in 0..archive.len() {
                let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
                let raw = file.name().to_string();
                let relative = raw.trim_end_matches('/').to_string();
                if relative.is_empty() || file.is_dir() {
                    continue;
                }
                let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();

                if is_default_custom(&filename) && zip_dc_path.is_none() {
                    let mut buf = String::new();
                    file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                    zip_dc_path = Some(relative);
                    zip_dc_content = Some(buf);
                } else if filename == "rime.lua"
                    && !relative.contains('/')
                    && zip_rime_lua_path.is_none()
                {
                    let mut buf = String::new();
                    file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
                    zip_rime_lua_path = Some(relative);
                    zip_rime_lua_content = Some(buf);
                } else if relative.starts_with("lua/") && !relative[4..].contains('/') {
                    zip_lua_filenames.insert(filename);
                }
            }

            // Merge default.custom.yaml
            let (dc_path, dc_content, schemas) =
                if let (Some(path), Some(content)) = (zip_dc_path, zip_dc_content) {
                    let existing = std::fs::read_to_string(dest.join("default.custom.yaml"))
                        .ok()
                        .or_else(|| std::fs::read_to_string(dest.join("default-custom.yaml")).ok());
                    let (merged, user) = merge_default_custom(existing.as_deref(), &content);
                    (Some(path), Some(merged), user)
                } else {
                    (None, None, Vec::new())
                };

            // Merge rime.lua
            let (rl_path, rl_content, renamed) = if let (Some(path), Some(zip_rl)) =
                (zip_rime_lua_path, zip_rime_lua_content)
            {
                if let Ok(local_rl) = std::fs::read_to_string(dest.join("rime.lua")) {
                    let (merged, renames) = merge_rime_lua(&local_rl, &zip_rl, &zip_lua_filenames);
                    // Read local lua files that need renaming before zip overwrites them
                    let renamed_contents: Vec<(String, Vec<u8>)> = renames
                        .iter()
                        .filter_map(|(old, new)| {
                            let local_file = dest.join("lua").join(format!("{}.lua", old));
                            std::fs::read(&local_file)
                                .ok()
                                .map(|bytes| (new.clone(), bytes))
                        })
                        .collect();
                    (Some(path), Some(merged), renamed_contents)
                } else {
                    (Some(path), Some(zip_rl), Vec::new())
                }
            } else {
                (None, None, Vec::new())
            };

            (dc_path, dc_content, schemas, rl_path, rl_content, renamed)
        };

        // Second pass: smart extraction
        let cursor = std::io::Cursor::new(&zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败: {e}"))?;
        let total = archive.len();
        let mut logs: Vec<String> = Vec::new();

        for i in 0..total {
            let (relative, is_dir, content) = {
                let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
                let raw = file.name().to_string();
                let relative = raw.trim_end_matches('/').to_string();
                if relative.is_empty() {
                    continue;
                }
                let is_dir = file.is_dir();
                let mut buf = Vec::new();
                if !is_dir {
                    use std::io::Read;
                    file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                }
                (relative, is_dir, buf)
            };

            if is_dir {
                if let Err(e) = std::fs::create_dir_all(dest.join(&relative)) {
                    logs.push(format!("[WARN] mkdir {relative}: {e}"));
                }
            } else if Some(&relative) == merged_dc_path.as_ref() {
                if let Some(ref mc) = merged_dc_content {
                    let out = dest.join(&relative);
                    match write_file_force(&out, mc.as_bytes()) {
                        Ok(tag) => logs.push(format!("[MERGED]{tag} {relative}")),
                        Err(e) => {
                            logs.push(format!("[ERROR] {relative}: {e}"));
                            return Err(e);
                        }
                    }
                }
            } else if Some(&relative) == merged_rime_lua_path.as_ref() {
                if let Some(ref mc) = merged_rime_lua_content {
                    let out = dest.join(&relative);
                    match write_file_force(&out, mc.as_bytes()) {
                        Ok(tag) => logs.push(format!("[MERGED]{tag} {relative}")),
                        Err(e) => {
                            logs.push(format!("[ERROR] {relative}: {e}"));
                            return Err(e);
                        }
                    }
                }
            } else {
                let out = dest.join(&relative);
                match write_file_force(&out, &content) {
                    Ok(tag) => logs.push(format!("[OK]{tag} {relative}")),
                    Err(e) => {
                        logs.push(format!("[ERROR] {relative}: {e}"));
                        return Err(e);
                    }
                }
            }

            let percent = 61 + ((i + 1) * 39 / total) as u32;
            let fname = relative.rsplit('/').next().unwrap_or(&relative);
            emit(
                "extracting",
                percent,
                &format!("正在安装... {}/{}: {}", i + 1, total, fname),
            );
        }

        // Write renamed user lua files (saved before zip overwrote them)
        for (new_module, bytes) in &renamed_lua_files {
            let out = dest.join("lua").join(format!("{}.lua", new_module));
            match write_file_force(&out, bytes) {
                Ok(tag) => logs.push(format!("[RENAMED]{tag} lua/{new_module}.lua")),
                Err(e) => {
                    logs.push(format!("[ERROR] rename lua/{new_module}.lua: {e}"));
                    return Err(e);
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        logs.extend(
            keytao_core::invalidate_rime_build_artifacts(
                &dest,
                &package_schema_ids,
                &package_dictionary_ids,
            )?
            .into_iter()
            .map(|path| format!("[INVALIDATED] {path}")),
        );
        #[cfg(target_os = "windows")]
        logs.extend(finish_windows(
            &dest,
            &package_schema_ids,
            &package_dictionary_ids,
            &zip_bytes,
        )?);
        std::fs::remove_file(&zip_path).ok();
        emit("done", 100, "安装完成！");

        let verify = verify_install(
            &dest,
            merged_dc_content.as_deref(),
            merged_rime_lua_content.as_deref(),
            &zip_bytes,
        );

        Ok(InstallResult {
            merged_schemas,
            logs,
            verify,
        })
    }
    .await;
    result.map_err(CoreError::Other)
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

#[cfg(target_os = "android")]
pub fn finish_android_install(
    core: &Core,
    result: &serde_json::Value,
) -> Result<InstallResult, CoreError> {
    core.emit(CoreEvent::InstallProgress(InstallProgress {
        stage: "done".into(),
        percent: 100,
        message: "安装完成！".into(),
    }));
    Ok(install_result_from_value(result))
}

#[cfg(target_os = "android")]
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
