use super::*;
use serde_json::json;
use std::{fs, path::Path};

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: CoreEvent) {}
}

fn open_core(data_dir: &Path) -> Result<Arc<Core>, CoreError> {
    Core::new(
        AppEnv {
            data_dir: data_dir.to_owned(),
            cache_dir: data_dir.join("cache"),
            resource_dir: data_dir.join("resources"),
            app_version: "test".into(),
            platform: Platform::MacOs,
        },
        Arc::new(NullSink),
    )
}

#[test]
fn state_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path()).unwrap();
    let expected = AppState {
        auth_token: Some("test-token".into()),
        user: Some(json!({"id": 7, "name": "test"})),
        onboarding_completed: true,
        legacy_imported: true,
    };
    core.update_state(|state| *state = expected.clone())
        .unwrap();
    assert!(core.state() == expected);
    assert!(open_core(dir.path()).unwrap().state() == expected);
    let mut snapshot = core.state();
    snapshot.auth_token = None;
    assert!(core.state() == expected);
}

#[test]
fn missing_file_returns_default_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("missing");
    let core = open_core(&data_dir).unwrap();
    assert!(core.state() == AppState::default());
    assert!(core.recovered_state_file().is_none());
    assert!(!data_dir.exists());
}

#[test]
fn corrupt_file_returns_parse_error_and_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app-state.json");
    for bytes in [b"{invalid".as_slice(), b"{}", b"null"] {
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            AppState::load(dir.path()),
            Err(CoreError::Parse(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn core_recovers_corrupt_state_and_can_save_again() {
    for bytes in [b"{invalid".as_slice(), b"{}", b"null", b""] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app-state.json");
        fs::write(&path, bytes).unwrap();

        let core = open_core(dir.path()).unwrap();
        assert!(core.state() == AppState::default());
        assert!(!path.exists());
        let recovered = core.recovered_state_file().unwrap();
        assert_eq!(recovered.parent(), Some(dir.path()));
        assert!(
            recovered
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .strip_prefix("app-state.json.corrupt-")
                .unwrap()
                .parse::<u128>()
                .is_ok()
        );
        assert_eq!(fs::read(&recovered).unwrap(), bytes);

        core.update_state(|state| state.onboarding_completed = true)
            .unwrap();
        let saved: AppState = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(saved == core.state());
        assert!(saved.onboarding_completed);
        let reopened = open_core(dir.path()).unwrap();
        assert!(reopened.state() == saved);
        assert!(reopened.recovered_state_file().is_none());
        assert_eq!(fs::read(&recovered).unwrap(), bytes);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}

#[test]
fn core_load_io_error_is_not_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app-state.json");
    fs::create_dir(&path).unwrap();
    assert!(matches!(open_core(dir.path()), Err(CoreError::Io(_))));
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn atomic_replacement_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path()).unwrap();
    for value in [true, false, true] {
        core.update_state(|state| state.onboarding_completed = value)
            .unwrap();
        let paths: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(paths, ["app-state.json"]);
        assert_eq!(
            open_core(dir.path()).unwrap().state().onboarding_completed,
            value
        );
    }
}

#[test]
fn legacy_import_is_idempotent_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path()).unwrap();
    core.import_legacy_state(Some("legacy-token".into()), Some(json!({"id": 7})), true)
        .unwrap();
    assert!(core.state().legacy_imported);
    assert_eq!(core.state().auth_token.as_deref(), Some("legacy-token"));
    assert_eq!(core.state().user, Some(json!({"id": 7})));
    assert!(core.state().onboarding_completed);
    // Later core updates must never be overwritten by a repeated legacy import.
    core.update_state(|state| state.auth_token = Some("new-token".into()))
        .unwrap();
    let persisted = fs::read(dir.path().join("app-state.json")).unwrap();
    let reopened = open_core(dir.path()).unwrap();
    reopened.import_legacy_state(None, None, false).unwrap();
    assert!(reopened.state() == core.state());
    assert_eq!(
        fs::read(dir.path().join("app-state.json")).unwrap(),
        persisted
    );
}

#[test]
fn failed_write_preserves_snapshot_and_cleans_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path()).unwrap();
    let path = dir.path().join("app-state.json");
    // A directory at the destination forces rename to fail after writing/fsync.
    fs::create_dir(&path).unwrap();
    assert!(matches!(
        core.import_legacy_state(Some("test-token".into()), None, true),
        Err(CoreError::Io(_))
    ));
    assert!(core.state() == AppState::default());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    fs::remove_dir(&path).unwrap();
    core.import_legacy_state(None, None, true).unwrap();
    assert!(open_core(dir.path()).unwrap().state().legacy_imported);
}

#[test]
fn concurrent_updates_do_not_lose_changes() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path()).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..4 {
                    core.update_state(|state| {
                        let count = state
                            .user
                            .as_ref()
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0);
                        state.user = Some(json!(count + 1));
                    })
                    .unwrap();
                }
            });
        }
    });
    assert_eq!(core.state().user, Some(json!(32)));
    assert!(open_core(dir.path()).unwrap().state() == core.state());
}

#[test]
fn panicking_update_does_not_publish_partial_state() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path()).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = core.update_state(|state| {
            state.auth_token = Some("partial-token".into());
            panic!("interrupted update");
        });
    }));
    assert!(result.is_err());
    assert!(core.state() == AppState::default());
    assert!(!dir.path().join("app-state.json").exists());
    core.update_state(|state| state.onboarding_completed = true)
        .unwrap();
    assert!(open_core(dir.path()).unwrap().state() == core.state());
}

#[cfg(unix)]
#[test]
fn persisted_credentials_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path()).unwrap();
    core.import_legacy_state(Some("test-token".into()), None, false)
        .unwrap();
    let permissions = fs::metadata(dir.path().join("app-state.json"))
        .unwrap()
        .permissions();
    assert_eq!(permissions.mode() & 0o777, 0o600);
}
