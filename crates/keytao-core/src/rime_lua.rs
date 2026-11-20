#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn selected_package_lua_and_colliding_modules_follow_schema() {
        let root = std::env::temp_dir().join(format!("keytao-lua-owners-{}", std::process::id()));
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::create_dir_all(root.join("lua")).unwrap();
        for (key, content) in [("a", "return 'A'"), ("b", "return 'B'")] {
            let snapshot = root.join(format!(".keytao-package-lua/{key}/lua"));
            std::fs::create_dir_all(&snapshot).unwrap();
            let source = if key == "b" {
                snapshot.join("shared/init.lua")
            } else {
                snapshot.join("shared.lua")
            };
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(source, content).unwrap();
        }
        let schemas = "schema_list:\n  - schema: a\n  - schema: a2\n  - schema: b\n  - schema: plain\n  - schema: easy_en\n";
        std::fs::write(root.join("default.yaml"), schemas).unwrap();
        std::fs::write(root.join("build/default.yaml"), schemas).unwrap();
        for id in ["a", "a2", "b", "plain", "easy_en"] {
            std::fs::write(
                root.join(format!("build/{id}.schema.yaml")),
                format!("schema:\n  schema_id: {id}\n"),
            )
            .unwrap();
        }
        let marker = serde_json::json!({
            "root": root.canonicalize().unwrap(),
            "packages": {
                "a": {"schemas": ["a", "a2"], "rime_lua": "owner = 'A'\n", "lua_files": {"lua/shared.lua": ".keytao-package-lua/a/lua/shared.lua"}},
                "b": {"schemas": ["b"], "rime_lua": "owner = 'B'\n", "lua_files": {"lua/shared/init.lua": ".keytao-package-lua/b/lua/shared/init.lua"}},
                "plain": {"schemas": ["plain"], "rime_lua": null}
            }
        });
        std::fs::write(root.join(ACTIVE_SCHEME_MANIFEST), marker.to_string()).unwrap();
        persist_selected_schema(&root, "a").unwrap();
        regenerate_active_rime_lua(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("rime.lua")).unwrap(),
            "owner = 'A'\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("lua/shared.lua")).unwrap(),
            "return 'A'"
        );
        publish_current_schema(&root, "b");
        assert_eq!(
            std::fs::read_to_string(root.join("rime.lua")).unwrap(),
            "owner = 'B'\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("lua/shared/init.lua")).unwrap(),
            "return 'B'"
        );
        assert!(!root.join("lua/shared.lua").exists());
        let stamp = ReloadStamp::current_signature(&root);
        assert!(stamp.is_some());
        publish_current_schema(&root, "a");
        let stamp = ReloadStamp::current_signature(&root).unwrap();
        publish_current_schema(&root, "a2");
        assert_eq!(ReloadStamp::current_signature(&root).unwrap(), stamp);
        std::fs::write(root.join("rime.lua"), "manual = true\n").unwrap();
        publish_current_schema(&root, "plain");
        assert!(std::fs::read(root.join("rime.lua"))
            .unwrap_or_default()
            .is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join(".keytao-rime-user-0.lua")).unwrap(),
            "manual = true\n"
        );
        assert!(!root.join("lua/shared.lua").exists());
        assert!(!root.join("lua/shared/init.lua").exists());
        assert!(active_package_schema_ids(&root).contains("b"));
        // A persisted selection without its build file falls back with the session.
        persist_selected_schema(&root, "b").unwrap();
        std::fs::remove_file(root.join("build/b.schema.yaml")).unwrap();
        assert_eq!(deployed_schema_id_from_dir(&root).as_deref(), Some("a"));
        regenerate_active_rime_lua(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("rime.lua")).unwrap(),
            "owner = 'A'\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("lua/shared.lua")).unwrap(),
            "return 'A'"
        );
        assert!(!root.join("lua/shared/init.lua").exists());
        schema_switch_request::request_schema(&root, "b").unwrap();
        regenerate_active_rime_lua(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("rime.lua")).unwrap(),
            "owner = 'A'\n"
        );
        std::fs::write(
            root.join("build/b.schema.yaml"),
            "schema:\n  schema_id: b\n",
        )
        .unwrap();
        persist_selected_schema(&root, "a").unwrap();
        regenerate_active_rime_lua(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("rime.lua")).unwrap(),
            "owner = 'B'\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_lua_swap_restores_entry_point_modules_and_manifest() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("keytao-lua-rollback-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut packages = serde_json::Map::new();
        for key in ["a", "b"] {
            let mut files = serde_json::Map::new();
            let variant = if key == "a" {
                "lua/c.lua"
            } else {
                "lua/c/init.lua"
            };
            for relative in ["lua/a.lua", variant, "lua/locked/z.lua"] {
                let source = format!(".keytao-package-lua/{key}/{relative}");
                std::fs::create_dir_all(root.join(&source).parent().unwrap()).unwrap();
                std::fs::write(root.join(&source), key).unwrap();
                files.insert(relative.into(), source.into());
            }
            packages.insert(
                key.into(),
                serde_json::json!({"schemas": [key], "rime_lua": key, "lua_files": files}),
            );
        }
        std::fs::write(
            root.join(ACTIVE_SCHEME_MANIFEST),
            serde_json::json!({"root": root.canonicalize().unwrap(), "packages": packages})
                .to_string(),
        )
        .unwrap();
        super::select(&root, "a").unwrap();
        let manifest = std::fs::read(root.join(ACTIVE_SCHEME_MANIFEST)).unwrap();
        std::fs::write(root.join("rime.lua"), "manual = true\n").unwrap();
        let locked = root.join("lua/locked");
        let permissions = std::fs::metadata(&locked).unwrap().permissions();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = super::select(&root, "b");
        std::fs::set_permissions(&locked, permissions).unwrap();
        assert!(
            result.is_err(),
            "the read-only module directory must reject the swap"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("rime.lua")).unwrap(),
            "manual = true\n"
        );
        for relative in ["lua/a.lua", "lua/c.lua", "lua/locked/z.lua"] {
            assert_eq!(std::fs::read_to_string(root.join(relative)).unwrap(), "a");
        }
        assert!(!root.join("lua/c/init.lua").exists());
        assert_eq!(
            std::fs::read(root.join(ACTIVE_SCHEME_MANIFEST)).unwrap(),
            manifest
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".keytao-rime-user-0.lua")).unwrap(),
            "manual = true\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
