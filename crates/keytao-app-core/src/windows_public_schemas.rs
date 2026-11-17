//! Public Rime packages for sandboxed Windows text hosts. Never source these
//! files from the user's Rime directory: it contains private learned words.

#[cfg(target_os = "windows")]
use crate::{
    Core, CoreEvent,
    addon::{
        EASY_EN_ADDON_ID, addon_schema_status_at, official_source_files, prepare_public_package,
    },
    events::InstallProgress,
    scheme::{build_client, fetch_scheme_release},
};
#[cfg(target_os = "windows")]
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

static PUBLISH_LOCK: Mutex<()> = Mutex::new(());
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const LIBRIME_VERSION_FILE: &str = "build/keytao-librime-version.txt";

pub struct PublicSchemaUpdate {
    root: PathBuf,
    previous: Option<String>,
    _backup_lock: Option<std::fs::File>,
    backup: Option<tempfile::TempDir>,
    published: Option<String>,
    published_sources: String,
}

impl PublicSchemaUpdate {
    pub fn rollback(self) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
        let _guard = PUBLISH_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = read_generation(&self.root)?;
        if current != self.published {
            // The child adds build files; a concurrent source update must survive rollback.
            let files = read_snapshot(&self.root)?;
            if snapshot_hash(files.iter().filter(|(path, _)| !path.starts_with("build")))
                != self.published_sources
            {
                return Err("公共方案已被更新，无法自动恢复，请重新部署".into());
            }
        }
        match self.previous {
            Some(generation) => {
                let snapshot = self.root.join(&generation);
                if !snapshot.exists() {
                    std::fs::rename(
                        self.backup.as_ref().unwrap().path().join(&generation),
                        snapshot,
                    )
                    .map_err(|error| format!("restore public schema snapshot: {error}"))?;
                }
                crate::scheme_files::write_file_atomic(
                    &self.root.join("current.txt"),
                    generation.as_bytes(),
                )
                .map_err(|error| error.to_string())
            }
            None => match std::fs::remove_file(self.root.join("current.txt")) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            },
        }?;
        prune_snapshots(&self.root, read_generation(&self.root)?.as_deref());
        Ok(())
    }
}

pub fn update_public_schemas(
    update: impl FnOnce() -> Result<(), String>,
) -> Result<PublicSchemaUpdate, String> {
    #[cfg(target_os = "windows")]
    let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
    update_public_schemas_at(public_root()?, update)
}

fn update_public_schemas_at(
    root: PathBuf,
    update: impl FnOnce() -> Result<(), String>,
) -> Result<PublicSchemaUpdate, String> {
    let previous = read_generation(&root)?;
    let backup = previous
        .as_ref()
        .map(|generation| {
            let backup = tempfile::Builder::new()
                .prefix(".rollback-")
                .tempdir_in(&root)
                .map_err(|error| error.to_string())?;
            let lease = std::fs::File::create(backup.path().join("lease"))
                .map_err(|error| error.to_string())?;
            lease.lock().map_err(|error| error.to_string())?;
            link_snapshot(&root.join(generation), &backup.path().join(generation))?;
            Ok::<_, String>((lease, backup))
        })
        .transpose()?;
    let (_backup_lock, backup) = backup
        .map(|(lease, backup)| (Some(lease), Some(backup)))
        .unwrap_or_default();
    let result = update();
    let files = read_snapshot(&root)?;
    let receipt = PublicSchemaUpdate {
        published: read_generation(&root)?,
        published_sources: snapshot_hash(
            files.iter().filter(|(path, _)| !path.starts_with("build")),
        ),
        previous,
        _backup_lock,
        backup,
        root,
    };
    if let Err(error) = result {
        receipt
            .rollback()
            .map_err(|rollback| format!("{error}；恢复公共方案失败：{rollback}"))?;
        return Err(error);
    }
    Ok(receipt)
}

fn link_snapshot(source: &Path, target: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(source)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("symbolic link in public schema snapshot".into());
    }
    std::fs::create_dir(target).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let output = target.join(entry.file_name());
        if kind.is_dir() {
            link_snapshot(&entry.path(), &output)?;
        } else if kind.is_file() {
            // Keep rollback artifacts without duplicating large dictionaries in memory or on disk.
            std::fs::hard_link(entry.path(), output).map_err(|error| error.to_string())?;
        } else {
            return Err("unsupported file in public schema snapshot".into());
        }
    }
    Ok(())
}

