use crate::{desktop, runtime_dirs::PREVIOUS_BUILD, DeployProgress, DeployStep, RuntimeDirs};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

pub const PACKAGE_TARGETS_ENV: &str = "KEYTAO_DEPLOY_PACKAGES";
const STAGE: &str = ".keytao-deploy-stage";
const RECEIPT: &str = ".keytao-desktop-inputs.json";
const SCHEMA_RECEIPT: &str = ".keytao-schema-inputs.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDeployTarget {
    pub package_key: String,
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize_error: Option<String>,
}

pub fn deploy_packages(
    control_root: &Path,
    shared: &Path,
    package_data_root: Option<&Path>,
    targets: &[PackageDeployTarget],
    progress: impl FnMut(DeployProgress),
) -> Result<(), String> {
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
    let _lock = lock(&control_root.join(".keytao-package-deploy.lock"))?;
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
            deploy_package(control_root, &dirs, |stage| {
                compile(stage, &mut |step| emit(step, None))
            })
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

fn deploy_package(
    control_root: &Path,
    dirs: &RuntimeDirs,
    compile: impl FnOnce(&RuntimeDirs) -> Result<(), String>,
) -> Result<DeployStep, String> {
    let user = &dirs.user_data_dir;
    let build = &user.join("build");
    let _lock = lock(&user.join(".keytao-materialize.lock"))?;
    recover_build(build)?;
    let inputs = input_signature(control_root, dirs)?;
    if !cfg!(target_os = "ios") && receipt_matches(build, &inputs) {
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
    compile(&staged_dirs)?;
    if input_signature(control_root, dirs)? != inputs {
        return Err("package inputs changed during deployment".into());
    }
    if !cfg!(target_os = "ios") {
        write_receipt(&staged_dirs.staging_dir, inputs)?;
    }
    promote_build(&staged_dirs.staging_dir, build)?;
    Ok(DeployStep::Done)
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
            if name.starts_with('.')
                || name.contains(".userdb")
                || name.ends_with(".leveldb")
                || matches!(
                    name.as_ref(),
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
            {
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
        assert_eq!(
            fs::read(broken.staging_dir.join("default.yaml")).unwrap(),
            b"replaced build"
        );

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
