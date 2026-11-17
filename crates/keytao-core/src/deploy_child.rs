//! JSON-lines protocol for deployment in a short-lived desktop process.

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub const DEPLOY_SUBCOMMAND: &str = "--deploy";
const MAX_LINE_BYTES: u64 = 64 * 1024;

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
        progress: String,
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

/// Handle exactly `--deploy`; binaries must exit with the returned code.
/// No engine session or caller-supplied data paths are accepted.
pub fn run_deploy_child_main() -> i32 {
    run_deploy_child_main_with_post_deploy(|| Ok(()))
}

/// Publish platform artifacts before acknowledging a successful deployment.
pub fn run_deploy_child_main_with_post_deploy(
    post_deploy: impl FnOnce() -> Result<(), String>,
) -> i32 {
    child_main(
        std::env::args_os().skip(1),
        &mut io::stdout().lock(),
        || {
            #[cfg(target_os = "linux")]
            let _lock = inherited_linux_deploy_lock()?;
            let user_dir = crate::default_user_data_dir()
                .ok_or("cannot determine the librime user data directory")?;
            let shared_dir = crate::default_shared_data_dir();
            #[cfg(target_os = "windows")]
            {
                crate::clear_windows_rime_build_repair_marker(&user_dir)?;
                crate::invalidate_active_windows_rime_build(&user_dir)?;
            }
            crate::deploy(user_dir.to_string_lossy().into_owned(), shared_dir)?;
            post_deploy()?;
            #[cfg(target_os = "windows")]
            crate::mark_windows_rime_build_repair_complete(&user_dir)?;
            Ok(())
        },
    )
}

fn child_main(
    args: impl Iterator<Item = OsString>,
    output: &mut impl Write,
    deploy: impl FnOnce() -> Result<(), String>,
) -> i32 {
    let valid_args = args.eq([OsString::from(DEPLOY_SUBCOMMAND)]);
    let result = if valid_args {
        write_message(
            output,
            &Message::Progress {
                progress: "Deploying librime...".into(),
            },
        )
        .and_then(|()| deploy())
    } else {
        Err("usage: --deploy (no additional arguments)".to_string())
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
}

impl Protocol {
    fn line(&mut self, line: &[u8]) -> Result<Option<String>, String> {
        if self.final_result.is_some() {
            return Err("deploy child emitted output after its final result".into());
        }
        match serde_json::from_slice(line)
            .map_err(|error| format!("invalid deployment protocol line: {error}"))?
        {
            Message::Progress { progress } => return Ok(Some(progress)),
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

    fn finish(self, exit_code: Option<i32>) -> Result<(), String> {
        let result = self
            .final_result
            .ok_or("deploy child did not emit a final result")?;
        if exit_code != Some(0) {
            return Err(format!(
                "deploy child exited unsuccessfully ({exit_code:?}): {}",
                result.err().unwrap_or_default()
            ));
        }
        result
    }
}

/// Execute a trusted binary directly with `--deploy`, killing and reaping it on failure.
pub fn run_deploy_child(exe: &Path, timeout: Duration) -> Result<(), String> {
    run_deploy_child_with_progress(exe, timeout, |_| {})
}

/// Like [`run_deploy_child`], forwarding progress on the calling thread.
/// The callback must return promptly so the timeout can be enforced.
pub fn run_deploy_child_with_progress(
    exe: &Path,
    timeout: Duration,
    progress: impl FnMut(&str),
) -> Result<(), String> {
    run_deploy_child_command_with_progress(&mut Command::new(exe), timeout, progress)
}

/// Run a command configured with platform environment and inherited handles.
pub fn run_deploy_child_command_with_progress(
    command: &mut Command,
    timeout: Duration,
    mut progress: impl FnMut(&str),
) -> Result<(), String> {
    let mut child = command
        .arg(DEPLOY_SUBCOMMAND)
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
    progress: &mut impl FnMut(&str),
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
    let mut protocol = Protocol::default();
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
                return protocol.finish(status.code());
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

    #[test]
    fn protocol_golden_cases() {
        for (output, code, expected) in [
            (
                "{\"progress\":\"Compiling...\"}\n{\"ok\":true}\n",
                Some(0),
                Ok(()),
            ),
            (
                "{\"ok\":false,\"error\":\"compile failed\"}\n",
                Some(0),
                Err("compile failed"),
            ),
            (
                "{\"progress\":\"Compiling...\"}\n",
                Some(0),
                Err("deploy child did not emit a final result"),
            ),
            ("", Some(0), Err("deploy child did not emit a final result")),
            (
                "{\"ok\":true}\n",
                Some(1),
                Err("deploy child exited unsuccessfully (Some(1)): "),
            ),
            (
                "{\"ok\":true}\n",
                None,
                Err("deploy child exited unsuccessfully (None): "),
            ),
        ] {
            let mut protocol = Protocol::default();
            let mut progress = Vec::new();
            for line in output.lines() {
                if let Some(message) = protocol.line(line.as_bytes()).unwrap() {
                    progress.push(message);
                }
            }
            assert_eq!(
                protocol.finish(code),
                expected.map_err(str::to_owned),
                "{output:?}"
            );
            assert_eq!(
                progress,
                if output.starts_with("{\"progress\"") {
                    vec!["Compiling..."]
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
    fn child_rejects_path_and_extra_arguments_without_deploying() {
        for args in [
            vec![],
            vec!["--other"],
            vec!["--deploy", "/tmp/data"],
            vec!["--deploy", "--deploy"],
        ] {
            let mut output = Vec::new();
            assert_eq!(
                child_main(args.into_iter().map(OsString::from), &mut output, || {
                    panic!("invalid arguments must not deploy")
                }),
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
                || result.clone(),
            );
            let mut protocol = Protocol::default();
            for line in output
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                protocol.line(line).unwrap();
            }
            assert_eq!(code, if result.is_ok() { 0 } else { 1 });
            assert_eq!(protocol.final_result, Some(result));
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
printf '%s\n' '{"progress":"Compiling..."}' '{"ok":true}'
"#,
        );
        let mut progress = Vec::new();
        let result = run_deploy_child_with_progress(&exe, Duration::from_secs(5), |message| {
            progress.push(message.to_owned())
        });
        std::fs::remove_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(progress, ["Compiling..."]);
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
