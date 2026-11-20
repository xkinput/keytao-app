//! JSON-lines protocol for deployment in a short-lived desktop process.

pub use crate::isolated_deploy::{PackageDeployTarget, PACKAGE_TARGETS_ENV};
pub use crate::{CandidatePreview, PreviewRequest};
use crate::{DeployProgress, DeployStep, RuntimeDirs};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub const DEPLOY_SUBCOMMAND: &str = "--deploy";
pub const PREVIEW_SUBCOMMAND: &str = "--preview-candidates";
pub const FLAT_TARGET_ENV: &str = "KEYTAO_DEPLOY_FLAT_TARGET";
const PACKAGE_DEPLOY_ARG: &str = "--packages";
const PROGRESS_VERSION_ENV: &str = "KEYTAO_DEPLOY_PROGRESS_VERSION";
const STRUCTURED_PROGRESS_VERSION: &str = "2";
const MAX_LINE_BYTES: u64 = 64 * 1024;

#[cfg(any(target_os = "windows", test))]
const WINDOWS_BUILD_SIGNATURE_FILE: &str = ".keytao-windows-build-signature.json";

#[cfg(any(target_os = "windows", test))]
fn windows_build_signature(user_dir: &Path, librime_version: &str) -> Result<Vec<u8>, String> {
    let dirs = crate::RuntimeDirs::resolve(user_dir, user_dir)?;
    let user_dir = dirs.user_data_dir.as_path();
    let mut schemas = crate::configured_schema_list_from_dir(user_dir);
    schemas.sort();
    schemas.dedup();
    serde_json::to_vec(&(
        crate::deployment_fingerprint(user_dir)?,
        librime_version,
        schemas,
    ))
    .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", test))]
fn windows_build_cleanup(user_dir: &Path, signature: &[u8]) -> Option<Vec<String>> {
    let dirs = crate::RuntimeDirs::resolve(user_dir, user_dir).ok()?;
    // None retains the old full repair path; Some lists only proven obsolete outputs.
    if std::fs::read_to_string(user_dir.join(crate::WINDOWS_RIME_BUILD_REPAIR_MARKER)).ok()?
        != crate::WINDOWS_RIME_BUILD_REPAIR_MARKER_CONTENT
        || dirs.staging_dir.join(crate::DEPLOYING_FILE).exists()
    {
        return None;
    }
    let previous = std::fs::read(dirs.staging_dir.join(WINDOWS_BUILD_SIGNATURE_FILE)).ok()?;
    let previous: (String, String, Vec<String>) = serde_json::from_slice(&previous).ok()?;
    let current: (String, String, Vec<String>) = serde_json::from_slice(signature).ok()?;
    if previous.0 != current.0
        || previous.1 != current.1
        || previous
            .2
            .iter()
            .chain(&current.2)
            .any(|id| !crate::valid_rime_build_id(id))
    {
        return None;
    }
    if previous.2 == current.2 {
        return crate::schema_install_state(user_dir)
            .deployed
            .then(Vec::new);
    }

    // A newly selected schema has no build yet: validate the *previous* deployment.
    let marker = std::fs::read(dirs.staging_dir.join(crate::DEPLOYED_FILE)).ok()?;
    let marker: crate::DeploymentMarker = serde_json::from_slice(&marker).ok()?;
    let deployed = crate::effective_schema_list(user_dir, previous.2.clone());
    if marker.fingerprint != previous.0
        || deployed.is_empty()
        || marker.schemas.is_empty()
        || crate::managed_primary_schemas_from_list(user_dir, deployed.clone())
            .iter()
            .any(|id| !marker.schemas.contains(id))
        || deployed
            .iter()
            .any(|id| !dirs.staging_dir.join(format!("{id}.schema.yaml")).is_file())
        || marker.schemas.iter().any(|id| {
            !previous.2.contains(id)
                || !dirs.staging_dir.join(format!("{id}.schema.yaml")).is_file()
        })
    {
        return None;
    }
    if previous.2.iter().all(|id| current.2.contains(id)) {
        return Some(Vec::new());
    }

    let before = windows_schema_build_outputs(user_dir, &deployed, false)?;
    let after = windows_schema_build_outputs(user_dir, &current.2, true)?;
    Some(before.difference(&after).cloned().collect())
}

#[cfg(any(target_os = "windows", test))]
fn windows_schema_build_outputs(
    user_dir: &Path,
    schemas: &[String],
    require_current_inputs: bool,
) -> Option<std::collections::BTreeSet<String>> {
    crate::build_prune::reachable_rime_build_outputs(
        user_dir,
        None,
        schemas,
        require_current_inputs,
        false,
    )
}

#[cfg(any(target_os = "windows", test))]
fn prepare_windows_deploy(user_dir: &Path, signature: &[u8]) -> Result<(), String> {
    // Decide before closing the runtime gate; failures must leave the next deploy in repair mode.
    let dirs = crate::RuntimeDirs::resolve(user_dir, user_dir)?;
    let cleanup = windows_build_cleanup(user_dir, signature);
    crate::clear_windows_rime_build_repair_marker(user_dir)?;
    crate::ReloadStamp::write(user_dir)?;
    match cleanup {
        // Obsolete outputs are pruned only after the deployment succeeds.
        Some(_) => {}
        None => {
            crate::remove_rime_build_artifact(
                &dirs.staging_dir.join(".keytao-desktop-inputs.json"),
            )?;
            crate::invalidate_active_windows_rime_build(&dirs.user_data_dir)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
const LOCK_FD_ENV: &str = "KEYTAO_LIBRIME_UPGRADE_LOCK_FD";

/// Keep the parent's deployment lock alive in the child; hold `lock` until spawn.
#[cfg(target_os = "linux")]
pub fn inherit_linux_deploy_lock(command: &mut Command, lock: &std::fs::File) {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};

    let lock_fd = lock.as_raw_fd();
    let wayland_fd = std::env::var("WAYLAND_SOCKET")
        .ok()
        .and_then(|value| value.parse::<i32>().ok());
    command
        .env(LOCK_FD_ENV, lock_fd.to_string())
        .env_remove("WAYLAND_SOCKET");
    unsafe {
        command.pre_exec(move || {
            let flags = libc::fcntl(lock_fd, libc::F_GETFD);
            if flags == -1 || libc::fcntl(lock_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // The deploy child must not keep KWin's private socket alive.
            if let Some(fd) = wayland_fd.filter(|fd| *fd > 2 && *fd != lock_fd) {
                libc::close(fd);
            }
            Ok(())
        });
    }
}

/// Adopt the inherited open file so deployment stays locked if the parent exits.
#[cfg(target_os = "linux")]
pub fn inherited_linux_deploy_lock() -> Result<std::fs::File, String> {
    use std::os::fd::FromRawFd;

    let lock_fd = std::env::var(LOCK_FD_ENV)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|fd| *fd > 2)
        .ok_or("librime deploy child requires an inherited deployment lock")?;
    if unsafe { libc::fcntl(lock_fd, libc::F_GETFD) } == -1 {
        return Err("librime deploy child received an invalid deployment lock".into());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(lock_fd) })
}

#[derive(Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
enum Message {
    Progress {
        progress: DeployProgress,
    },
    LegacyProgress {
        progress: String,
    },
    Preview {
        preview: CandidatePreview,
    },
    Final {
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

fn write_message(output: &mut impl Write, message: &Message) -> Result<(), String> {
    serde_json::to_writer(&mut *output, message)
        .map_err(|error| format!("cannot write deployment protocol: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("cannot flush deployment protocol: {error}"))
}

/// Handle `--deploy` or `--deploy --packages`; pass the code to [`exit_deploy_child`].
/// No engine session or caller-supplied data paths are accepted.
pub fn run_deploy_child_main() -> i32 {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    let package_plan =
        args.len() == 2 && args[0] == DEPLOY_SUBCOMMAND && args[1] == PACKAGE_DEPLOY_ARG;
    if package_plan {
        args.pop();
    }
    child_main_with_progress(
        args.into_iter(),
        &mut io::stdout().lock(),
        DEPLOY_SUBCOMMAND,
        |emit| {
            #[cfg(target_os = "linux")]
            let _lock = inherited_linux_deploy_lock()?;
            let user_dir = crate::default_user_data_dir()
                .ok_or("cannot determine the librime user data directory")?;
            if RuntimeDirs::isolation_enabled(&user_dir) {
                let targets = if package_plan {
                    let plan = std::env::var(PACKAGE_TARGETS_ENV)
                        .map_err(|error| format!("read package deployment plan: {error}"))?;
                    serde_json::from_str(&plan)
                        .map_err(|error| format!("invalid package deployment plan: {error}"))?
                } else {
                    let package_key = RuntimeDirs::active_package_key(&user_dir)
                        .ok_or("cannot determine active package for deployment")?;
                    vec![PackageDeployTarget {
                        package_name: package_key.clone(),
                        package_key,
                        materialize_error: None,
                        staged_inputs: None,
                    }]
                };
                crate::isolated_deploy::deploy_packages(
                    &user_dir,
                    Path::new(&crate::default_shared_data_dir()),
                    None,
                    &targets,
                    emit,
                )
            } else if package_plan {
                Err("package isolation is disabled".into())
            } else {
                emit(flat_compile_progress(Some(&user_dir)));
                deploy_directory(&user_dir)
            }
        },
    )
}

fn flat_compile_progress(user_dir: Option<&Path>) -> DeployProgress {
    if let Some(target) = std::env::var(FLAT_TARGET_ENV)
        .ok()
        .and_then(|target| serde_json::from_str::<PackageDeployTarget>(&target).ok())
    {
        return DeployProgress::flat(
            &target.package_key,
            &target.package_name,
            DeployStep::Compile,
            "Deploying librime...",
        );
    }
    let key = user_dir.and_then(RuntimeDirs::flat_active_package_key);
    let key = key.as_deref().unwrap_or("librime");
    DeployProgress::flat(key, key, DeployStep::Compile, "Deploying librime...")
}

/// Compile a host-owned isolated deployment directory in a short-lived process.
pub fn deploy_directory(user_dir: &Path) -> Result<(), String> {
    let shared_dir = crate::default_shared_data_dir();
    #[cfg(target_os = "windows")]
    let signature = windows_build_signature(
        user_dir,
        &crate::librime_runtime_version().ok_or("cannot determine the librime version")?,
    )?;
    #[cfg(target_os = "windows")]
    prepare_windows_deploy(user_dir, &signature)?;
    crate::deploy_for_process(
        user_dir.to_string_lossy().into_owned(),
        shared_dir,
        crate::DeployProcess::DeployChild,
    )?;
    #[cfg(target_os = "windows")]
    {
        crate::write_file_atomic(
            &crate::RuntimeDirs::resolve(user_dir, user_dir)?
                .staging_dir
                .join(WINDOWS_BUILD_SIGNATURE_FILE),
            &signature,
        )
        .map_err(|error| format!("write Windows build signature: {error}"))?;
        crate::mark_windows_rime_build_repair_complete(user_dir)?;
    }
    Ok(())
}

/// Inspect deployed data in an isolated user directory, never running deployment.
pub fn run_preview_child_main() -> i32 {
    preview_child_main(
        std::env::args_os().skip(1),
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        |request| {
            let user_dir = crate::default_user_data_dir()
                .ok_or("cannot determine the librime user data directory")?;
            crate::desktop::preview_deployed_candidates(&user_dir, request)
        },
    )
}

fn preview_child_main(
    args: impl Iterator<Item = OsString>,
    input: &mut impl Read,
    output: &mut impl Write,
    action: impl FnOnce(&PreviewRequest) -> Result<CandidatePreview, String>,
) -> i32 {
    let valid_args = args.eq([OsString::from(PREVIEW_SUBCOMMAND)]);
    let result = (|| {
        if !valid_args {
            return Err(format!(
                "usage: {PREVIEW_SUBCOMMAND} (no additional arguments)"
            ));
        }
        let mut request = Vec::new();
        input
            .take(MAX_LINE_BYTES + 1)
            .read_to_end(&mut request)
            .map_err(|e| format!("read candidate preview request: {e}"))?;
        if request.len() as u64 > MAX_LINE_BYTES {
            return Err("candidate preview request is too long".into());
        }
        let request: PreviewRequest = serde_json::from_slice(&request)
            .map_err(|e| format!("invalid candidate preview request: {e}"))?;
        request.validate()?;
        let preview = action(&request)?;
        write_message(output, &Message::Preview { preview })
    })();
    let code = if !valid_args {
        2
    } else if result.is_ok() {
        0
    } else {
        1
    };
    match write_message(
        output,
        &Message::Final {
            ok: result.is_ok(),
            error: result.err(),
        },
    ) {
        Ok(()) => code,
        Err(_) => 1,
    }
}

/// Handle exactly one trusted subcommand without running the private deployment.
pub fn run_deploy_child_main_with_action(
    subcommand: &str,
    action: impl FnOnce() -> Result<(), String>,
) -> i32 {
    child_main_with_progress(
        std::env::args_os().skip(1),
        &mut io::stdout().lock(),
        subcommand,
        |emit| {
            let user_dir = crate::default_user_data_dir();
            emit(flat_compile_progress(user_dir.as_deref()));
            action()
        },
    )
}

/// Flush deployment output and terminate without running librime's static destructors.
pub fn exit_deploy_child(code: i32) -> ! {
    let code = if io::stdout().flush().is_ok() {
        code
    } else {
        1
    };
    let _ = io::stderr().flush();
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn _exit(code: i32) -> !;
        }
        // A failed deploy can leave Deployer's maintenance thread joinable.
        _exit(code)
    }
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn TerminateProcess(process: *mut std::ffi::c_void, code: u32) -> i32;
        }
        // ExitProcess still runs DLL detach handlers and their C++ destructors.
        TerminateProcess(GetCurrentProcess(), code as u32);
        std::process::abort();
    }
}

#[cfg(test)]
fn child_main(
    args: impl Iterator<Item = OsString>,
    output: &mut impl Write,
    subcommand: &str,
    action: impl FnOnce() -> Result<(), String>,
) -> i32 {
    child_main_with_progress(args, output, subcommand, |emit| {
        emit(flat_compile_progress(None));
        action()
    })
}

fn child_main_with_progress(
    args: impl Iterator<Item = OsString>,
    output: &mut impl Write,
    subcommand: &str,
    action: impl FnOnce(&mut dyn FnMut(DeployProgress)) -> Result<(), String>,
) -> i32 {
    let valid_args = args.eq([OsString::from(subcommand)]);
    let result = if valid_args {
        // Unversioned parents only understand string progress lines.
        let structured_progress =
            std::env::var(PROGRESS_VERSION_ENV).as_deref() == Ok(STRUCTURED_PROGRESS_VERSION);
        let mut protocol_result = Ok(());
        let result = action(&mut |progress| {
            if protocol_result.is_ok() {
                let message = if structured_progress {
                    Message::Progress { progress }
                } else {
                    Message::LegacyProgress {
                        progress: progress.message(),
                    }
                };
                protocol_result = write_message(output, &message);
            }
        });
        protocol_result.and(result)
    } else {
        Err(format!("usage: {subcommand} (no additional arguments)"))
    };
    let exit_code = if !valid_args {
        2
    } else if result.is_ok() {
        0
    } else {
        1
    };
    if let Err(error) = &result {
        eprintln!("{error}");
    }
    let message = Message::Final {
        ok: result.is_ok(),
        error: result.err(),
    };
    if let Err(error) = write_message(output, &message) {
        eprintln!("{error}");
        return 1;
    }
    exit_code
}

#[derive(Default)]
struct Protocol {
    final_result: Option<Result<(), String>>,
    expect_preview: bool,
    preview: Option<CandidatePreview>,
}

impl Protocol {
    fn line(&mut self, line: &[u8]) -> Result<Option<DeployProgress>, String> {
        if self.final_result.is_some() {
            return Err("deploy child emitted output after its final result".into());
        }
        match serde_json::from_slice(line)
            .map_err(|error| format!("invalid deployment protocol line: {error}"))?
        {
            Message::Progress { progress } => return Ok(Some(progress)),
            // App and installed IME can temporarily run different versions.
            Message::LegacyProgress { progress } => {
                return Ok(Some(DeployProgress::flat(
                    "librime",
                    "librime",
                    DeployStep::Compile,
                    progress,
                )));
            }
            Message::Preview { preview } => {
                if !self.expect_preview
                    || self.preview.is_some()
                    || preview.page_size == 0
                    || preview.candidates.len() > usize::from(preview.page_size)
                {
                    return Err("invalid candidate preview result".into());
                }
                self.preview = Some(preview);
            }
            Message::Final {
                ok: true,
                error: None,
            } => self.final_result = Some(Ok(())),
            Message::Final {
                ok: false,
                error: Some(error),
            } => {
                self.final_result = Some(Err(error));
            }
            Message::Final { .. } => return Err("invalid deployment final result".into()),
        }
        Ok(None)
    }

    fn finish(&self, status: ExitStatus) -> Result<(), String> {
        match &self.final_result {
            Some(Err(error)) => Err(error.clone()),
            _ if !status.success() => Err(format!("deploy child exited unsuccessfully ({status})")),
            Some(Ok(())) if self.expect_preview && self.preview.is_none() => {
                Err("preview child did not emit candidates".into())
            }
            Some(Ok(())) => Ok(()),
            None => Err("deploy child did not emit a final result".into()),
        }
    }
}

pub fn run_preview_child(
    exe: &Path,
    request: &PreviewRequest,
    timeout: Duration,
) -> Result<CandidatePreview, String> {
    run_preview_child_command(&mut Command::new(exe), request, timeout)
}

/// Like deployment, the caller must verify the executable before constructing this command.
pub fn run_preview_child_command(
    command: &mut Command,
    request: &PreviewRequest,
    timeout: Duration,
) -> Result<CandidatePreview, String> {
    request.validate()?;
    let input = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    // The parent owns cleanup even when the bounded child is forcibly terminated.
    let workspace = crate::preview::PreviewDirectory::create()?;
    let mut child = command
        .arg(PREVIEW_SUBCOMMAND)
        .env(
            crate::preview::PREVIEW_DIRECTORY_ENV,
            workspace
                .path()
                .file_name()
                .ok_or("invalid preview workspace name")?,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot spawn preview child: {e}"))?;
    let guard = ChildGuard(&mut child);
    guard
        .0
        .stdin
        .take()
        .ok_or("preview child stdin is not piped")?
        .write_all(&input)
        .map_err(|e| format!("write preview request: {e}"))?;
    let mut protocol = Protocol {
        expect_preview: true,
        ..Protocol::default()
    };
    wait_for_child_protocol(guard.0, timeout, &mut |_| {}, &mut protocol)?;
    protocol
        .preview
        .ok_or("preview child did not emit candidates".into())
}

/// Execute a trusted binary directly with `--deploy`, killing and reaping it on failure.
pub fn run_deploy_child(exe: &Path, timeout: Duration) -> Result<(), String> {
    run_deploy_child_with_progress(exe, timeout, |_| {})
}

/// Execute a trusted binary with one subcommand, using the deployment protocol.
pub fn run_deploy_child_with_subcommand(
    exe: &Path,
    subcommand: &str,
    timeout: Duration,
) -> Result<(), String> {
    run_deploy_child_command_with_subcommand(&mut Command::new(exe), subcommand, timeout, |_| {})
}

/// Like [`run_deploy_child`], forwarding progress on the calling thread.
/// The callback must return promptly so the timeout can be enforced.
pub fn run_deploy_child_with_progress(
    exe: &Path,
    timeout: Duration,
    progress: impl FnMut(&DeployProgress),
) -> Result<(), String> {
    run_deploy_child_command_with_progress(&mut Command::new(exe), timeout, progress)
}

/// Run a command configured with platform environment and inherited handles.
pub fn run_deploy_child_command_with_progress(
    command: &mut Command,
    timeout: Duration,
    progress: impl FnMut(&DeployProgress),
) -> Result<(), String> {
    run_deploy_child_command_with_subcommand(command, DEPLOY_SUBCOMMAND, timeout, progress)
}

pub fn run_deploy_child_command_with_subcommand(
    command: &mut Command,
    subcommand: &str,
    timeout: Duration,
    mut progress: impl FnMut(&DeployProgress),
) -> Result<(), String> {
    let package_plan = subcommand == DEPLOY_SUBCOMMAND
        && command
            .get_envs()
            .any(|(name, value)| name == PACKAGE_TARGETS_ENV && value.is_some());
    command.arg(subcommand);
    if package_plan {
        // A pre-isolation helper rejects the extra argument before touching the active build.
        command.arg(PACKAGE_DEPLOY_ARG);
    }
    let mut child = command
        .env(PROGRESS_VERSION_ENV, STRUCTURED_PROGRESS_VERSION)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot spawn deploy child: {error}"))?;
    wait_for_child(&mut child, timeout, &mut progress)
}

struct ChildGuard<'a>(&'a mut Child);

impl Drop for ChildGuard<'_> {
    fn drop(&mut self) {
        // Also reap the child if a progress callback unwinds.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for_child(
    child: &mut Child,
    timeout: Duration,
    progress: &mut impl FnMut(&DeployProgress),
) -> Result<(), String> {
    wait_for_child_protocol(child, timeout, progress, &mut Protocol::default())
}

fn wait_for_child_protocol(
    child: &mut Child,
    timeout: Duration,
    progress: &mut impl FnMut(&DeployProgress),
    protocol: &mut Protocol,
) -> Result<(), String> {
    let guard = ChildGuard(child);
    let child = &mut *guard.0;
    let started = Instant::now();
    let stdout = child
        .stdout
        .take()
        .ok_or("deploy child stdout is not piped")?;
    // Bound both queued output and individual lines, including unterminated garbage.
    let (sender, receiver) = mpsc::sync_channel(1);
    // shortcut: descendants retaining stdout retain this reader; add process-tree cleanup in launchers.
    thread::Builder::new()
        .name("deploy-child-stdout".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let result = match reader
                    .by_ref()
                    .take(MAX_LINE_BYTES + 1)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => return,
                    Ok(count) if count as u64 <= MAX_LINE_BYTES => Ok(line),
                    Ok(_) => Err("deploy child protocol line is too long".into()),
                    Err(error) => Err(format!("cannot read deploy child stdout: {error}")),
                };
                let failed = result.is_err();
                if sender.send(result).is_err() || failed {
                    return;
                }
            }
        })
        .map_err(|error| format!("cannot read deploy child output: {error}"))?;
    let mut stdout_closed = false;
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err("deploy child timed out".into());
        }
        if stdout_closed {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("cannot wait for deploy child: {error}"))?
            {
                return protocol.finish(status);
            }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        } else {
            match receiver.recv_timeout(remaining) {
                Ok(line) => {
                    if let Some(message) = protocol.line(&line?)? {
                        progress(&message);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err("deploy child timed out".into());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => stdout_closed = true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS_BUILD_SIGNATURE: &str = "build/.keytao-windows-build-signature.json";

    #[test]
    fn windows_deploy_reuses_only_trusted_builds() {
        let root =
            std::env::temp_dir().join(format!("keytao-windows-incremental-{}", std::process::id()));
        std::fs::create_dir_all(root.join("build")).unwrap();
        let preserved = [
            (
                "default.custom.yaml",
                "patch:\n  schema_list: [{schema: keydo}]\n",
            ),
            (
                "keydo.schema.yaml",
                "schema: {schema_id: keydo}\ntranslator: {dictionary: keydo}\n",
            ),
            ("keydo.custom.yaml", "patch: {menu/page_size: 7}\n"),
            ("keydo.user.dict.yaml", "user words"),
            ("keydo.userdb/data", "learned words"),
        ];
        std::fs::create_dir_all(root.join("keydo.userdb")).unwrap();
        for (path, content) in preserved {
            std::fs::write(root.join(path), content).unwrap();
        }
        std::fs::write(
            root.join("build/keydo.schema.yaml"),
            "schema: {schema_id: keydo}\n",
        )
        .unwrap();
        let signature = windows_build_signature(&root, "1.17.0").unwrap();
        let trust = || {
            crate::write_deployed_marker(&root, &["keydo".into()]).unwrap();
            crate::mark_windows_rime_build_repair_complete(&root).unwrap();
            std::fs::write(root.join(WINDOWS_BUILD_SIGNATURE), &signature).unwrap();
        };
        trust();
        assert_eq!(windows_build_cleanup(&root, &signature), Some(vec![]));
        assert_eq!(
            windows_build_cleanup(&root, &windows_build_signature(&root, "1.18.0").unwrap()),
            None
        );
        for path in [
            crate::WINDOWS_RIME_BUILD_REPAIR_MARKER,
            "build/.keytao-deployed",
            WINDOWS_BUILD_SIGNATURE,
        ] {
            std::fs::remove_file(root.join(path)).unwrap();
            assert!(
                windows_build_cleanup(&root, &signature).is_none(),
                "missing {path}"
            );
            std::fs::write(root.join(path), b"invalid marker").unwrap();
            assert!(
                windows_build_cleanup(&root, &signature).is_none(),
                "invalid {path}"
            );
            trust();
        }
        std::fs::write(root.join("build/.keytao-deploying"), "interrupted").unwrap();
        assert!(windows_build_cleanup(&root, &signature).is_none());
        std::fs::remove_file(root.join("build/.keytao-deploying")).unwrap();
        std::fs::write(
            root.join("default.custom.yaml"),
            "patch:\n  schema_list: [{schema: keydo}, {schema: keytao}]\n",
        )
        .unwrap();
        let changed = windows_build_signature(&root, "1.17.0").unwrap();
        assert_ne!(signature, changed);
        assert_eq!(windows_build_cleanup(&root, &changed), Some(vec![]));
        std::fs::write(root.join("default.custom.yaml"), preserved[0].1).unwrap();

        for suffix in ["prism", "table", "reverse"] {
            std::fs::write(root.join(format!("build/keydo.{suffix}.bin")), suffix).unwrap();
        }
        prepare_windows_deploy(&root, &signature).unwrap();
        for suffix in ["prism", "table", "reverse"] {
            assert_eq!(
                std::fs::read(root.join(format!("build/keydo.{suffix}.bin"))).unwrap(),
                suffix.as_bytes()
            );
        }
        // A failed/interrupted deploy leaves the gate closed and forces the next repair.
        assert!(windows_build_cleanup(&root, &signature).is_none());
        prepare_windows_deploy(&root, &signature).unwrap();
        for suffix in ["prism", "table", "reverse"] {
            assert!(!root.join(format!("build/keydo.{suffix}.bin")).exists());
        }
        for (path, content) in preserved {
            assert_eq!(
                std::fs::read(root.join(path)).unwrap(),
                content.as_bytes(),
                "{path}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn windows_schema_removal_keeps_referenced_outputs_and_user_data() {
        for mode in [
            "exclusive",
            "shared",
            "shared_prism",
            "dependency",
            "packs",
            "removed_pack",
            "custom",
        ] {
            let root = std::env::temp_dir().join(format!(
                "keytao-windows-removal-{mode}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("build")).unwrap();
            let selection = |schemas: &[&str]| {
                let schemas: Vec<_> = schemas
                    .iter()
                    .map(|id| serde_json::json!({"schema": id}))
                    .collect();
                std::fs::write(
                    root.join("default.custom.yaml"),
                    serde_json::json!({"patch": {"schema_list": schemas}}).to_string(),
                )
                .unwrap();
            };
            selection(&["keydo", "keytao"]);
            let dictionary = if matches!(mode, "shared" | "shared_prism") {
                "keydo"
            } else {
                "keytao"
            };
            let extra = match mode {
                "dependency" => "  dependencies: [keydo]\n",
                _ => "",
            };
            for (id, dictionary, extra) in [("keydo", "keydo", ""), ("keytao", dictionary, extra)] {
                let source = format!("schema:\n  schema_id: {id}\n{extra}translator:\n  dictionary: {dictionary}\n  prism: {id}\n");
                std::fs::write(root.join(format!("{id}.schema.yaml")), &source).unwrap();
                let mut compiled = source;
                if mode == "shared_prism" {
                    compiled = compiled.replace(&format!("  prism: {id}\n"), "");
                }
                if id == "keytao" && mode == "packs" {
                    compiled.push_str("  packs: [keydo]\n");
                }
                if id == "keydo" && mode == "removed_pack" {
                    compiled.push_str("  packs: [extra]\n");
                    for suffix in ["table", "reverse"] {
                        std::fs::write(root.join(format!("build/extra.{suffix}.bin")), suffix)
                            .unwrap();
                    }
                }
                if id == "keytao" && mode == "custom" {
                    compiled = compiled.replace("dictionary: keytao", "dictionary: keydo");
                    std::fs::write(
                        root.join("keytao.custom.yaml"),
                        "patch: {translator/dictionary: keydo}\n",
                    )
                    .unwrap();
                }
                compiled.push_str(&format!(
                    "__build_info:\n  timestamps:\n    {id}.schema: 1\n    {id}.custom: 0\n"
                ));
                std::fs::write(root.join(format!("build/{id}.schema.yaml")), compiled).unwrap();
                for suffix in ["prism", "table", "reverse"] {
                    std::fs::write(root.join(format!("build/{id}.{suffix}.bin")), suffix).unwrap();
                }
            }
            let preserved = [
                ("keydo.custom.yaml", "patch: {menu/page_size: 7}\n"),
                ("keydo.user.dict.yaml", "user words"),
                ("keydo.userdb/data", "learned words"),
                ("sync/device/words", "synced words"),
            ];
            for (path, content) in preserved {
                std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
                std::fs::write(root.join(path), content).unwrap();
            }
            crate::record_schema_input_fingerprints(&root, &["keydo".into(), "keytao".into()])
                .unwrap();
            crate::write_deployed_marker(&root, &["keydo".into(), "keytao".into()]).unwrap();
            crate::mark_windows_rime_build_repair_complete(&root).unwrap();
            let signature = windows_build_signature(&root, "1.17.0").unwrap();
            std::fs::write(root.join(WINDOWS_BUILD_SIGNATURE), &signature).unwrap();
            selection(&["keytao"]);
            let signature = windows_build_signature(&root, "1.17.0").unwrap();
            let expected = match mode {
                "dependency" => vec![],
                "shared_prism" => vec!["keydo.schema.yaml"],
                "exclusive" => vec![
                    "keydo.prism.bin",
                    "keydo.reverse.bin",
                    "keydo.schema.yaml",
                    "keydo.table.bin",
                ],
                "removed_pack" => vec![
                    "extra.table.bin",
                    "keydo.prism.bin",
                    "keydo.reverse.bin",
                    "keydo.schema.yaml",
                    "keydo.table.bin",
                ],
                _ => vec!["keydo.prism.bin", "keydo.schema.yaml"],
            };
            let outputs = windows_build_cleanup(&root, &signature).unwrap();
            assert_eq!(outputs, expected, "{mode}");
            if mode == "exclusive" {
                let custom = root.join("keytao.custom.yaml");
                std::fs::write(&custom, "patch: {translator/dictionary: keydo}\n").unwrap();
                assert!(windows_build_cleanup(&root, &signature).is_none());
                std::fs::remove_file(custom).unwrap();
                let receipt = root.join("build/.keytao-schema-inputs.json");
                let saved = std::fs::read(&receipt).unwrap();
                std::fs::remove_file(&receipt).unwrap();
                assert!(windows_build_cleanup(&root, &signature).is_none());
                std::fs::write(receipt, saved).unwrap();
                selection(&["keytao", "keytao_new"]);
                assert!(windows_build_cleanup(
                    &root,
                    &windows_build_signature(&root, "1.17.0").unwrap()
                )
                .is_none());
                selection(&["keytao"]);
                assert_eq!(windows_build_cleanup(&root, &signature).unwrap(), outputs);
            }
            let retained: Vec<_> = std::fs::read_dir(root.join("build"))
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    let bytes = std::fs::read(&path).unwrap();
                    (path, bytes)
                })
                .filter(|(path, _)| !outputs.iter().any(|name| path.ends_with(name)))
                .collect();
            prepare_windows_deploy(&root, &signature).unwrap();
            for name in outputs {
                assert!(root.join("build").join(name).exists());
            }
            for (path, bytes) in retained {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
            std::fs::write(
                root.join("build/default.yaml"),
                "schema_list: [{schema: keytao}]\n",
            )
            .unwrap();
            std::fs::write(root.join("rime.lua"), "require('keytao_filter')\n").unwrap();
            std::fs::create_dir_all(root.join("lua")).unwrap();
            std::fs::write(root.join("lua/keytao_filter.lua"), "local dict = env.engine.schema.config:get_string('translator/dictionary')\nlocal db = ReverseDb('build/' .. dict .. '.reverse.bin')\nlocal arithmetic = load('return 1 + 1')\n").unwrap();
            crate::begin_deployment(&root).unwrap();
            crate::complete_deployment_with_cleanup(&root, || {
                crate::prune_rime_build_artifacts(&root, None).map(|_| ())
            })
            .unwrap();
            let removed = matches!(mode, "exclusive" | "removed_pack");
            for suffix in ["schema.yaml", "table.bin", "reverse.bin", "prism.bin"] {
                assert_eq!(
                    root.join(format!("build/keydo.{suffix}")).exists(),
                    !removed,
                    "{mode}: {suffix}"
                );
            }
            assert!(root.join("build/keytao.schema.yaml").is_file());
            assert!(root.join(format!("build/{dictionary}.table.bin")).is_file());
            if mode == "removed_pack" {
                assert!(!root.join("build/extra.table.bin").exists());
                assert!(!root.join("build/extra.reverse.bin").exists());
            }
            assert!(crate::schema_install_state(&root).deployed);
            assert!(!root.join("build/.keytao-deploying").exists());
            assert!(crate::ReloadStamp::path(&root).is_file());
            for (path, content) in preserved {
                assert_eq!(std::fs::read(root.join(path)).unwrap(), content.as_bytes());
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn windows_schema_selection_accepts_primary_only_deployment_marker() {
        let root =
            std::env::temp_dir().join(format!("keytao-windows-addon-{}", std::process::id()));
        std::fs::create_dir_all(root.join("build")).unwrap();
        let selection = |ids: &[&str]| {
            let schemas: Vec<_> = ids
                .iter()
                .map(|id| serde_json::json!({"schema": id}))
                .collect();
            std::fs::write(
                root.join("default.custom.yaml"),
                serde_json::json!({"patch": {"schema_list": schemas}}).to_string(),
            )
            .unwrap();
        };
        selection(&["keydo", "easy_en"]);
        for id in ["keydo", "easy_en"] {
            let source = format!("schema: {{schema_id: {id}}}\ntranslator: {{dictionary: {id}}}\n");
            std::fs::write(root.join(format!("{id}.schema.yaml")), &source).unwrap();
            std::fs::write(
                root.join(format!("build/{id}.schema.yaml")),
                format!("{source}__build_info:\n  timestamps: {{{id}.schema: 1}}\n"),
            )
            .unwrap();
        }
        crate::complete_deployment(&root).unwrap();
        let marker = std::fs::read(root.join("build").join(crate::DEPLOYED_FILE)).unwrap();
        let marker: crate::DeploymentMarker = serde_json::from_slice(&marker).unwrap();
        assert_eq!(marker.schemas, ["keydo"]);
        crate::mark_windows_rime_build_repair_complete(&root).unwrap();
        std::fs::write(
            root.join(WINDOWS_BUILD_SIGNATURE),
            windows_build_signature(&root, "1.17.0").unwrap(),
        )
        .unwrap();
        selection(&["keydo", "easy_en", "keytao_new"]);
        assert_eq!(
            windows_build_cleanup(&root, &windows_build_signature(&root, "1.17.0").unwrap()),
            Some(vec![])
        );
        selection(&["keydo"]);
        assert_eq!(
            windows_build_cleanup(&root, &windows_build_signature(&root, "1.17.0").unwrap())
                .unwrap(),
            [
                "easy_en.prism.bin",
                "easy_en.reverse.bin",
                "easy_en.schema.yaml",
                "easy_en.table.bin"
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protocol_golden_cases() {
        for (output, code, expected) in [
            (
                "{\"progress\":\"Compiling...\"}\n{\"ok\":true}\n",
                0,
                Ok(()),
            ),
            (
                "{\"ok\":false,\"error\":\"compile failed\"}\n",
                0,
                Err("compile failed"),
            ),
            (
                "{\"ok\":false,\"error\":\"compile failed\"}\n",
                1,
                Err("compile failed"),
            ),
            (
                "{\"progress\":\"Compiling...\"}\n",
                0,
                Err("deploy child did not emit a final result"),
            ),
            ("", 0, Err("deploy child did not emit a final result")),
            (
                "{\"ok\":true}\n",
                1,
                Err(if cfg!(unix) {
                    "deploy child exited unsuccessfully (exit status: 1)"
                } else {
                    "deploy child exited unsuccessfully (exit code: 1)"
                }),
            ),
        ] {
            let mut protocol = Protocol::default();
            let mut progress = Vec::new();
            for line in output.lines() {
                if let Some(message) = protocol.line(line.as_bytes()).unwrap() {
                    progress.push(message);
                }
            }
            #[cfg(unix)]
            let status = {
                use std::os::unix::process::ExitStatusExt;
                ExitStatus::from_raw(code << 8)
            };
            #[cfg(windows)]
            let status = {
                use std::os::windows::process::ExitStatusExt;
                ExitStatus::from_raw(code)
            };
            assert_eq!(
                protocol.finish(status),
                expected.map_err(str::to_owned),
                "{output:?}"
            );
            assert_eq!(
                progress,
                if output.starts_with("{\"progress\"") {
                    vec![DeployProgress::flat(
                        "librime",
                        "librime",
                        DeployStep::Compile,
                        "Compiling...",
                    )]
                } else {
                    vec![]
                }
            );
        }
    }

    #[test]
    fn protocol_rejects_garbage_and_nonfinal_results() {
        for output in [
            "garbage\n{\"ok\":true}",
            "\n{\"ok\":true}",
            "{\"unrelated\":true}",
            "{\"progress\":3}",
            "{\"ok\":\"true\"}",
            "{\"ok\":false}",
            "{\"ok\":true,\"error\":\"failed\"}",
            "{\"ok\":true,\"extra\":1}",
            "{\"ok\":true,\"progress\":\"ambiguous\"}",
            "{\"ok\":true}\ngarbage",
            "{\"ok\":true}\n{\"progress\":\"late\"}",
            "{\"ok\":true}\n{\"ok\":true}",
            "{\"ok\":false,\"error\":\"failed\"}\n{\"ok\":true}",
        ] {
            let mut protocol = Protocol::default();
            assert!(
                output
                    .lines()
                    .any(|line| protocol.line(line.as_bytes()).is_err()),
                "{output:?}"
            );
        }
        assert!(Protocol::default().line(b"\xff").is_err());
    }

    #[test]
    fn preview_child_validates_requests_and_protocol() {
        for input in [
            b"{\"page_size\":0}".as_slice(),
            b"{\"path\":\"/tmp/data\"}",
            b"{} {}",
        ] {
            let mut output = Vec::new();
            let code = preview_child_main(
                [OsString::from(PREVIEW_SUBCOMMAND)].into_iter(),
                &mut &input[..],
                &mut output,
                |_| panic!("invalid request must not initialize librime"),
            );
            assert_eq!(code, 1);
            assert!(String::from_utf8(output).unwrap().contains("\"ok\":false"));
        }
        let mut protocol = Protocol {
            expect_preview: true,
            ..Protocol::default()
        };
        protocol.line(b"{\"preview\":{\"candidates\":[{\"text\":\"real\",\"comment\":\"actual\"}],\"page_size\":1}}").unwrap();
        assert!(protocol
            .line(b"{\"preview\":{\"candidates\":[],\"page_size\":1}}")
            .is_err());
        assert!(Protocol::default()
            .line(b"{\"preview\":{\"candidates\":[],\"page_size\":1}}")
            .is_err());
    }

    #[test]
    fn real_librime_preview_child_preserves_user_config() {
        const FIXTURE: &str = "KEYTAO_PREVIEW_FIXTURE";
        const MODE: &str = "KEYTAO_PREVIEW_FIXTURE_MODE";
        const RESULT: &str = "KEYTAO_PREVIEW_RESULT=";
        if let Some(directory) = std::env::var_os(FIXTURE) {
            let directory = std::path::PathBuf::from(directory);
            let mode = std::env::var(MODE).unwrap();
            if mode == "deploy" {
                crate::deploy(
                    directory.to_string_lossy().into_owned(),
                    directory.to_string_lossy().into_owned(),
                )
                .unwrap();
                exit_deploy_child(0);
            }
            let before = std::fs::read(directory.join("user.yaml")).unwrap();
            let preview = if mode == "live" {
                crate::desktop::setup_only(
                    directory.to_string_lossy().into_owned(),
                    directory.to_string_lossy().into_owned(),
                )
                .unwrap();
                crate::desktop::preview_session(
                    "keytao_preview",
                    &PreviewRequest { page_size: Some(3) },
                    false,
                )
                .unwrap()
            } else {
                let mut output = Vec::new();
                let input = if mode == "scheme" {
                    "{}"
                } else {
                    "{\"page_size\":3}"
                };
                assert_eq!(
                    preview_child_main(
                        [OsString::from(PREVIEW_SUBCOMMAND)].into_iter(),
                        &mut input.as_bytes(),
                        &mut output,
                        |request| crate::desktop::preview_deployed_candidates(&directory, request),
                    ),
                    0,
                    "{}",
                    String::from_utf8_lossy(&output)
                );
                output
                    .split(|byte| *byte == b'\n')
                    .find_map(|line| match serde_json::from_slice::<Message>(line).ok()? {
                        Message::Preview { preview } => Some(preview),
                        _ => None,
                    })
                    .unwrap()
            };
            assert_eq!(std::fs::read(directory.join("user.yaml")).unwrap(), before);
            println!("{RESULT}{}", serde_json::to_string(&preview).unwrap());
            exit_deploy_child(0);
        }
        let directory =
            std::env::temp_dir().join(format!("keytao-preview-fixture-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("default.yaml"), "config_version: '1'\nschema_list:\n  - schema: keytao_decoy\n  - schema: keytao_preview\n").unwrap();
        std::fs::write(
            directory.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao_decoy\n    - schema: keytao_preview\n",
        )
        .unwrap();
        let schema = "schema:\n  schema_id: keytao_preview\n  name: Preview Fixture\n  version: '1'\nmenu:\n  page_size: 2\nengine:\n  processors: [speller, selector, express_editor]\n  segmentors: [abc_segmentor, fallback_segmentor]\n  translators: [table_translator]\nspeller:\n  alphabet: fabcd\ntranslator:\n  dictionary: keytao_preview\n  enable_completion: true\n  enable_sentence: false\n  enable_user_dict: false\n";
        for name in ["keytao_preview", "keytao_decoy"] {
            std::fs::write(
                directory.join(format!("{name}.schema.yaml")),
                schema.replace("schema_id: keytao_preview", &format!("schema_id: {name}")),
            )
            .unwrap();
        }
        std::fs::write(directory.join("keytao_preview.dict.yaml"), "---\nname: keytao_preview\nversion: '1'\nsort: by_weight\nuse_preset_vocabulary: false\n...\n甲\tfa\t100\n乙\tfb\t90\n丙\tfc\t80\n丁\tfd\t70\n").unwrap();
        let child = |mode: &str| {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "deploy_child::tests::real_librime_preview_child_preserves_user_config",
                    "--nocapture",
                ])
                .env(FIXTURE, &directory)
                .env(MODE, mode)
                .output()
                .unwrap()
        };
        let deployed = child("deploy");
        assert!(
            deployed.status.success(),
            "{}",
            String::from_utf8_lossy(&deployed.stderr)
        );
        let user =
            b"var:\n  previously_selected_schema: keytao_preview\n  preview_fixture: keep-me\n";
        std::fs::write(directory.join("user.yaml"), user).unwrap();
        let originals: Vec<_> = std::fs::read_dir(directory.join("build"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        for mode in ["scheme", "override", "live"] {
            let output = child(mode);
            assert!(
                output.status.success(),
                "{mode}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let preview: CandidatePreview = serde_json::from_str(
                stdout
                    .lines()
                    .find_map(|line| line.strip_prefix(RESULT))
                    .unwrap(),
            )
            .unwrap();
            let count = if mode == "scheme" { 2 } else { 3 };
            assert_eq!(usize::from(preview.page_size), count);
            assert_eq!(preview.candidates.len(), count);
            assert_eq!(
                preview
                    .candidates
                    .iter()
                    .map(|candidate| candidate.text.as_str())
                    .collect::<Vec<_>>(),
                ["甲", "乙", "丙"][..count]
            );
            assert_eq!(
                preview
                    .candidates
                    .iter()
                    .map(|candidate| candidate.comment.as_str())
                    .collect::<Vec<_>>(),
                ["~a", "~b", "~c"][..count]
            );
            assert_eq!(std::fs::read(directory.join("user.yaml")).unwrap(), user);
            for (path, bytes) in &originals {
                assert_eq!(std::fs::read(path).unwrap(), *bytes, "{}", path.display());
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn isolated_deploy_child_compiles_each_package_then_skips_the_unchanged_build() {
        use std::{collections::BTreeMap, fs, path::PathBuf};

        const FIXTURE: &str = "KEYTAO_ISOLATED_DEPLOY_FIXTURE";
        const SCHEMA: &str = "keytao_isolated_fixture";
        if let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) {
            let shared = root.join("bundled");
            let dirs = crate::RuntimeDirs::resolve(&root, &shared).unwrap();
            assert_eq!(dirs.active_package_key.as_deref(), Some("local-fixture"));
            assert_eq!(dirs.user_data_dir, root.join("runtime/local-fixture"));
            assert_eq!(dirs.staging_dir, dirs.user_data_dir.join("build"));
            assert_eq!(
                dirs.shared_data_dir,
                root.join("scheme-packages/local/local-fixture/files")
            );
            assert_eq!(dirs.prebuilt_data_dir, dirs.shared_data_dir.join("build"));
            let targets = [
                PackageDeployTarget {
                    package_key: "local-other".into(),
                    package_name: "Other".into(),
                    materialize_error: None,
                    staged_inputs: None,
                },
                PackageDeployTarget {
                    package_key: "local-fixture".into(),
                    package_name: "Active".into(),
                    materialize_error: None,
                    staged_inputs: None,
                },
            ];
            let code = child_main_with_progress(
                [OsString::from(DEPLOY_SUBCOMMAND)].into_iter(),
                &mut io::stdout().lock(),
                DEPLOY_SUBCOMMAND,
                |emit| {
                    crate::isolated_deploy::deploy_packages(&root, &shared, None, &targets, emit)?;
                    assert!(crate::schema_install_state(&root).deployed);
                    Ok(())
                },
            );
            exit_deploy_child(code);
        }

        fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
            let mut entries = BTreeMap::new();
            let mut pending = vec![root.to_path_buf()];
            while let Some(dir) = pending.pop() {
                for entry in fs::read_dir(dir).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    let relative = path.strip_prefix(root).unwrap().to_path_buf();
                    if entry.file_type().unwrap().is_dir() {
                        entries.insert(relative, None);
                        pending.push(path);
                    } else {
                        entries.insert(relative, Some(fs::read(path).unwrap()));
                    }
                }
            }
            entries
        }

        #[cfg(target_os = "macos")]
        fn md5(entries: &BTreeMap<PathBuf, Option<Vec<u8>>>) -> Vec<u8> {
            let mut child = Command::new("md5")
                .arg("-q")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(entries).unwrap())
                .unwrap();
            let result = child.wait_with_output().unwrap();
            assert!(result.status.success());
            result.stdout
        }

        let root =
            std::env::temp_dir().join(format!("keytao-isolated-deploy-{}", std::process::id()));
        let package = root.join("scheme-packages/local/local-fixture/files");
        let runtime = root.join("runtime/local-fixture");
        let other = root.join("runtime/local-other");
        let other_package = root.join("scheme-packages/local/local-other/files");
        let bundled = root.join("bundled");
        for dir in [
            package.join("build"),
            runtime.clone(),
            other.clone(),
            other_package.clone(),
            bundled.clone(),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(
            root.join(crate::RuntimeDirs::SETTINGS_FILE),
            r#"{"runtime_isolation":true}"#,
        )
        .unwrap();
        fs::write(
            root.join("scheme-packages/active-manifest.json"),
            r#"{"active_package":"local-fixture"}"#,
        )
        .unwrap();
        fs::write(
            bundled.join("default.yaml"),
            "config_version: '1'\nschema_list: []\n",
        )
        .unwrap();
        fs::write(
            package.join("default.yaml"),
            "config_version: '1'\nschema_list: []\n",
        )
        .unwrap();
        fs::write(
            package.join(format!("{SCHEMA}.schema.yaml")),
            format!(
                "schema: {{schema_id: {SCHEMA}, name: Isolation Fixture, version: '1'}}\n\
                 menu: {{page_size: 2}}\n\
                 engine:\n  processors: [speller, selector, express_editor]\n  segmentors: [abc_segmentor]\n  translators: [table_translator]\n\
                 translator: {{dictionary: {SCHEMA}, enable_user_dict: false}}\n"
            ),
        )
        .unwrap();
        fs::write(
            package.join(format!("{SCHEMA}.dict.yaml")),
            format!("---\nname: {SCHEMA}\nversion: '1'\nuse_preset_vocabulary: false\n...\n甲\ta\t100\n"),
        )
        .unwrap();
        fs::write(
            runtime.join("default.custom.yaml"),
            format!("patch:\n  schema_list: [{{schema: {SCHEMA}}}]\n"),
        )
        .unwrap();
        fs::write(
            runtime.join(format!("{SCHEMA}.custom.yaml")),
            "patch: {menu/page_size: 7}\n",
        )
        .unwrap();
        for name in [
            "default.yaml".into(),
            format!("{SCHEMA}.schema.yaml"),
            format!("{SCHEMA}.dict.yaml"),
        ] {
            fs::copy(package.join(&name), other_package.join(&name)).unwrap();
        }
        fs::copy(
            other_package.join("default.yaml"),
            bundled.join("default.yaml"),
        )
        .unwrap();
        fs::copy(
            runtime.join("default.custom.yaml"),
            other.join("default.custom.yaml"),
        )
        .unwrap();
        fs::write(
            other.join(format!("{SCHEMA}.custom.yaml")),
            "patch: {menu/page_size: 3}\n",
        )
        .unwrap();

        let run = || {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "deploy_child::tests::isolated_deploy_child_compiles_each_package_then_skips_the_unchanged_build",
                    "--nocapture",
                ])
                .env(FIXTURE, &root)
                .env(PROGRESS_VERSION_ENV, STRUCTURED_PROGRESS_VERSION)
                .env("KEYTAO_RIME_SHARED_DATA_DIR", &bundled)
                .env_remove("KEYTAO_RUNTIME_ISOLATION")
                .output()
                .unwrap();
            let messages = std::str::from_utf8(&output.stdout)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<Message>(line).ok())
                .collect::<Vec<_>>();
            (output, messages)
        };
        let (output, messages) = run();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(matches!(
            messages.last(),
            Some(Message::Final {
                ok: true,
                error: None
            })
        ));
        let steps = |messages: &[Message]| {
            messages
                .iter()
                .filter_map(|message| match message {
                    Message::Progress { progress } => {
                        Some((progress.index, progress.total, progress.step))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        use DeployStep::*;
        assert_eq!(
            steps(&messages),
            [1, 2]
                .into_iter()
                .flat_map(|index| {
                    [Materialize, Compile, Validate, Prune, Done]
                        .into_iter()
                        .map(move |step| (index, 2, step))
                })
                .collect::<Vec<_>>()
        );
        let build = runtime.join("build");
        let other_build = other.join("build");
        for (build, size) in [(&build, 7), (&other_build, 3)] {
            let compiled: serde_yaml::Value = serde_yaml::from_slice(
                &fs::read(build.join(format!("{SCHEMA}.schema.yaml"))).unwrap(),
            )
            .unwrap();
            assert_eq!(compiled["menu"]["page_size"].as_u64(), Some(size));
            assert!(build.join(format!("{SCHEMA}.table.bin")).is_file());
        }
        assert!(!root.join("build").exists());
        let originals = [tree(&build), tree(&other_build)];
        let (output, messages) = run();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            steps(&messages),
            [
                (1, 2, Materialize),
                (1, 2, Skip),
                (2, 2, Materialize),
                (2, 2, Skip)
            ]
        );

        fs::write(
            runtime.join("default.custom.yaml"),
            "patch: {schema_list: [{schema: missing}]}\n",
        )
        .unwrap();
        let (output, messages) = run();
        assert!(!output.status.success(), "{output:?}");
        assert!(messages.iter().any(|message| matches!(message, Message::Progress { progress } if progress.index == 1 && progress.step == Failed && progress.error.is_some())));
        assert!(steps(&messages).ends_with(&[(1, 2, Failed), (2, 2, Materialize), (2, 2, Skip)]));
        assert_eq!(
            RuntimeDirs::active_package_key(&root).as_deref(),
            Some("local-fixture")
        );
        assert!(!crate::schema_install_state(&root).deployed);
        for (original, build) in originals.iter().zip([&build, &other_build]) {
            let after = tree(build);
            #[cfg(target_os = "macos")]
            assert_eq!(md5(original), md5(&after));
            assert_eq!(*original, after);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn child_rejects_path_and_extra_arguments_without_deploying() {
        for args in [
            vec![],
            vec!["--other"],
            vec!["--deploy", "/tmp/data"],
            vec!["--deploy", "--deploy"],
        ] {
            let mut output = Vec::new();
            assert_eq!(
                child_main(
                    args.into_iter().map(OsString::from),
                    &mut output,
                    DEPLOY_SUBCOMMAND,
                    || panic!("invalid arguments must not deploy"),
                ),
                2
            );
            assert_eq!(
                String::from_utf8(output).unwrap(),
                "{\"ok\":false,\"error\":\"usage: --deploy (no additional arguments)\"}\n"
            );
        }
    }

    #[test]
    fn child_acknowledges_failure_before_reporting_success() {
        for result in [Ok(()), Err("public publication failed".to_owned())] {
            let mut output = Vec::new();
            let code = child_main(
                [OsString::from(DEPLOY_SUBCOMMAND)].into_iter(),
                &mut output,
                DEPLOY_SUBCOMMAND,
                || result.clone(),
            );
            let mut protocol = Protocol::default();
            let mut progress = Vec::new();
            for line in output
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                progress.extend(protocol.line(line).unwrap());
            }
            assert_eq!(progress.len(), 1);
            assert_eq!(progress[0].step, DeployStep::Compile);
            assert_eq!((progress[0].index, progress[0].total), (1, 1));
            assert_eq!(progress[0].message(), "Deploying librime...");
            assert_eq!(code, if result.is_ok() { 0 } else { 1 });
            assert_eq!(protocol.final_result, Some(result));
        }
    }

    #[test]
    fn child_progress_defaults_to_text_and_supports_version_two() {
        const CHILD: &str = "KEYTAO_DEPLOY_PROGRESS_TEST";
        if std::env::var_os(CHILD).is_some() {
            let code = child_main(
                [OsString::from(DEPLOY_SUBCOMMAND)].into_iter(),
                &mut io::stdout().lock(),
                DEPLOY_SUBCOMMAND,
                || Ok(()),
            );
            exit_deploy_child(code);
        }

        for version in [None, Some("99"), Some("2")] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "deploy_child::tests::child_progress_defaults_to_text_and_supports_version_two",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env_remove(FLAT_TARGET_ENV);
            match version {
                Some(version) => command.env(PROGRESS_VERSION_ENV, version),
                None => command.env_remove(PROGRESS_VERSION_ENV),
            };
            let output = command.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let stdout = std::str::from_utf8(&output.stdout).unwrap();
            let line = stdout.lines().find(|line| line.starts_with('{')).unwrap();
            if version == Some("2") {
                assert!(matches!(
                    serde_json::from_str::<Message>(line).unwrap(),
                    Message::Progress { progress } if progress.step == DeployStep::Compile
                ));
            } else {
                assert_eq!(line, r#"{"progress":"Deploying librime..."}"#);
            }
        }
    }

    #[test]
    fn child_exit_preserves_result_and_skips_static_destructors() {
        const CHILD: &str = "KEYTAO_DEPLOY_EXIT_TEST";
        if let Ok(outcome) = std::env::var(CHILD) {
            #[cfg(unix)]
            unsafe {
                extern "C" {
                    fn atexit(callback: extern "C" fn()) -> i32;
                }
                extern "C" fn abort_on_normal_exit() {
                    std::process::abort();
                }
                assert_eq!(atexit(abort_on_normal_exit), 0);
            }
            let code = child_main(
                [OsString::from(DEPLOY_SUBCOMMAND)].into_iter(),
                &mut io::stdout().lock(),
                DEPLOY_SUBCOMMAND,
                || match outcome.as_str() {
                    "success" => Ok(()),
                    "failure" => Err("compile failed".into()),
                    _ => panic!("unexpected child outcome"),
                },
            );
            exit_deploy_child(code);
        }
        for (outcome, code, final_line) in [
            ("success", 0, "{\"ok\":true}\n"),
            (
                "failure",
                1,
                "{\"ok\":false,\"error\":\"compile failed\"}\n",
            ),
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "deploy_child::tests::child_exit_preserves_result_and_skips_static_destructors",
                    "--nocapture",
                ])
                .env(CHILD, outcome)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(code), "{output:?}");
            assert!(output.stdout.ends_with(final_line.as_bytes()), "{output:?}");
        }
    }

    #[test]
    fn action_child_runs_only_requested_action_and_rejects_extra_arguments() {
        for args in [
            vec!["--deploy-public"],
            vec!["--deploy-public", "/tmp/data"],
            vec!["--deploy"],
        ] {
            let valid = args.len() == 1 && args[0] == "--deploy-public";
            let mut output = Vec::new();
            let mut action_ran = false;
            let code = child_main(
                args.into_iter().map(OsString::from),
                &mut output,
                "--deploy-public",
                || {
                    action_ran = true;
                    Ok(())
                },
            );
            assert_eq!(action_ran, valid);
            assert_eq!(code, if valid { 0 } else { 2 });
            let mut protocol = Protocol::default();
            for line in output
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                protocol.line(line).unwrap();
            }
            assert_eq!(
                protocol.final_result,
                Some(if valid {
                    Ok(())
                } else {
                    Err("usage: --deploy-public (no additional arguments)".into())
                })
            );
        }
    }

    #[cfg(unix)]
    fn helper_script(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("keytao-deploy-child-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("helper");
        std::fs::write(&exe, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        exe
    }

    #[cfg(unix)]
    #[test]
    fn parent_passes_only_subcommand_and_null_stdin_and_forwards_progress() {
        let exe = helper_script(
            "success",
            r#"
case "$#:$1" in 1:--deploy) ;; *) exit 2 ;; esac
if read ignored; then exit 3; fi
case "$KEYTAO_DEPLOY_PROGRESS_VERSION" in 2) ;; *) exit 4 ;; esac
printf '%s\n' '{"progress":"Compiling..."}' \
    '{"progress":{"package_key":"local-fixture","package_name":"Fixture","index":1,"total":2,"step":"validate"}}' \
    '{"ok":true}'
"#,
        );
        let mut progress = Vec::new();
        let result = run_deploy_child_with_progress(&exe, Duration::from_secs(5), |message| {
            progress.push(message.to_owned())
        });
        std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(progress.len(), 2);
        assert_eq!(
            progress[0],
            DeployProgress::flat("librime", "librime", DeployStep::Compile, "Compiling...")
        );
        assert_eq!(progress[1].package_key, "local-fixture");
        assert_eq!(progress[1].step, DeployStep::Validate);
        assert_eq!((progress[1].index, progress[1].total), (1, 2));
    }

    #[cfg(unix)]
    #[test]
    fn package_plan_passes_an_argument_older_children_will_reject() {
        let exe = helper_script(
            "package-plan",
            r#"
case "$#:$1:$2:$KEYTAO_DEPLOY_PACKAGES" in '2:--deploy:--packages:[]') ;; *) exit 2 ;; esac
printf '%s\n' '{"ok":true}'
"#,
        );
        let mut command = Command::new(&exe);
        command.env(PACKAGE_TARGETS_ENV, "[]");
        let result =
            run_deploy_child_command_with_progress(&mut command, Duration::from_secs(5), |_| {});
        std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(result, Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn parent_preserves_final_error_after_signal() {
        let exe = helper_script(
            "signal-error",
            "printf '%s\\n' '{\"ok\":false,\"error\":\"Rime deploy failed: compile failed\"}'; kill -TERM $$",
        );
        let result = run_deploy_child(&exe, Duration::from_secs(5));
        std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(result, Err("Rime deploy failed: compile failed".into()));
    }

    #[cfg(unix)]
    #[test]
    fn parent_reports_signal_without_treating_done_as_success() {
        for (name, body) in [
            ("signal-silent", "kill -TERM $$"),
            (
                "signal-done",
                "printf '%s\\n' '{\"ok\":true}'; kill -TERM $$",
            ),
        ] {
            let exe = helper_script(name, body);
            let error = run_deploy_child(&exe, Duration::from_secs(5)).unwrap_err();
            std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
            assert!(
                error.contains("deploy child exited unsuccessfully"),
                "{error}"
            );
            assert!(error.contains("signal: 15"), "{error}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inherited_deploy_lock_survives_parent_file_close() {
        let exe = helper_script("inherited-lock", "exec sleep 30");
        let dir = exe.parent().unwrap();
        let lock = crate::linux_deploy_lock(dir, Duration::ZERO)
            .unwrap()
            .unwrap();
        let mut command = Command::new(&exe);
        inherit_linux_deploy_lock(&mut command, &lock);
        let mut child = command.spawn().unwrap();
        let guard = ChildGuard(&mut child);
        drop(lock);
        assert!(crate::linux_deploy_lock(dir, Duration::ZERO)
            .unwrap()
            .is_none());
        guard.0.kill().unwrap();
        guard.0.wait().unwrap();
        assert!(crate::linux_deploy_lock(dir, Duration::ZERO)
            .unwrap()
            .is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_and_reaps_child_even_after_final_or_stdout_close() {
        for (name, body) in [
            ("silent", "exec sleep 30"),
            ("final", "printf '%s\\n' '{\"ok\":true}'; exec sleep 30"),
            ("closed", "exec 1>&-; exec sleep 30"),
        ] {
            let exe = helper_script(name, body);
            let mut child = Command::new(&exe).stdout(Stdio::piped()).spawn().unwrap();
            let started = Instant::now();
            let result = wait_for_child(&mut child, Duration::from_millis(100), &mut |_| {});
            let status = child.try_wait().unwrap();
            std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
            assert_eq!(result, Err("deploy child timed out".into()));
            assert!(started.elapsed() < Duration::from_secs(5));
            assert!(status.is_some_and(|status| !status.success()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn preview_parent_reads_bounded_result_and_enforces_timeout() {
        let exe = helper_script(
            "preview-result",
            r#"
case "$#:$1" in 1:--preview-candidates) ;; *) exit 2 ;; esac
cat >/dev/null
printf '%s' "$KEYTAO_CANDIDATE_PREVIEW_WORKSPACE" > "$0.workspace"
printf '%s\n' '{"preview":{"candidates":[{"text":"real","comment":"actual"}],"page_size":1}}' '{"ok":true}'
"#,
        );
        let preview = run_preview_child(
            &exe,
            &PreviewRequest { page_size: Some(1) },
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(preview.candidates[0].comment, "actual");
        let workspace = std::fs::read_to_string(exe.with_extension("workspace")).unwrap();
        assert!(!std::env::temp_dir().join(workspace).exists());
        std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
        let exe = helper_script(
            "preview-timeout",
            "printf '%s' \"$KEYTAO_CANDIDATE_PREVIEW_WORKSPACE\" > \"$0.workspace\"\nexec sleep 30",
        );
        let started = Instant::now();
        assert!(
            run_preview_child(&exe, &PreviewRequest::default(), Duration::from_secs(1))
                .unwrap_err()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let workspace = std::fs::read_to_string(exe.with_extension("workspace")).unwrap();
        assert!(!std::env::temp_dir().join(workspace).exists());
        std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn callback_unwind_kills_and_reaps_child() {
        let exe = helper_script(
            "callback-panic",
            "printf '%s\\n' '{\"progress\":\"Compiling...\"}'; exec sleep 30",
        );
        let mut child = Command::new(&exe).stdout(Stdio::piped()).spawn().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wait_for_child(&mut child, Duration::from_secs(5), &mut |_| {
                panic!("progress callback failed")
            })
        }));
        let status = child.try_wait().unwrap();
        std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
        assert!(result.is_err());
        assert!(status.is_some_and(|status| !status.success()));
    }
}
