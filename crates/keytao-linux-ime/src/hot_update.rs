use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BinarySnapshot {
    device: u64,
    inode: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

fn binary_changed(started: BinarySnapshot, current: BinarySnapshot) -> bool {
    started != current
}

fn executable_path(path: &Path) -> (PathBuf, bool) {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let bytes = path.as_os_str().as_bytes();
    match bytes.strip_suffix(b" (deleted)") {
        Some(original) => (std::ffi::OsString::from_vec(original.to_vec()).into(), true),
        None => (path.to_path_buf(), false),
    }
}

#[cfg(target_os = "linux")]
mod runtime {
    use super::{binary_changed, executable_path, BinarySnapshot};
    use keytao_core::ImeRuntimeSession;
    use std::{
        ffi::OsString,
        os::{linux::fs::MetadataExt, unix::process::CommandExt},
        path::PathBuf,
        sync::{Arc, Mutex, OnceLock, Weak},
        time::Duration,
    };

    #[derive(Default)]
    struct State {
        active: usize,
        sessions: Vec<Weak<ImeRuntimeSession>>,
    }

    static STATE: OnceLock<Mutex<State>> = OnceLock::new();

    fn state() -> &'static Mutex<State> {
        STATE.get_or_init(|| Mutex::new(State::default()))
    }

    pub struct Activity;

    // Keep the event active until zbus has sent its boolean reply.
    pub struct KeyReply {
        accepted: bool,
        _activity: Activity,
    }

    impl KeyReply {
        pub fn new(accepted: bool, activity: Activity) -> Self {
            Self {
                accepted,
                _activity: activity,
            }
        }
    }

    impl zbus::zvariant::Type for KeyReply {
        fn signature() -> zbus::zvariant::Signature<'static> {
            <bool as zbus::zvariant::Type>::signature()
        }
    }

    impl zbus::export::serde::Serialize for KeyReply {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: zbus::export::serde::Serializer,
        {
            serializer.serialize_bool(self.accepted)
        }
    }

    pub fn activity() -> Activity {
        state().lock().unwrap().active += 1;
        Activity
    }

    impl Drop for Activity {
        fn drop(&mut self) {
            state().lock().unwrap().active -= 1;
        }
    }

    pub fn register_session(session: &Arc<ImeRuntimeSession>) {
        let mut state = state().lock().unwrap();
        state.sessions.retain(|session| session.strong_count() != 0);
        state.sessions.push(Arc::downgrade(session));
    }

    fn snapshot(metadata: std::fs::Metadata) -> BinarySnapshot {
        BinarySnapshot {
            device: metadata.st_dev(),
            inode: metadata.st_ino(),
            modified_seconds: metadata.st_mtime(),
            modified_nanoseconds: metadata.st_mtime_nsec(),
        }
    }

    pub fn start(kwin_socket: bool) {
        let executable = match std::fs::read_link("/proc/self/exe") {
            Ok(path) => executable_path(&path).0,
            Err(error) => {
                tracing::warn!("cannot record executable for hot update: {error}");
                return;
            }
        };
        let started = match std::fs::metadata("/proc/self/exe") {
            Ok(metadata) => snapshot(metadata),
            Err(error) => {
                tracing::warn!("cannot stat running executable for hot update: {error}");
                return;
            }
        };
        let args: Vec<OsString> = std::env::args_os().collect();
        if let Err(error) = std::thread::Builder::new()
            .name("hot-update".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                restart_if_idle(&executable, started, &args, kwin_socket);
            })
        {
            tracing::warn!("cannot start executable update guard: {error}");
        }
    }

    fn restart_if_idle(
        executable: &PathBuf,
        started: BinarySnapshot,
        args: &[OsString],
        kwin_socket: bool,
    ) {
        let Ok(metadata) = std::fs::metadata(executable) else {
            return;
        };
        let deleted = std::fs::read_link("/proc/self/exe")
            .map(|path| executable_path(&path).1)
            .unwrap_or(false);
        let current = snapshot(metadata);
        if !deleted && !binary_changed(started, current) {
            return;
        }

        // Probe outside the event gate so a failed replacement cannot block typing.
        match std::process::Command::new(executable)
            .arg("--version")
            .output()
        {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                tracing::warn!(
                    "updated input method probe failed ({}); keeping current process",
                    output.status
                );
                return;
            }
            Err(error) => {
                tracing::warn!(
                    "cannot execute updated input method; keeping current process: {error}"
                );
                return;
            }
        }
        if std::fs::metadata(executable).map(snapshot).ok() != Some(current) {
            return;
        }

        // Exclude new events until exec, including async commit publication.
        let mut state = state().lock().unwrap();
        if state.active != 0 {
            return;
        }
        state.sessions.retain(|session| session.strong_count() != 0);
        if state
            .sessions
            .iter()
            .filter_map(Weak::upgrade)
            .any(|session| {
                match session.raw_input() {
                    Some(input) if input.is_empty() => {}
                    _ => return true,
                }
                let state = session.state();
                !state.preedit.is_empty() || !state.candidates.is_empty()
            })
        {
            return;
        }

        tracing::info!("executable replaced; restarting {}", executable.display());
        if kwin_socket {
            // KWin must supply a fresh socket; exec cannot reuse this connection.
            crate::wait_for_backend_threads(true);
        }
        let mut command = std::process::Command::new(executable);
        if let Some(arg0) = args.first() {
            command.arg0(arg0);
        }
        let error = command.args(args.iter().skip(1)).exec();
        tracing::warn!("cannot exec updated input method; keeping current process: {error}");
    }

    #[cfg(test)]
    mod tests {
        use super::{activity, KeyReply};
        use zbus::zvariant::{serialized::Context, to_bytes, Type, LE};

        #[test]
        fn key_replies_keep_the_boolean_wire_format() {
            assert_eq!(KeyReply::signature(), bool::signature());
            for accepted in [false, true] {
                let reply = KeyReply::new(accepted, activity());
                let context = Context::new_dbus(LE, 0);
                assert_eq!(
                    to_bytes(context, &reply).unwrap().bytes(),
                    to_bytes(context, &accepted).unwrap().bytes()
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use runtime::{activity, register_session, start, KeyReply};

#[cfg(test)]
mod tests {
    use super::{binary_changed, executable_path, BinarySnapshot};
    use std::path::Path;

    const STARTED: BinarySnapshot = BinarySnapshot {
        device: 1,
        inode: 2,
        modified_seconds: 3,
        modified_nanoseconds: 4,
    };

    #[test]
    fn an_unchanged_binary_does_not_restart() {
        assert!(!binary_changed(STARTED, STARTED));
    }

    #[test]
    fn a_replaced_inode_restarts_even_with_the_same_timestamp() {
        assert!(binary_changed(
            STARTED,
            BinarySnapshot {
                inode: 5,
                ..STARTED
            }
        ));
    }

    #[test]
    fn a_different_device_restarts_even_with_the_same_inode() {
        assert!(binary_changed(
            STARTED,
            BinarySnapshot {
                device: 5,
                ..STARTED
            }
        ));
    }

    #[test]
    fn an_in_place_timestamp_change_restarts() {
        assert!(binary_changed(
            STARTED,
            BinarySnapshot {
                modified_seconds: 5,
                ..STARTED
            }
        ));
        assert!(binary_changed(
            STARTED,
            BinarySnapshot {
                modified_nanoseconds: 5,
                ..STARTED
            }
        ));
    }

    #[test]
    fn a_deleted_executable_uses_its_original_path() {
        assert_eq!(
            executable_path(Path::new("/usr/lib/KeyTao/keytao-ime (deleted)")),
            (Path::new("/usr/lib/KeyTao/keytao-ime").to_path_buf(), true)
        );
    }

    #[test]
    fn an_ordinary_executable_keeps_its_path() {
        let path = Path::new("/usr/lib/KeyTao/keytao-ime");
        assert_eq!(executable_path(path), (path.to_path_buf(), false));
    }

    #[test]
    fn only_a_trailing_deleted_marker_is_removed() {
        let path = Path::new("/tmp/ (deleted)/keytao-ime");
        assert_eq!(executable_path(path), (path.to_path_buf(), false));
    }

    #[test]
    fn non_utf8_paths_survive_deleted_marker_removal() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let path = std::ffi::OsString::from_vec(b"/tmp/keytao-\xff (deleted)".to_vec());
        let (original, deleted) = executable_path(Path::new(&path));
        assert!(deleted);
        assert_eq!(original.as_os_str().as_bytes(), b"/tmp/keytao-\xff");
    }
}
