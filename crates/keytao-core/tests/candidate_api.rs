//! Candidate selection, paging and input-context policy against a fixture that
//! makes the synthesized-key fallbacks visibly wrong.
//!
//! The fixture publishes no `menu/select_keys`, keeps `-`, `=` and the digits
//! in the speller's alphabet and has no `key_binder/bindings`, so every
//! fallback stroke lands in the composition instead of doing what it was meant
//! to do. A page also holds more candidates than `DEFAULT_SELECT_KEYS`, so the
//! last one of a page cannot be reached by a select key at all. An assertion
//! below can therefore only hold when librime's own entry points ran.
//!
//! Needs the librime the crate links against at run time, e.g.
//! `DYLD_FALLBACK_LIBRARY_PATH="$RIME_LIB_DIR" cargo test -p keytao-core`.

#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{
    ImeRuntime, ImeRuntimeSession, ImeState, InputContextPolicy, SchemaSwitch, DEFAULT_SELECT_KEYS,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// There is one librime per process, so the tests in this binary take turns.
static LIBRIME: Mutex<()> = Mutex::new(());

fn serialized() -> MutexGuard<'static, ()> {
    LIBRIME.lock().unwrap_or_else(PoisonError::into_inner)
}

fn deployed_runtime(name: &str) -> (ImeRuntime, std::path::PathBuf) {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write(&dir).expect("write the fixture");
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the fixture");
    (runtime, dir)
}

fn deployed_runtime_with_user_dictionary(name: &str) -> (ImeRuntime, std::path::PathBuf) {
    let dir = smoke_fixture::scratch_dir(name);
    smoke_fixture::write_with_user_dictionary(&dir, true).expect("write the fixture");
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the fixture");
    (runtime, dir)
}

fn type_code(session: &ImeRuntimeSession) -> ImeState {
    session.reset().expect("session has no engine");
    let mut state = ImeState::empty();
    for character in smoke_fixture::CODE.chars() {
        state = session
            .process_key_result(character as u32, 0)
            .expect("session has no engine")
            .state;
    }
    state
}

fn page_words(state: &ImeState) -> Vec<&str> {
    state
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect()
}

#[test]
fn schema_switches_follow_the_active_schema() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("schema-switches");
    let session = runtime.create_session().expect("create a session");

    assert_eq!(
        session.schema_switches().expect("session has no engine"),
        vec![
            SchemaSwitch {
                name: Some("ascii_mode".into()),
                options: vec![],
                states: vec!["中文".into(), "西文".into()],
                reset: Some(0),
            },
            SchemaSwitch {
                name: None,
                options: vec!["punctuation_ascii".into(), "punctuation_cjk".into()],
                states: vec!["西文标点".into(), "中文标点".into()],
                reset: Some(1),
            },
        ]
    );

    drop(session);
    cleanup(&dir);
}

#[test]
fn candidate_deletion_is_limited_to_user_dictionary_entries() {
    const USER_PHRASE: &str = smoke_fixture::WORDS[0];

    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime_with_user_dictionary("candidate-user-source");
    let session = runtime.create_session().expect("create a session");

    let initial = type_code(&session);
    let initial_index = initial
        .candidates
        .iter()
        .position(|candidate| candidate.text == USER_PHRASE)
        .expect("built-in phrase is not on the first page");
    assert_eq!(
        session.candidate_is_user_phrase_on_page(initial_index),
        Some(false)
    );
    let committed = session
        .select_candidate_on_page(initial_index)
        .expect("select_candidate_on_page");
    assert_eq!(committed.committed.as_deref(), Some(USER_PHRASE));

    // A newly created Engine must recover learned-word provenance from the
    // persisted user dictionary, not only from commits seen by this session.
    drop(session);
    runtime
        .reload_without_deploy()
        .expect("reinitialize librime for a cold-restart check");
    let session = runtime.create_session().expect("recreate a session");

    let state = type_code(&session);
    let index = state
        .candidates
        .iter()
        .position(|candidate| candidate.text == USER_PHRASE)
        .expect("learned phrase is not on the first page");
    assert_eq!(session.candidate_is_user_phrase_on_page(index), Some(true));

    let (_, deleted) = session
        .delete_candidate_on_page_result(index)
        .expect("delete_candidate_on_page_result");
    assert!(deleted);
    let after = type_code(&session);
    let system_index = after
        .candidates
        .iter()
        .position(|candidate| candidate.text == USER_PHRASE)
        .expect("the built-in dictionary entry should remain after forgetting its user record");
    assert_eq!(
        session.candidate_is_user_phrase_on_page(system_index),
        Some(false)
    );

    drop(session);
    cleanup(&dir);
}

