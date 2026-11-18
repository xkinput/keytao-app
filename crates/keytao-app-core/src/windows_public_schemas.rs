//! Public Rime packages for sandboxed Windows text hosts. Never source these
//! files from the user's Rime directory: it contains private learned words.

#[cfg(target_os = "windows")]
use crate::{
    Core, CoreEvent,
    addon::EASY_EN_ADDON_ID,
    events::InstallProgress,
    scheme::{build_client, fetch_scheme_release},
};
#[cfg(target_os = "windows")]
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

static PUBLISH_LOCK: Mutex<()> = Mutex::new(());
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const LIBRIME_VERSION_FILE: &str = "build/keytao-librime-version.txt";

// LSSP contains repeated patch keys outside this metadata; preserve them verbatim.
#[derive(serde::Deserialize, Default)]
struct SchemaMetadata {
    #[serde(default)]
    schema: SchemaInfo,
}

#[derive(serde::Deserialize, Default)]
struct SchemaInfo {
    #[serde(default)]
    schema_id: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

pub fn public_warning(stage: &str, error: &str) -> String {
    #[cfg(target_os = "windows")]
    if let Some(root) = keytao_core::default_user_data_dir() {
        keytao_core::runtime_log::init_for_engine(&root);
    }
    tracing::warn!(
        stage,
        error = error,
        "Windows public schemas were not updated"
    );
    keytao_core::rt_log!(
        keytao_core::runtime_log::Level::Info,
        "rime",
        "public_schema_warning",
        stage = stage,
        msg = error,
        details = public_error_details(error)
    );
    format!("{stage}：{error}")
}

fn public_error_details(error: &str) -> Vec<String> {
    let error = dirs::home_dir()
        .filter(|home| !home.as_os_str().is_empty())
        .map(|home| error.replace(home.to_string_lossy().as_ref(), "<user>"))
        .unwrap_or_else(|| error.to_owned());
    error
        .chars()
        .collect::<Vec<_>>()
        .chunks(128)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

pub fn after_private_deploy(
    private: Result<(), String>,
    public: impl FnOnce() -> Result<(), String>,
) -> Result<Option<String>, String> {
    private?;
    Ok(public()
        .err()
        .map(|error| public_warning("public-build", &error)))
}

pub struct PublicSchemaUpdate {
    root: PathBuf,
    previous: GenerationPointers,
    _backup_lock: Option<std::fs::File>,
    backup: Option<tempfile::TempDir>,
    published: GenerationPointers,
    published_sources: String,
}

#[derive(Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct GenerationPointers {
    current: Option<String>,
    pending: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct PublicSchemaCheckpoint {
    root: PathBuf,
    previous: GenerationPointers,
}

#[cfg(target_os = "windows")]
pub(crate) fn checkpoint(backup: &Path) -> Result<PublicSchemaCheckpoint, String> {
    #[cfg(target_os = "windows")]
    let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
    let _guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    checkpoint_at(public_root()?, backup)
}

pub(crate) fn checkpoint_at(
    root: PathBuf,
    backup: &Path,
) -> Result<PublicSchemaCheckpoint, String> {
    let previous = GenerationPointers::read(&root)?;
    std::fs::create_dir_all(backup).map_err(|error| error.to_string())?;
    for generation in previous.generations() {
        if !backup.join(generation).exists() {
            link_snapshot(&root.join(generation), &backup.join(generation))?;
        }
    }
    Ok(PublicSchemaCheckpoint { root, previous })
}

impl PublicSchemaCheckpoint {
    pub(crate) fn restore(&self, backup: &Path) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
        let _guard = PUBLISH_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        for generation in self.previous.generations() {
            if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("invalid public schema checkpoint identifier".into());
            }
            if !self.root.join(generation).exists() {
                let stage = tempfile::Builder::new()
                    .prefix(".restore-")
                    .tempdir_in(&self.root)
                    .map_err(|error| error.to_string())?;
                link_snapshot(&backup.join(generation), &stage.path().join(generation))?;
                std::fs::rename(stage.path().join(generation), self.root.join(generation))
                    .map_err(|error| error.to_string())?;
            }
        }
        #[cfg(target_os = "windows")]
        grant_appcontainer_read(&self.root)?;
        self.previous.write(&self.root)?;
        prune_snapshots(&self.root);
        Ok(())
    }
}

impl GenerationPointers {
    fn read(root: &Path) -> Result<Self, String> {
        Ok(Self {
            current: read_generation_pointer(root, "current.txt")?,
            pending: read_generation_pointer(root, "pending.txt")?,
        })
    }

    fn write(&self, root: &Path) -> Result<(), String> {
        for (name, generation) in [
            ("current.txt", &self.current),
            ("pending.txt", &self.pending),
        ] {
            if read_generation_pointer(root, name)?.as_ref() == generation.as_ref() {
                continue;
            }
            match generation {
                Some(generation) => {
                    crate::scheme_files::write_file_atomic(&root.join(name), generation.as_bytes())
                        .map_err(|error| format!("write public schema {name}: {error}"))?
                }
                None => match std::fs::remove_file(root.join(name)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("remove public schema {name}: {error}")),
                },
            }
        }
        Ok(())
    }

    fn generations(&self) -> impl Iterator<Item = &String> {
        self.current.iter().chain(self.pending.iter())
    }
}

