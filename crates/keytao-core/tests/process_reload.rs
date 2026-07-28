//! librime is a process-global: one service, one session map, one config and
//! dictionary cache. These tests drive two independent `ImeRuntime` instances
//! at the same time, the case where a per-runtime reload barrier and session
//! registry would let one runtime finalize librime under the other's sessions.
//!
//! Needs the librime the crate links against at run time, e.g.
//! `DYLD_FALLBACK_LIBRARY_PATH="$RIME_LIB_DIR" cargo test -p keytao-core`.

#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{ImeRuntime, ImeRuntimeSession};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// There is one librime per process, so the tests in this binary take turns.
static LIBRIME: Mutex<()> = Mutex::new(());

fn serialized() -> MutexGuard<'static, ()> {
    LIBRIME.lock().unwrap_or_else(PoisonError::into_inner)
}

fn compose(session: &ImeRuntimeSession) -> Result<usize, String> {
    session.reset().ok_or("session has no engine")?;
    let mut state = None;
    for character in smoke_fixture::CODE.chars() {
        state = Some(
            session
                .process_key_result(character as u32, 0)
                .ok_or("session has no engine")?
                .state,
        );
    }
    let state = state.ok_or("fixture code is empty")?;
    session.reset().ok_or("session has no engine")?;
    Ok(state.candidates.len())
}

fn report(failures: &Mutex<Vec<String>>, message: String) {
    failures
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(message);
}

fn assert_composes(session: &ImeRuntimeSession, stage: &str) {
    match compose(session) {
        Ok(0) => panic!("{stage}: the session produced no candidates"),
        Ok(_) => {}
        Err(error) => panic!("{stage}: {error}"),
    }
}

#[test]
fn independent_runtimes_share_one_librime() {
    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("process-reload");
    smoke_fixture::write(&dir).expect("write the fixture");
    let shared = dir.to_string_lossy().into_owned();

    let first = ImeRuntime::with_dirs(&dir, shared.clone());
    first.init().expect("deploy the fixture");

    // A second runtime for the same directories joins the librime that is
    // already up instead of deploying a second time.
    let second = ImeRuntime::with_dirs(&dir, shared.clone());
    second
        .init_without_deploy()
        .expect("attach the second runtime");

    let first_session = first
        .create_session()
        .expect("session on the first runtime");
    let second_session = second
        .create_session()
        .expect("session on the second runtime");
    assert_composes(&first_session, "before any reload");
    assert_composes(&second_session, "before any reload");

    // Reloading through one runtime finalizes the librime the other runtime's
    // session is using, so that session must have been drained and rebuilt.
    first
        .reload_without_deploy()
        .expect("reload through the first runtime");
    assert_composes(&second_session, "after the first runtime reloaded");
    assert_composes(&first_session, "after the first runtime reloaded");

    second
        .reload_without_deploy()
        .expect("reload through the second runtime");
    assert_composes(&first_session, "after the second runtime reloaded");
    assert_composes(&second_session, "after the second runtime reloaded");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_reloads_from_two_runtimes_keep_sessions_alive() {
    let _serialized = serialized();
    let dir = smoke_fixture::scratch_dir("process-reload-race");
    smoke_fixture::write(&dir).expect("write the fixture");
    let shared = dir.to_string_lossy().into_owned();

    let first = ImeRuntime::with_dirs(&dir, shared.clone());
    first.init().expect("deploy the fixture");
    let second = ImeRuntime::with_dirs(&dir, shared.clone());
    second
        .init_without_deploy()
        .expect("attach the second runtime");

    let stop = Arc::new(AtomicBool::new(false));
    let failures: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let reloads = Arc::new(AtomicU64::new(0));
    let mut workers = Vec::new();

    let mut sessions = Vec::new();
    for (index, runtime) in [&first, &second].into_iter().enumerate() {
        let session = runtime
            .create_session()
            .expect("session for the typing thread");
        sessions.push(session.clone());
        let typing_stop = stop.clone();
        let typing_failures = failures.clone();
        let typing_reloads = reloads.clone();
        workers.push(std::thread::spawn(move || {
            while !typing_stop.load(Ordering::Relaxed) {
                // A reload legitimately throws the composition in flight away,
                // so only a run that no reload completed during has to produce
                // candidates.
                let before = typing_reloads.load(Ordering::SeqCst);
                match compose(&session) {
                    Ok(0) if typing_reloads.load(Ordering::SeqCst) == before => report(
                        &typing_failures,
                        format!("runtime {index}: no candidates without a concurrent reload"),
                    ),
                    Ok(_) => {}
                    Err(error) => report(&typing_failures, format!("runtime {index}: {error}")),
                }
            }
        }));

        let runtime = runtime.clone();
        let reload_stop = stop.clone();
        let reload_failures = failures.clone();
        let reloads = reloads.clone();
        workers.push(std::thread::spawn(move || {
            while !reload_stop.load(Ordering::Relaxed) {
                if let Err(error) = runtime.reload_without_deploy() {
                    report(&reload_failures, format!("runtime {index} reload: {error}"));
                    return;
                }
                reloads.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
            }
        }));
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline
        && failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Relaxed);
    for worker in workers {
        worker.join().expect("worker thread panicked");
    }

    let collected = failures.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(collected.is_empty(), "{collected:#?}");
    drop(collected);
    assert!(
        reloads.load(Ordering::SeqCst) >= 2,
        "the reload threads did not run"
    );

    // The sessions that were live through every one of those reloads still
    // belong to the librime that is up now.
    for (index, session) in sessions.iter().enumerate() {
        assert_composes(session, &format!("runtime {index} after the reload storm"));
    }

    let _ = std::fs::remove_dir_all(&dir);
}
