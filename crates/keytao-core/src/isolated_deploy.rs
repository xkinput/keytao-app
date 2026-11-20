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
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

pub const PACKAGE_TARGETS_ENV: &str = "KEYTAO_DEPLOY_PACKAGES";
const STAGE: &str = ".keytao-deploy-stage";
pub const STAGED_INPUTS_PREFIX: &str = ".keytao-inputs-";
pub const STAGED_INPUTS_MANIFEST: &str = ".edits.json";
const RECEIPT: &str = ".keytao-desktop-inputs.json";
const SCHEMA_RECEIPT: &str = ".keytao-schema-inputs.json";
const DEPLOY_LOCK: &str = ".keytao-package-deploy.lock";
const MATERIALIZE_LOCK: &str = ".keytao-materialize.lock";

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
    if !RuntimeDirs::isolation_enabled(control_root) {
        return Err("package isolation is disabled".into());
    }
    let _deploy = deploy_lock(control_root)?;
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
    if !RuntimeDirs::isolation_enabled(control_root) {
        return Err("package isolation is disabled".into());
    }
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
                let error = error
                    .lines()
                    .next()
                    .unwrap_or("deployment failed")
                    .to_owned();
                failures.push(format!("{}: {error}", target.package_name));
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

fn lock(path: &Path) -> Result<fs::File, String> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(format!("invalid deployment lock: {}", path.display()));
    }
    let file = fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| format!("open deployment lock: {error}"))?;
    file.lock()
        .map_err(|error| format!("lock deployment: {error}"))?;
    Ok(file)
}

pub fn with_isolated_package_lock<T>(
    control_root: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _deploy = RuntimeDirs::isolation_enabled(control_root)
        .then(|| deploy_lock(control_root))
        .transpose()?;
    action()
}

fn deploy_lock(control_root: &Path) -> Result<fs::File, String> {
    let runtime = control_root.join("runtime");
    match fs::create_dir(&runtime) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    if !fs::symlink_metadata(&runtime).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("invalid isolated runtime directory".into());
    }
    lock(&runtime.join(DEPLOY_LOCK))
}

pub fn package_build_ready(control_root: &Path, dirs: &RuntimeDirs) -> bool {
    dirs.active_package_key.is_some()
        && crate::resolved_schema_install_state(dirs).deployed
        && package_receipt_matches(control_root, dirs)
}

