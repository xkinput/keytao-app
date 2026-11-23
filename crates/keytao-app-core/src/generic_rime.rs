//! Generic Squirrel/Weasel-compatible Rime archive extraction for isolated scheme directories.

use crate::wanxiang::{self, InstalledFile};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

const MAX_FILES: usize = 10_000;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const DEFAULT_MAX_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn ships_with_scheme(name: &str) -> bool {
    name.ends_with(".user.dict.yaml") || name.ends_with("_user.txt") || name == "user_dict.txt"
}

pub(crate) fn is_trime_asset(relative: &str) -> bool {
    let top = relative.split('/').next().unwrap_or(relative);
    let name = relative.rsplit('/').next().unwrap_or(relative).to_ascii_lowercase();
    top.eq_ignore_ascii_case("backgrounds")
        || matches!(name.as_str(), "trime.yaml" | "trime.custom.yaml")
        || name.ends_with(".trime.yaml")
        || name.ends_with(".trime.custom.yaml")
}

pub(crate) fn is_excluded_path(relative: &str) -> bool {
    let top = relative.split('/').next().unwrap_or(relative);
    if matches!(
        top,
        "sync"
            | "log"
            | "logs"
            | "installation.yaml"
            | "user.yaml"
            | "squirrel.yaml"
            | "squirrel.custom.yaml"
            | "weasel.yaml"
            | "weasel.custom.yaml"
            | "__MACOSX"
    ) {
        return true;
    }
    relative.split('/').any(|part| {
        part.starts_with('.')
            || part == "__MACOSX"
            || part.eq_ignore_ascii_case("Thumbs.db")
            || part.eq_ignore_ascii_case("desktop.ini")
            || part.eq_ignore_ascii_case("System Volume Information")
            || part.eq_ignore_ascii_case("$RECYCLE.BIN")
            || part.eq_ignore_ascii_case("app-state.json")
            || part.ends_with(".userdb")
            || part.ends_with(".leveldb")
            || (part != "user.yaml"
                && !ships_with_scheme(part)
                && crate::install::is_protected_user_file(part))
    })
}

