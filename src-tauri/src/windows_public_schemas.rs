//! Public Rime packages for sandboxed Windows text hosts. Never source these
//! files from the user's Rime directory: it contains private learned words.

use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

static PUBLISH_LOCK: Mutex<()> = Mutex::new(());
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;

fn public_root() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|path| PathBuf::from(path).join("KeyTao/appcontainer-schemas"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable for public input schemas".into())
}

fn checked_relative(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.to_string_lossy().contains(':')
    {
        return Err(format!("invalid public schema path: {}", path.display()));
    }
    Ok(path.to_path_buf())
}

fn is_public_asset(path: &Path) -> bool {
    if path.components().any(|part| {
        let name = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        name.starts_with('.') || name.ends_with(".userdb") || name == "log"
    }) {
        return false;
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if matches!(name.as_str(), "user.yaml" | "installation.yaml") {
        return false;
    }
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yaml" | "lua" | "txt" | "json" | "bin" | "ocd2")
    )
}

/// The bytes must come directly from a public package download, before merging
/// local customizations. Use publish_files for addons that require patching.
pub fn publish_package(zip_bytes: &[u8]) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|error| format!("read public schema package: {error}"))?;
    let mut files = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let relative = checked_relative(Path::new(entry.name()))?;
        if !is_public_asset(&relative) {
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("public schema packages may not contain symbolic links".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("public schema package is too large")?;
        if total > MAX_PACKAGE_BYTES {
            return Err("public schema package is too large".into());
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        files.push((relative, bytes));
    }
    publish_files(&files)
}

/// Publish only package-derived assets. Callers may patch a public addon (for
/// example, disable optional large models) before passing its files here.
pub fn publish_files(files: &[(PathBuf, Vec<u8>)]) -> Result<(), String> {
    let _guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    publish_at(&public_root()?, files, true)
}

pub fn publish_source_files(files: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut contents = Vec::with_capacity(files.len());
    let mut total = 0u64;
    for (relative, source) in files {
        let metadata = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("public addon source must be a regular staged file".into());
        }
        total = total
            .checked_add(metadata.len())
            .ok_or("public addon is too large")?;
        if total > MAX_PACKAGE_BYTES {
            return Err("public addon is too large".into());
        }
        contents.push((
            relative.clone(),
            std::fs::read(source).map_err(|error| error.to_string())?,
        ));
    }
    publish_files(&contents)
}

pub fn has_schemas() -> bool {
    let Ok(root) = public_root() else {
        return false;
    };
    let Ok(current) = std::fs::read_to_string(root.join("current.txt")) else {
        return false;
    };
    let current = current.trim();
    if current.len() != 64 || !current.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    let snapshot = root.join(current);
    let Ok(config) = std::fs::read_to_string(snapshot.join("default.custom.yaml")) else {
        return false;
    };
    super::parse_schema_list(&config).iter().any(|schema| {
        ["keytao", "xmjd", "txjx", "keydo"]
            .iter()
            .any(|prefix| schema.starts_with(prefix))
            && !schema.contains(['/', '\\', ':'])
            && snapshot.join(format!("{schema}.schema.yaml")).is_file()
    })
}

pub fn remove_schema(schema_id: &str, package_paths: &[PathBuf]) -> Result<(), String> {
    if schema_id.is_empty()
        || !schema_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("invalid schema identifier".into());
    }
    let _guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = public_root()?;
    let mut files = read_snapshot(&root)?;
    for relative in package_paths {
        files.remove(&checked_relative(relative)?);
    }
    files.remove(&PathBuf::from(format!("{schema_id}.schema.yaml")));
    for extension in ["schema.yaml", "prism.bin", "table.bin", "reverse.bin"] {
        files.remove(&PathBuf::from(format!("build/{schema_id}.{extension}")));
    }
    if let Some(content) = files.get_mut(Path::new("default.custom.yaml")) {
        let mut document: serde_yaml::Value =
            serde_yaml::from_slice(content).map_err(|error| error.to_string())?;
        if let Some(list) = document
            .get_mut("patch")
            .and_then(|patch| patch.get_mut("schema_list"))
            .and_then(serde_yaml::Value::as_sequence_mut)
        {
            list.retain(|entry| {
                entry.get("schema").and_then(serde_yaml::Value::as_str) != Some(schema_id)
            });
        }
        *content = serde_yaml::to_string(&document)
            .map_err(|error| error.to_string())?
            .into_bytes();
    }
    store_snapshot(&root, &files, true)
}