/// The IME can select a committed build. Only the app can prepare an unready package.
pub fn switch_ready_package(control_root: &Path, shared: &Path, key: &str) -> Result<(), String> {
    let not_ready = || "请在键道 App 部署此方案包".to_owned();
    let dirs = RuntimeDirs::for_package(control_root, shared, key).ok_or_else(not_ready)?;
    if !dirs.user_data_dir.is_dir() {
        return Err(not_ready());
    }
    let _deploy = deploy_lock(control_root)?;
    let _materialize = lock(&dirs.user_data_dir.join(MATERIALIZE_LOCK))?;
    let dirs = RuntimeDirs::for_package(control_root, shared, key).ok_or_else(not_ready)?;
    if !package_build_ready(control_root, &dirs) {
        return Err(not_ready());
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
    let _lock = lock(&user.join(MATERIALIZE_LOCK))?;
    let versioned_build =
        use_versioned_build(cfg!(target_os = "windows"), dirs.active_package_key.as_deref());
    let build = if versioned_build {
        runtime_build_dir(user)?
    } else {
        let build = user.join("build");
        recover_build(&build)?;
        build
    };
    let live_inputs = input_signature(control_root, dirs)?;
    let edits = staged_inputs
        .map(|name| read_input_edits(user, name))
        .transpose()?
        .unwrap_or_default();
    if edits.is_empty() && !cfg!(target_os = "ios") && receipt_matches(&build, &live_inputs) {
        return Ok(DeployStep::Skip);
    }

    let stage = Stage::create(user.join(STAGE))?;
    walk_files(user, false, &mut |relative, path| {
        let copy = stage.0.join(relative);
        fs::create_dir_all(copy.parent().unwrap())?;
        fs::copy(path, copy)?;
        Ok(())
    })
    .map_err(|error| format!("stage deployment inputs: {error}"))?;
    let staged_dirs = RuntimeDirs {
        user_data_dir: stage.0.clone(),
        staging_dir: stage.0.join("build"),
        ..dirs.clone()
    };
    for edit in &edits {
        edit.write(&stage.0, edit.next.as_deref())?;
    }
    let inputs = if edits.is_empty() {
        live_inputs
    } else {
        input_signature(control_root, &staged_dirs)?
    };
    let skip = !cfg!(target_os = "ios") && receipt_matches(&build, &inputs);
    if !skip {
        compile(&staged_dirs)?;
        write_receipt(&staged_dirs.staging_dir, inputs)?;
    }
    if input_signature(control_root, dirs)? != live_inputs {
        return Err("package inputs changed during deployment".into());
    }
    for edit in &edits {
        if optional_file(&edit_path(user, &edit.relative)?)? != edit.previous {
            return Err(format!(
                "package input changed during deployment: {}",
                edit.relative.display()
            ));
        }
    }
    let mut applied = 0;
    let publish: Result<_, String> = (|| {
        for edit in &edits {
            edit.write(user, edit.next.as_deref())?;
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
            if let Err(error) = edit.write(user, edit.previous.as_deref()) {
                failure.push_str(&format!(
                    "; restoring package input {}: {error}",
                    edit.relative.display()
                ));
            }
        }
        return Err(failure);
    }
    publish
}

struct InputEdit {
    relative: PathBuf,
    previous: Option<Vec<u8>>,
    next: Option<Vec<u8>>,
}

impl InputEdit {
    fn write(&self, root: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
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
            let previous = optional_file(&edit_path(runtime, &relative)?)?;
            if previous
                .as_ref()
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                != edit.previous_sha256
            {
                return Err(format!(
                    "package input changed before deployment: {}",
                    relative.display()
                ));
            }
            let next = if edit.replace {
                Some(
                    optional_file(&edit_path(&stage, &relative)?)?.ok_or_else(|| {
                        format!("missing staged package input: {}", relative.display())
                    })?,
                )
            } else {
                None
            };
            Ok(InputEdit {
                relative,
                previous,
                next,
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
            if excluded_input(&name, false)
                || matches!(name.as_str(), "scheme-packages" | "rime-data")
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
    input_signature(control_root, dirs)
        .is_ok_and(|inputs| receipt_matches(&dirs.staging_dir, &inputs))
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
    io::copy(&mut regular_file(path)?, &mut hash)?;
    Ok(hash.finalize().into())
}

fn input_signature(control_root: &Path, dirs: &RuntimeDirs) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    hash.update(b"keytao-package-inputs/v1\0");
    hash.update(crate::deployment_fingerprint(control_root)?);
    hash.update([0]);
    hash.update(crate::librime_runtime_version().ok_or("librime version is unavailable")?);
    let roots = [(&dirs.user_data_dir, false), (&dirs.shared_data_dir, true)];
    let prebuilt = (dirs.prebuilt_data_dir != dirs.shared_data_dir.join("build")
        && dirs.prebuilt_data_dir.is_dir())
    .then_some((&dirs.prebuilt_data_dir, true));
    for (index, (root, shared)) in roots.into_iter().chain(prebuilt).enumerate() {
        hash.update([index as u8]);
        let mut files = BTreeMap::new();
        walk_files(root, shared, &mut |relative, path| {
            files.insert(relative.to_path_buf(), file_hash(path)?);
            Ok(())
        })
        .map_err(|error| format!("read deployment inputs: {error}"))?;
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
            } else if shared
                || matches!(
                    relative
                        .extension()
                        .and_then(|extension| extension.to_str()),
                    Some("yaml" | "lua" | "txt" | "bin" | "ocd2" | "gram")
                )
                || relative.starts_with("opencc")
                || relative.starts_with("lua")
            {
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
        fs::write(dirs.staging_dir.join("default.yaml"), b"old build").unwrap();

        let name = ".keytao-inputs-regression";
        let preparation = user.join(name);
        fs::create_dir_all(preparation.join("lua")).unwrap();
        fs::write(preparation.join("rime.lua"), b"new entry").unwrap();
        fs::write(preparation.join("lua/new.lua"), b"new module").unwrap();
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
            root.join(RuntimeDirs::SETTINGS_FILE),
            br#"{"runtime_isolation":true}"#,
        )
        .unwrap();
        fs::write(
            store.join("active-manifest.json"),
            serde_json::to_vec(&serde_json::json!({"active_package": key})).unwrap(),
        )
        .unwrap();
        let dirs = RuntimeDirs::for_package(root, &shared, key).unwrap();
        let patch = dirs.user_data_dir.join("default.custom.yaml");
        let selection = b"patch: {schema_list: [{schema: keytao_fixture}]}\n";
        fs::create_dir_all(&dirs.user_data_dir).unwrap();
        fs::write(&patch, selection).unwrap();

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
        assert!(crate::schema_install_state(root).deployed, "{key}");
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
    fn receipt_hit_does_not_compile_and_failure_does_not_stop_later_packages() {
        let fixture = crate::preview::PreviewDirectory::create().unwrap();
        let root = fixture.path();
        let bundled = root.join("bundled");
        fs::create_dir(&bundled).unwrap();
        fs::write(bundled.join("default.yaml"), "schema_list: []\n").unwrap();
        fs::write(
            root.join(RuntimeDirs::SETTINGS_FILE),
            br#"{"runtime_isolation":true}"#,
        )
        .unwrap();
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
                (2, 3, DeployStep::Failed, Some("compile failed".into())),
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
