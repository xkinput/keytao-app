use crate::{
    desktop,
    runtime_dirs::{
        BUILD_GENERATION_MARKER, BUILD_GENERATION_PREFIX, PREVIOUS_BUILD, runtime_build_dir,
    },
    DeployProgress, DeployStep, RuntimeDirs,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const PACKAGE_TARGETS_ENV: &str = "KEYTAO_DEPLOY_PACKAGES";
const STAGE: &str = ".keytao-deploy-stage";
pub const STAGED_INPUTS_PREFIX: &str = ".keytao-inputs-";
pub const STAGED_INPUTS_MANIFEST: &str = ".edits.json";
const RECEIPT: &str = ".keytao-desktop-inputs.json";
const INPUT_CACHE: &str = ".keytao-package-input-cache.json";
const SCHEMA_RECEIPT: &str = ".keytao-schema-inputs.json";
const DEPLOY_LOCK: &str = ".keytao-package-deploy.lock";
const MIGRATION_LOCK: &str = ".keytao-migration.lock";
const MATERIALIZE_LOCK: &str = ".keytao-materialize.lock";

// Lua scripts update these files while typing and learning phrases.
// shortcut: keep the paths explicit until scheme packages can declare mutable data.
pub const LUA_RUNTIME_STATE: &[&str] = &[
    "JD6_userdata/JD6_overrides.tsv",
    "JD6_userdata/JD6_usage.tsv",
    "JD6_userdata/JD6_usage_events.tsv",
    "lua/kc_count.txt",
    "lua/TJ/direct_pending.txt",
    "lua/TJ/stats.txt",
    "lua/TJ/stats.txt.tmp",
    "white_replace/whitkey_created.txt",
    "white_replace/whitkey_otj_stats.txt",
    "white_replace/whitkey_pinned.txt",
];

/// User-writable runtime inputs that package materialization should preserve.
/// Callers separately validate paths and filter reserved files.
pub fn preserves_runtime_data(relative: &Path) -> bool {
    let Some(name) = relative.file_name() else {
        return false;
    };
    let name = name.to_string_lossy().to_ascii_lowercase();
    let is_yaml = name.ends_with(".yaml") || name.ends_with(".yml");
    !is_yaml
        || matches!(name.as_str(), "user.dict.yaml" | "user.dict.yml")
        || name.ends_with(".user.dict.yaml")
        || name.ends_with(".user.dict.yml")
        || relative
            .iter()
            .next()
            .is_some_and(|part| part.eq_ignore_ascii_case("lua"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageDeployTarget {
    pub package_key: String,
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_inputs: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct StagedInput {
    pub previous_sha256: Option<String>,
    pub replace: bool,
}

pub fn deploy_packages(
    control_root: &Path,
    shared: &Path,
    package_data_root: Option<&Path>,
    targets: &[PackageDeployTarget],
    progress: impl FnMut(DeployProgress),
) -> Result<(), String> {
    let _deploy = deploy_lock(control_root, true)?;
    crate::with_rime_finalized(|| {
        deploy_packages_with(
            control_root,
            shared,
            package_data_root,
            targets,
            progress,
            |dirs, steps| desktop::deploy_resolved_runtime(control_root, dirs, steps),
        )
    })
}

fn deploy_packages_with(
    control_root: &Path,
    shared: &Path,
    package_data_root: Option<&Path>,
    targets: &[PackageDeployTarget],
    mut progress: impl FnMut(DeployProgress),
    mut compile: impl FnMut(&RuntimeDirs, &mut dyn FnMut(DeployStep)) -> Result<(), String>,
) -> Result<(), String> {
    let unique: HashSet<_> = targets.iter().map(|target| &target.package_key).collect();
    if targets.is_empty() || unique.len() != targets.len() {
        return Err("invalid package deployment targets".into());
    }
    let _hint = ProgressHint(control_root);
    let active = RuntimeDirs::active_package_key(control_root)
        .or_else(|| package_data_root.and_then(RuntimeDirs::active_package_key));
    let mut ordered: Vec<_> = targets.iter().collect();
    ordered.sort_by_key(|target| active.as_deref() != Some(&target.package_key));
    let total = u32::try_from(ordered.len()).map_err(|error| error.to_string())?;
    let mut failures = Vec::new();
    for (offset, target) in ordered.iter().enumerate() {
        let mut emit = |step, error| {
            let event = DeployProgress {
                package_key: target.package_key.clone(),
                package_name: target.package_name.clone(),
                index: offset as u32 + 1,
                total,
                step,
                error,
                legacy_message: None,
                queued_package_keys: if offset == 0 && step == DeployStep::Materialize {
                    ordered[1..]
                        .iter()
                        .map(|target| target.package_key.clone())
                        .collect()
                } else {
                    Vec::new()
                },
            };
            if matches!(
                step,
                DeployStep::Materialize
                    | DeployStep::Compile
                    | DeployStep::Validate
                    | DeployStep::Prune
            ) {
                let _ = crate::deployment_state::write_deploy_progress(control_root, &event);
            } else {
                crate::deployment_state::clear_deploy_progress(control_root);
            }
            progress(event);
        };
        emit(DeployStep::Materialize, None);
        let result = (|| {
            if let Some(error) = &target.materialize_error {
                return Err(error.clone());
            }
            let dirs = package_data_root
                .and_then(|root| {
                    RuntimeDirs::for_package_in_store(
                        control_root,
                        shared,
                        root,
                        &target.package_key,
                    )
                })
                .or_else(|| RuntimeDirs::for_package(control_root, shared, &target.package_key))
                .ok_or_else(|| format!("cannot resolve runtime for {}", target.package_key))?;
            deploy_package(
                control_root,
                &dirs,
                target.staged_inputs.as_deref(),
                |stage| compile(stage, &mut |step| emit(step, None)),
            )
        })();
        match result {
            Ok(step) => emit(step, None),
            Err(error) => {
                let message = if total == 1 {
                    &error
                } else {
                    // Detailed errors are sent separately to keep the batch reply short.
                    error.lines().next().unwrap_or("deployment failed")
                };
                failures.push(format!("{}: {message}", target.package_name));
                emit(DeployStep::Failed, Some(error));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

struct ProgressHint<'a>(&'a Path);

impl Drop for ProgressHint<'_> {
    fn drop(&mut self) {
        crate::deployment_state::clear_deploy_progress(self.0);
    }
}

fn lock(path: &Path, wait: bool) -> Result<fs::File, String> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(format!("invalid deployment lock: {}", path.display()));
    }
    let file = fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| format!("open deployment lock: {error}"))?;
    crate::lock_file(&file, wait).map_err(|error| format!("lock deployment: {error}"))?;
    Ok(file)
}

pub fn with_isolated_package_lock<T>(
    control_root: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _deploy = deploy_lock(control_root, true)?;
    action()
}

pub fn with_migration_lock<T>(
    control_root: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _migration = lock(&runtime_lock_directory(control_root)?.join(MIGRATION_LOCK), true)?;
    action()
}

fn deploy_lock(control_root: &Path, wait: bool) -> Result<fs::File, String> {
    lock(&runtime_lock_directory(control_root)?.join(DEPLOY_LOCK), wait)
}

fn runtime_lock_directory(control_root: &Path) -> Result<PathBuf, String> {
    let runtime = control_root.join("runtime");
    match fs::create_dir(&runtime) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    if !fs::symlink_metadata(&runtime).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("invalid isolated runtime directory".into());
    }
    Ok(runtime)
}

pub fn rime_build_cache_files(
    control_root: &Path,
    key: &str,
) -> Result<Option<Vec<PathBuf>>, String> {
    let Some(user) = RuntimeDirs::isolated_user_data_dir(control_root, key) else {
        return Ok(None);
    };
    let Ok(published) = runtime_build_dir(&user) else {
        return Ok(None);
    };
    let directories = std::collections::BTreeSet::from([
        published,
        user.join("build"),
        user.join(PREVIOUS_BUILD),
    ]);
    let files = (|| -> io::Result<Option<Vec<_>>> {
        let mut files = Vec::new();
        for directory in directories {
            match fs::symlink_metadata(&directory) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Ok(None),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { return Ok(None) };
                let compiled = crate::build_prune::output_id(name).is_some_and(|id| {
                    let id = id.to_ascii_lowercase();
                    id != "user.dict" && !id.contains("userdb") && !id.contains(".user.dict")
                });
                let generated = compiled || matches!(name, RECEIPT | SCHEMA_RECEIPT | "default.yaml"
                    | crate::DEPLOYED_FILE | crate::DEPLOYING_FILE | crate::SKIPPED_SCHEMAS_FILE
                    | ".keytao-deploy-failed");
                if !generated || !entry.file_type()?.is_file() {
                    return Ok(None);
                }
                files.push(entry.path());
            }
        }
        files.sort_unstable_by_key(|path| {
            !matches!(path.file_name().and_then(|name| name.to_str()), Some(RECEIPT | crate::DEPLOYED_FILE))
        });
        Ok(Some(files))
    })();
    files.map_err(|error| format!("read Rime build cache: {error}"))
}

pub fn release_rime_build_cache(control_root: &Path, key: &str) -> Result<(), String> {
    let files = rime_build_cache_files(control_root, key)?
        .ok_or("编译目录含无法确认可重建的内容")?;
    let mut flat_builds = std::collections::BTreeSet::new();
    for file in files {
        if let Some(parent) = file.parent() {
            if matches!(
                parent.file_name().and_then(|name| name.to_str()),
                Some("build" | PREVIOUS_BUILD)
            ) {
                flat_builds.insert(parent.to_owned());
            }
        }
        crate::remove_rime_build_artifact(&file)?;
    }
    for directory in flat_builds {
        match fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty) => {}
            Err(error) => return Err(format!("remove {}: {error}", directory.display())),
        }
    }
    Ok(())
}