/// Run only in the short-lived deployment child, using public sources exclusively.
pub fn deploy_and_publish() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
    let _guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let version =
        keytao_core::librime_runtime_version().ok_or("无法读取 librime 版本，需要重新部署")?;
    let shared = keytao_core::default_shared_data_dir();
    deploy_and_publish_at(&public_root()?, &version, true, |stage| {
        let user = stage.to_string_lossy().into_owned();
        // A second deployment must switch away from the private user runtime first.
        keytao_core::reinitialize(user.clone(), shared.clone())?;
        keytao_core::deploy(user, shared)
    })
}

fn deploy_and_publish_at(
    root: &Path,
    version: &str,
    grant_access: bool,
    deploy: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let mut files = read_snapshot(root)?;
    if !files.keys().any(|path| {
        path.components().count() == 1 && path.to_string_lossy().ends_with(".schema.yaml")
    }) {
        return Ok(());
    }
    files.retain(|path, _| !path.starts_with("build"));
    // The stage never receives desktop sources, customizations, or learned words.
    let stage = tempfile::tempdir().map_err(|error| error.to_string())?;
    for (path, content) in &files {
        let output = stage.path().join(path);
        std::fs::create_dir_all(output.parent().unwrap()).map_err(|error| error.to_string())?;
        let mut file = std::fs::File::create(output).map_err(|error| error.to_string())?;
        file.write_all(content).map_err(|error| error.to_string())?;
        // librime embeds source mtimes; fresh stages must produce the same build bytes.
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .map_err(|error| error.to_string())?;
    }
    deploy(stage.path())?;
    let build = stage.path().join("build");
    let mut compiled = BTreeMap::new();
    read_snapshot_files(stage.path(), &build, &mut compiled)?;
    if !compiled
        .keys()
        .any(|path| path.to_string_lossy().ends_with(".schema.yaml"))
    {
        return Err("公共方案缺少编译产物，需要重新部署".into());
    }
    files.extend(compiled);
    files.insert(
        LIBRIME_VERSION_FILE.into(),
        version.trim().as_bytes().to_vec(),
    );
    store_snapshot(root, &files, grant_access)
}