use crate::{
    generate_rime_lua_content, installed_addon_rime_lua_lines, write_file_atomic,
    ACTIVE_SCHEME_MANIFEST,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{atomic::AtomicBool, Mutex},
};

pub(crate) static RELOAD_PENDING: AtomicBool = AtomicBool::new(false);
pub(crate) static SAVE_RETRY: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn checked_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.contains('\\')
        || !Path::new(relative)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(format!("invalid Lua path: {relative}"));
    }
    let mut path = root.to_path_buf();
    for part in Path::new(relative).components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!("refuse symbolic link: {}", path.display()))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(path)
}

fn read_manifest(root: &Path) -> Result<Option<Value>, String> {
    let path = checked_path(root, ACTIVE_SCHEME_MANIFEST)?;
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("parse {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

pub(crate) fn signature(root: &Path) -> Result<Option<String>, String> {
    let dirs = crate::RuntimeDirs::resolve(root, root)?;
    signature_for_runtime(root, &dirs)
}

pub(crate) fn signature_for_runtime(
    root: &Path,
    dirs: &crate::RuntimeDirs,
) -> Result<Option<String>, String> {
    if let Some(key) = &dirs.active_package_key {
        let manifest = dirs.user_data_dir.join(crate::RuntimeDirs::MANIFEST_FILE);
        let signature = crate::ReloadStamp::signature_at(&manifest).unwrap_or_default();
        return Ok(Some(format!("{key}:{signature}")));
    }
    Ok(read_manifest(root)?.and_then(|manifest| {
        let owner = manifest.get("lua_owner")?;
        Some(format!(
            "{}:{}",
            owner["sha256"].as_str()?,
            owner["modules_sha256"].as_str().unwrap_or_default()
        ))
    }))
}

pub(crate) fn rebase_manifest(root: &Path) -> Result<(), String> {
    let Some(mut manifest) = read_manifest(root)? else {
        return Ok(());
    };
    manifest["root"] = root
        .canonicalize()
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .as_ref()
        .into();
    write_file_atomic(
        &root.join(ACTIVE_SCHEME_MANIFEST),
        &serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn regenerate(root: &Path) -> Result<(), String> {
    let dirs = crate::RuntimeDirs::resolve(root, root)?;
    if dirs.active_package_key.is_some() {
        return Ok(());
    }
    let Some(_) = read_manifest(root)? else {
        return Ok(());
    };
    let unsaved_selection = crate::lock_ignore_poison(&SAVE_RETRY)
        .as_ref()
        .is_some_and(|(saved_root, _)| saved_root == root);
    let selected = unsaved_selection
        .then(|| crate::deployed_schema_id_from_dir(root))
        .flatten()
        .or(crate::schema_switch_request::pending_for_process(root)?
            .and_then(|request| request.schema_id)
            .filter(|id| {
                crate::valid_rime_build_id(id)
                    && dirs.staging_dir.join(format!("{id}.schema.yaml")).is_file()
                    && crate::deployed_schemas(root)
                        .iter()
                        .any(|schema| schema.id == *id)
            }))
        .or_else(|| crate::deployed_schema_id_from_dir(root))
        .unwrap_or_default();
    select(root, &selected).map(|_| ())
}

pub(crate) fn select(root: &Path, schema: &str) -> Result<bool, String> {
    let dirs = crate::RuntimeDirs::resolve(root, root)?;
    if dirs.active_package_key.is_some() {
        return Ok(false);
    }
    // File transactions must not contend with native key handling.
    let _files = crate::lock_ignore_poison(&FILE_LOCK);
    let Some(mut manifest) = read_manifest(root)? else {
        return Ok(false);
    };
    let canonical = root.canonicalize().map_err(|error| error.to_string())?;
    if manifest["root"].as_str().map(Path::new) != Some(canonical.as_path()) {
        return Err(format!(
            "active scheme root does not match {}",
            canonical.display()
        ));
    }
    let Some(packages) = manifest.get("packages").and_then(Value::as_object) else {
        return crate::regenerate_legacy_rime_lua(root).map(|_| false);
    };
    if packages.is_empty() {
        return crate::regenerate_legacy_rime_lua(root).map(|_| false);
    }
    let user_dir = dirs.user_data_dir;
    let selected = packages.iter().find(|(_, package)| {
        package["schemas"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(schema)))
    });
    let content = selected.and_then(|(_, package)| package.get("rime_lua"));
    let content = match content {
        Some(Value::String(content)) => Some(content.as_str()),
        None | Some(Value::Null) => None,
        _ => return Err("package rime_lua must be a string or null".into()),
    };
    // An empty user entry point also blocks an unrelated shared-directory rime.lua.
    let generated = generate_rime_lua_content(content, installed_addon_rime_lua_lines(root))
        .unwrap_or_default();
    let mut modules: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    for (key, package) in packages {
        if let Some(files) = package.get("lua_files").and_then(Value::as_object) {
            for (relative, source) in files {
                if !relative.starts_with("lua/") {
                    return Err(format!("invalid Lua module path: {relative}"));
                }
                checked_path(&user_dir, relative)?;
                let source = source
                    .as_str()
                    .ok_or("Lua snapshot path must be a string")?;
                if !source.starts_with(&format!(".keytao-package-lua/{key}/lua/")) {
                    return Err(format!("invalid Lua snapshot path: {source}"));
                }
                let bytes = fs::read(checked_path(root, source)?)
                    .map_err(|error| format!("read {source}: {error}"))?;
                modules
                    .entry(relative.clone())
                    .or_default()
                    .insert(key.clone(), bytes);
            }
        }
    }
    if let Some(files) = manifest["lua_owner"]["files"].as_object() {
        for relative in files.keys() {
            if !relative.starts_with("lua/") {
                return Err(format!("invalid Lua module path: {relative}"));
            }
            checked_path(&user_dir, relative)?;
            modules.entry(relative.clone()).or_default();
        }
    }
    let key = selected.map(|(key, _)| key.as_str()).unwrap_or_default();
    let module_names = |path: &str| {
        let path = path.strip_suffix(".lua").unwrap_or(path);
        std::iter::once(path.to_owned())
            .chain(path.strip_suffix("/init").map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let mut module_hashes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (relative, variants) in &modules {
        for name in module_names(relative) {
            module_hashes
                .entry(name)
                .or_default()
                .extend(variants.values().map(|bytes| hash(bytes)));
        }
    }
    let mut module_digest = Sha256::new();
    let mut writes = Vec::new();
    let mut owned_files = BTreeMap::new();
    for (relative, variants) in &modules {
        let hashes: BTreeSet<_> = variants.values().map(|bytes| hash(bytes)).collect();
        let collision = module_names(relative)
            .iter()
            .any(|name| module_hashes[name].len() > 1);
        let desired = if collision {
            variants.get(key)
        } else {
            variants.values().next()
        };
        if let Some(bytes) = desired {
            owned_files.insert(relative.clone(), hash(bytes));
        }
        if collision {
            module_digest.update(relative.as_bytes());
            module_digest.update([0]);
            module_digest.update(
                desired
                    .map(|bytes| hash(bytes))
                    .unwrap_or_default()
                    .as_bytes(),
            );
        }
        let target = checked_path(&user_dir, relative)?;
        let current = match fs::read(&target) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("read {}: {error}", target.display())),
        };
        if current.as_ref() != desired {
            if current.as_ref().is_some_and(|bytes| {
                !hashes.contains(&hash(bytes))
                    && manifest["lua_owner"]["files"][relative].as_str()
                        != Some(hash(bytes).as_str())
            }) {
                continue;
            }
            writes.push((target, desired.cloned(), current));
        }
    }
    let owner = serde_json::json!({"key": key, "sha256": hash(generated.as_bytes()), "modules_sha256": format!("{:x}", module_digest.finalize()), "files": owned_files});
    let changed = manifest["lua_owner"]["sha256"] != owner["sha256"]
        || manifest["lua_owner"]["modules_sha256"] != owner["modules_sha256"];
    let path = checked_path(&user_dir, "rime.lua")?;
    let current = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    // Package snapshots are managed content; only preserve edits made by the user.
    if let Some(bytes) = &current {
        let managed = manifest["lua_owner"]["sha256"].as_str() == Some(hash(bytes).as_str())
            || packages.values().any(|package| {
                generate_rime_lua_content(
                    package["rime_lua"].as_str(),
                    installed_addon_rime_lua_lines(root),
                )
                .as_deref()
                .map(str::as_bytes)
                    == Some(bytes.as_slice())
            });
        if !managed {
            crate::preserve_rime_lua_user_edits(root, Some(&generated))?;
        }
    }
    if current.as_deref() != Some(generated.as_bytes()) {
        writes.insert(0, (path, Some(generated.into_bytes()), current));
    }
    if manifest["lua_owner"] != owner {
        manifest["lua_owner"] = owner;
        let path = checked_path(root, ACTIVE_SCHEME_MANIFEST)?;
        let current = fs::read(&path).map_err(|error| error.to_string())?;
        writes.push((
            path,
            Some(serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?),
            Some(current),
        ));
    }
    let write = |path: &Path, bytes: Option<&[u8]>| -> std::io::Result<()> {
        match bytes {
            Some(bytes) => {
                fs::create_dir_all(path.parent().unwrap())?;
                write_file_atomic(path, bytes)
            }
            None => fs::remove_file(path),
        }
    };
    for (index, (path, bytes, _)) in writes.iter().enumerate() {
        if let Err(error) = write(path, bytes.as_deref()) {
            let mut errors = vec![format!("write {}: {error}", path.display())];
            for (path, _, previous) in writes[..index].iter().rev() {
                if let Err(error) = write(path, previous.as_deref()) {
                    errors.push(format!("rollback {}: {error}", path.display()));
                }
            }
            return Err(errors.join("; "));
        }
    }
    Ok(changed)
}