pub fn package_build_ready(control_root: &Path, dirs: &RuntimeDirs) -> bool {
    package_build_ready_in_store(control_root, None, dirs)
}

fn package_build_ready_in_store(
    control_root: &Path,
    data_root: Option<&Path>,
    dirs: &RuntimeDirs,
) -> bool {
    if crate::runtime_libjd::package_libjd_ready_in_store(
        control_root,
        data_root,
        dirs.active_package_key.as_deref(),
    ) {
        return true;
    }
    rime_package_build_ready(control_root, dirs)
}

pub fn rime_package_build_ready(control_root: &Path, dirs: &RuntimeDirs) -> bool {
    has_published_package_build(dirs) && package_receipt_matches(control_root, dirs)
}

pub(crate) fn has_published_package_build(dirs: &RuntimeDirs) -> bool {
    dirs.active_package_key.is_some()
        && dirs.staging_dir.join(RECEIPT).is_file()
        && crate::resolved_schema_install_state(dirs).deployed
}

/// The IME can select a committed build. Only the app can prepare an unready package.
pub fn switch_ready_package(control_root: &Path, shared: &Path, key: &str) -> Result<(), String> {
    switch_package(control_root, shared, None, key, None, true, false)
}

pub fn switch_ready_package_with_schema(
    control_root: &Path,
    shared: &Path,
    key: &str,
    schema_id: Option<&str>,
) -> Result<(), String> {
    switch_package(control_root, shared, None, key, schema_id, true, false)
}

pub fn switch_ready_package_in_store(
    control_root: &Path,
    shared: &Path,
    data_root: Option<&Path>,
    key: &str,
    schema_id: Option<&str>,
) -> Result<(), String> {
    switch_package(control_root, shared, data_root, key, schema_id, true, false)
}

pub(crate) fn try_switch_ready_package_with_schema(
    control_root: &Path,
    shared: &Path,
    key: &str,
    schema_id: Option<&str>,
    libjd_only: bool,
) -> Result<(), String> {
    switch_package(control_root, shared, None, key, schema_id, false, libjd_only)
}

fn switch_package(
    control_root: &Path,
    shared: &Path,
    data_root: Option<&Path>,
    key: &str,
    schema_id: Option<&str>,
    wait: bool,
    libjd_only: bool,
) -> Result<(), String> {
    let not_ready = || "请在键道 App 部署此方案包".to_owned();
    if RuntimeDirs::needs_migration(control_root) {
        return Err(not_ready());
    }
    let resolve = || {
        data_root
            .and_then(|data| RuntimeDirs::for_package_in_store(control_root, shared, data, key))
            .or_else(|| RuntimeDirs::for_package(control_root, shared, key))
            .ok_or_else(not_ready)
    };
    let _deploy = deploy_lock(control_root, wait)?;
    let libjd_ready =
        crate::runtime_libjd::package_libjd_ready_in_store(control_root, data_root, Some(key));
    if libjd_only && !libjd_ready {
        return Err("请在键道 App 转换此方案的词库".into());
    }
    let dirs = resolve()?;
    if !dirs.user_data_dir.is_dir() {
        if libjd_ready {
            fs::create_dir_all(&dirs.user_data_dir).map_err(|error| error.to_string())?;
        } else {
            return Err(not_ready());
        }
    }
    let _materialize = lock(&dirs.user_data_dir.join(MATERIALIZE_LOCK), wait)?;
    let dirs = resolve()?;
    if !package_build_ready_in_store(control_root, data_root, &dirs) {
        return Err(not_ready());
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android",
        target_os = "ios"
    ))]
    if libjd_only {
        use crate::jd_backend::{JdBackend, DEFAULT_PAGE_SIZE};

        let (dictionary, schema) =
            crate::runtime_libjd::imported_dictionary(control_root, data_root, Some(key))
                .ok_or("请在键道 App 转换此方案的词库")?;
        let library = crate::libjd::bundled_engine_library().ok_or("找不到内置 ime-jd 引擎")?;
        // Validation opens no context, so the current engine can keep serving keys.
        unsafe { JdBackend::prepare_dictionary(&library, &dictionary, DEFAULT_PAGE_SIZE, schema) }?;
    }
    if !libjd_ready {
        if let Some(schema_id) = schema_id.filter(|id| !id.is_empty()) {
            crate::persist_selected_schema_in_dirs(&dirs, schema_id)?;
        }
    }

    let path = crate::package_store::active_manifest(control_root);
    let parent = path.parent().unwrap();
    if !parent.exists() {
        fs::create_dir(parent).or_else(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists { Ok(()) } else { Err(error) }
        }).map_err(|error| error.to_string())?;
    }
    if !fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("invalid active package store".into());
    }
    let previous = optional_file(&path)?;
    let mut manifest: serde_json::Map<String, serde_json::Value> = previous.as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|error| format!("read active package manifest: {error}"))?
        .unwrap_or_default();
    manifest.insert("active_package".into(), key.into());
    if let Some(version) = RuntimeDirs::migration_version(control_root) {
        manifest.insert("runtime_migration_version".into(), version.into());
    }
    // Reload readers retry if they observe the manifest before the stamp.
    crate::write_file_atomic(&path, &serde_json::to_vec(&manifest).map_err(|error| error.to_string())?)
        .map_err(|error| format!("write active package manifest: {error}"))?;
    if let Err(error) = crate::ReloadStamp::write_package(control_root, key) {
        let rollback = match previous {
            Some(bytes) => crate::write_file_atomic(&path, &bytes),
            None => fs::remove_file(&path),
        };
        return Err(match rollback {
            Ok(()) => error,
            Err(rollback) => format!("{error}; restore active package manifest: {rollback}"),
        });
    }
    Ok(())
}