#[cfg(target_os = "windows")]
pub async fn prepare_search_schemas(core: &Core) -> Result<(), String> {
    static PREPARING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = PREPARING.lock().await;
    if has_schemas() {
        return Ok(());
    }
    let root =
        keytao_core::default_user_data_dir().ok_or("Cannot determine keytao data directory")?;
    let installed = keytao_core::schema_install_state(&root);
    if !installed.installed {
        return Ok(());
    }
    // Infer the public distribution from schema IDs only. Personal source and
    // learned dictionaries must never be copied into the sandbox-readable seed.
    let scheme = installed
        .schemas
        .iter()
        .find_map(|id| {
            if id.starts_with("xmjd6") {
                Some("xmjd")
            } else if id.starts_with("txjx") {
                Some("txjx")
            } else if id.starts_with("keydo") {
                Some("keydo")
            } else if id.starts_with("keytao") {
                Some("keytao")
            } else {
                None
            }
        })
        .ok_or("无法识别公共方案来源，请在方案页重新安装主方案以启用系统搜索框输入")?;
    core.emit(CoreEvent::InstallProgress(InstallProgress {
        stage: "windows-search".into(),
        percent: 0,
        message: "正在准备 Windows 系统搜索框使用的公共方案…".into(),
    }));
    let release = fetch_scheme_release(core, scheme.into())
        .await
        .map_err(|e| e.to_string())?;
    let response = build_client(core)?
        .get(&release.download_url)
        .timeout(std::time::Duration::from_secs(180))
        .send()
        .await
        .map_err(|e| format!("下载搜索框公共方案失败：{e}"))?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if bytes.len().saturating_add(chunk.len()) > 128 * 1024 * 1024 {
            return Err("搜索框公共方案压缩包过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    publish_package(&bytes)?;
    if addon_schema_status_at(&root, EASY_EN_ADDON_ID).installed {
        prepare_public_package(&root, &core.env().data_dir.join("easy-en-public")).await?;
        publish_english_addon(core)?;
    }
    publish_english_settings(
        &root,
        keytao_core::english_mode::read_english_mode(&root)
            == keytao_core::english_mode::EnglishMode::Schema,
    )?;
    core.emit(CoreEvent::InstallProgress(InstallProgress {
        stage: "done".into(),
        percent: 100,
        message: "公共方案已准备，请重新部署以启用系统搜索框输入".into(),
    }));
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn publish_english_addon(core: &Core) -> Result<(), String> {
    let files = official_source_files(&core.env().data_dir.join("easy-en-public"))?;
    publish_files(&files)
}

fn public_root() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|path| PathBuf::from(path).join("KeyTao/appcontainer-schemas"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable for public input schemas".into())
}

fn checked_relative(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty()
        || path.to_string_lossy().starts_with('\\')
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
    if path == Path::new(keytao_core::SCHEMA_SWITCH_REQUEST_FILE) {
        return true;
    }
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
    #[cfg(target_os = "windows")]
    let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
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
    crate::scheme_files::parse_schema_list(&config)
        .iter()
        .any(|schema| {
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
    #[cfg(target_os = "windows")]
    let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
    let _guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = public_root()?;
    let mut files = read_snapshot(&root)?;
    files.retain(|path, _| !path.starts_with("build"));
    for relative in package_paths {
        files.remove(&checked_relative(relative)?);
    }
    files.remove(&PathBuf::from(format!("{schema_id}.schema.yaml")));
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

/// Publish input preferences without exposing private metadata or dictionaries.
pub fn publish_english_settings(root: &Path, enabled: bool) -> Result<(), String> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "englishMode": if enabled { "schema" } else { "ascii" },
        "candidatePageSize": crate::ime_settings::read_candidate_page_size(root),
    }))
    .map_err(|error| error.to_string())?;
    publish_files(&[
        (PathBuf::from("keytao-ime.json"), bytes),
        crate::ime_settings::schema_switch_settings_file(root)?,
    ])
}

pub fn publish_schema_switches(root: &Path) -> Result<(), String> {
    publish_files(&[crate::ime_settings::schema_switch_settings_file(root)?])
}

fn read_generation(root: &Path) -> Result<Option<String>, String> {
    let current = match std::fs::read_to_string(root.join("current.txt")) {
        Ok(current) => current,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let current = current.trim();
    if current.len() != 64 || !current.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid public schema snapshot identifier".into());
    }
    Ok(Some(current.to_owned()))
}

fn read_snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    let Some(current) = read_generation(root)? else {
        return Ok(files);
    };
    let snapshot = root.join(current);
    if std::fs::symlink_metadata(&snapshot)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("symbolic link at public schema snapshot root".into());
    }
    read_snapshot_files(&snapshot, &snapshot, &mut files)?;
    Ok(files)
}

