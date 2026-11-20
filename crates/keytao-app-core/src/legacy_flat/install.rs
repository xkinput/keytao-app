use crate::{
    Core, CoreError, CoreEvent,
    events::InstallProgress,
    install::{
        is_default_custom, is_protected_user_file, merge_default_custom, merge_local_default_custom,
        prepare_protected_file_install, prepare_rime_lua_install,
    },
    scheme_types::{InstallResult, VerifyEntry},
};
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "windows")]
use std::path::Path;
use std::{collections::HashSet, path::PathBuf};

pub(crate) fn write_package_file(
    root: &std::path::Path,
    relative: &str,
    content: &[u8],
    logs: &mut Vec<String>,
) -> Result<(), String> {
    let out = crate::packages::safe_path(root, relative)?;
    let existing = if is_protected_user_file(relative) {
        match std::fs::read(&out) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(crate::package_fs::context("读取文件", &out, error).to_string());
            }
        }
    } else {
        None
    };
    let decision = prepare_protected_file_install(relative, existing.as_deref());
    if let Some(backup_name) = &decision.backup_name {
        let backup = root.join(backup_name);
        let renamed = match std::fs::symlink_metadata(&backup) {
            Ok(_) => Err(format!("backup already exists: {backup_name}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                crate::package_fs::rename(&out, &backup).map_err(|error| error.to_string())
            }
            Err(error) => Err(crate::package_fs::context("检查备份文件", &backup, error).to_string()),
        };
        if let Err(error) = renamed {
            logs.push(format!(
                "{}; backup rename failed: {error}; left untouched",
                decision.log.as_deref().unwrap_or_default()
            ));
            return Ok(());
        }
    }
    if let Some(log) = decision.log {
        logs.push(log);
    }
    if !decision.write {
        return Ok(());
    }
    let tag = write_file_force(&out, content)?;
    logs.push(format!("[OK]{tag} {relative}"));
    Ok(())
}

fn install_rime_lua(
    root: &std::path::Path,
    package_content: Option<&str>,
) -> Result<Option<String>, String> {
    let content = prepare_rime_lua_install(
        package_content,
        keytao_core::installed_addon_rime_lua_lines(root),
    );
    let path = crate::packages::safe_path(root, "rime.lua")?;
    match &content {
        Some(content) => {
            write_file_force(&path, content.as_bytes())?;
        }
        None => match crate::package_fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        },
    }
    Ok(content)
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

    for (name, expected) in [
        ("default.custom.yaml", expected_dc),
        ("rime.lua", expected_rl),
    ] {
        let Some(expected) = expected else {
            continue;
        };
        let path = dest.join(name);
        let label = name.to_string();
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

    if expected_rl.is_none() {
        entries.push(VerifyEntry {
            path: "rime.lua".into(),
            ok: !dest.join("rime.lua").exists(),
            note: "当前方案和扩展无需 rime.lua".into(),
        });
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
                    || is_protected_user_file(&relative)
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
    // Rime uses source mtimes to decide which schemas to rebuild.
    if std::fs::read(path).is_ok_and(|existing| existing == content) {
        return Ok("");
    }
    if let Some(p) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(p) {
            return write_file_privileged_fallback(path, content, error);
        }
    }
    let error = match crate::package_fs::retry_windows(|| {
        #[cfg(target_os = "windows")]
        crate::package_fs::clear_readonly(path)?;
        std::fs::write(path, content)
    }) {
        Ok(()) => return Ok(""),
        Err(error) => error,
    };
    // Try chmod before falling back to root — works when we own the file but it's read-only
    #[cfg(unix)]
    let error = if path.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
        match std::fs::write(path, content) {
            Ok(()) => return Ok(" [forced]"),
            Err(error) => error,
        }
    } else {
        error
    };
    write_file_privileged_fallback(path, content, error)
}

#[cfg(target_os = "linux")]
fn write_file_privileged_fallback(
    path: &std::path::Path,
    content: &[u8],
    _error: std::io::Error,
) -> Result<&'static str, String> {
    let mut tmp = tempfile::NamedTempFile::new().map_err(|e| format!("临时文件写入失败: {e}"))?;
    tmp.write_all(content)
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| format!("临时文件写入失败: {e}"))?;
    let result = std::process::Command::new("pkexec")
        .arg("cp")
        .arg("--")
        .arg(tmp.path())
        .arg(path)
        .output();
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
    error: std::io::Error,
) -> Result<&'static str, String> {
    Err(crate::package_fs::context("写入文件", path, error).to_string())
}

