use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub const USER_DATA_SYNC_REQUEST_FILE: &str = ".keytao-user-data-sync.json";
pub const USER_DATA_SYNC_RESULT_FILE: &str = ".keytao-user-data-sync.result.json";
const APPLIED_FILE: &str = ".keytao-user-data-sync.applied";
const REQUEST_LIFETIME_MS: u64 = 30_000;

fn lock_request(root: &Path) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".keytao-user-data-sync.lock"))
        .map_err(|error| format!("open sync request lock: {error}"))?;
    file.lock()
        .map_err(|error| format!("lock sync request: {error}"))?;
    Ok(file)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct UserDataSyncRequest {
    pub generation: u64,
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDataSyncResult {
    pub generation: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub finished_at: u64,
    #[serde(default)]
    pub process_id: u32,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("read {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn save_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    crate::write_file_atomic(path, &bytes)
        .map_err(|error| format!("save {}: {error}", path.display()))
}

fn applied_generation(root: &Path) -> Result<u64, String> {
    match std::fs::read_to_string(root.join(APPLIED_FILE)) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|error| format!("read sync generation: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(format!("read sync generation: {error}")),
    }
}

pub fn request_user_data_sync(root: &Path) -> Result<u64, String> {
    let _guard = lock_request(root)?;
    let previous: UserDataSyncRequest =
        read_json(&root.join(USER_DATA_SYNC_REQUEST_FILE))?.unwrap_or_default();
    let generation = previous
        .generation
        .max(applied_generation(root)?)
        .checked_add(1)
        .ok_or("sync generation exhausted")?;
    save_json(
        &root.join(USER_DATA_SYNC_REQUEST_FILE),
        &UserDataSyncRequest {
            generation,
            expires_at: now_ms().saturating_add(REQUEST_LIFETIME_MS),
        },
    )?;
    Ok(generation)
}

pub fn cancel_user_data_sync(root: &Path, generation: u64) -> Result<bool, String> {
    let _guard = lock_request(root)?;
    if read_user_data_sync_result(root)?.is_some_and(|result| result.generation == generation) {
        return Ok(false);
    }
    if let Some(mut request) =
        read_json::<UserDataSyncRequest>(&root.join(USER_DATA_SYNC_REQUEST_FILE))?
    {
        if request.generation == generation {
            request.expires_at = 0;
            save_json(&root.join(USER_DATA_SYNC_REQUEST_FILE), &request)?;
        }
    }
    Ok(true)
}

pub fn read_user_data_sync_result(root: &Path) -> Result<Option<UserDataSyncResult>, String> {
    read_json(&root.join(USER_DATA_SYNC_RESULT_FILE))
}

#[cfg(test)]
fn pending(root: &Path) -> Result<bool, String> {
    let Some(request) = read_json::<UserDataSyncRequest>(&root.join(USER_DATA_SYNC_REQUEST_FILE))?
    else {
        return Ok(false);
    };
    Ok(request.generation > applied_generation(root)? && request.expires_at > now_ms())
}

pub(crate) fn claim(root: &Path) -> Result<Option<u64>, String> {
    let _guard = lock_request(root)?;
    let Some(request) = read_json::<UserDataSyncRequest>(&root.join(USER_DATA_SYNC_REQUEST_FILE))?
    else {
        return Ok(None);
    };
    if request.generation <= applied_generation(root)? || request.expires_at <= now_ms() {
        return Ok(None);
    }
    // Claim before execution so a crash or failed result write cannot replay a sync.
    crate::write_file_atomic(
        &root.join(APPLIED_FILE),
        request.generation.to_string().as_bytes(),
    )
    .map_err(|error| format!("claim sync request: {error}"))?;
    save_json(
        &root.join(USER_DATA_SYNC_RESULT_FILE),
        &UserDataSyncResult {
            generation: request.generation,
            ok: false,
            error: None,
            finished_at: 0,
            process_id: std::process::id(),
        },
    )?;
    Ok(Some(request.generation))
}

pub(crate) fn finish(
    root: &Path,
    generation: u64,
    outcome: Result<(), String>,
) -> Result<(), String> {
    save_json(
        &root.join(USER_DATA_SYNC_RESULT_FILE),
        &UserDataSyncResult {
            generation,
            ok: outcome.is_ok(),
            error: outcome.err(),
            finished_at: now_ms(),
            process_id: std::process::id(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_requests_get_distinct_generations() {
        let root =
            std::env::temp_dir().join(format!("keytao-sync-concurrent-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let barrier = std::sync::Barrier::new(8);
        let mut generations = std::thread::scope(|scope| {
            let requests = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        request_user_data_sync(&root).unwrap()
                    })
                })
                .collect::<Vec<_>>();
            requests
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        generations.sort_unstable();
        assert_eq!(generations, (1..=8).collect::<Vec<_>>());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generations_are_claimed_once_and_cancelled_or_expired_requests_do_not_replay() {
        let root =
            std::env::temp_dir().join(format!("keytao-sync-generation-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = request_user_data_sync(&root).unwrap();
        assert_eq!(claim(&root).unwrap(), Some(first));
        assert_eq!(claim(&root).unwrap(), None);
        let started = read_user_data_sync_result(&root).unwrap().unwrap();
        assert_eq!(started.generation, first);
        assert_eq!(started.finished_at, 0);
        assert_eq!(started.process_id, std::process::id());
        assert!(!started.ok);
        assert_eq!(started.error, None);
        assert!(!cancel_user_data_sync(&root, first).unwrap());
        let request: UserDataSyncRequest = read_json(&root.join(USER_DATA_SYNC_REQUEST_FILE))
            .unwrap()
            .unwrap();
        assert!(request.expires_at > now_ms());
        finish(&root, first, Err("sync failed".into())).unwrap();
        let result = read_user_data_sync_result(&root).unwrap().unwrap();
        assert_eq!(result.generation, first);
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("sync failed"));
        assert!(result.finished_at > 0);
        assert_eq!(result.process_id, started.process_id);
        let second = request_user_data_sync(&root).unwrap();
        assert!(second > first);
        assert!(!cancel_user_data_sync(&root, first).unwrap());
        assert!(pending(&root).unwrap());
        assert!(cancel_user_data_sync(&root, second).unwrap());
        assert_eq!(claim(&root).unwrap(), None);
        save_json(
            &root.join(USER_DATA_SYNC_REQUEST_FILE),
            &UserDataSyncRequest {
                generation: second + 1,
                expires_at: now_ms() - 1,
            },
        )
        .unwrap();
        assert_eq!(claim(&root).unwrap(), None);
        let third = request_user_data_sync(&root).unwrap();
        assert!(third > second + 1);
        assert_eq!(claim(&root).unwrap(), Some(third));
        assert_eq!(claim(&root).unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    #[test]
    fn real_librime_sync_uses_updated_installation_and_keeps_the_ime_working() {
        const ROOT_ENV: &str = "KEYTAO_REAL_SYNC_TEST_ROOT";
        if let Some(root) = std::env::var_os(ROOT_ENV) {
            let root = std::path::PathBuf::from(root);
            let user = root.join("user");
            let shared = root.join("shared");
            let sync_dir = root.join("selected-sync");
            std::fs::create_dir_all(&user).unwrap();
            std::fs::create_dir_all(&shared).unwrap();
            std::fs::write(
                shared.join("default.yaml"),
                "config_version: '1.0'\nschema_list:\n  - schema: keytao\n",
            )
            .unwrap();
            std::fs::write(
                user.join("default.custom.yaml"),
                "patch:\n  schema_list:\n    - schema: keytao\n",
            )
            .unwrap();
            std::fs::write(user.join("keytao.schema.yaml"), "schema:\n  schema_id: keytao\n  name: Sync test\n  version: '1.0'\nengine:\n  processors: [speller, selector, express_editor]\n  segmentors: [abc_segmentor]\n  translators: [table_translator]\ntranslator:\n  dictionary: sync_test\n  enable_user_dict: true\nspeller:\n  alphabet: abcdefghijklmnopqrstuvwxyz\n").unwrap();
            std::fs::write(
                user.join("sync_test.dict.yaml"),
                "---\nname: sync_test\nversion: '1.0'\nsort: by_weight\n...\nhello\ta\t1\n",
            )
            .unwrap();
            let runtime =
                crate::ImeRuntime::with_dirs(&user, shared.to_string_lossy()).with_ime_host();
            runtime.init().unwrap();
            let session = runtime.create_session().unwrap();
            assert!(session.process_key_result(b'a' as u32, 0).unwrap().accepted);
            assert_eq!(session.state().candidates[0].text, "hello");
            assert_eq!(
                session
                    .process_key_result(b' ' as u32, 0)
                    .unwrap()
                    .state
                    .committed
                    .as_deref(),
                Some("hello")
            );
            let build_signature = std::fs::read(user.join("build/.keytao-deployed")).unwrap();
            let mut installation: serde_yaml::Value =
                serde_yaml::from_slice(&std::fs::read(user.join("installation.yaml")).unwrap())
                    .unwrap();
            let old_id = installation["installation_id"].as_str().unwrap().to_owned();
            installation["installation_id"] = "scratch-device".into();
            installation["sync_dir"] = sync_dir.to_string_lossy().into_owned().into();
            std::fs::write(
                user.join("installation.yaml"),
                serde_yaml::to_string(&installation).unwrap(),
            )
            .unwrap();
            // Keep an excluded schema broken: a sync must never compile it.
            std::fs::write(user.join("skipped.schema.yaml"), "invalid: [").unwrap();
            let skipped_marker = br#"{"skipped":"fixture skipped schema"}"#;
            std::fs::write(
                user.join("build/.keytao-skipped-schemas.json"),
                skipped_marker,
            )
            .unwrap();
            let generation = request_user_data_sync(&user).unwrap();
            crate::ReloadStamp::write(&user).unwrap();
            // An app preview may reload, but it must not claim an IME request.
            let preview = crate::ImeRuntime::with_dirs(&user, shared.to_string_lossy());
            preview.reload_without_deploy().unwrap();
            preview.prewarm(Some(&session)).unwrap();
            assert!(read_user_data_sync_result(&user).unwrap().is_none());
            assert!(pending(&user).unwrap());
            // Reopen the table translator's live userdb before the IME owns the sync.
            assert!(session.process_key_result(b'a' as u32, 0).unwrap().accepted);
            assert_eq!(session.state().candidates[0].text, "hello");
            runtime.reload_without_deploy().unwrap();
            runtime.prewarm(Some(&session)).unwrap();
            let result = read_user_data_sync_result(&user).unwrap().unwrap();
            assert_eq!(result.generation, generation);
            assert!(result.ok, "{:?}", result.error);
            let snapshot = sync_dir.join("scratch-device/sync_test.userdb.txt");
            assert!(
                snapshot.is_file(),
                "snapshot missing at {}",
                snapshot.display()
            );
            assert!(std::fs::read_to_string(&snapshot)
                .unwrap()
                .contains("hello"));
            assert!(!sync_dir.join(old_id).exists());
            assert_eq!(
                std::fs::read(user.join("build/.keytao-deployed")).unwrap(),
                build_signature
            );
            assert!(!user.join("build/skipped.schema.yaml").exists());
            assert_eq!(
                std::fs::read(user.join("build/.keytao-skipped-schemas.json")).unwrap(),
                skipped_marker
            );
            let after: serde_yaml::Value =
                serde_yaml::from_slice(&std::fs::read(user.join("installation.yaml")).unwrap())
                    .unwrap();
            assert_eq!(after["installation_id"], installation["installation_id"]);
            assert_eq!(after["sync_dir"], installation["sync_dir"]);
            assert!(session.process_key_result(b'a' as u32, 0).is_some());
            assert!(!session.state().preedit.is_empty());
            assert_eq!(session.candidate_is_user_phrase_on_page(0), Some(true));
            assert!(!pending(&user).unwrap());
            let peer = sync_dir.join("peer");
            std::fs::create_dir_all(&peer).unwrap();
            let peer_snapshot = std::fs::read_to_string(&snapshot)
                .unwrap()
                .replace("scratch-device", "peer")
                .replace("a \thello", "b \tworld");
            assert!(peer_snapshot.contains("b \tworld"));
            std::fs::write(peer.join("sync_test.userdb.txt"), peer_snapshot).unwrap();
            let remote_generation = request_user_data_sync(&user).unwrap();
            crate::ReloadStamp::write(&user).unwrap();
            runtime.reload_without_deploy().unwrap();
            runtime.prewarm(Some(&session)).unwrap();
            let result = read_user_data_sync_result(&user).unwrap().unwrap();
            assert_eq!(result.generation, remote_generation);
            assert!(result.ok, "{:?}", result.error);
            assert!(session.process_key_result(b'b' as u32, 0).unwrap().accepted);
            assert_eq!(session.state().candidates[0].text, "world");
            assert_eq!(session.candidate_is_user_phrase_on_page(0), Some(true));
            assert!(session.delete_candidate_on_page_result(0).unwrap().1);
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "keytao-real-sync-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "user_data_sync::tests::real_librime_sync_uses_updated_installation_and_keeps_the_ime_working", "--nocapture"])
            .env(ROOT_ENV, &root)
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