fn deploy_package(
    control_root: &Path,
    dirs: &RuntimeDirs,
    staged_inputs: Option<&str>,
    compile: impl FnOnce(&RuntimeDirs) -> Result<(), String>,
) -> Result<DeployStep, String> {
    let user = &dirs.user_data_dir;
    let _lock = lock(&user.join(MATERIALIZE_LOCK), true)?;
    let versioned_build =
        use_versioned_build(cfg!(target_os = "windows"), dirs.active_package_key.as_deref());
    let build = if versioned_build {
        runtime_build_dir(user)?
    } else {
        let build = user.join("build");
        recover_build(&build)?;
        build
    };
    let edits = staged_inputs
        .map(|name| read_input_edits(user, name))
        .transpose()?
        .unwrap_or_default();
    let live_stats = input_stat_signature(control_root, dirs).ok();
    let cutoff = input_stat_cutoff().ok();
    let mut file_hashes = HashMap::<PathBuf, (InputStat, [u8; 32])>::new();
    let mut hash_file = |path: &Path| {
        let before = cutoff.and_then(|cutoff| cacheable_input_stat(path, cutoff).ok());
        if let Some((stat, hash)) = file_hashes.get(path) {
            if before.as_ref() == Some(stat) {
                return Ok(*hash);
            }
        }
        let hash = file_hash(path)?;
        if let Some(stat) = before.filter(|stat| input_stat(path).is_ok_and(|after| &after == stat))
        {
            file_hashes.insert(path.to_path_buf(), (stat, hash));
        }
        Ok(hash)
    };
    let live_inputs = if edits.is_empty() && !cfg!(target_os = "ios") {
        let (inputs, matches) =
            check_package_receipt_with(control_root, dirs, &build, live_stats, || {
                input_signature(control_root, dirs, &mut hash_file)
            })?;
        if matches {
            return Ok(DeployStep::Skip);
        }
        inputs
    } else {
        input_signature(control_root, dirs, &mut hash_file)?
    };

    let stage = Stage::create(user.join(STAGE))?;
    walk_files(user, false, &mut |relative, path| {
        let copy = stage.0.join(relative);
        fs::create_dir_all(copy.parent().unwrap())?;
        // A compiler or Lua script can write its copy while the live IME is typing.
        if preserves_runtime_data(relative) || fs::hard_link(path, &copy).is_err() {
            fs::copy(path, copy)?;
        }
        Ok(())
    })
    .map_err(|error| format!("stage deployment inputs: {error}"))?;
    let staged_dirs = RuntimeDirs {
        user_data_dir: stage.0.clone(),
        staging_dir: stage.0.join("build"),
        ..dirs.clone()
    };
    for edit in &edits {
        edit.apply_staged(&stage.0, true)?;
    }
    let inputs = if edits.is_empty() {
        live_inputs
    } else {
        input_signature(control_root, &staged_dirs, &mut hash_file)?
    };
    let skip = !edits.is_empty() && !cfg!(target_os = "ios") && receipt_matches(&build, &inputs);
    if !skip {
        compile(&staged_dirs)?;
        write_receipt(&staged_dirs.staging_dir, inputs)?;
    }
    let unchanged = live_stats.is_some_and(|before| {
        input_stat_signature(control_root, dirs).is_ok_and(|after| after == before)
    });
    if !unchanged && input_signature(control_root, dirs, &mut hash_file)? != live_inputs {
        return Err("package inputs changed during deployment".into());
    }
    for edit in &edits {
        if optional_file_sha256(&edit_path(user, &edit.relative)?)? != edit.previous_sha256 {
            return Err(format!(
                "package input changed during deployment: {}",
                edit.relative.display()
            ));
        }
    }
    let mut applied = 0;
    let publish: Result<_, String> = (|| {
        for edit in &edits {
            edit.apply_staged(user, false)?;
            applied += 1;
        }
        if !skip {
            if versioned_build {
                promote_versioned_build(&staged_dirs.staging_dir, user)?;
            } else {
                promote_build(&staged_dirs.staging_dir, &build)?;
            }
        }
        Ok(if skip {
            DeployStep::Skip
        } else {
            DeployStep::Done
        })
    })();
    if let Err(mut failure) = publish {
        for edit in edits[..applied].iter().rev() {
            if let Err(error) = edit.write_bytes(user, edit.previous.as_deref()) {
                failure.push_str(&format!(
                    "; restoring package input {}: {error}",
                    edit.relative.display()
                ));
            }
        }
        return Err(failure);
    }
    for edit in &edits {
        if edit.staged_source.is_none()
            && edit.previous_sha256.is_some()
            && (edit.relative == Path::new("essay.txt") || edit.relative.starts_with("opencc"))
        {
            crate::rt_log!(
                crate::runtime_log::Level::Info,
                "app",
                "runtime_common_file_removed",
                path = user.join(&edit.relative).display().to_string()
            );
        }
    }
    publish
}

struct InputEdit {
    relative: PathBuf,
    previous_sha256: Option<String>,
    previous: Option<Vec<u8>>,
    staged_source: Option<PathBuf>,
}

impl InputEdit {
    fn apply_staged(&self, root: &Path, compiling: bool) -> Result<(), String> {
        let path = edit_path(root, &self.relative)?;
        let result = if let Some(source) = &self.staged_source {
            if compiling && preserves_runtime_data(&self.relative) {
                fs::create_dir_all(path.parent().unwrap())
                    .and_then(|()| fs::copy(source, &path))
                    .map(|_| ())
            } else {
                link_or_copy_atomic(source, &path)
            }
        } else if path.exists() {
            fs::remove_file(&path)
        } else {
            Ok(())
        };
        result.map_err(|error| format!("write package input {}: {error}", path.display()))
    }

    fn write_bytes(&self, root: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
        let path = edit_path(root, &self.relative)?;
        let result = if let Some(bytes) = bytes {
            fs::create_dir_all(path.parent().unwrap())
                .and_then(|()| crate::write_file_atomic(&path, bytes))
        } else if path.exists() {
            fs::remove_file(&path)
        } else {
            Ok(())
        };
        result.map_err(|error| format!("write package input {}: {error}", path.display()))
    }
}

fn link_or_copy_atomic(source: &Path, destination: &Path) -> io::Result<()> {
    let parent = destination.parent().unwrap();
    fs::create_dir_all(parent)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(
        ".keytao-link-{}-{nonce}",
        std::process::id()
    ));
    let _ = fs::remove_file(&temp);
    if fs::hard_link(source, &temp).is_ok() {
        if let Err(error) = fs::rename(&temp, destination) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        return Ok(());
    }
    let bytes = fs::read(source)?;
    crate::write_file_atomic(destination, &bytes)
}

fn optional_file_sha256(path: &Path) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => file_hash(path)
            .map(|bytes| {
                let mut hex = String::with_capacity(64);
                for byte in bytes {
                    use std::fmt::Write;
                    let _ = write!(&mut hex, "{byte:02x}");
                }
                Some(hex)
            })
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        _ => Err(format!("invalid package input: {}", path.display())),
    }
}

fn optional_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            fs::read(path).map(Some).map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        _ => Err(format!("invalid package input: {}", path.display())),
    }
}