#[cfg(target_os = "windows")]
pub type WindowsInstallFinish =
    fn(&Path, &[String], &[String], &[u8]) -> Result<Vec<String>, String>;

#[cfg(target_os = "windows")]
pub fn finish_windows_scheme_install(
    dest: &Path,
    package_schema_ids: &[String],
    package_dictionary_ids: &[String],
    _zip_bytes: &[u8],
) -> Result<Vec<String>, String> {
    use crate::{
        ime_paths::write_keytao_ime_reload_stamp,
        ime_status::windows::{WindowsImeEngineInitGuard, WindowsImeProfileDeploymentGuard},
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
    Ok(logs)
}

pub(crate) fn smart_install_archive(
    core: &Core,
    zip_path: String,
    dest_path: String,
    scheme_key: Option<&str>,
    #[cfg(target_os = "windows")] finish_windows: WindowsInstallFinish,
) -> Result<InstallResult, CoreError> {
    use std::io::Read;
    let zip_bytes = std::fs::read(&zip_path)?;
    crate::packages::archive_paths(&zip_bytes, std::path::Path::new(&dest_path))
        .map_err(CoreError::Other)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&zip_bytes))
        .map_err(|error| CoreError::Other(error.to_string()))?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| CoreError::Other(error.to_string()))?;
        if !file.is_dir() {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            files.push((file.name().to_owned(), bytes));
        }
    }
    drop(archive);
    let result = smart_install_entries(
        core,
        files,
        zip_bytes,
        dest_path,
        false,
        scheme_key,
        #[cfg(target_os = "windows")]
        finish_windows,
    );
    if result.is_ok() {
        std::fs::remove_file(zip_path).ok();
    }
    result
}

pub(crate) async fn smart_install_files(
    core: &Core,
    files: Vec<(String, Vec<u8>)>,
    dest_path: String,
    preserve_default_settings: bool,
    scheme_key: Option<&str>,
    #[cfg(target_os = "windows")] finish_windows: WindowsInstallFinish,
) -> Result<InstallResult, CoreError> {
    // Existing receipts and platform verification use a transient archive.
    let zip_bytes = crate::packages::archive_from_files(&files)?;
    smart_install_entries(
        core,
        files,
        zip_bytes,
        dest_path,
        preserve_default_settings,
        scheme_key,
        #[cfg(target_os = "windows")]
        finish_windows,
    )
}

fn check_package_file_conflicts(
    root: &std::path::Path,
    files: &[(String, Vec<u8>)],
    records: &std::collections::BTreeMap<String, crate::package_activation::Activation>,
    scheme_key: Option<&str>,
) -> Result<(), String> {
    let mut conflicts = Vec::new();
    for (relative, content) in files {
        if crate::packages::excluded_from_manifest(relative) {
            continue;
        }
        let path = crate::packages::safe_path(root, relative)?;
        let mut owners: Vec<_> = records
            .iter()
            .filter(|(key, record)| {
                Some(key.as_str()) != scheme_key
                    && record.owned.iter().any(|file| {
                        crate::package_activation::same_file_path(root, &file.path, relative)
                    })
            })
            .map(|(key, _)| key.as_str())
            .collect();
        if owners.is_empty() {
            continue;
        }
        let current = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(crate::package_fs::context("读取文件", &path, error).to_string());
            }
        };
        let digest = |bytes: &[u8]| -> Result<String, String> {
            if relative.ends_with(".custom.yaml") {
                let content = crate::managed_custom::strip(
                    std::str::from_utf8(bytes).map_err(|e| e.to_string())?,
                )?;
                crate::packages::sha256_reader(content.as_bytes()).map_err(|e| e.to_string())
            } else {
                crate::packages::sha256_reader(bytes).map_err(|e| e.to_string())
            }
        };
        let current_sha256 = digest(&current)?;
        if current_sha256 != digest(content)? {
            owners.retain(|key| records[*key].owned.iter().any(|file| {
                crate::package_activation::same_file_path(root, &file.path, relative)
                    && file.sha256 == current_sha256
            }));
            if !owners.is_empty() {
                conflicts.push(format!("{relative}（{}）", owners.join("、")));
            }
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(format!("文件冲突：{}", conflicts.join("、")))
    }
}

