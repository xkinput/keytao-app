//! Contend with the actual runtime locks without creating a librime session.

use super::*;
use std::sync::mpsc;

static SERIAL: Mutex<()> = Mutex::new(());

fn empty_runtime_session() -> (ImeRuntime, ImeRuntimeSession) {
    let runtime = ImeRuntime::with_dirs(
        std::env::temp_dir().join("keytao-no-engine-contention-test"),
        "unused-shared-directory",
    );
    let session = ImeRuntimeSession {
        shared: runtime.0.clone(),
        inner: Arc::new(Mutex::new(ImeRuntimeSessionInner {
            engine: None,
            generation: PROCESS_RIME.generation.load(Ordering::SeqCst),
            carried_ascii_mode: None,
            carried_english_mode: None,
            english_mode: english_mode::EnglishModeController::default(),
        })),
        policy: Arc::new(SessionInputPolicy::new(InputContextPolicy::default())),
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