fn read_input_edits(runtime: &Path, name: &str) -> Result<Vec<InputEdit>, String> {
    if !name
        .strip_prefix(STAGED_INPUTS_PREFIX)
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err("invalid package input stage name".into());
    }
    let stage = runtime.join(name);
    if !fs::symlink_metadata(&stage).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(format!("invalid package input stage: {}", stage.display()));
    }
    let plan: BTreeMap<PathBuf, StagedInput> = serde_json::from_reader(
        regular_file(&stage.join(STAGED_INPUTS_MANIFEST)).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let mut edits: Vec<_> = plan
        .into_iter()
        .map(|(relative, edit)| {
            let existing_path = edit_path(runtime, &relative)?;
            let actual_sha256 = optional_file_sha256(&existing_path)?;
            if actual_sha256 != edit.previous_sha256 {
                return Err(format!(
                    "package input changed before deployment: {}",
                    relative.display()
                ));
            }
            let previous = if actual_sha256.is_some() {
                optional_file(&existing_path)?
            } else {
                None
            };
            let staged_source = if edit.replace {
                let staged = edit_path(&stage, &relative)?;
                if !fs::symlink_metadata(&staged).is_ok_and(|meta| meta.is_file()) {
                    return Err(format!("missing staged package input: {}", relative.display()));
                }
                Some(staged)
            } else {
                None
            };
            Ok(InputEdit {
                relative,
                previous_sha256: edit.previous_sha256,
                previous,
                staged_source,
            })
        })
        .collect::<Result<_, String>>()?;
    edits.sort_by_key(|edit| is_materialization_metadata(&edit.relative));
    Ok(edits)
}

fn is_materialization_metadata(path: &Path) -> bool {
    path == Path::new(RuntimeDirs::MANIFEST_FILE)
        || path == Path::new(".keytao-runtime.pending.json")
}

fn edit_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    use std::path::Component;

    let invalid = || format!("invalid package input path: {}", relative.display());
    let metadata = is_materialization_metadata(relative);
    let mut path = root.to_path_buf();
    let mut parts = relative.components().peekable();
    if parts.peek().is_none() {
        return Err(invalid());
    }
    while let Some(part) = parts.next() {
        let Component::Normal(part) = part else {
            return Err(invalid());
        };
        let part = part.to_str().ok_or_else(invalid)?;
        if part.contains(['\\', ':']) || part.ends_with(['.', ' ']) {
            return Err(invalid());
        }
        if !metadata {
            // Windows and default macOS volumes also resolve case variants of reserved paths.
            let name = part.to_ascii_lowercase();
            let is_nomedia = part == ".nomedia" && parts.peek().is_none();
            if !is_nomedia
                && (excluded_input(&name, false)
                    || matches!(name.as_str(), "scheme-packages" | "rime-data"))
            {
                return Err(invalid());
            }
        }
        path.push(part);
        if fs::symlink_metadata(&path).is_ok_and(|metadata| {
            if parts.peek().is_some() {
                !metadata.is_dir()
            } else {
                !metadata.is_file()
            }
        }) {
            return Err(invalid());
        }
    }
    Ok(path)
}

struct Stage(PathBuf);

impl Stage {
    fn create(path: PathBuf) -> Result<Self, String> {
        remove_directory(&path).map_err(|error| format!("clear deployment stage: {error}"))?;
        fs::create_dir(&path).map_err(|error| format!("create deployment stage: {error}"))?;
        Ok(Self(path))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = remove_directory(&self.0);
    }
}

fn remove_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => Err(io::Error::other(format!(
            "not a directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn recover_build(build: &Path) -> Result<(), String> {
    let previous = build.with_file_name(PREVIOUS_BUILD);
    if fs::symlink_metadata(&previous).is_ok_and(|metadata| metadata.is_dir()) && !build.exists() {
        fs::rename(previous, build).map_err(|error| format!("restore previous build: {error}"))?;
    }
    Ok(())
}

fn use_versioned_build(windows: bool, package_key: Option<&str>) -> bool {
    windows && package_key.is_some()
}

fn promote_versioned_build(staged: &Path, user: &Path) -> Result<(), String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let name = format!("{BUILD_GENERATION_PREFIX}{}-{nonce}", std::process::id());
    let generation = user.join(&name);
    if generation.exists() {
        return Err("package build generation already exists".into());
    }
    fs::rename(staged, &generation).map_err(|error| format!("install build: {error}"))?;
    if let Err(error) = crate::write_file_atomic(&user.join(BUILD_GENERATION_MARKER), name.as_bytes())
    {
        let _ = remove_directory(&generation);
        return Err(format!("publish build: {error}"));
    }
    // shortcut: old generations stay readable by mapped hosts until there is a reader lease protocol.
    Ok(())
}

fn promote_build(staged: &Path, build: &Path) -> Result<(), String> {
    if !build.exists() {
        return fs::rename(staged, build).map_err(|error| format!("install build: {error}"));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let staged = CString::new(staged.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let build = CString::new(build.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        #[cfg(target_os = "macos")]
        unsafe extern "C" {
            fn renamex_np(
                from: *const std::ffi::c_char,
                to: *const std::ffi::c_char,
                flags: u32,
            ) -> std::ffi::c_int;
        }
        #[cfg(target_os = "macos")]
        let status = {
            const RENAME_SWAP: u32 = 2;
            unsafe { renamex_np(staged.as_ptr(), build.as_ptr(), RENAME_SWAP) }
        };
        #[cfg(target_os = "linux")]
        let status = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                staged.as_ptr(),
                libc::AT_FDCWD,
                build.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if status == 0 {
            return Ok(());
        }
        return Err(format!("replace build: {}", io::Error::last_os_error()));
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let previous = build.with_file_name(PREVIOUS_BUILD);
        remove_directory(&previous).map_err(|error| format!("clear previous build: {error}"))?;
        fs::rename(build, &previous)
            .map_err(|error| format!("preserve previous build: {error}"))?;
        if let Err(error) = fs::rename(staged, build) {
            recover_build(build)
                .map_err(|recovery| format!("install build: {error}; {recovery}"))?;
            return Err(format!("install build: {error}"));
        }
        let _ = remove_directory(&previous);
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    package_inputs: [u8; 32],
    schema_receipt: [u8; 32],
    outputs: BTreeMap<PathBuf, u64>,
}

fn regular_file(path: &Path) -> io::Result<fs::File> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other(format!(
            "not a regular file: {}",
            path.display()
        )));
    }
    fs::File::open(path)
}

pub(crate) fn package_receipt_matches(control_root: &Path, dirs: &RuntimeDirs) -> bool {
    check_package_receipt(control_root, dirs, &dirs.staging_dir).is_ok_and(|(_, matches)| matches)
}

fn check_package_receipt(
    control_root: &Path,
    dirs: &RuntimeDirs,
    build: &Path,
) -> Result<([u8; 32], bool), String> {
    check_package_receipt_with(
        control_root,
        dirs,
        build,
        input_stat_signature(control_root, dirs).ok(),
        || input_signature(control_root, dirs, file_hash),
    )
}

fn check_package_receipt_with(
    control_root: &Path,
    dirs: &RuntimeDirs,
    build: &Path,
    stats: Option<[u8; 32]>,
    content_signature: impl FnOnce() -> Result<[u8; 32], String>,
) -> Result<([u8; 32], bool), String> {
    let cache_path = dirs.user_data_dir.join(INPUT_CACHE);
    if let Some(stats) = stats.as_ref() {
        if let Some(cache) = regular_file(&cache_path)
            .ok()
            .and_then(|file| serde_json::from_reader::<_, InputCache>(file).ok())
            .filter(|cache| cache.stats == *stats && receipt_matches(build, &cache.inputs))
        {
            return Ok((cache.inputs, true));
        }
    }
    let inputs = content_signature()?;
    if !receipt_matches(build, &inputs) {
        return Ok((inputs, false));
    }
    if let Some(stats) = stats {
        // Never pair the content digest with metadata from a concurrent edit.
        if input_stat_signature(control_root, dirs).is_ok_and(|current| current == stats) {
            if let Ok(bytes) = serde_json::to_vec(&InputCache { stats, inputs }) {
                let _ = crate::write_file_atomic(&cache_path, &bytes);
            }
        }
    }
    Ok((inputs, true))
}

#[derive(Deserialize, Serialize)]
struct InputCache {
    stats: [u8; 32],
    inputs: [u8; 32],
}

fn receipt_matches(build: &Path, inputs: &[u8; 32]) -> bool {
    (|| {
        let receipt: Receipt =
            serde_json::from_reader(regular_file(&build.join(RECEIPT)).ok()?).ok()?;
        let schema_receipt = file_hash(&build.join(SCHEMA_RECEIPT)).ok()?;
        let outputs = outputs(build).ok()?;
        Some(
            receipt.package_inputs == *inputs
                && receipt.schema_receipt == schema_receipt
                && receipt.outputs == outputs,
        )
    })() == Some(true)
}

fn write_receipt(build: &Path, inputs: [u8; 32]) -> Result<(), String> {
    let receipt = Receipt {
        package_inputs: inputs,
        schema_receipt: file_hash(&build.join(SCHEMA_RECEIPT))
            .map_err(|error| error.to_string())?,
        outputs: outputs(build).map_err(|error| error.to_string())?,
    };
    let bytes = serde_json::to_vec(&receipt).map_err(|error| error.to_string())?;
    crate::write_file_atomic(&build.join(RECEIPT), &bytes).map_err(|error| error.to_string())
}

fn outputs(build: &Path) -> io::Result<BTreeMap<PathBuf, u64>> {
    let mut outputs = BTreeMap::new();
    for entry in fs::read_dir(build)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == RECEIPT || name == SCHEMA_RECEIPT {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_file() {
            outputs.insert(name.into(), metadata.len());
        }
    }
    for required in [crate::DEPLOYED_FILE, "default.yaml"] {
        if !outputs.contains_key(Path::new(required)) {
            return Err(io::Error::other(format!(
                "missing build output: {required}"
            )));
        }
    }
    Ok(outputs)
}

fn file_hash(path: &Path) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    io::copy(&mut io::BufReader::new(regular_file(path)?), &mut hash)?;
    Ok(hash.finalize().into())
}