#[test]
fn selection_and_paging_go_through_librime() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("candidate-api");

    let capabilities = runtime.capabilities();
    assert!(
        capabilities.supports_candidate_selection(),
        "the linked librime cannot select candidates: {capabilities:?}"
    );
    assert!(
        capabilities.supports_native_paging(),
        "the linked librime cannot turn pages: {capabilities:?}"
    );
    assert!(
        capabilities.supports_candidate_highlight(),
        "the linked librime cannot move the highlight: {capabilities:?}"
    );
    assert!(
        capabilities.supports_candidate_deletion(),
        "the linked librime cannot classify candidate deletion: {capabilities:?}"
    );

    let session = runtime.create_session().expect("create a session");
    assert!(session.supports_candidate_selection());
    assert!(session.supports_native_paging());

    let first = type_code(&session);
    assert_eq!(first.preedit, smoke_fixture::CODE);
    assert_eq!(
        first.select_keys.as_deref().unwrap_or_default(),
        "",
        "the fixture must not publish select keys"
    );
    assert_eq!(first.page, 0);
    assert!(
        !first.is_last_page,
        "the fixture must produce several pages"
    );
    assert_eq!(
        page_words(&first),
        &smoke_fixture::WORDS[..smoke_fixture::PAGE_SIZE]
    );
    assert!(
        first.candidates.len() > DEFAULT_SELECT_KEYS.chars().count(),
        "a page must outgrow the select-key fallback"
    );

    // A synthesized `=` would land in the speller instead of turning the page.
    let second = session.change_page(false).expect("change_page(forward)");
    assert_eq!(second.page, 1);
    assert_eq!(second.preedit, first.preedit);
    assert!(second.is_last_page);
    assert_eq!(
        page_words(&second),
        &smoke_fixture::WORDS[smoke_fixture::PAGE_SIZE..]
    );

    let back = session.change_page(true).expect("change_page(backward)");
    assert_eq!(back.page, 0);
    assert_eq!(back.preedit, first.preedit);
    assert_eq!(page_words(&back), page_words(&first));

    // The highlight moves without committing and without editing the code.
    let last = smoke_fixture::PAGE_SIZE - 1;
    let highlighted = session
        .highlight_candidate_on_page(last)
        .expect("highlight_candidate_on_page");
    assert_eq!(highlighted.highlighted_candidate_index, last);
    assert_eq!(highlighted.committed, None);
    assert_eq!(highlighted.preedit, first.preedit);

    // The last candidate of the page has no select key, so a fallback stroke
    // could not reach it at all.
    let selected = session
        .select_candidate_on_page(last)
        .expect("select_candidate_on_page");
    assert_eq!(
        selected.committed.as_deref(),
        Some(smoke_fixture::WORDS[last])
    );
    assert!(selected.preedit.is_empty());

    // The first candidate is the one a synthesized `1` would have aimed at; the
    // digit is part of the alphabet, so a fallback would extend the code.
    type_code(&session);
    let head = session
        .select_candidate_on_page(0)
        .expect("select_candidate_on_page");
    assert_eq!(head.committed.as_deref(), Some(smoke_fixture::WORDS[0]));
    assert!(head.preedit.is_empty());

    // Built-in table entries are not learned phrases. The native deletion
    // result is the source-of-truth distinction used by the mobile menu.
    type_code(&session);
    let (unchanged, deleted) = session
        .delete_candidate_on_page_result(0)
        .expect("delete_candidate_on_page_result");
    assert!(!deleted);
    assert_eq!(page_words(&unchanged), page_words(&first));
    assert_eq!(unchanged.preedit, first.preedit);

    // Discarding and committing are the focus-change paths.
    type_code(&session);
    let cleared = session.clear_composition().expect("clear_composition");
    assert!(cleared.preedit.is_empty());
    assert!(cleared.candidates.is_empty());
    assert_eq!(cleared.committed, None);

    type_code(&session);
    let committed = session.commit_composition().expect("commit_composition");
    assert!(
        committed.committed.is_some(),
        "commit_composition produced no text"
    );
    assert!(committed.preedit.is_empty());

    type_code(&session);
    let raw = session.raw_input().expect("raw_input");
    assert_eq!(raw, smoke_fixture::CODE);
    let raw_commit = session.commit_raw_input().expect("commit_raw_input");
    assert_eq!(raw_commit.committed.as_deref(), Some(smoke_fixture::CODE));
    assert!(raw_commit.preedit.is_empty());

    drop(session);
    cleanup(&dir);
}

/// The policy check and the librime call it guards share one critical section,
/// so a context that has turned sensitive can never end up composing — not even
/// for the key another thread was already handling when it turned.
#[test]
fn a_sensitive_context_never_composes_under_concurrent_typing() {
    let _serialized = serialized();
    let (runtime, dir) = deployed_runtime("candidate-api-policy");
    let session = runtime.create_session().expect("create a session");

    let stop = AtomicBool::new(false);
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let composed = AtomicU64::new(0);

    std::thread::scope(|scope| {
        let typist = &session;
        let typing_stop = &stop;
        scope.spawn(move || {
            while !typing_stop.load(Ordering::Relaxed) {
                let _ = typist.reset();
                for character in smoke_fixture::CODE.chars() {
                    let Some(result) = typist.process_key_result(character as u32, 0) else {
                        continue;
                    };
                    if !result.accepted && !result.state.preedit.is_empty() {
                        // A rejected key must not have composed anything.
                        break;
                    }
                }
            }
        });

        for _ in 0..300 {
            let switched = session
                .set_input_policy(InputContextPolicy::sensitive())
                .expect("set_input_policy");
            if !switched.preedit.is_empty() || !switched.candidates.is_empty() {
                failures
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(format!(
                        "turning sensitive left preedit {:?}",
                        switched.preedit
                    ));
            }
            for _ in 0..16 {
                let state = session.state();
                if !state.preedit.is_empty() || !state.candidates.is_empty() {
                    failures
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(format!("sensitive context composed {:?}", state.preedit));
                    break;
                }
            }
            session
                .set_input_policy(InputContextPolicy::default())
                .expect("set_input_policy");
            for _ in 0..16 {
                if !session.state().preedit.is_empty() {
                    composed.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        stop.store(true, Ordering::Relaxed);
    });

    let failures = failures.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(
        composed.load(Ordering::Relaxed) > 0,
        "the typing thread never composed, so the test proved nothing"
    );

    drop(session);
    cleanup(&dir);
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}
