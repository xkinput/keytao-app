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
mod authorization;

#[cfg(any(windows, test))]
mod handshake;

#[cfg(any(windows, test))]
mod pipe_io;

#[cfg(any(windows, test))]
fn is_staged_libjd_dictionary(root: &std::path::Path, path: &std::path::Path) -> bool {
    let Ok(relative) = path.strip_prefix(root.join("dictionaries")) else {
        return false;
    };
    let mut parts = relative.iter();
    let valid_key = parts
        .next()
        .and_then(|key| key.to_str())
        .is_some_and(|key| {
            !key.is_empty()
                && key.len() <= 128
                && key.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
                })
        });
    let valid_stage = parts
        .next()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".libjd-"))
        .is_some_and(|suffix| !suffix.is_empty());
    valid_key && valid_stage && parts.eq([std::ffi::OsStr::new("dictionary.txt")])
}

#[cfg(windows)]
fn main() {
    use keytao_app_core::ime_status::windows::LIBJD_CHECK_SUBCOMMAND;
    use keytao_core::deploy_child::DEPLOY_SUBCOMMAND;

    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == LIBJD_CHECK_SUBCOMMAND) {
        let result = (|| -> Result<(), String> {
            let path = std::path::PathBuf::from(args.next().ok_or("missing ime-jd dictionary path")?);
            if args.next().as_deref() != Some(std::ffi::OsStr::new("--build-dictionary"))
                || args.next().as_deref() != Some(std::ffi::OsStr::new(DEPLOY_SUBCOMMAND))
                || args.next().is_some()
                || !path.is_absolute()
                || !std::fs::symlink_metadata(&path).is_ok_and(|entry| entry.is_file())
            {
                return Err("invalid ime-jd dictionary build arguments".into());
            }
            let data = std::env::var_os("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or("missing LOCALAPPDATA")?;
            let root = keytao_core::libjd::engine_root(&data.join("ink.rea.keytao-app"))
                .and_then(|root| root.canonicalize().ok())
                .ok_or("invalid ime-jd installation root")?;
            let path = path
                .canonicalize()
                .map_err(|error| format!("resolve ime-jd dictionary source: {error}"))?;
            if !is_staged_libjd_dictionary(&root, &path) {
                return Err("invalid ime-jd staging dictionary".into());
            }
            use std::io::Read;
            const MAX_TEXT: u64 = keytao_core::libjd::MAX_DICTIONARY_TEXT_BYTES as u64;
            let mut text = Vec::new();
            std::fs::File::open(&path)
                .and_then(|file| file.take(MAX_TEXT + 1).read_to_end(&mut text))
                .map_err(|error| error.to_string())?;
            if text.len() as u64 > MAX_TEXT {
                return Err("ime-jd dictionary source is too large".into());
            }
            let engine = keytao_core::libjd::bundled_engine_library()
                .ok_or("bundled ime-jd engine is missing")?;
            unsafe {
                keytao_core::jd_backend::JdBackend::build_dictionary(
                    &engine,
                    &text,
                    &path.with_file_name("dictionary.bin"),
                )
            }
        })();
        let ok = result.is_ok();
        println!("{}", serde_json::json!({"ok": ok, "error": result.err()}));
        keytao_core::deploy_child::exit_deploy_child(i32::from(!ok));
    }
    if let Some(subcommand) = std::env::args_os().skip(1).find(|arg| {
        arg == "--deploy"
            || arg == keytao_core::deploy_child::RELOAD_SUBCOMMAND
            || arg == "--deploy-public"
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
            } else {
                keytao_core::deploy_child::run_deploy_child_main_with_prepare(|root| {
                    keytao_app_core::package_migration::migrate_default_if_needed(
                        root,
                        std::path::Path::new(&keytao_core::default_shared_data_dir()),
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
                })
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
    fn libjd_build_accepts_only_staged_dictionary_text() {
        use std::path::Path;

        let root = Path::new("engines/libjd");
        let staged = "dictionaries/xmjd-feiding/.libjd-123456/dictionary.txt";
        assert!(super::is_staged_libjd_dictionary(root, &root.join(staged)));
        assert!(!super::is_staged_libjd_dictionary(root, Path::new(staged)));
        for invalid in [
            "latest/.libjd-123456/libjd.dll",
            "dictionaries/xmjd/.libjd-123456/libjd.dll",
            "dictionaries/xmjd/.libjd-/dictionary.txt",
            "dictionaries/xmjd/.libjd-123456/nested/dictionary.txt",
            "dictionaries/../.libjd-123456/dictionary.txt",
        ] {
            assert!(!super::is_staged_libjd_dictionary(root, &root.join(invalid)));
        }
    }

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
