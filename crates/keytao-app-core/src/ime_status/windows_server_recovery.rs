//! Recovery ends before the caller is allowed to send Deploy.

use std::io;

pub(super) fn can_manage_server(elevated: bool, shell_launch_available: bool) -> bool {
    !elevated || shell_launch_available
}

pub(super) fn require_absent_server(pipe: io::Result<()>) -> Result<(), String> {
    match pipe {
        // Win32 ERROR_FILE_NOT_FOUND; keep this check portable for recovery tests.
        Err(error) if error.raw_os_error() == Some(2) => Ok(()),
        Ok(()) => Err("IME server pipe exists; in-process deploy is forbidden".into()),
        Err(error) => Err(format!(
            "cannot confirm IME server pipe is absent; in-process deploy is forbidden: {error}"
        )),
    }
}

#[derive(Debug)]
pub(super) struct HelloFailure {
    pub reason: String,
    pub eof_before_reply: bool,
}

impl HelloFailure {
    pub fn other(reason: String) -> Self {
        Self {
            reason,
            eof_before_reply: false,
        }
    }

    pub fn read(error: io::Error, bytes_received: usize) -> Self {
        Self {
            reason: format!("read IME server hello: {error}"),
            eof_before_reply: bytes_received == 0
                && matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe
                ),
        }
    }
}

pub(super) fn deploy_once<C>(
    management: Result<(), String>,
    connect: impl FnMut() -> Result<Option<(C, bool)>, String>,
    hello: impl FnMut(&mut C) -> Result<(), HelloFailure>,
    replace: impl FnMut(C, bool) -> Result<(), String>,
    deploy: impl FnOnce(C) -> Result<(), String>,
) -> Result<bool, String> {
    // A failed Deploy write may already have reached the server. Recovery is only preparation.
    match prepare_server(management, connect, hello, replace)? {
        Some(connection) => deploy(connection).map(|()| true),
        None => Ok(false),
    }
}