fn smart_install_entries(
    core: &Core,
    files: Vec<(String, Vec<u8>)>,
    zip_bytes: Vec<u8>,
    dest_path: String,
    preserve_default_settings: bool,
    scheme_key: Option<&str>,
    #[cfg(target_os = "windows")] finish_windows: WindowsInstallFinish,
) -> Result<InstallResult, CoreError> {
    let dest = PathBuf::from(&dest_path);
    let installed_records = if dest.try_exists()? {
        crate::package_activation::read_all(core, &dest)?
    } else {
        Default::default()
    };
    check_package_file_conflicts(&dest, &files, &installed_records, scheme_key)
        .map_err(CoreError::Other)?;
    let result: Result<_, String> = (|| {
        let emit = |stage: &str, percent: u32, message: &str| {
            core.emit(CoreEvent::InstallProgress(InstallProgress {
                stage: stage.to_string(),
                percent,
                message: message.to_string(),
            }));
        };

        emit("extracting", 61, "正在解压...");

        let dest = PathBuf::from(&dest_path);
        *core
            .pending_package
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        crate::addon_store::migrate(core, &dest).map_err(|e| e.to_string())?;
        let previous = crate::package_activation::read(core, &dest).map_err(|e| e.to_string())?;
        let mut removed_files = previous
            .as_ref()
            .filter(|active| !active.key.is_empty())
            .map(|active| {
                crate::package_store::PackageStore::new(core)
                    .read_files(&active.key)
                    .map(|(files, _)| files)
            })
            .transpose()
            .map_err(|e| e.to_string())?;
        if removed_files.is_none() {
            let store = crate::package_store::PackageStore::new(core);
            if let Ok(bytes) = std::fs::read(
                store
                    .checked_active_manifest_path()
                    .map_err(|e| e.to_string())?,
            ) {
                if let Ok(marker) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if marker["root"].as_str().map(PathBuf::from) == dest.canonicalize().ok() {
                        if let Some(key) = core.state().active_scheme {
                            removed_files = store.read_files(&key).ok().map(|(files, _)| files);
                        }
                    }
                }
            }
        }
        let changed_dictionaries = if removed_files.is_some() {
            Some(
                crate::package_artifacts::changed_dictionary_ids(&dest, &files)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let (mut package_schema_ids, mut package_dictionary_ids) = rime_package_build_ids(&zip_bytes)?;
        let schema_inputs_before = keytao_core::schema_input_fingerprints(&dest, &package_schema_ids)?;
        let mut paths =
            crate::package_activation::transaction_paths(&dest, &files, previous.as_ref())
                .map_err(|e| e.to_string())?;
        let snow = files
            .iter()
            .any(|(path, _)| path == "snow_jiandao-NOTICE.txt");
        if snow {
            if let Some(active) =
                crate::addon_store::snow_activation(core, &dest).map_err(|e| e.to_string())?
            {
                paths.extend(
                    active
                        .owned
                        .into_iter()
                        .chain(active.generated)
                        .map(|file| file.path),
                );
            }
        }
        let mut transaction = crate::package_activation::Transaction::begin(core, &dest, paths)
            .map_err(|e| e.to_string())?;
        let mut legacy_logs = Vec::new();
        if snow
            && crate::addon_store::snow_activation(core, &dest)
                .map_err(|e| e.to_string())?
                .is_some()
        {
            transaction
                .snapshot_store_file("addons/snow_jiandao/activation.json")
                .map_err(|e| e.to_string())?;
            legacy_logs =
                crate::addon_store::take_over_snow(core, &dest).map_err(|e| e.to_string())?;
        }
        let replacing_root = dest.canonicalize().ok();
        core.update_state(|state| {
            if state
                .last_deploy
                .as_ref()
                .is_some_and(|last| Some(&last.root) == replacing_root.as_ref())
            {
                state.last_deploy = None;
            }
        })
        .map_err(|e| e.to_string())?;
        // Validate the entire archive and manifest before deleting or writing files.
        let package_rime_lua = super::rime_lua_from_archive(&zip_bytes)?;
        let addons =
            crate::package_activation::addon_ownership(core, &dest).map_err(|e| e.to_string())?;
        let mut installed_ownership = std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
        for file in installed_records.values().flat_map(|record| &record.owned) {
            installed_ownership.entry(file.path.clone()).or_default().insert(file.sha256.clone());
        }
        crate::packages::archive_paths(&zip_bytes, &dest)?;
        let retained: std::collections::BTreeSet<_> =
            crate::package_activation::retained(&legacy_logs)
                .into_iter()
                .collect();
        let mut activation = crate::package_activation::Activation {
            root: dest
                .canonicalize()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
            kept_backups: crate::package_activation::read(core, &dest)
                .map_err(|e| e.to_string())?
                .map(|active| active.kept_backups)
                .unwrap_or_default(),
            ..Default::default()
        };
        for relative in ["default.custom.yaml", "default-custom.yaml", "rime.lua"] {
            if relative == "rime.lua" && installed_records.values().any(|record| {
                prepare_rime_lua_install(record.rime_lua.as_deref(), keytao_core::installed_addon_rime_lua_lines(&dest))
                    == std::fs::read_to_string(dest.join(relative)).ok()
            }) {
                continue;
            }
            if let Some(backup) = crate::package_activation::backup(core, &dest, relative)
                .map_err(|e| e.to_string())?
            {
                activation.backups.push(backup);
            }
        }
        if let Some(changed) = changed_dictionaries {
            package_dictionary_ids.retain(|id| changed.contains(id));
        }

        // First pass: merge default settings.
        let (merged_dc_path, merged_dc_content, merged_schemas) = {
            let mut zip_dc_path: Option<String> = None;
            let mut zip_dc_content: Option<String> = None;

            for (relative, bytes) in &files {
                let filename = relative.rsplit('/').next().unwrap_or(&relative).to_string();

                if is_default_custom(&filename) && zip_dc_path.is_none() {
                    zip_dc_path = Some(relative.clone());
                    zip_dc_content =
                        Some(String::from_utf8(bytes.clone()).map_err(|error| error.to_string())?);
                }
            }

            // Merge default.custom.yaml
            let (dc_path, dc_content, schemas) =
                if let (Some(path), Some(content)) = (zip_dc_path, zip_dc_content) {
                    let mut existing = std::fs::read_to_string(dest.join("default.custom.yaml"))
                        .ok()
                        .or_else(|| std::fs::read_to_string(dest.join("default-custom.yaml")).ok());
                    if let Some(current) = existing.as_mut() {
                        let selected = crate::scheme_files::parse_schema_list(&content);
                        for id in &package_schema_ids {
                            if !selected.contains(id) && !addons.contains_key(&format!("{id}.schema.yaml")) {
                                *current = crate::addon::update_addon_schema_list_content(current, id, false)?;
                            }
                        }
                    }
                    let (merged, user) = if preserve_default_settings {
                        merge_local_default_custom(existing.as_deref(), &content)?
                    } else {
                        merge_default_custom(existing.as_deref(), &content)
                    };
                    (Some(path), Some(merged), user)
                } else {
                    (None, None, Vec::new())
                };

            (dc_path, dc_content, schemas)
        };

        // Second pass: smart extraction
        let total = files.len();
        let mut logs = legacy_logs;
        let mut manifest = std::collections::BTreeMap::new();

        for (i, (relative, content)) in files.iter().enumerate() {
            if Some(relative) == merged_dc_path.as_ref() {
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
            } else if relative == "rime.lua" {
                // Regenerate once after extraction, including packages without rime.lua.
            } else if retained.contains(relative) {
                continue;
            } else if is_protected_user_file(relative)
                && crate::packages::safe_path(&dest, relative)?
                    .try_exists()
                    .map_err(|e| e.to_string())?
            {
                logs.push(format!("[KEPT] {relative}"));
            } else if crate::package_activation::user_file(relative)
                && !is_protected_user_file(relative)
            {
                logs.push(format!("[KEPT] {relative}"));
            } else if addons.contains_key(relative) {
                logs.push(format!("[KEPT] Add-on file: {relative}"));
            } else {
                let existing = crate::packages::safe_path(&dest, relative)?;
                let ownership = installed_ownership.iter().find(|(path, _)| {
                    crate::package_activation::same_file_path(&dest, path, relative)
                });
                if let Some((_, expected)) = ownership {
                    let current = if relative.ends_with(".custom.yaml") && existing.is_file() {
                        let content = std::fs::read_to_string(&existing).map_err(|e| e.to_string())?;
                        Some(crate::packages::sha256_reader(crate::managed_custom::strip(&content)?.as_bytes()).map_err(|e| e.to_string())?)
                    } else {
                        crate::package_activation::fingerprint(&dest, relative).map_err(|e| e.to_string())?.map(|file| file.sha256)
                    };
                    if current.is_some_and(|current| !expected.contains(&current)) {
                        logs.push(format!("[KEPT] User-modified file: {relative}"));
                        continue;
                    }
                }
                if !crate::packages::excluded_from_manifest(relative) && existing.is_file() {
                    if ownership.is_none() {
                    if let Some(backup) = crate::package_activation::backup(core, &dest, relative)
                        .map_err(|e| e.to_string())?
                    {
                        activation.backups.push(backup);
                    }
                    }
                }
                match write_package_file(&dest, &relative, &content, &mut logs) {
                    Ok(()) => {
                        if !crate::packages::excluded_from_manifest(&relative) {
                            use sha2::{Digest, Sha256};
                            manifest.insert(
                                relative.clone(),
                                format!("{:x}", Sha256::digest(&content)),
                            );
                        }
                    }
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

        let merged_rime_lua_content = install_rime_lua(&dest, package_rime_lua.as_deref())?;
        activation.rime_lua = package_rime_lua.clone();
        logs.push("[REGENERATED] rime.lua".into());

        let mut generated_schemas: std::collections::BTreeSet<_> =
            package_schema_ids.iter().cloned().collect();
        generated_schemas.extend(
            crate::managed_custom::active_schema_ids_from_default(&dest)?
                .into_iter()
                .filter(|id| keytao_core::is_addon_schema(id) || installed_ownership.contains_key(&format!("{id}.schema.yaml"))),
        );
        crate::managed_custom::regenerate(&dest, &generated_schemas)?;
        for relative in manifest.keys() {
            if let Some(file) = crate::package_activation::fingerprint(&dest, relative)
                .map_err(|e| e.to_string())?
            {
                let sha256 = if relative.ends_with(".custom.yaml") {
                    let content =
                        std::fs::read_to_string(crate::packages::safe_path(&dest, relative)?)
                            .map_err(|e| e.to_string())?;
                    crate::packages::sha256_reader(
                        crate::managed_custom::strip(&content)?.as_bytes(),
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    manifest[relative].clone()
                };
                activation.owned.push(crate::package_activation::OwnedFile {
                    path: file.path,
                    sha256,
                    move_file: false,
                });
            }
        }
        super::refresh_generated(core, &dest, &mut activation).map_err(|e| e.to_string())?;
        for name in ["default.custom.yaml", "default-custom.yaml"] {
            if let Ok(bytes) = std::fs::read(dest.join(name)) {
                if let Ok(value) = serde_yaml::from_slice(&bytes) {
                    activation.default_applied.insert(name.into(), value);
                }
            }
        }
        if let Some(removed_files) = &removed_files {
            let mut remaining: std::collections::BTreeSet<_> = files
                .iter()
                .filter_map(|(p, _)| p.strip_suffix(".schema.yaml").map(str::to_owned))
                .collect();
            for name in ["default.custom.yaml", "default-custom.yaml"] {
                if let Ok(content) = std::fs::read_to_string(dest.join(name)) {
                    remaining.extend(crate::scheme_files::parse_schema_list(&content));
                }
            }
            logs.extend(
                crate::package_artifacts::cleanup(&dest, removed_files, &remaining)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|p| format!("[REMOVED BUILD] {p}")),
            );
        }

        let schema_inputs_after = keytao_core::schema_input_fingerprints(&dest, &package_schema_ids)?;
        package_schema_ids = schema_inputs_before.into_iter()
            .filter(|(id, before)| before.is_none() || schema_inputs_after.get(id) != Some(before))
            .map(|(id, _)| id)
            .collect();

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
        super::stage_install(
            core,
            &zip_bytes,
            manifest,
            dest.canonicalize()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
        )
        .map_err(|e| e.to_string())?;
        if let Some(pending) = core
            .pending_package
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            pending.activation = Some(activation);
            pending.transaction = Some(transaction);
        }
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
    })();
    if result.is_err() {
        *core.lock_state() = crate::AppState::load(&core.env().data_dir)?;
    }
    result.map_err(CoreError::Other)
}