fn read_snapshot_files(
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
            read_snapshot_files(base, &entry.path(), files)?;
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

fn compilation_inputs(path: &Path) -> bool {
    !path.starts_with("build")
        && path != Path::new(keytao_core::english_mode::SETTINGS_FILE)
        && path != Path::new(keytao_core::SCHEMA_SWITCH_REQUEST_FILE)
}

fn publish_at(
    root: &Path,
    incoming: &[(PathBuf, Vec<u8>)],
    grant_access: bool,
) -> Result<(), String> {
    let mut files = read_snapshot(root)?;
    let previous_sources = snapshot_hash(files.iter().filter(|(path, _)| compilation_inputs(path)));
    let has_schema_list = incoming.iter().any(|(path, _)| {
        path == Path::new("default.custom.yaml") || path == Path::new("default-custom.yaml")
    });
    let mut addon_schemas = Vec::new();
    for (path, content) in incoming {
        let relative = checked_relative(path)?;
        if !is_public_asset(&relative) || relative.starts_with("build") {
            continue;
        }
        if relative == Path::new("default.custom.yaml")
            || relative == Path::new("default-custom.yaml")
        {
            let old = files
                .get(Path::new("default.custom.yaml"))
                .and_then(|bytes| std::str::from_utf8(bytes).ok());
            let new = std::str::from_utf8(content).map_err(|error| error.to_string())?;
            let (merged, _) = crate::install::merge_default_custom(old, new);
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
    if let Some(settings) = files.get(Path::new(keytao_core::english_mode::SETTINGS_FILE)) {
        let size = serde_json::from_slice::<serde_json::Value>(settings)
            .ok()
            .and_then(|settings| settings["candidatePageSize"].as_u64())
            .filter(|size| (1..=10).contains(size))
            .map(|size| size as u8);
        let custom_paths = files
            .keys()
            .filter(|path| path.components().count() == 1)
            .filter_map(|path| path.to_str()?.strip_suffix(".schema.yaml"))
            .map(|id| PathBuf::from(format!("{id}.custom.yaml")))
            .collect::<Vec<_>>();
        for path in custom_paths {
            let result = (|| -> Result<(), String> {
                let content = files
                    .get(&path)
                    .map(|bytes| std::str::from_utf8(bytes))
                    .transpose()
                    .map_err(|error| error.to_string())?
                    .unwrap_or("");
                let patched = crate::candidate_page_size::patch_candidate_page_size(content, size)?;
                if patched != content {
                    files.insert(path.clone(), patched.into_bytes());
                }
                Ok(())
            })();
            if let Err(error) = result {
                tracing::warn!(path = %path.display(), %error, "Skipped public candidate page size override");
            }
        }
    }
    if previous_sources != snapshot_hash(files.iter().filter(|(path, _)| compilation_inputs(path)))
    {
        files.retain(|path, _| !path.starts_with("build"));
    }
    store_snapshot(root, &files, grant_access)
}

fn snapshot_hash<'a>(files: impl Iterator<Item = (&'a PathBuf, &'a Vec<u8>)>) -> String {
    let mut hash = Sha256::new();
    for (path, content) in files {
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    format!("{:x}", hash.finalize())
}

fn store_snapshot(
    root: &Path,
    files: &BTreeMap<PathBuf, Vec<u8>>,
    grant_access: bool,
) -> Result<(), String> {
    let generation = snapshot_hash(files.iter());
    let snapshot = root.join(&generation);
    std::fs::create_dir_all(&snapshot).map_err(|error| error.to_string())?;
    for (path, content) in files {
        let output = snapshot.join(path);
        std::fs::create_dir_all(output.parent().unwrap()).map_err(|error| error.to_string())?;
        crate::scheme_files::write_file_atomic(&output, content)
            .map_err(|error| error.to_string())?;
    }
    if grant_access {
        grant_appcontainer_read(root)?;
    }
    crate::scheme_files::write_file_atomic(&root.join("current.txt"), generation.as_bytes())
        .map_err(|error| error.to_string())?;
    prune_snapshots(root, Some(&generation));
    Ok(())
}

fn prune_snapshots(root: &Path, generation: Option<&str>) {
    let cleanup = || -> std::io::Result<()> {
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let old_snapshot = Some(name.as_ref()) != generation
                && name.len() == 64
                && name.bytes().all(|byte| byte.is_ascii_hexdigit());
            // A live transaction holds the lease; crashes release it for the next cleanup.
            let lease = if name.starts_with(".rollback-") {
                match std::fs::File::open(entry.path().join("lease")) {
                    Ok(lease) if lease.try_lock().is_ok() => Some(lease),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    _ => continue,
                }
            } else if old_snapshot {
                None
            } else {
                continue;
            };
            if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                tracing::warn!(%error, "old public schema snapshot will be removed on the next publication");
            }
            drop(lease);
        }
        Ok(())
    };
    if let Err(error) = cleanup() {
        tracing::warn!(%error, "public schema cleanup will be retried on the next publication");
    }
}

#[cfg(target_os = "windows")]
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

#[cfg(not(target_os = "windows"))]
fn grant_appcontainer_read(_root: &Path) -> Result<(), String> {
    Err("public schema ACLs require Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_deploys_reuse_generation_and_prune_old_snapshots() {
        let root = tempfile::tempdir().unwrap();
        publish_at(
            root.path(),
            &[(
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao}".to_vec(),
            )],
            false,
        )
        .unwrap();
        let sources = read_generation(root.path()).unwrap().unwrap();
        let unrelated = root.path().join("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        let mut generation = None;
        for _ in 0..3 {
            deploy_and_publish_at(root.path(), "1.0", false, |stage| {
                let modified = std::fs::metadata(stage.join("keytao.schema.yaml"))
                    .unwrap()
                    .modified()
                    .unwrap();
                assert_eq!(
                    modified,
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)
                );
                std::fs::create_dir(stage.join("build")).unwrap();
                std::fs::write(
                    stage.join("build/keytao.schema.yaml"),
                    format!("__build_info: {{timestamps: {{keytao: {modified:?}}}}}"),
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
            let current = read_generation(root.path()).unwrap();
            if let Some(previous) = &generation {
                assert_eq!(previous, &current);
            }
            generation = Some(current);
            assert!(!root.path().join(&sources).exists());
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 3);
            assert!(unrelated.is_dir());
        }
    }

    #[test]
    fn pruning_keeps_live_rollback_backups_and_reclaims_abandoned_ones() {
        let root = tempfile::tempdir().unwrap();
        publish_at(
            root.path(),
            &[("first.txt".into(), b"first".to_vec())],
            false,
        )
        .unwrap();
        let update = update_public_schemas_at(root.path().into(), || {
            publish_at(
                root.path(),
                &[("second.txt".into(), b"second".to_vec())],
                false,
            )
        })
        .unwrap();
        let live = update.backup.as_ref().unwrap().path().to_owned();
        assert!(live.is_dir());
        let abandoned = tempfile::Builder::new()
            .prefix(".rollback-")
            .tempdir_in(root.path())
            .unwrap();
        std::fs::write(abandoned.path().join("lease"), b"").unwrap();
        let abandoned = abandoned.keep();
        publish_at(
            root.path(),
            &[("third.txt".into(), b"third".to_vec())],
            false,
        )
        .unwrap();
        assert!(live.is_dir());
        assert!(!abandoned.exists());
        drop(update);
        assert!(!live.exists());
    }

    #[test]
    fn addon_rollback_restores_prior_builds_and_preserves_concurrent_publication() {
        let root = tempfile::tempdir().unwrap();
        let sources = vec![(
            "keytao.schema.yaml".into(),
            b"schema: {schema_id: keytao}".to_vec(),
        )];
        publish_at(root.path(), &sources, false).unwrap();
        let mut initial = read_snapshot(root.path()).unwrap();
        initial.insert(
            "build/keytao.table.bin".into(),
            b"original dictionary".to_vec(),
        );
        initial.insert(LIBRIME_VERSION_FILE.into(), b"1.0".to_vec());
        store_snapshot(root.path(), &initial, false).unwrap();
        let original = read_generation(root.path()).unwrap();
        let addon = vec![(
            "easy_en.schema.yaml".into(),
            b"schema: {schema_id: easy_en}".to_vec(),
        )];
        let update = update_public_schemas_at(root.path().into(), || {
            publish_at(root.path(), &addon, false)?;
            publish_at(
                root.path(),
                &[("keytao-ime.json".into(), b"{}".to_vec())],
                false,
            )
        })
        .unwrap();
        let mut compiled = read_snapshot(root.path()).unwrap();
        compiled.insert(LIBRIME_VERSION_FILE.into(), b"1.0".to_vec());
        store_snapshot(root.path(), &compiled, false).unwrap();
        update.rollback().unwrap();
        assert_eq!(read_generation(root.path()).unwrap(), original);
        assert_eq!(read_snapshot(root.path()).unwrap(), initial);
        assert_eq!(
            read_snapshot(root.path()).unwrap()[Path::new("keytao.schema.yaml")],
            sources[0].1
        );

        let update = update_public_schemas_at(root.path().into(), || {
            publish_at(root.path(), &addon, false)
        })
        .unwrap();
        publish_at(
            root.path(),
            &[(
                "another.schema.yaml".into(),
                b"schema: {schema_id: another}".to_vec(),
            )],
            false,
        )
        .unwrap();
        let newer = read_generation(root.path()).unwrap();
        assert!(update.rollback().is_err());
        assert_eq!(read_generation(root.path()).unwrap(), newer);
        assert!(
            update_public_schemas_at(root.path().into(), || {
                publish_at(root.path(), &sources, false)?;
                Err("publication failed".into())
            })
            .is_err()
        );
        assert_eq!(read_generation(root.path()).unwrap(), newer);
    }

    #[test]
    fn compiled_publication_uses_only_clean_public_sources_and_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::from([
            (
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao}".to_vec(),
            ),
            ("build/old.table.bin".into(), b"obsolete artifact".to_vec()),
            (LIBRIME_VERSION_FILE.into(), b"old version".to_vec()),
        ]);
        store_snapshot(root.path(), &files, false).unwrap();
        let before = std::fs::read(root.path().join("current.txt")).unwrap();
        let failure = deploy_and_publish_at(root.path(), "1.0", false, |stage| {
            assert!(!stage.join("build").exists());
            assert!(!stage.join("keytao.userdb").exists());
            assert_eq!(
                std::fs::read(stage.join("keytao.schema.yaml")).unwrap(),
                files[Path::new("keytao.schema.yaml")]
            );
            Err("public compile failed".into())
        });
        assert_eq!(failure, Err("public compile failed".into()));
        assert_eq!(
            std::fs::read(root.path().join("current.txt")).unwrap(),
            before
        );
        deploy_and_publish_at(root.path(), "1.0", false, |stage| {
            assert!(!stage.join("build").exists());
            std::fs::create_dir(stage.join("build")).unwrap();
            std::fs::write(
                stage.join("build/keytao.schema.yaml"),
                "compiled public schema",
            )
            .unwrap();
            std::fs::write(
                stage.join("build/keytao.table.bin"),
                "compiled public words",
            )
            .unwrap();
            std::fs::create_dir(stage.join("build/private.userdb")).unwrap();
            std::fs::write(stage.join("build/private.userdb/LOG"), "private").unwrap();
            Ok(())
        })
        .unwrap();
        files = read_snapshot(root.path()).unwrap();
        assert_eq!(files[Path::new(LIBRIME_VERSION_FILE)], b"1.0");
        assert_eq!(
            files[Path::new("build/keytao.table.bin")],
            b"compiled public words"
        );
        assert!(!files.contains_key(Path::new("build/old.table.bin")));
        assert!(!files.contains_key(Path::new("build/private.userdb/LOG")));
        assert!(
            !root
                .path()
                .join(std::str::from_utf8(&before).unwrap())
                .exists()
        );

        publish_at(
            root.path(),
            &[(
                keytao_core::SCHEMA_SWITCH_REQUEST_FILE.into(),
                b"{}".to_vec(),
            )],
            false,
        )
        .unwrap();
        assert!(
            read_snapshot(root.path())
                .unwrap()
                .contains_key(Path::new(LIBRIME_VERSION_FILE))
        );
        publish_at(
            root.path(),
            &[(
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao, version: '2'}".to_vec(),
            )],
            false,
        )
        .unwrap();
        assert!(
            !read_snapshot(root.path())
                .unwrap()
                .keys()
                .any(|path| path.starts_with("build"))
        );
    }

    #[test]
    fn incoming_builds_cannot_forge_a_stamp_and_candidate_changes_invalidate_builds() {
        let root = tempfile::tempdir().unwrap();
        publish_at(
            root.path(),
            &[
                (
                    "keytao.schema.yaml".into(),
                    b"schema: {schema_id: keytao}".to_vec(),
                ),
                (LIBRIME_VERSION_FILE.into(), b"forged".to_vec()),
            ],
            false,
        )
        .unwrap();
        let mut files = read_snapshot(root.path()).unwrap();
        assert!(!files.contains_key(Path::new(LIBRIME_VERSION_FILE)));
        files.insert(LIBRIME_VERSION_FILE.into(), b"1.0".to_vec());
        store_snapshot(root.path(), &files, false).unwrap();
        publish_at(
            root.path(),
            &[(
                "keytao-ime.json".into(),
                br#"{"candidatePageSize":7}"#.to_vec(),
            )],
            false,
        )
        .unwrap();
        let files = read_snapshot(root.path()).unwrap();
        assert!(files.contains_key(Path::new("keytao.custom.yaml")));
        assert!(!files.contains_key(Path::new(LIBRIME_VERSION_FILE)));

        let empty = tempfile::tempdir().unwrap();
        publish_at(
            empty.path(),
            &[("keytao-ime.json".into(), b"{}".to_vec())],
            false,
        )
        .unwrap();
        deploy_and_publish_at(empty.path(), "1.0", false, |_| {
            panic!("no public schemas to compile")
        })
        .unwrap();
    }

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
        assert!(is_public_asset(Path::new(
            keytao_core::SCHEMA_SWITCH_REQUEST_FILE
        )));
        assert!(!is_public_asset(Path::new(
            keytao_core::SCHEMA_SWITCH_APPLIED_FILE
        )));
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
                (
                    keytao_core::SCHEMA_SWITCH_REQUEST_FILE.into(),
                    br#"{"generation":1,"options":{"mars":false}}"#.to_vec(),
                ),
                (
                    keytao_core::SCHEMA_SWITCH_APPLIED_FILE.into(),
                    b"1".to_vec(),
                ),
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
        assert_eq!(
            files[Path::new(keytao_core::SCHEMA_SWITCH_REQUEST_FILE)],
            br#"{"generation":1,"options":{"mars":false}}"#
        );
        assert!(!files.contains_key(Path::new(keytao_core::SCHEMA_SWITCH_APPLIED_FILE)));
        assert!(!root.join(first).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