/// This single preference is safe to share; no other user settings or learned
/// dictionaries are read by the publisher.
pub fn publish_english_settings(enabled: bool) -> Result<(), String> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "englishMode": if enabled { "schema" } else { "ascii" }
    }))
    .map_err(|error| error.to_string())?;
    publish_files(&[(PathBuf::from("keytao-ime.json"), bytes)])
}

fn read_snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    let Ok(current) = std::fs::read_to_string(root.join("current.txt")) else {
        return Ok(files);
    };
    let current = current.trim();
    if current.len() != 64 || !current.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid public schema snapshot identifier".into());
    }
    fn visit(
        base: &Path,
        dir: &Path,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                return Err("symbolic link in public schema snapshot".into());
            }
            if kind.is_dir() {
                visit(base, &entry.path(), files)?;
            } else if kind.is_file() {
                let relative = entry.path().strip_prefix(base).unwrap().to_path_buf();
                if is_public_asset(&relative) {
                    files.insert(
                        relative,
                        std::fs::read(entry.path()).map_err(|error| error.to_string())?,
                    );
                }
            }
        }
        Ok(())
    }
    let snapshot = root.join(current);
    if std::fs::symlink_metadata(&snapshot)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("symbolic link at public schema snapshot root".into());
    }
    visit(&snapshot, &snapshot, &mut files)?;
    Ok(files)
}

fn publish_at(
    root: &Path,
    incoming: &[(PathBuf, Vec<u8>)],
    grant_access: bool,
) -> Result<(), String> {
    let mut files = read_snapshot(root)?;
    let has_schema_list = incoming.iter().any(|(path, _)| {
        path == Path::new("default.custom.yaml") || path == Path::new("default-custom.yaml")
    });
    let mut addon_schemas = Vec::new();
    for (path, content) in incoming {
        let relative = checked_relative(path)?;
        if !is_public_asset(&relative) {
            continue;
        }
        if relative == Path::new("default.custom.yaml")
            || relative == Path::new("default-custom.yaml")
        {
            let old = files
                .get(Path::new("default.custom.yaml"))
                .and_then(|bytes| std::str::from_utf8(bytes).ok());
            let new = std::str::from_utf8(content).map_err(|error| error.to_string())?;
            let (merged, _) = super::merge_default_custom(old, new);
            files.insert(PathBuf::from("default.custom.yaml"), merged.into_bytes());
        } else {
            if !has_schema_list
                && relative.components().count() == 1
                && relative.to_string_lossy().ends_with(".schema.yaml")
            {
                let schema: serde_yaml::Value =
                    serde_yaml::from_slice(content).map_err(|error| error.to_string())?;
                if let Some(id) = schema
                    .get("schema")
                    .and_then(|value| value.get("schema_id"))
                    .and_then(serde_yaml::Value::as_str)
                {
                    if !id.is_empty()
                        && id
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    {
                        addon_schemas.push(id.to_owned());
                    }
                }
            }
            files.insert(relative, content.clone());
        }
    }
    if !addon_schemas.is_empty() {
        let name = PathBuf::from("default.custom.yaml");
        let mut config: serde_yaml::Value = files
            .get(&name)
            .map(|bytes| serde_yaml::from_slice(bytes))
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| serde_yaml::from_str("patch: {schema_list: []}").unwrap());
        let list = config
            .get_mut("patch")
            .and_then(|patch| patch.get_mut("schema_list"))
            .and_then(serde_yaml::Value::as_sequence_mut)
            .ok_or("public default.custom.yaml is missing patch/schema_list")?;
        for id in addon_schemas {
            if !list
                .iter()
                .any(|entry| entry.get("schema").and_then(serde_yaml::Value::as_str) == Some(&id))
            {
                let mut entry = serde_yaml::Mapping::new();
                entry.insert(
                    serde_yaml::Value::String("schema".into()),
                    serde_yaml::Value::String(id),
                );
                list.push(serde_yaml::Value::Mapping(entry));
            }
        }
        files.insert(
            name,
            serde_yaml::to_string(&config)
                .map_err(|error| error.to_string())?
                .into_bytes(),
        );
    }
    store_snapshot(root, &files, grant_access)
}

