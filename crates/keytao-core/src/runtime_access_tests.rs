//! Exercise reloads and contention with isolated native fixtures and runtime locks.

use super::*;
use std::sync::mpsc;

#[path = "../tests/support/smoke_fixture.rs"]
mod smoke_fixture;

thread_local! {
    static ENGINE_CREATION_GATE: std::cell::RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn pause_engine_creation() {
    ENGINE_CREATION_GATE.with(|gate| {
        if let Some((ready, resume)) = gate.take() {
            ready.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    });
}

fn native_fixture(name: &str) -> Option<(ImeRuntime, PathBuf)> {
    const CASE: &str = "KEYTAO_RUNTIME_RELOAD_TEST";
    if std::env::var(CASE).as_deref() != Ok(name) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("runtime_access_tests::{name}"),
                "--nocapture",
            ])
            .env(CASE, name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned()).with_ime_host();
    runtime.init().unwrap();
    Some((runtime, dir))
}

#[test]
fn plain_reload_rebuilds_only_the_next_used_session() {
    let Some((runtime, dir)) = native_fixture("plain_reload_rebuilds_only_the_next_used_session")
    else {
        return;
    };
    let sessions: Vec<_> = (0..3).map(|_| runtime.create_session().unwrap()).collect();
    sessions[0].set_ascii_mode(true).unwrap();
    let previous = runtime.generation();
    runtime.reload_without_deploy().unwrap();
    assert!(runtime.generation() > previous);
    assert!(
        sessions
            .iter()
            .all(|session| lock_ignore_poison(&session.inner).engine.is_none()),
        "plain reload eagerly rebuilt a session"
    );
    assert!(
        sessions[0]
            .process_key_result(b'a' as u32, 0)
            .unwrap()
            .state
            .ascii_mode
    );
    for (index, session) in sessions.iter().enumerate() {
        let inner = lock_ignore_poison(&session.inner);
        assert_eq!(inner.engine.is_some(), index == 0);
        assert_eq!(inner.generation == runtime.generation(), index == 0);
    }
    drop(sessions);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_switch_reload_rebuilds_every_session_before_acknowledging() {
    let Some((runtime, dir)) =
        native_fixture("a_switch_reload_rebuilds_every_session_before_acknowledging")
    else {
        return;
    };
    let sessions: Vec<_> = (0..3).map(|_| runtime.create_session().unwrap()).collect();
    set_schema_switch(&dir, smoke_fixture::SCHEMA_ID, "ascii_mode", 1).unwrap();
    runtime.reload_without_deploy().unwrap();
    assert_eq!(schema_switch_applied_generation(&dir).unwrap(), 1);
    for session in &sessions {
        let inner = lock_ignore_poison(&session.inner);
        assert_eq!(inner.generation, runtime.generation());
        assert!(inner
            .engine
            .as_ref()
            .expect("acknowledged before rebuilding")
            .is_ascii_mode());
    }
    drop(sessions);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn keys_f4_and_creation_do_not_wait_for_another_sessions_rebuild() {
    let Some((runtime, dir)) =
        native_fixture("keys_f4_and_creation_do_not_wait_for_another_sessions_rebuild")
    else {
        return;
    };
    let first = runtime.create_session().unwrap();
    let second = runtime.create_session().unwrap();
    runtime.reload_without_deploy().unwrap();
    // Force a rebuild even on the old eager path so this test also catches
    // native-lock contention independently of the lazy-reload assertion.
    lock_ignore_poison(&first.inner).generation = runtime.generation().wrapping_sub(1);
    for creating in [false, true] {
        let (ready, started) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let worker_runtime = runtime.clone();
        let worker_session = first.clone();
        let worker = std::thread::spawn(move || {
            ENGINE_CREATION_GATE.with(|gate| *gate.borrow_mut() = Some((ready, resumed)));
            if creating {
                worker_runtime.create_session().is_ok()
            } else {
                worker_session.process_key_result(b'a' as u32, 0).is_some()
            }
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let initialized_free = try_lock_ignore_poison(&PROCESS_RIME.initialized).is_some();
        let (sent, received) = mpsc::channel();
        let ui_runtime = runtime.clone();
        let ui_session = second.clone();
        let ui = std::thread::spawn(move || {
            let key = ui_session.process_key_result(b'a' as u32, 0).is_none();
            let f4 = ui_session
                .process_key_result(key_policy::XK_F4, 0)
                .is_none();
            let creation = ui_runtime.create_session().is_err();
            sent.send([key, f4, creation]).unwrap();
        });
        let result = received.recv_timeout(Duration::from_secs(1));
        resume.send(()).unwrap();
        assert!(worker.join().unwrap());
        ui.join().unwrap();
        assert!(initialized_free, "session build held the directory mutex");
        assert!(result
            .expect("another session waited for a native session build")
            .into_iter()
            .all(|ok| ok));
    }
    assert!(second.process_key_result(b'a' as u32, 0).is_some());
    drop((first, second));
    std::fs::remove_dir_all(dir).unwrap();
}

static SERIAL: Mutex<()> = Mutex::new(());

fn empty_runtime_session() -> (ImeRuntime, ImeRuntimeSession) {
    let runtime = ImeRuntime::with_dirs(
        std::env::temp_dir().join("keytao-no-engine-contention-test"),
        "unused-shared-directory",
    );
    let session = ImeRuntimeSession {
        dirs: runtime.data_dirs().unwrap(),
        inner: Arc::new(Mutex::new(ImeRuntimeSessionInner {
            user_data_dir: runtime.data_dirs().unwrap().user,
            ime_host: false,
            engine: None,
            generation: PROCESS_RIME.generation.load(Ordering::SeqCst),
            carried_ascii_mode: None,
            carried_english_mode: None,
            english_mode: english_mode::EnglishModeController::default(),
            applied_input_version: 0,
            last_input_state: None,
        })),
        policy: Arc::new(SessionInputPolicy::new(InputContextPolicy::default())),
        context_ascii_mode: Arc::new(AtomicU8::new(0)),
    };
    (runtime, session)
}

#[test]
fn ui_session_operations_do_not_wait_for_a_deployment_write_lock() {
    let _serial = lock_ignore_poison(&SERIAL);
    let (runtime, session) = empty_runtime_session();
    // Pretend setup completed without touching librime. The held write lock
    // must stop every tested entry point before an Engine could be built.
    let previous_initialized = {
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        std::mem::replace(&mut *initialized, Some(runtime.data_dirs().unwrap()))
    };
    let barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
    let (sent, received) = mpsc::channel();
    let ui_session = session.clone();
    let ui = std::thread::spawn(move || {
        let creation_busy = runtime.create_session().is_err();
        let key_forwarded = ui_session.process_key_result(b'a' as u32, 0).is_none();
        let enter_forwarded = ui_session.process_enter().is_none();
        let empty = ui_session.state();
        let candidates_hidden = empty.preedit.is_empty() && empty.candidates.is_empty();
        let paging_disabled = !ui_session.supports_native_paging();
        let selection_disabled = !ui_session.supports_candidate_selection();
        let policy_deferred = ui_session
            .set_input_policy(InputContextPolicy::sensitive())
            .is_none();
        let sensitive = ui_session.input_policy() == InputContextPolicy::sensitive();
        sent.send([
            creation_busy,
            key_forwarded,
            enter_forwarded,
            candidates_hidden,
            paging_disabled,
            selection_disabled,
            policy_deferred,
            sensitive,
        ])
        .unwrap();
    });
    // On regression, release the writer before joining so the failure is
    // bounded instead of hanging the test runner forever.
    let result = received.recv_timeout(Duration::from_secs(1));
    drop(barrier);
    *lock_ignore_poison(&PROCESS_RIME.initialized) = previous_initialized;
    ui.join().unwrap();
    assert!(result
        .expect("UI waited for deployment")
        .into_iter()
        .all(|ok| ok));
    assert_eq!(session.input_policy(), InputContextPolicy::sensitive());
    // Even after deployment finishes a password key must never reach librime.
    let result = session.process_key_result(b'a' as u32, 0).unwrap();
    assert!(!result.accepted);
    assert!(result.state.preedit.is_empty());
    assert!(result.state.candidates.is_empty());
    assert!(!session.process_enter().unwrap().accepted);
    assert_eq!(
        session.policy.take_for_engine(),
        (InputContextPolicy::sensitive(), true)
    );
}

#[test]
fn initialization_and_session_mutex_contention_do_not_block_ui_or_lose_policy() {
    let _serial = lock_ignore_poison(&SERIAL);
    let (runtime, session) = empty_runtime_session();
    let initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
    let inner = lock_ignore_poison(&session.inner);
    let (sent, received) = mpsc::channel();
    let ui_session = session.clone();
    let ui = std::thread::spawn(move || {
        let creation_busy = runtime.create_session().is_err();
        let key_forwarded = ui_session.process_key_result(b'a' as u32, 0).is_none();
        let policy_deferred = ui_session
            .set_input_policy(InputContextPolicy::sensitive())
            .is_none();
        let sensitive = ui_session.input_policy() == InputContextPolicy::sensitive();
        sent.send([creation_busy, key_forwarded, policy_deferred, sensitive])
            .unwrap();
    });
    let result = received.recv_timeout(Duration::from_secs(1));
    drop(inner);
    drop(initialized);
    ui.join().unwrap();
    assert!(result
        .expect("UI waited for runtime mutexes")
        .into_iter()
        .all(|ok| ok));
    assert_eq!(session.input_policy(), InputContextPolicy::sensitive());
}

#[test]
fn a_sensitive_transition_while_busy_still_discards_the_previous_composition() {
    let policy = SessionInputPolicy::new(InputContextPolicy::default());
    policy.set(InputContextPolicy::sensitive());
    // Focus can move again before deployment releases the session. Returning
    // to a normal field must not revive the previous field's composition.
    policy.set(InputContextPolicy::private());
    assert_eq!(policy.current(), InputContextPolicy::private());
    assert_eq!(
        policy.take_for_engine(),
        (InputContextPolicy::private(), true)
    );
    assert_eq!(
        policy.take_for_engine(),
        (InputContextPolicy::private(), false)
    );
}