impl PublicSchemaUpdate {
    pub fn rollback(self) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
        let _guard = PUBLISH_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = GenerationPointers::read(&self.root)?;
        if current != self.published {
            // The child adds build files; a concurrent source update must survive rollback.
            let files = read_snapshot(&self.root)?;
            if snapshot_hash(files.iter().filter(|(path, _)| !path.starts_with("build")))
                != self.published_sources
            {
                return Err("公共方案已被更新，无法自动恢复，请重新部署".into());
            }
        }
        for generation in self.previous.generations() {
            let snapshot = self.root.join(generation);
            if !snapshot.exists() {
                std::fs::rename(
                    self.backup.as_ref().unwrap().path().join(generation),
                    snapshot,
                )
                .map_err(|error| format!("restore public schema snapshot: {error}"))?;
            }
        }
        self.previous.write(&self.root)?;
        prune_snapshots(&self.root);
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
    let previous = GenerationPointers::read(&root)?;
    let backup = if previous.generations().next().is_some() {
        Some((|| {
            let backup = tempfile::Builder::new()
                .prefix(".rollback-")
                .tempdir_in(&root)
                .map_err(|error| error.to_string())?;
            let lease = std::fs::File::create(backup.path().join("lease"))
                .map_err(|error| error.to_string())?;
            lease.lock().map_err(|error| error.to_string())?;
            for generation in previous.generations() {
                if !backup.path().join(generation).exists() {
                    link_snapshot(&root.join(generation), &backup.path().join(generation))?;
                }
            }
            Ok::<_, String>((lease, backup))
        })()?)
    } else {
        None
    };
    let (_backup_lock, backup) = backup
        .map(|(lease, backup)| (Some(lease), Some(backup)))
        .unwrap_or_default();
    let result = update();
    let files = read_snapshot(&root)?;
    let receipt = PublicSchemaUpdate {
        published: GenerationPointers::read(&root)?,
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
            // Redirected app data can put the durable transaction on a different volume.
            std::fs::hard_link(entry.path(), &output)
                .or_else(|_| std::fs::copy(entry.path(), output).map(|_| ()))
                .map_err(|error| error.to_string())?;
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
        keytao_core::deploy(user, shared)
    })
}

fn deploy_and_publish_at(
    root: &Path,
    version: &str,
    grant_access: bool,
    deploy: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let mut files = read_snapshot(root).map_err(|error| format!("read-snapshot: {error}"))?;
    if !files.keys().any(|path| {
        path.components().count() == 1 && path.to_string_lossy().ends_with(".schema.yaml")
    }) {
        return Ok(());
    }
    files.retain(|path, _| !path.starts_with("build"));
    // The stage never receives desktop sources, customizations, or learned words.
    let stage = tempfile::tempdir().map_err(|error| format!("create-stage: {error}"))?;
    for (path, content) in &files {
        let output = stage.path().join(path);
        std::fs::create_dir_all(output.parent().unwrap())
            .map_err(|error| format!("stage-source {}: {error}", path.display()))?;
        let mut file = std::fs::File::create(output)
            .map_err(|error| format!("stage-source {}: {error}", path.display()))?;
        file.write_all(content)
            .map_err(|error| format!("stage-source {}: {error}", path.display()))?;
        // librime embeds source mtimes; fresh stages must produce the same build bytes.
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .map_err(|error| format!("stage-source {}: {error}", path.display()))?;
    }
    trim_missing_dependencies(stage.path())
        .map_err(|error| format!("prepare-dependencies: {error}"))?;
    deploy(stage.path()).map_err(|error| format!("compile: {error}"))?;
    let build = stage.path().join("build");
    let mut compiled = BTreeMap::new();
    read_snapshot_files(stage.path(), &build, &mut compiled)
        .map_err(|error| format!("collect-artifacts: {error}"))?;
    if !compiled
        .keys()
        .any(|path| path.to_string_lossy().ends_with(".schema.yaml"))
    {
        return Err("collect-artifacts: 公共方案缺少编译产物，需要重新部署".into());
    }
    files.extend(compiled);
    files.insert(
        LIBRIME_VERSION_FILE.into(),
        version.trim().as_bytes().to_vec(),
    );
    store_snapshot(root, &files, grant_access).map_err(|error| format!("publish: {error}"))
}

