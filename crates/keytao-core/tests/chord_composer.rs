//! The real keytao-bj chord settings with an isolated, deterministic dictionary.
#[path = "support/smoke_fixture.rs"]
mod smoke_fixture;

use keytao_core::{ImeRuntime, RIME_RELEASE_MASK};

#[test]
fn keytao_bj_chord_finishes_only_after_all_matching_releases() {
    let dir = smoke_fixture::scratch_dir("keytao-bj-chord");
    smoke_fixture::write(&dir).unwrap();
    for (name, contents) in [
        (
            "keytao-bj.schema.yaml",
            include_str!("fixtures/chord/keytao-bj.schema.yaml"),
        ),
        (
            "keytao-bj.dict.yaml",
            include_str!("fixtures/chord/keytao-bj.dict.yaml"),
        ),
    ] {
        std::fs::write(dir.join(name), contents).unwrap();
    }
    std::fs::write(
        dir.join("default.custom.yaml"),
        format!(
            "patch:\n  schema_list:\n    - schema: {}\n    - schema: keytao-bj\n",
            smoke_fixture::SCHEMA_ID
        ),
    )
    .unwrap();
    let runtime = ImeRuntime::with_dirs(&dir, dir.to_string_lossy().into_owned());
    runtime.init().expect("deploy the isolated chord fixture");
    let session = runtime.create_session().unwrap();
    assert!(session.select_schema("keytao-bj").unwrap().chord_composer);

    for key in [b'd', b'k'] {
        assert!(session.process_key_result(key as u32, 0).unwrap().accepted);
    }
    // Without macOS release synthesis the composition remains stuck here.
    let pending = session.state();
    assert_eq!(pending.preedit, "[dk]");
    assert!(pending.candidates.is_empty());
    assert!(pending.committed.is_none());

    let partial = session
        .process_key_result(b'd' as u32, RIME_RELEASE_MASK)
        .unwrap();
    assert!(partial.accepted);
    assert_eq!(partial.state.preedit, "[dk]");
    assert!(partial.state.candidates.is_empty());

    let finished = session
        .process_key_result(b'k' as u32, RIME_RELEASE_MASK)
        .unwrap();
    assert!(finished.accepted);
    assert_eq!(finished.state.preedit, "dk");
    assert_eq!(
        finished
            .state
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<Vec<_>>(),
        ["并击", "对抗"]
    );
    assert!(finished.state.committed.is_none());

    for (keys, code, word) in [(b"df", "y", "左"), (b"jk", "a", "右")] {
        session.reset().unwrap();
        for key in keys {
            session.process_key_result(*key as u32, 0).unwrap();
        }
        for key in keys {
            session
                .process_key_result(*key as u32, RIME_RELEASE_MASK)
                .unwrap();
        }
        let state = session.state();
        assert_eq!(state.preedit, code);
        assert_eq!(state.candidates[0].text, word);
    }

    let ordinary = session.select_schema(smoke_fixture::SCHEMA_ID).unwrap();
    assert!(!ordinary.chord_composer);
    for key in smoke_fixture::CODE.bytes() {
        session.process_key_result(key as u32, 0).unwrap();
    }
    let before_release = session.state();
    assert_eq!(before_release.preedit, smoke_fixture::CODE);
    assert_eq!(before_release.candidates.len(), smoke_fixture::PAGE_SIZE);
    let after_release = session
        .process_key_result(b'a' as u32, RIME_RELEASE_MASK)
        .unwrap();
    assert!(!after_release.accepted);
    assert_eq!(after_release.state.preedit, before_release.preedit);
    assert_eq!(
        after_release.state.candidates.len(),
        before_release.candidates.len()
    );
    drop(session);
    drop(runtime);
    std::fs::remove_dir_all(dir).unwrap();
}