pub(crate) fn is_orphan_prebuilt_bin<S: AsRef<str>>(
    relative: &str,
    has_source: impl Fn(&str) -> bool,
    _marker: Option<S>,
) -> bool {
    let Some(name) = relative.strip_prefix("build/") else {
        return false;
    };
    if name.contains('/') || !name.ends_with(".bin") {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("");
    !stem.is_empty()
        && !has_source(&format!("{stem}.dict.yaml"))
        && !has_source(&format!("{stem}.schema.yaml"))
}

pub(crate) fn is_installable_path(path: &str) -> bool {
    if crate::packages::validate_relative(path).is_err() || is_excluded_path(path) {
        return false;
    }
    if let Some(name) = path.strip_prefix("build/") {
        return !name.contains('/') && name.ends_with(".bin");
    }
    true
}

fn valid_yaml_mapping(bytes: &[u8]) -> bool {
    matches!(
        serde_yaml::from_slice::<serde_yaml::Value>(bytes),
        Ok(serde_yaml::Value::Mapping(_))
    )
}

fn detect_wrapper_prefix(names: &[String]) -> Option<String> {
    let has_root_schema = names
        .iter()
        .any(|name| !name.contains('/') && name.ends_with(".schema.yaml") && !is_excluded_path(name));
    if has_root_schema {
        return None;
    }
    let mut roots = BTreeSet::new();
    let mut has_nested_schema = false;
    for name in names {
        let first = name.split('/').next().unwrap_or(name);
        if first == "__MACOSX" || first.starts_with('.') {
            continue;
        }
        let Some((root, rest)) = name.split_once('/') else {
            return None;
        };
        roots.insert(root.to_owned());
        if !rest.contains('/') && rest.ends_with(".schema.yaml") && !is_excluded_path(rest) {
            has_nested_schema = true;
        }
    }
    if roots.len() == 1 && has_nested_schema {
        roots.into_iter().next().map(|root| format!("{root}/"))
    } else {
        None
    }
}

pub(crate) fn extract_archive(
    archive_path: &Path,
    stage: &Path,
    primary_schema: Option<&str>,
    max_expanded_bytes: u64,
) -> Result<Vec<InstalledFile>, String> {
    let result = extract_archive_inner(archive_path, stage, primary_schema, max_expanded_bytes);
    if result.is_err() {
        let _ = fs::remove_dir_all(stage);
    }
    result
}

fn extract_archive_inner(
    archive_path: &Path,
    stage: &Path,
    primary_schema: Option<&str>,
    max_expanded_bytes: u64,
) -> Result<Vec<InstalledFile>, String> {
    let file = File::open(archive_path).map_err(|e| format!("无法打开压缩包：{e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("无法读取方案压缩包：{e}"))?;
    if archive.len() > MAX_FILES * 2 {
        return Err("方案压缩包文件过多".into());
    }

    let mut raw_entries = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|e| e.to_string())?;
        if entry.is_symlink() {
            return Err(format!("方案压缩包不能包含符号链接：{}", entry.name()));
        }
        if entry.is_dir() || entry.name().ends_with('/') {
            continue;
        }
        let raw_name = entry.name().to_owned();
        wanxiang::safe_target(stage, &raw_name)?;
        raw_entries.push((index, raw_name));
    }

    let raw_names: Vec<String> = raw_entries.iter().map(|(_, name)| name.clone()).collect();
    let wrapper_prefix = detect_wrapper_prefix(&raw_names);

    let mapped: Vec<(usize, String)> = raw_entries
        .into_iter()
        .filter_map(|(index, raw_name)| {
            let relative = match &wrapper_prefix {
                Some(prefix) => raw_name.strip_prefix(prefix)?.to_owned(),
                None => raw_name,
            };
            if relative.is_empty() || is_excluded_path(&relative) {
                return None;
            }
            Some((index, relative))
        })
        .collect();

    let all_non_build: BTreeSet<String> = mapped
        .iter()
        .map(|(_, rel)| rel.clone())
        .filter(|rel| !rel.starts_with("build/"))
        .collect();

    fs::create_dir_all(stage).map_err(|e| e.to_string())?;
    let mut seen_lower = BTreeSet::new();
    let mut installed = BTreeMap::<String, InstalledFile>::new();
    let mut total_bytes = 0u64;

    for (index, relative) in mapped {
        if relative.starts_with("build/") {
            if !is_orphan_prebuilt_bin(&relative, |candidate| all_non_build.contains(candidate), None::<&str>) {
                continue;
            }
        }
        if let Some(stem) = relative.strip_suffix(".custom.yaml") {
            if relative.contains('/') {
                continue;
            }
            let valid_target = stem == "default"
                || all_non_build.contains(&format!("{stem}.schema.yaml"))
                || all_non_build.contains(&format!("{stem}.yaml"));
            if !valid_target {
                continue;
            }
        }
        if installed.len() >= MAX_FILES {
            return Err("方案压缩包文件过多".into());
        }
        if !seen_lower.insert(relative.to_ascii_lowercase()) {
            return Err(format!("方案压缩包存在大小写冲突文件：{relative}"));
        }

        let entry = archive.by_index(index).map_err(|e| e.to_string())?;
        if entry.size() > MAX_FILE_BYTES {
            return Err(format!("方案文件过大：{relative}"));
        }
        let target = wanxiang::safe_target(stage, &relative)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let mut bytes = Vec::new();
        entry
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("方案文件过大：{relative}"));
        }
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > max_expanded_bytes {
            return Err("方案压缩包解压后超出大小限制".into());
        }

        if matches!(relative.as_str(), "default.custom.yaml" | "default-custom.yaml")
            && !valid_yaml_mapping(&bytes)
        {
            seen_lower.remove(&relative.to_ascii_lowercase());
            continue;
        }

        let mut out = File::create(&target).map_err(|e| e.to_string())?;
        out.write_all(&bytes).map_err(|e| e.to_string())?;
        let (sha256, size) = wanxiang::fingerprint(&target)?;
        installed.insert(
            relative.clone(),
            InstalledFile {
                path: relative,
                sha256,
                size,
            },
        );
    }

    if let Some(schema_id) = primary_schema {
        let required = format!("{schema_id}.schema.yaml");
        if !installed.contains_key(&required) {
            return Err(format!("方案发布包缺少必要文件：{required}"));
        }
    } else if !installed
        .keys()
        .any(|path| !path.contains('/') && path.ends_with(".schema.yaml"))
    {
        return Err("方案压缩包根部未找到 .schema.yaml 方案文件".into());
    }

    Ok(installed.into_values().collect())
}

pub(crate) fn append_notice(
    stage: &Path,
    files: &mut Vec<InstalledFile>,
    name: &str,
    content: &str,
) -> Result<(), String> {
    let target = wanxiang::safe_target(stage, name)?;
    fs::write(&target, content).map_err(|e| e.to_string())?;
    let (sha256, size) = wanxiang::fingerprint(&target)?;
    files.retain(|file| file.path != name);
    files.push(InstalledFile {
        path: name.into(),
        sha256,
        size,
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(())
}