fn store_snapshot(
    root: &Path,
    files: &BTreeMap<PathBuf, Vec<u8>>,
    grant_access: bool,
) -> Result<(), String> {
    let mut hash = Sha256::new();
    for (path, content) in files {
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    let generation = format!("{:x}", hash.finalize());
    let snapshot = root.join(&generation);
    std::fs::create_dir_all(&snapshot).map_err(|error| error.to_string())?;
    for (path, content) in files {
        let output = snapshot.join(path);
        std::fs::create_dir_all(output.parent().unwrap()).map_err(|error| error.to_string())?;
        super::write_file_atomic(&output, content).map_err(|error| error.to_string())?;
    }
    if grant_access {
        grant_appcontainer_read(root)?;
    }
    super::write_file_atomic(&root.join("current.txt"), generation.as_bytes())
        .map_err(|error| error.to_string())
}

fn grant_appcontainer_read(root: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let icacls = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("SystemRoot is unavailable")?
        .join("System32/icacls.exe");
    let output = std::process::Command::new(icacls)
        .arg(root)
        .args(["/grant", "*S-1-15-2-1:(OI)(CI)(RX)", "/T", "/Q"])
        .creation_flags(0x08000000)
        .output()
        .map_err(|error| format!("grant public schema read access: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "grant public schema read access failed: {}",
            output.status
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_packages_reject_escape_paths_and_never_publish_private_runtime_files() {
        for path in [
            "../secret.yaml",
            "C:\\secret.yaml",
            "\\secret.yaml",
            "config.yaml:stream",
        ] {
            assert!(checked_relative(Path::new(path)).is_err(), "{path}");
        }
        for path in [
            "user.yaml",
            "installation.yaml",
            "keytao.userdb/LOG",
            "log/host.txt",
            ".private.yaml",
            "plugin.dll",
        ] {
            assert!(!is_public_asset(Path::new(path)), "{path}");
        }
        assert!(is_public_asset(Path::new("lua/english.lua")));
        assert!(is_public_asset(Path::new("build/keytao.table.bin")));
    }

    #[test]
    fn publishing_an_addon_keeps_the_prior_public_schema_and_excludes_userdb() {
        let root = std::env::temp_dir().join(format!(
            "keytao-public-schema-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        publish_at(
            &root,
            &[
                (
                    "default.custom.yaml".into(),
                    b"patch:\n  schema_list:\n    - schema: keytao\n".to_vec(),
                ),
                (
                    "keytao.schema.yaml".into(),
                    b"schema: {schema_id: keytao}".to_vec(),
                ),
                ("keytao.userdb/LOG".into(), b"private".to_vec()),
            ],
            false,
        )
        .unwrap();
        let first = std::fs::read_to_string(root.join("current.txt")).unwrap();
        publish_at(
            &root,
            &[(
                "easy_en.schema.yaml".into(),
                b"schema: {schema_id: easy_en}".to_vec(),
            )],
            false,
        )
        .unwrap();
        let files = read_snapshot(&root).unwrap();
        assert!(files.contains_key(Path::new("keytao.schema.yaml")));
        assert!(files.contains_key(Path::new("easy_en.schema.yaml")));
        assert!(!files.contains_key(Path::new("keytao.userdb/LOG")));
        assert!(root.join(first).join("keytao.schema.yaml").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