fn prepare_server<C>(
    management: Result<(), String>,
    mut connect: impl FnMut() -> Result<Option<(C, bool)>, String>,
    mut hello: impl FnMut(&mut C) -> Result<(), HelloFailure>,
    mut replace: impl FnMut(C, bool) -> Result<(), String>,
) -> Result<Option<C>, String> {
    let Some((mut connection, current)) = connect()? else {
        return Ok(None);
    };
    let handshake = hello(&mut connection);
    if handshake.is_ok() && current {
        return Ok(Some(connection));
    }
    let reason = handshake
        .as_ref()
        .err()
        .map_or("outdated IME server", |error| error.reason.as_str());
    let reason = reason.to_owned();
    if current
        && !handshake
            .as_ref()
            .is_err_and(|error| error.eof_before_reply)
    {
        return Err(reason);
    }
    management.map_err(|error| format!("{reason}; {error}"))?;
    replace(connection, handshake.is_ok())
        .map_err(|error| format!("{reason}; IME server recovery failed: {error}"))?;
    let (mut connection, current) = connect()
        .map_err(|error| format!("{reason}; reconnect IME server: {error}"))?
        .ok_or_else(|| format!("{reason}; replacement IME server pipe is missing"))?;
    hello(&mut connection).map_err(|error| {
        format!(
            "{reason}; IME server handshake still failed: {}",
            error.reason
        )
    })?;
    if !current {
        return Err(format!(
            "{reason}; replacement IME server is still outdated"
        ));
    }
    Ok(Some(connection))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn in_process_deploy_requires_a_definitely_absent_server_pipe() {
        assert_eq!(
            require_absent_server(Err(io::Error::from_raw_os_error(2))),
            Ok(())
        );
        assert!(
            require_absent_server(Ok(()))
                .unwrap_err()
                .contains("in-process deploy is forbidden")
        );
        for error in [
            io::Error::from_raw_os_error(231),
            io::Error::from_raw_os_error(5),
            io::Error::from_raw_os_error(3),
            io::Error::other("pipe lookup failed"),
        ] {
            let reason = error.to_string();
            let failure = require_absent_server(Err(error)).unwrap_err();
            assert!(failure.contains("in-process deploy is forbidden"));
            assert!(failure.contains(&reason));
        }
    }

    #[test]
    fn healthy_current_server_needs_no_recovery() {
        assert_eq!(
            prepare_server(
                Ok(()),
                || Ok(Some((42, true))),
                |_| Ok(()),
                |_, _| panic!("unexpected recovery")
            ),
            Ok(Some(42))
        );
    }

    #[test]
    fn eof_before_reply_recovers_once_before_deploy_can_be_sent() {
        let events = RefCell::new(Vec::new());
        let mut attempts = 0;
        let connection = prepare_server(
            Ok(()),
            || {
                attempts += 1;
                events.borrow_mut().push("connect");
                Ok(Some((attempts, true)))
            },
            |connection| {
                events.borrow_mut().push("hello");
                if *connection == 1 {
                    Err(HelloFailure::read(io::ErrorKind::UnexpectedEof.into(), 0))
                } else {
                    Ok(())
                }
            },
            |connection, authenticated| {
                assert_eq!(connection, 1);
                assert!(!authenticated);
                events.borrow_mut().push("replace");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(connection, Some(2));
        events.borrow_mut().push("deploy");
        assert_eq!(
            *events.borrow(),
            ["connect", "hello", "replace", "connect", "hello", "deploy"]
        );
    }

    #[test]
    fn uncertain_deploy_write_or_response_never_retries_or_recovers() {
        use std::cell::Cell;
        for failure in ["send IME server deploy", "read IME server deploy"] {
            let connections = Cell::new(0);
            let deployments = Cell::new(0);
            let error = deploy_once(
                Ok(()),
                || {
                    connections.set(connections.get() + 1);
                    Ok(Some(((), true)))
                },
                |_| Ok(()),
                |_, _| panic!("Deploy must not trigger recovery"),
                |_| {
                    deployments.set(deployments.get() + 1);
                    Err(failure.into())
                },
            )
            .unwrap_err();
            assert_eq!(error, failure);
            assert_eq!(connections.get(), 1);
            assert_eq!(deployments.get(), 1);
        }
    }

    #[test]
    fn old_authenticated_server_uses_graceful_replacement() {
        let mut attempts = 0;
        assert_eq!(
            prepare_server(
                Ok(()),
                || {
                    attempts += 1;
                    Ok(Some((attempts, attempts == 2)))
                },
                |_| Ok(()),
                |_, authenticated| {
                    assert!(authenticated);
                    Ok(())
                },
            ),
            Ok(Some(2))
        );
    }

    #[test]
    fn repeated_rejection_keeps_both_reasons_without_a_second_recovery() {
        let mut replacements = 0;
        let error = prepare_server(
            Ok(()),
            || Ok(Some(((), false))),
            |_| {
                Err(HelloFailure::other(
                    "client authentication failed: access denied".into(),
                ))
            },
            |_, _| {
                replacements += 1;
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(replacements, 1);
        assert!(error.starts_with("client authentication failed: access denied;"));
        assert!(
            error.contains("handshake still failed: client authentication failed: access denied")
        );
    }

    #[test]
    fn unverified_peer_and_failed_shutdown_cannot_reach_a_retry() {
        let error = prepare_server::<()>(
            Ok(()),
            || Err("unverified server image".into()),
            |_| panic!("unexpected hello"),
            |_, _| panic!("unexpected termination"),
        )
        .unwrap_err();
        assert_eq!(error, "unverified server image");
        let mut connections = 0;
        let error = prepare_server(
            Ok(()),
            || {
                connections += 1;
                Ok(Some(((), false)))
            },
            |_| Ok(()),
            |_, authenticated| {
                assert!(authenticated);
                Err("shutdown timed out".into())
            },
        )
        .unwrap_err();
        assert_eq!(connections, 1);
        assert!(error.contains("shutdown timed out"));
    }

    #[test]
    fn management_requires_a_shell_launcher_only_for_elevated_apps() {
        assert!(can_manage_server(false, false));
        assert!(can_manage_server(false, true));
        assert!(!can_manage_server(true, false));
        assert!(can_manage_server(true, true));
    }

    #[test]
    fn unavailable_shell_launcher_cannot_replace_or_deploy() {
        let hint = "当前 KeyTao 以管理员权限运行，无法使用交互式桌面令牌启动输入法服务";
        for current in [false, true] {
            for failure in ["eof", "timeout", "HelloErr: access denied", "healthy"] {
                if current && failure == "healthy" {
                    continue;
                }
                let mut reason = String::new();
                let error = deploy_once(
                    Err(hint.into()),
                    || Ok(Some(((), current))),
                    |_| {
                        if failure == "healthy" {
                            reason = "outdated IME server".into();
                            return Ok(());
                        }
                        let error = if failure == "eof" {
                            HelloFailure::read(io::ErrorKind::UnexpectedEof.into(), 0)
                        } else {
                            HelloFailure::other(failure.into())
                        };
                        reason = error.reason.clone();
                        Err(error)
                    },
                    |_, _| panic!("unavailable shell launcher must not terminate or respawn"),
                    |_| panic!("unprepared server must not deploy"),
                )
                .unwrap_err();
                if current && failure != "eof" {
                    assert_eq!(error, reason);
                } else {
                    assert_eq!(error, format!("{reason}; {hint}"));
                }
            }
        }
    }

    #[test]
    fn elevated_app_with_shell_launcher_replaces_stale_or_silent_server() {
        assert!(can_manage_server(true, true));
        for (current, silent) in [(false, false), (false, true), (true, true)] {
            let mut connections = 0;
            let mut replacements = 0;
            assert_eq!(
                deploy_once(
                    Ok(()),
                    || {
                        connections += 1;
                        Ok(Some((connections, current || connections == 2)))
                    },
                    |connection| {
                        if silent && *connection == 1 {
                            Err(HelloFailure::read(io::ErrorKind::UnexpectedEof.into(), 0))
                        } else {
                            Ok(())
                        }
                    },
                    |connection, authenticated| {
                        assert_eq!(connection, 1);
                        assert_eq!(authenticated, !silent);
                        replacements += 1;
                        Ok(())
                    },
                    |connection| {
                        assert_eq!(connection, 2);
                        Ok(())
                    },
                ),
                Ok(true)
            );
            assert_eq!(connections, 2);
            assert_eq!(replacements, 1);
        }
    }

    #[test]
    fn current_authenticated_server_needs_no_management_capability() {
        assert_eq!(
            deploy_once(
                Err("shell launcher unavailable".into()),
                || Ok(Some((42, true))),
                |_| Ok(()),
                |_, _| panic!("current server must not be replaced"),
                |connection| {
                    assert_eq!(connection, 42);
                    Ok(())
                },
            ),
            Ok(true)
        );
    }

    #[test]
    fn no_server_fallback_remains_the_callers_decision() {
        assert_eq!(
            deploy_once::<()>(
                Err("shell launcher unavailable".into()),
                || Ok(None),
                |_| panic!("no server to authenticate"),
                |_, _| panic!("elevated App must not spawn"),
                |_| panic!("no server to deploy"),
            ),
            Ok(false)
        );
        assert_eq!(
            deploy_once::<()>(
                Err("shell launcher unavailable".into()),
                || Err("server pipe remains busy".into()),
                |_| panic!("no connection"),
                |_, _| panic!("elevated App must not spawn"),
                |_| panic!("no connection"),
            ),
            Err("server pipe remains busy".into())
        );
    }

    #[test]
    fn current_server_timeout_rejection_and_partial_reply_never_terminate() {
        for failure in [
            HelloFailure::read(io::ErrorKind::TimedOut.into(), 0),
            HelloFailure::other("HelloErr: client authentication failed: access denied".into()),
            HelloFailure::read(io::ErrorKind::UnexpectedEof.into(), 2),
            HelloFailure::other("invalid response".into()),
        ] {
            for management in [Ok(()), Err("当前 KeyTao 以管理员权限运行".into())] {
                let reason = failure.reason.clone();
                assert_eq!(
                    deploy_once(
                        management,
                        || Ok(Some(((), true))),
                        |_| {
                            Err(HelloFailure {
                                reason: failure.reason.clone(),
                                eof_before_reply: failure.eof_before_reply,
                            })
                        },
                        |_, _| panic!("current server must not be terminated"),
                        |_| panic!("failed handshake must not deploy"),
                    ),
                    Err(reason)
                );
            }
        }
    }

    #[test]
    fn stale_server_can_be_terminated_after_timeout_or_rejection() {
        for timeout in [false, true] {
            let mut attempts = 0;
            let mut hellos = 0;
            assert_eq!(
                prepare_server(
                    Ok(()),
                    || {
                        attempts += 1;
                        Ok(Some((attempts, attempts == 2)))
                    },
                    |_| {
                        hellos += 1;
                        if hellos == 1 {
                            Err(if timeout {
                                HelloFailure::read(io::ErrorKind::TimedOut.into(), 0)
                            } else {
                                HelloFailure::other("HelloErr: unsupported protocol".into())
                            })
                        } else {
                            Ok(())
                        }
                    },
                    |_, authenticated| {
                        assert!(!authenticated);
                        Ok(())
                    },
                ),
                Ok(Some(2))
            );
        }
    }

    #[test]
    fn only_eof_without_any_reply_bytes_is_silent() {
        for kind in [io::ErrorKind::UnexpectedEof, io::ErrorKind::BrokenPipe] {
            assert!(HelloFailure::read(kind.into(), 0).eof_before_reply);
            assert!(!HelloFailure::read(kind.into(), 1).eof_before_reply);
        }
        assert!(!HelloFailure::read(io::ErrorKind::TimedOut.into(), 0).eof_before_reply);
        assert!(!HelloFailure::other("HelloErr".into()).eof_before_reply);
    }
}