fn input_signature(
    control_root: &Path,
    dirs: &RuntimeDirs,
    hash_file: impl FnMut(&Path) -> io::Result<[u8; 32]>,
) -> Result<[u8; 32], String> {
    #[cfg(test)]
    FULL_INPUT_CHECKS.with(|checks| checks.set(checks.get() + 1));
    input_signature_with(control_root, dirs, b"keytao-package-inputs/v1\0", hash_file)
}

#[cfg(test)]
thread_local! {
    static FULL_INPUT_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn input_stat_cutoff() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_secs()).ok())
        .and_then(|now| now.checked_sub(2))
        .ok_or_else(|| "cannot determine input metadata cutoff".into())
}

fn input_stat_signature(control_root: &Path, dirs: &RuntimeDirs) -> Result<[u8; 32], String> {
    let cutoff = input_stat_cutoff()?;
    input_signature_with(
        control_root,
        dirs,
        b"keytao-package-input-stats/v1\0",
        |path| cacheable_input_stat(path, cutoff),
    )
}

fn cacheable_input_stat(path: &Path, cutoff: i64) -> io::Result<InputStat> {
    let stat = input_stat(path)?;
    // Coarse timestamps can hide same-tick rewrites.
    if stat.changed[0] <= 0 || stat.changed[0] >= cutoff || stat.modified[0] >= cutoff {
        return Err(io::Error::other("input metadata cannot be cached"));
    }
    Ok(stat)
}

#[doc(hidden)]
pub fn stable_input_stat(path: &Path) -> io::Result<InputStat> {
    cacheable_input_stat(path, input_stat_cutoff().map_err(io::Error::other)?)
}

#[doc(hidden)]
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct InputStat {
    volume: u64,
    file_id: [u8; 16],
    length: u64,
    modified: [i64; 2],
    changed: [i64; 2],
}

#[cfg(unix)]
fn input_stat(path: &Path) -> io::Result<InputStat> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("input is not a regular file"));
    }
    Ok(InputStat {
        volume: metadata.dev(),
        file_id: u128::from(metadata.ino()).to_le_bytes(),
        length: metadata.len(),
        modified: [metadata.mtime(), metadata.mtime_nsec()],
        changed: [metadata.ctime(), metadata.ctime_nsec()],
    })
}

#[cfg(windows)]
fn input_stat(path: &Path) -> io::Result<InputStat> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_BASIC_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_SHARE_READ,
        },
    };

    // A write handle can defer publishing the new change time until close.
    let file = fs::File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(io::Error::other("input is not a regular file"));
    }
    let handle = HANDLE(file.as_raw_handle());
    let mut identity = FILE_ID_INFO::default();
    let mut times = FILE_BASIC_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut identity as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
        .map_err(io::Error::other)?;
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            (&mut times as *mut FILE_BASIC_INFO).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
        .map_err(io::Error::other)?;
    }
    if identity.FileId.Identifier == [0; 16] {
        return Err(io::Error::other("input file identity is unavailable"));
    }
    let timestamp = |ticks| [ticks / 10_000_000 - 11_644_473_600, ticks % 10_000_000 * 100];
    Ok(InputStat {
        volume: identity.VolumeSerialNumber,
        file_id: identity.FileId.Identifier,
        length: metadata.len(),
        modified: timestamp(times.LastWriteTime),
        changed: timestamp(times.ChangeTime),
    })
}

fn input_signature_with<T: Clone + Serialize>(
    control_root: &Path,
    dirs: &RuntimeDirs,
    domain: &[u8],
    mut file_value: impl FnMut(&Path) -> io::Result<T>,
) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(crate::deployment_fingerprint(control_root)?);
    hash.update([0]);
    hash.update(crate::librime_runtime_version().ok_or("librime version is unavailable")?);
    let mut collect = |root: &Path, shared| -> Result<BTreeMap<PathBuf, T>, String> {
        let mut files = BTreeMap::new();
        walk_files(root, shared, &mut |relative, path| {
            if shared || !LUA_RUNTIME_STATE.iter().any(|name| relative == Path::new(name)) {
                files.insert(relative.to_path_buf(), file_value(path)?);
            }
            Ok(())
        })
        .map_err(|error| format!("read deployment inputs: {error}"))?;
        Ok(files)
    };
    let shared = collect(&dirs.shared_data_dir, true)?;
    let mut user = collect(&dirs.user_data_dir, false)?;
    // Use the packaged template for mutable copies to retain receipts written before they changed.
    for name in LUA_RUNTIME_STATE {
        if let Some((path, template)) = shared.get_key_value(Path::new(name)) {
            user.insert(path.clone(), template.clone());
        }
    }
    let prebuilt = (dirs.prebuilt_data_dir != dirs.shared_data_dir.join("build")
        && dirs.prebuilt_data_dir.is_dir())
    .then(|| collect(&dirs.prebuilt_data_dir, true))
    .transpose()?;
    for (index, files) in [user, shared].into_iter().chain(prebuilt).enumerate() {
        hash.update([index as u8]);
        hash.update(serde_json::to_vec(&files).map_err(|error| error.to_string())?);
    }
    Ok(hash.finalize().into())
}