fn trim_missing_dependencies(stage: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(stage).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if !path.to_string_lossy().ends_with(".schema.yaml") {
            continue;
        }
        let content = std::fs::read(&path).map_err(|error| error.to_string())?;
        let metadata: SchemaMetadata = serde_yaml::from_slice(&content)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        for dependency in &metadata.schema.dependencies {
            checked_relative(Path::new(&format!("{dependency}.schema.yaml")))?;
        }
        if metadata
            .schema
            .dependencies
            .iter()
            .all(|id| stage.join(format!("{id}.schema.yaml")).is_file())
        {
            continue;
        }
        let mut schema: serde_yaml::Value = serde_yaml::from_slice(&content)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let Some(dependencies) = schema
            .get_mut("schema")
            .and_then(|schema| schema.get_mut("dependencies"))
            .and_then(serde_yaml::Value::as_sequence_mut)
        else {
            continue;
        };
        for dependency in dependencies.iter().filter_map(serde_yaml::Value::as_str) {
            checked_relative(Path::new(&format!("{dependency}.schema.yaml")))?;
        }
        let before = dependencies.len();
        dependencies.retain(|dependency| {
            dependency
                .as_str()
                .is_none_or(|id| stage.join(format!("{id}.schema.yaml")).is_file())
        });
        if before != dependencies.len() {
            // Only the isolated public build may omit dependencies absent from its package.
            std::fs::write(
                &path,
                serde_yaml::to_string(&schema).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|file| {
                    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
                })
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
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
    if !installed.installed
        || installed
            .schemas
            .iter()
            .all(|id| keytao_core::is_addon_schema(id))
    {
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
            } else if id == crate::rime_snow::SCHEMA_ID {
                Some(crate::rime_snow::SCHEMA_ID)
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
    let bytes = if scheme == crate::rime_snow::SCHEMA_ID {
        crate::rime_snow::prepare_archive(core, bytes).map_err(|error| error.to_string())?
    } else {
        bytes
    };
    publish_package(&bytes)?;
    if crate::addon::addon_schema_status(core, || Ok(root.clone()), EASY_EN_ADDON_ID.into())
        .map_err(|error| error.to_string())?.installed {
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
    let files = crate::addon_store::public_files(core, EASY_EN_ADDON_ID)?;
    let files = files.into_iter().map(|(path, relative)| {
        std::fs::read(path).map(|bytes| (relative, bytes)).map_err(|error| error.to_string())
    }).collect::<Result<Vec<_>, _>>()?;
    publish_files(&files)
}

fn public_root() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|path| PathBuf::from(path).join("KeyTao/appcontainer-schemas"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable for public input schemas".into())
}

#[cfg(target_os = "windows")]
pub(crate) fn is_default_user_root(root: &Path) -> bool {
    keytao_core::default_user_data_dir().is_some_and(|default| same_directory(&default, root))
}

fn same_directory(left: &Path, right: &Path) -> bool {
    left.canonicalize()
        .ok()
        .zip(right.canonicalize().ok())
        .is_some_and(|(left, right)| left == right)
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
    if path == Path::new(keytao_core::SCHEMA_SWITCH_REQUEST_FILE)
        || path == Path::new("build/.keytao-skipped-schemas.json")
        || path == Path::new("lua/rime_ice/lunar.db")
        || path == Path::new("zh-moqi.gram")
    {
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

/// Both directories must be package-store sources, never the private Rime root.
pub fn publish_activation(
    core: &crate::Core,
    incoming: &Path,
    previous: Option<&Path>,
    private_root: &Path,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
    let _guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    publish_activation_at(
        &public_root()?,
        Some(incoming),
        previous,
        &crate::package_activation::addon_ownership(core, private_root)
            .map_err(|error| error.to_string())?,
        true,
    )
}

pub fn remove_package(core: &crate::Core, directory: &Path, private_root: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let _engine_guard = crate::ime_status::windows::WindowsImeEngineInitGuard::acquire()?;
    let _guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    publish_activation_at(&public_root()?, None, Some(directory),
        &crate::package_activation::addon_ownership(core, private_root)
            .map_err(|error| error.to_string())?, true)
}

fn package_directory_files(directory: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    fn collect(
        base: &Path,
        directory: &Path,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
        total: &mut u64,
    ) -> Result<(), String> {
        if std::fs::symlink_metadata(directory)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("symbolic link in public package directory".into());
        }
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let relative = checked_relative(entry.path().strip_prefix(base).unwrap())?;
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                return Err("symbolic link in public package directory".into());
            }
            if kind.is_dir() {
                collect(base, &entry.path(), files, total)?;
            } else if kind.is_file() && is_public_asset(&relative) && !relative.starts_with("build")
            {
                *total = total
                    .checked_add(metadata.len())
                    .ok_or("public package is too large")?;
                if *total > MAX_PACKAGE_BYTES {
                    return Err("public package is too large".into());
                }
                files.insert(
                    relative,
                    std::fs::read(entry.path()).map_err(|error| error.to_string())?,
                );
            } else if !kind.is_file() {
                return Err("unsupported file in public package directory".into());
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    collect(directory, directory, &mut files, &mut 0)?;
    Ok(files)
}

fn publish_activation_at(
    root: &Path,
    incoming: Option<&Path>,
    previous: Option<&Path>,
    addons: &BTreeMap<String, String>,
    grant_access: bool,
) -> Result<(), String> {
    let mut files = read_snapshot(root)?;
    let before = snapshot_hash(files.iter().filter(|(path, _)| compilation_inputs(path)));
    if let Some(previous) = previous {
        let previous = package_directory_files(previous)?
            .into_iter()
            .filter(|(path, _)| {
                !addons.contains_key(&path.to_string_lossy().replace('\\', "/"))
            })
            .collect::<BTreeMap<_, _>>();
        let schemas: BTreeSet<_> = previous
            .keys()
            .filter_map(|path| {
                path.to_str()?
                    .strip_suffix(".schema.yaml")
                    .map(str::to_owned)
            })
            .collect();
        for path in previous.keys() {
            if path != Path::new("default.custom.yaml") && path != Path::new("default-custom.yaml")
            {
                files.remove(path);
            }
        }
        for schema in &schemas {
            files.remove(&PathBuf::from(format!("{schema}.custom.yaml")));
        }
        if let Some(content) = files.get_mut(Path::new("default.custom.yaml")) {
            let mut document: serde_yaml::Value =
                serde_yaml::from_slice(content).map_err(|error| error.to_string())?;
            if let Some(patch) = document
                .get_mut("patch")
                .and_then(serde_yaml::Value::as_mapping_mut)
            {
                for name in ["default.custom.yaml", "default-custom.yaml"] {
                    if let Some(bytes) = previous.get(Path::new(name)) {
                        let original: serde_yaml::Value =
                            serde_yaml::from_slice(bytes).map_err(|error| error.to_string())?;
                        if let Some(original) = original
                            .get("patch")
                            .and_then(serde_yaml::Value::as_mapping)
                        {
                            for (key, value) in original {
                                if key.as_str() != Some("schema_list")
                                    && patch.get(key) == Some(value)
                                {
                                    patch.remove(key);
                                }
                            }
                        }
                    }
                }
                if let Some(list) = patch
                    .get_mut("schema_list")
                    .and_then(serde_yaml::Value::as_sequence_mut)
                {
                    list.retain(|entry| {
                        entry
                            .get("schema")
                            .and_then(serde_yaml::Value::as_str)
                            .is_none_or(|id| !schemas.contains(id))
                    });
                }
            }
            *content = serde_yaml::to_string(&document)
                .map_err(|error| error.to_string())?
                .into_bytes();
        }
    }
    if let Some(incoming) = incoming {
        let incoming = package_directory_files(incoming)?
            .into_iter()
            .filter(|(path, _)| {
                !crate::package_activation::user_file(&path.to_string_lossy().replace('\\', "/"))
                    && !addons.contains_key(&path.to_string_lossy().replace('\\', "/"))
            })
            .collect::<Vec<_>>();
        merge_public_files(&mut files, &incoming)?;
    }
    // Reuse the private activation's generators on an isolated, package-only stage.
    let stage = tempfile::tempdir().map_err(|error| error.to_string())?;
    for (path, content) in files.iter().filter(|(path, _)| !path.starts_with("build")) {
        let output = stage.path().join(path);
        std::fs::create_dir_all(output.parent().unwrap()).map_err(|error| error.to_string())?;
        std::fs::write(output, content).map_err(|error| error.to_string())?;
    }
    let schemas = crate::managed_custom::active_schema_ids_from_default(stage.path())?;
    crate::managed_custom::regenerate(stage.path(), &schemas)?;
    let mut generated = BTreeMap::new();
    read_snapshot_files(stage.path(), stage.path(), &mut generated)?;
    files.retain(|path, _| path.starts_with("build"));
    files.extend(generated);
    if before != snapshot_hash(files.iter().filter(|(path, _)| compilation_inputs(path))) {
        files.retain(|path, _| !path.starts_with("build"));
    }
    store_snapshot(root, &files, grant_access)
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
            ["keytao", "xmjd", "txjx", "keydo", "snow_jiandao"]
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
    remove_schema_at(&root, schema_id, package_paths, true)
}

fn remove_schema_at(
    root: &Path,
    schema_id: &str,
    package_paths: &[PathBuf],
    grant_access: bool,
) -> Result<(), String> {
    let mut files = read_snapshot(root)?;
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
    store_snapshot(root, &files, grant_access)
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
    let pointers = GenerationPointers::read(root)?;
    Ok(pointers.pending.or(pointers.current))
}

fn read_generation_pointer(root: &Path, name: &str) -> Result<Option<String>, String> {
    let current = match std::fs::read_to_string(root.join(name)) {
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

pub(crate) fn publish_at(
    root: &Path,
    incoming: &[(PathBuf, Vec<u8>)],
    grant_access: bool,
) -> Result<(), String> {
    let mut files = read_snapshot(root)?;
    let previous_sources = snapshot_hash(files.iter().filter(|(path, _)| compilation_inputs(path)));
    merge_public_files(&mut files, incoming)?;
    if previous_sources != snapshot_hash(files.iter().filter(|(path, _)| compilation_inputs(path)))
    {
        files.retain(|path, _| !path.starts_with("build"));
    }
    store_snapshot(root, &files, grant_access)
}

fn merge_public_files(
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
    incoming: &[(PathBuf, Vec<u8>)],
) -> Result<(), String> {
    let has_schema_list = incoming.iter().any(|(path, _)| {
        path == Path::new("default.custom.yaml") || path == Path::new("default-custom.yaml")
    });
    let mut dependency_schemas = BTreeSet::new();
    let mut has_official_addon = false;
    if !has_schema_list {
        for (path, content) in incoming {
            if path.components().count() == 1 && path.to_string_lossy().ends_with(".schema.yaml") {
                let schema: SchemaMetadata =
                    serde_yaml::from_slice(content).map_err(|error| error.to_string())?;
                has_official_addon |= keytao_core::is_addon_schema(&schema.schema.schema_id);
                dependency_schemas.extend(schema.schema.dependencies);
            }
        }
    }
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
                let schema: SchemaMetadata =
                    serde_yaml::from_slice(content).map_err(|error| error.to_string())?;
                let id = schema.schema.schema_id;
                if !id.is_empty()
                    && !dependency_schemas.contains(&id)
                    && (!has_official_addon || keytao_core::is_addon_schema(&id))
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    addon_schemas.push(id);
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
    Ok(())
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
    let previous = GenerationPointers::read(root)?;
    // An empty schema list needs no compile before retiring the current generation.
    let empty_schema_list = files
        .get(Path::new("default.custom.yaml"))
        .and_then(|content| serde_yaml::from_slice::<serde_yaml::Value>(content).ok())
        .and_then(|document| {
            document
                .get("patch")?
                .get("schema_list")?
                .as_sequence()
                .map(Vec::is_empty)
        })
        .unwrap_or(false);
    let preserve_current = !empty_schema_list
        && !files.contains_key(Path::new(LIBRIME_VERSION_FILE))
        && previous
            .current
            .as_ref()
            .map(|generation| compiled_generation(root, generation))
            .transpose()?
            .unwrap_or(false);
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
    let next = if preserve_current {
        GenerationPointers {
            current: previous.current.clone(),
            pending: Some(generation),
        }
    } else {
        GenerationPointers {
            current: Some(generation),
            pending: None,
        }
    };
    if let Err(error) = next.write(root) {
        previous
            .write(root)
            .map_err(|rollback| format!("{error}; restore public schema pointers: {rollback}"))?;
        return Err(error);
    }
    prune_snapshots(root);
    Ok(())
}

fn compiled_generation(root: &Path, generation: &str) -> Result<bool, String> {
    let snapshot = root.join(generation);
    for path in [
        snapshot.clone(),
        snapshot.join("build"),
        snapshot.join(LIBRIME_VERSION_FILE),
    ] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("symbolic link in compiled public schema snapshot".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(true)
}

fn prune_snapshots(root: &Path) {
    let pointers = match GenerationPointers::read(root) {
        Ok(pointers) => pointers,
        Err(error) => {
            tracing::warn!(%error, "cannot select public schemas to preserve during cleanup");
            return;
        }
    };
    let cleanup = || -> std::io::Result<()> {
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let old_snapshot = !pointers
                .generations()
                .any(|generation| generation == name.as_ref())
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
    fn directory_identity_compares_resolved_paths_and_rejects_missing_paths() {
        let directory = tempfile::tempdir().unwrap();
        let child = directory.path().join("child");
        std::fs::create_dir(&child).unwrap();
        assert!(same_directory(directory.path(), &child.join("..")));
        assert!(!same_directory(directory.path(), &child));
        assert!(!same_directory(
            directory.path(),
            &directory.path().join("missing")
        ));
        #[cfg(unix)]
        {
            let link = directory.path().join("alias");
            std::os::unix::fs::symlink(&child, &link).unwrap();
            assert!(same_directory(&child, &link));
        }
    }

    #[test]
    fn main_package_overlap_preserves_installed_addon_sources_and_generated_customs() {
        let root = tempfile::tempdir().unwrap();
        let package = tempfile::tempdir().unwrap();
        let addon = [
            (
                "easy_en.schema.yaml".into(),
                b"schema: {schema_id: easy_en, version: official}".to_vec(),
            ),
            ("easy_en.dict.yaml".into(), b"name: easy_en".to_vec()),
            ("lua/easy_en.lua".into(), b"-- official addon".to_vec()),
        ];
        let ownership: BTreeMap<String, String> = addon.iter()
            .map(|(path, _): &(PathBuf, Vec<u8>)| (path.to_string_lossy().into_owned(), "easy_en".into()))
            .chain([("easy_en.custom.yaml".into(), "easy_en".into())])
            .collect();
        publish_at(root.path(), &addon, false).unwrap();
        std::fs::create_dir(package.path().join("lua")).unwrap();
        for (path, _) in &addon {
            std::fs::write(package.path().join(path), "bundled: obsolete").unwrap();
        }
        std::fs::write(
            package.path().join("easy_en.custom.yaml"),
            "patch: {bundled: obsolete}",
        )
        .unwrap();
        std::fs::write(
            package.path().join("keytao.schema.yaml"),
            "schema: {schema_id: keytao}",
        )
        .unwrap();
        std::fs::write(
            package.path().join("default.custom.yaml"),
            "patch: {schema_list: [{schema: keytao}, {schema: easy_en}]}",
        )
        .unwrap();
        publish_activation_at(
            root.path(),
            Some(package.path()),
            None,
            &ownership,
            false,
        )
        .unwrap();
        let installed = read_snapshot(root.path()).unwrap();
        for (path, bytes) in &addon {
            assert_eq!(installed[path], *bytes);
        }
        let generated = installed[Path::new("easy_en.custom.yaml")].clone();
        assert!(
            !std::str::from_utf8(&generated)
                .unwrap()
                .contains("obsolete")
        );
        publish_activation_at(
            root.path(),
            None,
            Some(package.path()),
            &ownership,
            false,
        )
        .unwrap();
        let remaining = read_snapshot(root.path()).unwrap();
        for (path, bytes) in &addon {
            assert_eq!(remaining[path], *bytes);
        }
        assert_eq!(remaining[Path::new("easy_en.custom.yaml")], generated);
        assert!(!remaining.contains_key(Path::new("keytao.schema.yaml")));
        assert_eq!(
            crate::scheme_files::parse_schema_list(
                std::str::from_utf8(&remaining[Path::new("default.custom.yaml")]).unwrap()
            ),
            ["easy_en"]
        );
    }

    #[test]
    fn durable_checkpoint_restores_pruned_generations_after_a_crash() {
        for initially_empty in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let transaction = tempfile::tempdir().unwrap();
            if !initially_empty {
                let initial = BTreeMap::from([
                    (
                        "keytao.schema.yaml".into(),
                        b"schema: {schema_id: keytao}".to_vec(),
                    ),
                    ("build/keytao.schema.yaml".into(), b"compiled".to_vec()),
                    (LIBRIME_VERSION_FILE.into(), b"1.0".to_vec()),
                ]);
                store_snapshot(root.path(), &initial, false).unwrap();
                publish_at(
                    root.path(),
                    &[("extra.yaml".into(), b"pending: true".to_vec())],
                    false,
                )
                .unwrap();
            }
            let original = read_snapshot(root.path()).unwrap();
            let pointers = GenerationPointers::read(root.path()).unwrap();
            let checkpoint = checkpoint_at(root.path().into(), transaction.path()).unwrap();
            let journal = serde_json::to_vec(&checkpoint).unwrap();
            let replacement = BTreeMap::from([
                (
                    "replacement.schema.yaml".into(),
                    b"schema: {schema_id: replacement}".to_vec(),
                ),
                (LIBRIME_VERSION_FILE.into(), b"2.0".to_vec()),
            ]);
            store_snapshot(root.path(), &replacement, false).unwrap();
            prune_snapshots(root.path());
            assert!(
                pointers
                    .generations()
                    .all(|generation| !root.path().join(generation).exists())
            );
            let recovered: PublicSchemaCheckpoint = serde_json::from_slice(&journal).unwrap();
            recovered.restore(transaction.path()).unwrap();
            assert!(GenerationPointers::read(root.path()).unwrap() == pointers);
            assert_eq!(read_snapshot(root.path()).unwrap(), original);
            recovered.restore(transaction.path()).unwrap();
        }
    }

    #[test]
    fn directory_switch_and_delete_remove_all_package_sources_and_generated_customs() {
        let root = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for (path, content) in [
            (
                "default.custom.yaml",
                "patch: {schema_list: [{schema: keytao_a}], menu/page_size: 6}",
            ),
            (
                "keytao_a.schema.yaml",
                "schema: {schema_id: keytao_a}\nengine: {filters: [lua_filter@*existing]}",
            ),
            ("keytao_a.dict.yaml", "name: keytao_a"),
            (
                "keytao_a.custom.yaml",
                "patch: {translator/dictionary: keytao_a}",
            ),
            ("unused.custom.yaml", "patch: {menu/page_size: 9}"),
            ("squirrel.custom.yaml", "frontend: macos"),
            ("weasel.custom.yaml", "frontend: windows"),
            ("keytao_a.user.yaml", "private: true"),
            ("rime.lua", "-- first package"),
        ] {
            std::fs::write(first.path().join(path), content).unwrap();
        }
        std::fs::write(
            second.path().join("other.schema.yaml"),
            "schema: {schema_id: other}",
        )
        .unwrap();
        publish_at(
            root.path(),
            &[(
                "easy_en.schema.yaml".into(),
                b"schema: {schema_id: easy_en}".to_vec(),
            )],
            false,
        )
        .unwrap();
        publish_activation_at(root.path(), Some(first.path()), None, &BTreeMap::new(), false).unwrap();
        let files = read_snapshot(root.path()).unwrap();
        for name in [
            "squirrel.custom.yaml",
            "weasel.custom.yaml",
            "keytao_a.user.yaml",
        ] {
            assert!(!files.contains_key(Path::new(name)), "{name}");
        }
        let custom = std::str::from_utf8(&files[Path::new("keytao_a.custom.yaml")]).unwrap();
        assert!(custom.contains("# keytao-generated begin v1"));
        assert!(files.contains_key(Path::new(crate::remove_filter::SCRIPT_PATH)));
        publish_activation_at(
            root.path(),
            Some(second.path()),
            Some(first.path()),
            &BTreeMap::new(),
            false,
        )
        .unwrap();
        let files = read_snapshot(root.path()).unwrap();
        for name in [
            "keytao_a.schema.yaml",
            "keytao_a.dict.yaml",
            "keytao_a.custom.yaml",
            "unused.custom.yaml",
            "rime.lua",
            crate::remove_filter::SCRIPT_PATH,
        ] {
            assert!(!files.contains_key(Path::new(name)), "{name}");
        }
        let config = std::str::from_utf8(&files[Path::new("default.custom.yaml")]).unwrap();
        assert_eq!(
            crate::scheme_files::parse_schema_list(config),
            ["easy_en", "other"]
        );
        assert!(!config.contains("menu/page_size"));
        assert!(files.contains_key(Path::new("easy_en.custom.yaml")));
        publish_activation_at(root.path(), None, Some(second.path()), &BTreeMap::new(), false).unwrap();
        let files = read_snapshot(root.path()).unwrap();
        assert!(!files.contains_key(Path::new("other.schema.yaml")));
        let config = std::str::from_utf8(&files[Path::new("default.custom.yaml")]).unwrap();
        assert_eq!(crate::scheme_files::parse_schema_list(config), ["easy_en"]);
    }

    #[cfg(unix)]
    #[test]
    fn directory_publication_rejects_symbolic_links_without_changing_pointers() {
        let root = tempfile::tempdir().unwrap();
        let package = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.yaml"), "outside: true").unwrap();
        std::os::unix::fs::symlink(outside.path(), package.path().join("escape")).unwrap();
        assert!(
            publish_activation_at(root.path(), Some(package.path()), None, &BTreeMap::new(), false)
                .is_err()
        );
        assert!(GenerationPointers::read(root.path()).unwrap() == GenerationPointers::default());
    }

    #[test]
    fn public_build_preserves_skipped_schemas_for_prebuilt_validation() {
        let root = tempfile::tempdir().unwrap();
        publish_at(
            root.path(),
            &[
                (
                    "default.custom.yaml".into(),
                    b"patch: {schema_list: [{schema: keytao_broken}, {schema: keytao_good}]}"
                        .to_vec(),
                ),
                (
                    "keytao_broken.schema.yaml".into(),
                    b"schema: {schema_id: keytao_broken}".to_vec(),
                ),
                (
                    "keytao_good.schema.yaml".into(),
                    b"schema: {schema_id: keytao_good}".to_vec(),
                ),
            ],
            false,
        )
        .unwrap();
        let warning = "方案 keytao_broken 编译失败，已跳过：copy on write failed; incompatible node type: commit";
        deploy_and_publish_at(root.path(), "1.0", false, |stage| {
            std::fs::create_dir(stage.join("build")).unwrap();
            std::fs::write(
                stage.join("build/keytao_good.schema.yaml"),
                "schema: {schema_id: keytao_good}",
            )
            .unwrap();
            std::fs::write(
                stage.join("build/default.yaml"),
                "schema_list: [{schema: keytao_good}]",
            )
            .unwrap();
            std::fs::write(
                stage.join("build/.keytao-skipped-schemas.json"),
                serde_json::to_vec(&serde_json::json!({ "keytao_broken": warning })).unwrap(),
            )
            .unwrap();
            keytao_core::complete_deployment(stage)
        })
        .unwrap();
        let generation = read_generation(root.path()).unwrap().unwrap();
        let snapshot = root.path().join(generation);
        assert_eq!(
            keytao_core::deployment_warning(&snapshot).as_deref(),
            Some(warning)
        );
        let state = keytao_core::schema_install_state(&snapshot);
        assert!(state.installed);
        assert_eq!(state.schemas, ["keytao_good"]);
        keytao_core::complete_deployment(&snapshot).unwrap();
        assert!(keytao_core::schema_install_state(&snapshot).deployed);
        assert_eq!(
            keytao_core::deployed_schema_id_from_dir(&snapshot).as_deref(),
            Some("keytao_good")
        );
        for path in [
            "build/.other.json",
            ".private/build/.keytao-skipped-schemas.json",
            "user.yaml",
            "log/errors.txt",
        ] {
            assert!(!is_public_asset(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn removing_package_clears_public_sources_and_activates_an_empty_schema_list() {
        for keep_addon in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let config = format!(
                "patch:\n  schema_list:\n    - schema: keytao6\n    - schema: auxiliary\n{}",
                if keep_addon {
                    "    - schema: easy_en\n"
                } else {
                    ""
                }
            );
            let package_paths = [
                "keytao6.schema.yaml",
                "auxiliary.schema.yaml",
                "keytao.dict.yaml",
                "lua/main.lua",
            ]
            .map(PathBuf::from);
            let mut initial = BTreeMap::from([
                ("default.custom.yaml".into(), config.into_bytes()),
                ("easy_en.schema.yaml".into(), b"addon source".to_vec()),
                ("easy_en.dict.yaml".into(), b"addon dictionary".to_vec()),
                (
                    "build/keytao6.schema.yaml".into(),
                    b"compiled schema".to_vec(),
                ),
                (LIBRIME_VERSION_FILE.into(), b"1.0".to_vec()),
            ]);
            for path in &package_paths {
                initial.insert(path.clone(), b"package source".to_vec());
            }
            store_snapshot(root.path(), &initial, false).unwrap();
            let previous = GenerationPointers::read(root.path()).unwrap();
            let update = update_public_schemas_at(root.path().into(), || {
                for id in ["keytao6", "auxiliary"] {
                    remove_schema_at(root.path(), id, &package_paths, false)?;
                }
                Ok(())
            })
            .unwrap();
            let files = read_snapshot(root.path()).unwrap();
            let config = std::str::from_utf8(&files[Path::new("default.custom.yaml")]).unwrap();
            assert_eq!(
                crate::scheme_files::parse_schema_list(config),
                if keep_addon { vec!["easy_en"] } else { vec![] }
            );
            for path in &package_paths {
                assert!(!files.contains_key(path), "{path:?}");
            }
            assert_eq!(files[Path::new("easy_en.schema.yaml")], b"addon source");
            assert_eq!(files[Path::new("easy_en.dict.yaml")], b"addon dictionary");
            assert!(!files.keys().any(|path| path.starts_with("build")));
            let pointers = GenerationPointers::read(root.path()).unwrap();
            if keep_addon {
                assert_eq!(pointers.current, previous.current);
                assert!(pointers.pending.is_some());
            } else {
                assert_ne!(pointers.current, previous.current);
                assert!(pointers.pending.is_none());
            }
            update.rollback().unwrap();
            assert_eq!(read_snapshot(root.path()).unwrap(), initial);
        }
    }

    #[test]
    fn runtime_error_details_preserve_long_unicode_messages_without_string_truncation() {
        let error = format!("compile: {} dictionary failed", "词典🦀".repeat(130));
        let details = public_error_details(&error);
        assert!(details.iter().all(|chunk| chunk.chars().count() <= 128));
        assert_eq!(details.concat(), error);
        if let Some(home) = dirs::home_dir() {
            let error = format!("{}{} dictionary failed", "x".repeat(127), home.display());
            let details = public_error_details(&error).concat();
            assert_eq!(
                details,
                format!("{}<user> dictionary failed", "x".repeat(127))
            );
        }
    }

    #[test]
    fn updated_main_sources_wait_for_compile_without_replacing_the_previous_generation() {
        let root = tempfile::tempdir().unwrap();
        let previous = BTreeMap::from([
            (
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao}".to_vec(),
            ),
            (
                "build/keytao.schema.yaml".into(),
                b"previous compiled schema".to_vec(),
            ),
            (LIBRIME_VERSION_FILE.into(), b"1.0".to_vec()),
        ]);
        store_snapshot(root.path(), &previous, false).unwrap();
        let current = std::fs::read(root.path().join("current.txt")).unwrap();
        let current_snapshot = root.path().join(std::str::from_utf8(&current).unwrap());
        publish_at(
            root.path(),
            &[(
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao, version: '2'}".to_vec(),
            )],
            false,
        )
        .unwrap();
        let pending = std::fs::read(root.path().join("pending.txt")).unwrap();
        let result = after_private_deploy(Ok(()), || {
            deploy_and_publish_at(root.path(), "1.0", false, |_| {
                Err("new main scheme compile failed".into())
            })
        });
        assert_eq!(
            result,
            Ok(Some(
                "public-build：compile: new main scheme compile failed".into()
            ))
        );
        assert_eq!(
            std::fs::read(root.path().join("current.txt")).unwrap(),
            current
        );
        assert_eq!(
            std::fs::read(root.path().join("pending.txt")).unwrap(),
            pending
        );
        for (path, bytes) in previous {
            assert_eq!(std::fs::read(current_snapshot.join(path)).unwrap(), bytes);
        }
        assert!(
            !read_snapshot(root.path())
                .unwrap()
                .contains_key(Path::new(LIBRIME_VERSION_FILE))
        );
    }

    #[test]
    fn rollback_restores_both_exact_pointers_after_a_successful_child_publication() {
        for state in ["empty", "source-only", "compiled", "pending"] {
            let root = tempfile::tempdir().unwrap();
            if state != "empty" {
                let mut initial = BTreeMap::from([(
                    "keytao.schema.yaml".into(),
                    b"schema: {schema_id: keytao}".to_vec(),
                )]);
                if state != "source-only" {
                    initial.insert(
                        "build/keytao.schema.yaml".into(),
                        b"previous compiled schema".to_vec(),
                    );
                    initial.insert(LIBRIME_VERSION_FILE.into(), b"1.0".to_vec());
                }
                store_snapshot(root.path(), &initial, false).unwrap();
                if state == "pending" {
                    publish_at(
                        root.path(),
                        &[(
                            "keytao.schema.yaml".into(),
                            b"schema: {schema_id: keytao, version: 'pending'}".to_vec(),
                        )],
                        false,
                    )
                    .unwrap();
                }
            }
            let current = std::fs::read(root.path().join("current.txt")).ok();
            let pending = std::fs::read(root.path().join("pending.txt")).ok();
            let files = read_snapshot(root.path()).unwrap();
            let update = update_public_schemas_at(root.path().into(), || {
                publish_at(
                    root.path(),
                    &[(
                        "keytao.schema.yaml".into(),
                        b"schema: {schema_id: keytao, version: 'new'}".to_vec(),
                    )],
                    false,
                )
            })
            .unwrap();
            deploy_and_publish_at(root.path(), "1.0", false, |stage| {
                std::fs::create_dir(stage.join("build")).unwrap();
                std::fs::write(
                    stage.join("build/keytao.schema.yaml"),
                    "new compiled schema",
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
            assert!(!root.path().join("pending.txt").exists());
            update.rollback().unwrap();
            assert_eq!(
                std::fs::read(root.path().join("current.txt")).ok(),
                current,
                "{state}"
            );
            assert_eq!(
                std::fs::read(root.path().join("pending.txt")).ok(),
                pending,
                "{state}"
            );
            assert_eq!(read_snapshot(root.path()).unwrap(), files, "{state}");
            let pointers = GenerationPointers::read(root.path()).unwrap();
            for generation in pointers.generations() {
                assert!(root.path().join(generation).is_dir(), "{state}");
            }
        }
    }

    #[test]
    fn dependency_escape_is_rejected_before_compile_and_keeps_current_generation() {
        let root = tempfile::tempdir().unwrap();
        let initial = BTreeMap::from([
            (
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao}".to_vec(),
            ),
            (
                "build/keytao.schema.yaml".into(),
                b"previous compiled schema".to_vec(),
            ),
            (LIBRIME_VERSION_FILE.into(), b"1.0".to_vec()),
        ]);
        store_snapshot(root.path(), &initial, false).unwrap();
        let current = std::fs::read(root.path().join("current.txt")).unwrap();
        publish_at(
            root.path(),
            &[(
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao, dependencies: ['../secret']}".to_vec(),
            )],
            false,
        )
        .unwrap();
        let result = after_private_deploy(Ok(()), || {
            deploy_and_publish_at(root.path(), "1.0", false, |_| {
                panic!("invalid dependency must not reach librime")
            })
        });
        assert_eq!(result, Ok(Some("public-build：prepare-dependencies: invalid public schema path: ../secret.schema.yaml".into())));
        assert_eq!(
            std::fs::read(root.path().join("current.txt")).unwrap(),
            current
        );
    }

    #[test]
    fn public_failure_warns_after_private_success_and_restores_previous_generation() {
        let root = tempfile::tempdir().unwrap();
        let previous = BTreeMap::from([
            (
                "keytao.schema.yaml".into(),
                b"schema: {schema_id: keytao}".to_vec(),
            ),
            (
                "build/keytao.schema.yaml".into(),
                b"previous compiled schema".to_vec(),
            ),
            (LIBRIME_VERSION_FILE.into(), b"old version".to_vec()),
        ]);
        store_snapshot(root.path(), &previous, false).unwrap();
        let generation = read_generation(root.path()).unwrap();
        let result = after_private_deploy(Ok(()), || {
            update_public_schemas_at(root.path().into(), || {
                publish_at(
                    root.path(),
                    &[(
                        "keytao.schema.yaml".into(),
                        b"schema: {schema_id: keytao, version: '2'}".to_vec(),
                    )],
                    false,
                )?;
                deploy_and_publish_at(root.path(), "1.0", false, |_| {
                    Err("xmjd6 dictionary compile failed".into())
                })
            })
            .map(|_| ())
        });
        assert_eq!(
            result,
            Ok(Some(
                "public-build：compile: xmjd6 dictionary compile failed".into()
            ))
        );
        assert_eq!(read_generation(root.path()).unwrap(), generation);
        assert_eq!(read_snapshot(root.path()).unwrap(), previous);
        assert_eq!(
            after_private_deploy(Err("private deploy failed".into()), || {
                panic!("public build must not run after a private failure")
            }),
            Err("private deploy failed".into())
        );
    }

    #[test]
    fn public_build_omits_only_dependencies_absent_from_its_sources() {
        let root = tempfile::tempdir().unwrap();
        publish_at(
            root.path(),
            &[
                (
                    "default.custom.yaml".into(),
                    b"patch: {schema_list: [{schema: keytao}]}".to_vec(),
                ),
                (
                    "keytao.schema.yaml".into(),
                    b"schema:\n  schema_id: keytao\n  dependencies: [luna_quanpin, easy_en]\n"
                        .to_vec(),
                ),
                (
                    "easy_en.schema.yaml".into(),
                    b"schema: {schema_id: easy_en}".to_vec(),
                ),
            ],
            false,
        )
        .unwrap();
        deploy_and_publish_at(root.path(), "1.0", false, |stage| {
            let schema: serde_yaml::Value =
                serde_yaml::from_slice(&std::fs::read(stage.join("keytao.schema.yaml")).unwrap())
                    .unwrap();
            assert_eq!(
                schema["schema"]["dependencies"],
                serde_yaml::from_str::<serde_yaml::Value>("[easy_en]").unwrap()
            );
            assert!(!stage.join("luna_quanpin.schema.yaml").exists());
            std::fs::create_dir(stage.join("build")).unwrap();
            std::fs::write(
                stage.join("build/keytao.schema.yaml"),
                "compiled public schema",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
        assert!(
            read_snapshot(root.path())
                .unwrap()
                .contains_key(Path::new("build/keytao.schema.yaml"))
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn public_acl_failure_warns_without_selecting_an_unprotected_generation() {
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
        let generation = read_generation(root.path()).unwrap();
        let result = after_private_deploy(Ok(()), || {
            deploy_and_publish_at(root.path(), "1.0", true, |stage| {
                std::fs::create_dir(stage.join("build")).unwrap();
                std::fs::write(
                    stage.join("build/keytao.schema.yaml"),
                    "compiled public schema",
                )
                .unwrap();
                Ok(())
            })
        });
        assert_eq!(
            result,
            Ok(Some(
                "public-build：publish: public schema ACLs require Windows".into()
            ))
        );
        assert_eq!(read_generation(root.path()).unwrap(), generation);
    }

    #[test]
    fn lssp_repeated_patch_keys_survive_public_dependency_preparation() {
        let root = tempfile::tempdir().unwrap();
        let source = "schema: {schema_id: triple_pinyin_lssp, dependencies: [lssp_stroke]}\npreedit:\n  __patch: {__append: [one]}\n  __patch: {__append: [two]}\n";
        std::fs::write(root.path().join("triple_pinyin_lssp.schema.yaml"), source).unwrap();
        std::fs::write(root.path().join("lssp_stroke.schema.yaml"), "schema: {schema_id: lssp_stroke}").unwrap();
        trim_missing_dependencies(root.path()).unwrap();
        assert_eq!(std::fs::read_to_string(root.path().join("triple_pinyin_lssp.schema.yaml")).unwrap(), source);
    }

    #[test]
    fn publishing_addon_dependencies_registers_only_the_main_schema() {
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
        publish_at(
            root.path(),
            &[
                (
                    "rime_ice.schema.yaml".into(),
                    b"schema: {schema_id: rime_ice, dependencies: [melt_eng, radical_pinyin]}"
                        .to_vec(),
                ),
                (
                    "melt_eng.schema.yaml".into(),
                    b"schema: {schema_id: melt_eng}".to_vec(),
                ),
                (
                    "radical_pinyin.schema.yaml".into(),
                    b"schema: {schema_id: radical_pinyin}".to_vec(),
                ),
                (
                    "lua/rime_ice/lunar.db".into(),
                    b"public lunar database".to_vec(),
                ),
                (
                    "rime_mint.schema.yaml".into(),
                    b"schema: {schema_id: rime_mint, dependencies: [rime_mint_melt_eng]}".to_vec(),
                ),
                (
                    "rime_mint_melt_eng.schema.yaml".into(),
                    b"schema: {schema_id: rime_mint_melt_eng}".to_vec(),
                ),
                (
                    "rime_frost.schema.yaml".into(),
                    b"schema: {schema_id: rime_frost, dependencies: [rime_frost_melt_eng]}"
                        .to_vec(),
                ),
                (
                    "rime_frost_melt_eng.schema.yaml".into(),
                    b"schema: {schema_id: rime_frost_melt_eng}".to_vec(),
                ),
                ("zh-moqi.gram".into(), b"public grammar model".to_vec()),
                (
                    "triple_pinyin_lssp.schema.yaml".into(),
                    b"schema: {schema_id: triple_pinyin_lssp, dependencies: [lssp_stroke]}\npreedit:\n  __patch: {__append: [one]}\n  __patch: {__append: [two]}\n".to_vec(),
                ),
                (
                    "lssp_stroke.schema.yaml".into(),
                    b"schema: {schema_id: lssp_stroke}".to_vec(),
                ),
            ],
            false,
        )
        .unwrap();
        let files = read_snapshot(root.path()).unwrap();
        let config = std::str::from_utf8(&files[Path::new("default.custom.yaml")]).unwrap();
        assert_eq!(
            crate::scheme_files::parse_schema_list(config),
            [
                "keytao",
                "rime_ice",
                "rime_mint",
                "rime_frost",
                "triple_pinyin_lssp"
            ]
        );
        for dependency in [
            "melt_eng.schema.yaml",
            "radical_pinyin.schema.yaml",
            "rime_mint_melt_eng.schema.yaml",
            "rime_frost_melt_eng.schema.yaml",
            "lssp_stroke.schema.yaml",
        ] {
            assert!(files.contains_key(Path::new(dependency)));
        }
        assert_eq!(
            files[Path::new("lua/rime_ice/lunar.db")],
            b"public lunar database"
        );
        assert_eq!(files[Path::new("zh-moqi.gram")], b"public grammar model");
    }

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
        assert_eq!(failure, Err("compile: public compile failed".into()));
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
        assert!(is_public_asset(Path::new("zh-moqi.gram")));
        assert!(!is_public_asset(Path::new("private.gram")));
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
