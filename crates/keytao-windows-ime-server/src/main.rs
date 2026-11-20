//! The non-Windows entry point intentionally has no native engine dependencies.

// A GUI-subsystem exe never gets a console, however it is launched.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
use keytao_ime_ipc::engine_config;

#[cfg(windows)]
mod server;

#[cfg(any(windows, test))]
mod runtime_state;

#[cfg(any(windows, test))]
mod staged_deploy;

#[cfg(any(windows, test))]
mod authorization;

#[cfg(any(windows, test))]
mod handshake;

#[cfg(any(windows, test))]
mod pipe_io;

#[cfg(windows)]
fn main() {
    if let Some(subcommand) = std::env::args_os().skip(1).find(|arg| {
        arg == "--deploy"
            || arg == "--deploy-public"
            || arg == staged_deploy::SUBCOMMAND
            || arg == keytao_core::deploy_child::PREVIEW_SUBCOMMAND
    }) {
        let code = keytao_windows_ime::engine_backend::InProcEngine::deploy_server(|| {
            let code = if subcommand == keytao_core::deploy_child::PREVIEW_SUBCOMMAND {
                keytao_core::deploy_child::run_preview_child_main()
            } else if subcommand == "--deploy-public" {
                keytao_core::deploy_child::run_deploy_child_main_with_action(
                    "--deploy-public",
                    keytao_app_core::windows_public_schemas::deploy_and_publish,
                )
            } else if subcommand == staged_deploy::SUBCOMMAND {
                keytao_core::deploy_child::run_deploy_child_main_with_action(
                    staged_deploy::SUBCOMMAND,
                    || {
                        let root = keytao_core::default_user_data_dir()
                            .ok_or("cannot determine Rime directory")?;
                        let stage = staged_deploy::directory_from_environment(&root)?;
                        keytao_core::deploy_child::deploy_directory(&stage)
                    },
                )
            } else {
                keytao_core::deploy_child::run_deploy_child_main()
            };
            // Exit before deploy_server drops and unloads the librime DLL.
            keytao_core::deploy_child::exit_deploy_child(code)
        })
        .unwrap_or_else(|error| {
            eprintln!("KeyTao deploy child: {error}");
            println!("{}", serde_json::json!({"ok": false, "error": error}));
            1
        });
        keytao_core::deploy_child::exit_deploy_child(code);
    }
    if let Err(error) = server::run() {
        eprintln!("KeyTao IME server: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}

#[cfg(test)]
#[path = "../../keytao-windows-ime/src/engine_status.rs"]
mod engine_status;

#[cfg(test)]
mod tests {
    #[test]
    fn process_keys_keeps_press_and_release_commits() {
        struct FakeEngine {
            calls: Vec<(u32, u32)>,
            results: std::collections::VecDeque<keytao_core::KeyProcessResult>,
        }

        impl FakeEngine {
            fn process_key(
                &mut self,
                keysym: u32,
                mods: u32,
            ) -> Option<keytao_core::KeyProcessResult> {
                self.calls.push((keysym, mods));
                self.results.pop_front()
            }
        }

        let mut press = keytao_core::ImeState::empty();
        press.committed = Some("press".into());
        press.preedit = "after-press".into();
        let mut release = keytao_core::ImeState::empty();
        release.committed = Some("release".into());
        release.preedit = "after-release".into();
        release.ascii_mode = true;
        let mut engine = FakeEngine {
            calls: Vec::new(),
            results: [
                keytao_core::KeyProcessResult {
                    state: press,
                    accepted: true,
                },
                keytao_core::KeyProcessResult {
                    state: release,
                    accepted: false,
                },
            ]
            .into(),
        };
        let keys = [(0xffe1, 0), (0xffe1, 1 << 30)];
        let result = keytao_core::key_batch::process_keys(&keys, |keysym, mods| {
            engine.process_key(keysym, mods).unwrap()
        })
        .unwrap();

        assert_eq!(engine.calls, keys);
        assert_eq!(result.state.committed.as_deref(), Some("pressrelease"));
        assert!(result.accepted);
        assert_eq!(result.state.preedit, "after-release");
        assert!(result.state.ascii_mode);
    }

    #[test]
    fn status_writes_do_not_request_refresh() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-status-refresh-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        keytao_core::ReloadStamp::write(&dir).unwrap();
        let mut watch = keytao_core::ReloadStampGate::new(&dir);
        for schema in ["keytao", "pinyin_simp"] {
            crate::engine_status::write_status(&dir, None, Some(schema)).unwrap();
            assert!(watch.pending().is_none());
        }
        keytao_core::ReloadStamp::write(&dir).unwrap();
        let pending = watch.pending().unwrap();
        crate::engine_status::write_status(&dir, None, Some("keytao")).unwrap();
        assert_eq!(watch.pending(), Some(pending.clone()));
        watch.commit(pending);
        assert!(watch.pending().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