fn excluded_input(name: &str, shared: bool) -> bool {
    name.starts_with('.')
        || name.contains(".userdb")
        || name.ends_with(".leveldb")
        || matches!(
            name,
            "sync"
                | "log"
                | "runtime"
                | "prebuilt"
                | "user.yaml"
                | "installation.yaml"
                | "app-state.json"
                | "auth-tokens.json"
                | PREVIOUS_BUILD
        )
        || (!shared && name == "build")
}

fn walk_files(
    root: &Path,
    shared: bool,
    action: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    fn visit(
        root: &Path,
        directory: &Path,
        shared: bool,
        action: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if excluded_input(&name, shared) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                visit(root, &path, shared, action)?;
            } else {
                if !kind.is_file() {
                    return Err(io::Error::other(format!(
                        "unsafe deployment input: {}",
                        relative.display()
                    )));
                }
                action(relative, &path)?;
            }
        }
        Ok(())
    }
    visit(root, root, shared, action)
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[test]
    #[ignore = "requires the bundled ime-jd engine beside the test executable"]
    fn invalid_libjd_dictionary_preserves_active_package_and_reload_stamp() {
        let fixture = crate::preview::PreviewDirectory::create().unwrap();
        let root = fixture.path();
        let store = crate::package_store::root(root);
        fs::create_dir_all(store.join("cloud/converted/files")).unwrap();
        let active = store.join("active-manifest.json");
        let previous = br#"{"active_package":"previous"}"#;
        fs::write(&active, previous).unwrap();
        let stamp = crate::ReloadStamp::write_package(root, "previous").unwrap();
        let previous_stamp = fs::read(&stamp).unwrap();
        fs::write(
            root.join(crate::english_mode::SETTINGS_FILE),
            br#"{"engine":"libjd_engine","engines":{"converted":"libjd_engine"}}"#,
        )
        .unwrap();
        let dictionary = crate::libjd::dictionary_directory(root, "converted").unwrap();
        fs::create_dir_all(&dictionary).unwrap();
        fs::write(
            dictionary.join("schema.json"),
            br#"{"id":"converted","name":"Converted"}"#,
        )
        .unwrap();
        let blob = dictionary.join("dictionary.bin");
        fs::write(&blob, b"invalid ime-jd dictionary").unwrap();

        let error =
            try_switch_ready_package_with_schema(root, root, "converted", None, true).unwrap_err();
        assert!(error.contains("invalid ime-jd dictionary"), "{error}");
        assert_eq!(fs::read(&active).unwrap(), previous);
        assert_eq!(fs::read(&stamp).unwrap(), previous_stamp);

        let library = crate::libjd::bundled_engine_library().unwrap();
        unsafe {
            crate::jd_backend::JdBackend::build_dictionary(&library, b"converted\ta\n", &blob)
        }
        .unwrap();
        try_switch_ready_package_with_schema(root, root, "converted", None, true).unwrap();
        assert_eq!(
            RuntimeDirs::active_package_key(root).as_deref(),
            Some("converted")
        );
        assert_eq!(
            crate::ReloadStamp::package_key(&stamp).as_deref(),
            Some("converted")
        );
    }

    #[test]
    fn windows_swaps_build_generations_for_inactive_packages_too() {
        assert!(use_versioned_build(true, Some("active")));
        assert!(use_versioned_build(true, Some("just-left")));
        assert!(!use_versioned_build(false, Some("just-left")));
        assert!(!use_versioned_build(true, None));
    }

    #[test]
    fn failed_compile_preserves_live_lua_sources_and_build() {
        let fixture = crate::preview::PreviewDirectory::create().unwrap();
        let root = fixture.path();
        let shared = root.join("shared");
        let user = root.join("runtime");
        let dirs = RuntimeDirs::flat(&user, &shared);
        fs::create_dir(&shared).unwrap();
        fs::create_dir_all(user.join("lua")).unwrap();
        fs::create_dir(&dirs.staging_dir).unwrap();
        fs::write(user.join("rime.lua"), b"old entry").unwrap();
        fs::write(user.join("lua/old.lua"), b"old module").unwrap();
        let live_data = [
            ("lua/state.yml", "live Lua data"),
            ("custom_phrase.txt", "live custom phrases"),
            ("example.user.dict.yaml", "live user words"),
        ];
        for (path, content) in live_data {
            fs::write(user.join(path), content).unwrap();
        }
        let assert_live_data = || {
            for (path, content) in live_data {
                assert_eq!(fs::read_to_string(user.join(path)).unwrap(), content);
            }
        };
        fs::write(dirs.staging_dir.join("default.yaml"), b"old build").unwrap();

        let name = ".keytao-inputs-regression";
        let preparation = user.join(name);
        fs::create_dir_all(preparation.join("lua")).unwrap();
        fs::write(preparation.join("rime.lua"), b"new entry").unwrap();
        fs::write(preparation.join("lua/new.lua"), b"new module").unwrap();
        fs::write(preparation.join("lua/.nomedia"), b"").unwrap();
        let edits = BTreeMap::from([
            (
                "rime.lua",
                StagedInput {
                    previous_sha256: Some(format!("{:x}", Sha256::digest(b"old entry"))),
                    replace: true,
                },
            ),
            (
                "lua/old.lua",
                StagedInput {
                    previous_sha256: Some(format!("{:x}", Sha256::digest(b"old module"))),
                    replace: false,
                },
            ),
            (
                "lua/new.lua",
                StagedInput {
                    previous_sha256: None,
                    replace: true,
                },
            ),
            (
                "lua/.nomedia",
                StagedInput {
                    previous_sha256: None,
                    replace: true,
                },
            ),
        ]);
        fs::write(
            preparation.join(STAGED_INPUTS_MANIFEST),
            serde_json::to_vec(&edits).unwrap(),
        )
        .unwrap();

        let compile = |stage: &RuntimeDirs, fail| {
            assert_eq!(
                fs::read(stage.user_data_dir.join("rime.lua")).unwrap(),
                b"new entry"
            );
            assert_eq!(
                fs::read(stage.user_data_dir.join("lua/new.lua")).unwrap(),
                b"new module"
            );
            assert!(!stage.user_data_dir.join("lua/old.lua").exists());
            assert!(stage.user_data_dir.join("lua/.nomedia").is_file());
            for (path, content) in live_data {
                let path = stage.user_data_dir.join(path);
                assert_eq!(fs::read_to_string(&path).unwrap(), content);
                fs::write(path, b"compiler output").unwrap();
            }
            fs::write(stage.user_data_dir.join("lua/new.lua"), b"compiler output").unwrap();
            fs::create_dir(&stage.staging_dir).unwrap();
            for output in [crate::DEPLOYED_FILE, "default.yaml", SCHEMA_RECEIPT] {
                fs::write(stage.staging_dir.join(output), b"new build").unwrap();
            }
            if fail {
                Err("compile failed".into())
            } else {
                Ok(())
            }
        };
        assert_eq!(
            deploy_package(root, &dirs, Some(name), |stage| compile(stage, true)),
            Err("compile failed".into())
        );
        assert_eq!(fs::read(user.join("rime.lua")).unwrap(), b"old entry");
        assert_eq!(fs::read(user.join("lua/old.lua")).unwrap(), b"old module");
        assert!(!user.join("lua/new.lua").exists());
        assert!(!user.join("lua/.nomedia").exists());
        assert_eq!(
            fs::read(preparation.join("lua/new.lua")).unwrap(),
            b"new module"
        );
        assert_live_data();
        assert_eq!(
            fs::read(dirs.staging_dir.join("default.yaml")).unwrap(),
            b"old build"
        );

        assert_eq!(
            deploy_package(root, &dirs, Some(name), |stage| compile(stage, false)),
            Ok(DeployStep::Done)
        );
        assert_eq!(fs::read(user.join("rime.lua")).unwrap(), b"new entry");
        assert_eq!(fs::read(user.join("lua/new.lua")).unwrap(), b"new module");
        assert!(!user.join("lua/old.lua").exists());
        assert!(user.join("lua/.nomedia").is_file());
        assert_live_data();
        assert_eq!(
            fs::read(dirs.staging_dir.join("default.yaml")).unwrap(),
            b"new build"
        );
        assert_eq!(
            deploy_package(root, &dirs, None, |_| panic!(
                "successful retry wrote a matching receipt"
            )),
            Ok(DeployStep::Skip)
        );
    }

    #[test]
    fn staged_nomedia_exception_only_allows_a_file_leaf() {
        let fixture = crate::preview::PreviewDirectory::create().unwrap();
        let root = fixture.path();
        assert_eq!(
            edit_path(root, Path::new("assets/.nomedia")).unwrap(),
            root.join("assets/.nomedia")
        );
        for path in [
            ".nomedia/file",
            "assets/.other",
            "sync/.nomedia",
            "../.nomedia",
        ] {
            assert!(edit_path(root, Path::new(path)).is_err(), "{path}");
        }
        fs::create_dir(root.join(".nomedia")).unwrap();
        assert!(edit_path(root, Path::new(".nomedia")).is_err());
    }

    #[test]
    fn isolated_deployed_status_requires_current_package_receipt() {
        for (key, shared) in [
            ("local-fixture", "scheme-packages/local/local-fixture/files"),
            ("builtin/fixture", "rime-data"),
        ] {
            assert_current_package_receipt(key, shared);
        }
    }

    fn assert_current_package_receipt(key: &str, shared: &str) {
        let fixture = crate::preview::PreviewDirectory::create().unwrap();
        let root = fixture.path();
        let store = root.join("scheme-packages");
        let shared = root.join(shared);
        let source = shared.join("keytao_fixture.schema.yaml");
        let original = b"schema: {schema_id: keytao_fixture}\n";
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("default.yaml"), "schema_list: []\n").unwrap();
        fs::write(&source, original).unwrap();
        fs::write(root.join("default.yaml"), "schema_list: [{schema: legacy}]\n").unwrap();
        fs::write(
            store.join("active-manifest.json"),
            serde_json::to_vec(&serde_json::json!({"active_package": key})).unwrap(),
        )
        .unwrap();
        let dirs = RuntimeDirs::for_package(root, &shared, key).unwrap();
        let patch = dirs.user_data_dir.join("default.custom.yaml");
        let selection = b"patch: {schema_list: [{schema: keytao_fixture}]}\n";
        fs::create_dir_all(&dirs.user_data_dir).unwrap();
        fs::write(
            root.join(RuntimeDirs::MIGRATION_MARKER),
            serde_json::to_vec(&serde_json::json!({"version": 1, "runtime_package": key})).unwrap(),
        )
        .unwrap();
        fs::write(&patch, selection).unwrap();
        let overrides = "JD6_userdata/JD6_overrides.tsv";
        fs::create_dir(shared.join("JD6_userdata")).unwrap();
        fs::create_dir(dirs.user_data_dir.join("JD6_userdata")).unwrap();
        fs::write(shared.join(overrides), b"template\n").unwrap();
        fs::write(dirs.user_data_dir.join(overrides), b"template\n").unwrap();
        fs::create_dir_all(dirs.user_data_dir.join("lua/TJ")).unwrap();
        let lua_words = dirs.user_data_dir.join("lua/TJ/words.txt");
        fs::write(&lua_words, b"source words\n").unwrap();

        let target = PackageDeployTarget {
            package_key: key.into(),
            package_name: "Fixture".into(),
            materialize_error: None,
            staged_inputs: None,
        };
        deploy_packages_with(
            root,
            &shared,
            None,
            &[target],
            |_| {},
            |stage, _| {
                fs::create_dir(&stage.staging_dir).unwrap();
                fs::write(stage.staging_dir.join("default.yaml"), selection).unwrap();
                fs::write(
                    stage.staging_dir.join("keytao_fixture.schema.yaml"),
                    original,
                )
                .unwrap();
                crate::complete_resolved_deployment(
                    &stage.user_data_dir,
                    stage.package_shared(),
                    || Ok(()),
                )
            },
        )
        .unwrap();
        if key == "local-fixture" {
            std::thread::sleep(std::time::Duration::from_secs(3));
            switch_ready_package(root, &shared, key).unwrap();
            let full_checks = FULL_INPUT_CHECKS.get();
            switch_ready_package(root, &shared, key).unwrap();
            assert_eq!(FULL_INPUT_CHECKS.get(), full_checks);
            assert_eq!(
                deploy_package(root, &dirs, None, |_| panic!(
                    "cached receipt must skip compilation"
                )),
                Ok(DeployStep::Skip)
            );
            assert_eq!(FULL_INPUT_CHECKS.get(), full_checks);

            let empty = root.join("empty");
            fs::create_dir(&empty).unwrap();
            let empty_dirs = RuntimeDirs::flat(&empty, &shared);
            assert_eq!(
                deploy_package(root, &empty_dirs, None, |stage| {
                    fs::create_dir(&stage.staging_dir).unwrap();
                    for name in [crate::DEPLOYED_FILE, "default.yaml", SCHEMA_RECEIPT] {
                        fs::write(stage.staging_dir.join(name), b"build").unwrap();
                    }
                    Ok(())
                }),
                Ok(DeployStep::Done)
            );
            assert_eq!(FULL_INPUT_CHECKS.get(), full_checks + 1);
            let full_checks = FULL_INPUT_CHECKS.get();

            let modified = fs::metadata(&source).unwrap().modified().unwrap();
            let mut writer = fs::File::options()
                .write(true)
                .open(&source)
                .unwrap();
            io::Write::write_all(&mut writer, b"schema: {schema_id: keytao_xixture}\n").unwrap();
            writer.set_modified(modified).unwrap();
            #[cfg(windows)]
            assert!(input_stat(&source).is_err());
            assert_eq!(
                switch_ready_package(root, &shared, key),
                Err("请在键道 App 部署此方案包".into())
            );
            assert!(FULL_INPUT_CHECKS.get() > full_checks);
            drop(writer);
            #[cfg(windows)]
            assert!(switch_ready_package(root, &shared, key).is_err());
            let full_checks = FULL_INPUT_CHECKS.get();
            assert_eq!(
                deploy_package(root, &dirs, None, |_| Err(
                    "changed input requires compilation".into()
                )),
                Err("changed input requires compilation".into())
            );
            assert_eq!(FULL_INPUT_CHECKS.get(), full_checks + 1);
            fs::write(&source, original).unwrap();
        }
        assert!(crate::schema_install_state(root).deployed, "{key}");
        assert!(crate::schema_install_state_with_shared(root, &shared).deployed);

        fs::write(dirs.user_data_dir.join(overrides), b"learned phrase\n").unwrap();
        fs::create_dir(dirs.user_data_dir.join("white_replace")).unwrap();
        for path in [
            "JD6_userdata/JD6_usage.tsv",
            "JD6_userdata/JD6_usage_events.tsv",
            "lua/kc_count.txt",
            "lua/TJ/direct_pending.txt",
            "lua/TJ/stats.txt",
            "lua/TJ/stats.txt.tmp",
            "white_replace/whitkey_created.txt",
            "white_replace/whitkey_otj_stats.txt",
            "white_replace/whitkey_pinned.txt",
        ] {
            fs::write(dirs.user_data_dir.join(path), b"runtime state\n").unwrap();
        }
        assert!(
            crate::schema_install_state_with_shared(root, &shared).deployed,
            "learning and typing must not invalidate the published build"
        );
        assert_eq!(
            deploy_package(root, &dirs, None, |_| panic!("runtime state needs no compile")),
            Ok(DeployStep::Skip)
        );
        fs::write(&lua_words, b"edited source words\n").unwrap();
        assert!(!crate::schema_install_state_with_shared(root, &shared).deployed);
        fs::write(&lua_words, b"source words\n").unwrap();
        fs::write(shared.join(overrides), b"new upstream template\n").unwrap();
        assert!(!crate::schema_install_state_with_shared(root, &shared).deployed);
        fs::write(shared.join(overrides), b"template\n").unwrap();
        assert!(crate::schema_install_state_with_shared(root, &shared).deployed);

        fs::write(
            &source,
            b"schema: {schema_id: keytao_fixture, name: Edited}\n",
        )
        .unwrap();
        assert!(
            !crate::schema_install_state(root).deployed,
            "shared source changed"
        );
        assert!(crate::schema_build_is_available(root));
        fs::write(&source, original).unwrap();
        assert!(crate::schema_install_state(root).deployed);
        fs::write(&patch, [selection.as_slice(), b"# edited\n"].concat()).unwrap();
        assert!(
            !crate::schema_install_state(root).deployed,
            "runtime source changed"
        );
        fs::write(&patch, selection).unwrap();
        assert!(crate::schema_install_state(root).deployed);
        let build = runtime_build_dir(&dirs.user_data_dir).unwrap();
        fs::remove_file(build.join(RECEIPT)).unwrap();
        assert!(
            !crate::schema_install_state(root).deployed,
            "receipt missing"
        );
        assert!(crate::schema_build_is_available(root));
    }

    #[test]
    fn changed_input_during_compile_preserves_live_build() {
        let fixture = crate::preview::PreviewDirectory::create().unwrap();
        let root = fixture.path();
        let shared = root.join("shared");
        fs::create_dir(&shared).unwrap();
        let source = shared.join("default.yaml");
        fs::write(&source, b"old input").unwrap();
        let modified = fs::metadata(&source).unwrap().modified().unwrap();
        let dirs = RuntimeDirs::flat(&root.join("runtime"), &shared);
        fs::create_dir_all(&dirs.staging_dir).unwrap();
        fs::write(dirs.staging_dir.join("default.yaml"), b"old build").unwrap();

        assert_eq!(
            deploy_package(root, &dirs, None, |stage| {
                let mut writer = fs::File::options().write(true).open(&source).unwrap();
                io::Write::write_all(&mut writer, b"new input").unwrap();
                writer.set_modified(modified).unwrap();
                fs::create_dir(&stage.staging_dir).unwrap();
                for name in [crate::DEPLOYED_FILE, "default.yaml", SCHEMA_RECEIPT] {
                    fs::write(stage.staging_dir.join(name), b"new build").unwrap();
                }
                Ok(())
            }),
            Err("package inputs changed during deployment".into())
        );
        assert_eq!(
            fs::read(dirs.staging_dir.join("default.yaml")).unwrap(),
            b"old build"
        );
    }

    #[test]
    fn receipt_hit_does_not_compile_and_failure_does_not_stop_later_packages() {
        let fixture = crate::preview::PreviewDirectory::create().unwrap();
        let root = fixture.path();
        let bundled = root.join("bundled");
        fs::create_dir(&bundled).unwrap();
        fs::write(bundled.join("default.yaml"), "schema_list: []\n").unwrap();
        let targets = ["ready", "broken", "pending"].map(|name| PackageDeployTarget {
            package_key: format!("builtin/{name}"),
            package_name: name.into(),
            materialize_error: None,
            staged_inputs: None,
        });
        for target in &targets {
            let dirs = RuntimeDirs::for_package(root, &bundled, &target.package_key).unwrap();
            fs::create_dir_all(&dirs.user_data_dir).unwrap();
            fs::write(
                dirs.user_data_dir.join("default.custom.yaml"),
                &target.package_name,
            )
            .unwrap();
        }
        let complete = |stage: &RuntimeDirs, _: &mut dyn FnMut(DeployStep)| {
            fs::create_dir(&stage.staging_dir).unwrap();
            for name in [crate::DEPLOYED_FILE, "default.yaml", SCHEMA_RECEIPT] {
                fs::write(stage.staging_dir.join(name), b"old build").unwrap();
            }
            Ok(())
        };
        deploy_packages_with(root, &bundled, None, &targets[..2], |_| {}, complete).unwrap();
        let broken = RuntimeDirs::for_package(root, &bundled, &targets[1].package_key).unwrap();
        fs::create_dir(broken.user_data_dir.join("lua")).unwrap();
        fs::write(
            broken.user_data_dir.join("lua/sidecar.tsv"),
            b"changed input",
        )
        .unwrap();
        let mut compiled = Vec::new();
        let mut terminal = Vec::new();
        let result = deploy_packages_with(
            root,
            &bundled,
            None,
            &targets,
            |event| {
                if matches!(
                    event.step,
                    DeployStep::Skip | DeployStep::Done | DeployStep::Failed
                ) {
                    terminal.push((event.index, event.total, event.step, event.error));
                }
            },
            |stage, emit| {
                let key = stage.active_package_key.as_deref().unwrap();
                compiled.push(key.to_owned());
                complete(stage, emit)?;
                if key == "builtin/broken" {
                    assert_eq!(
                        fs::read(stage.user_data_dir.join("lua/sidecar.tsv")).unwrap(),
                        b"changed input"
                    );
                    fs::write(stage.staging_dir.join("default.yaml"), b"partial build").unwrap();
                    return Err("compile failed\nmore details".into());
                }
                Ok(())
            },
        );
        assert_eq!(compiled, ["builtin/broken", "builtin/pending"]);
        assert_eq!(
            terminal,
            [
                (1, 3, DeployStep::Skip, None),
                (2, 3, DeployStep::Failed, Some("compile failed\nmore details".into())),
                (3, 3, DeployStep::Done, None),
            ]
        );
        assert_eq!(result, Err("broken: compile failed".into()));
        assert_eq!(
            fs::read(broken.staging_dir.join("default.yaml")).unwrap(),
            b"old build"
        );

        deploy_packages_with(
            root,
            &bundled,
            None,
            &targets[1..2],
            |_| {},
            |stage, emit| {
                complete(stage, emit)?;
                fs::write(stage.staging_dir.join("default.yaml"), b"replaced build").unwrap();
                Ok(())
            },
        )
        .unwrap();
        let build = runtime_build_dir(&broken.user_data_dir).unwrap();
        assert_eq!(
            fs::read(build.join("default.yaml")).unwrap(),
            b"replaced build"
        );

        if cfg!(target_os = "windows") {
            assert_ne!(build, broken.staging_dir);
            assert_eq!(
                fs::read(broken.staging_dir.join("default.yaml")).unwrap(),
                b"old build"
            );
            return;
        }
        let previous = broken.staging_dir.with_file_name(PREVIOUS_BUILD);
        fs::rename(&broken.staging_dir, &previous).unwrap();
        assert_eq!(
            RuntimeDirs::for_package(root, &bundled, &targets[1].package_key)
                .unwrap()
                .staging_dir,
            previous
        );
        deploy_packages_with(
            root,
            &bundled,
            None,
            &targets[1..2],
            |_| {},
            |_, _| panic!("recovering a matching build must skip compilation"),
        )
        .unwrap();
        assert_eq!(
            fs::read(broken.staging_dir.join("default.yaml")).unwrap(),
            b"replaced build"
        );
    }
}
