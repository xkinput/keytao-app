//! Optional metadata failures must not invent restrictions or erase known ones.
use super::{merge_probe_failed_state, ContextInputState, ContextProbe, InputBlockReason};

fn unavailable() -> ContextInputState {
    ContextInputState {
        password: ContextProbe::Unavailable,
        ..ContextInputState::unrestricted()
    }
}

#[test]
fn unavailable_optional_metadata_allows_input_but_requires_another_probe() {
    let state = unavailable();
    assert!(!state.is_sensitive());
    assert_eq!(state.block_reason(), None);
    assert!(state.needs_retry());

    let merged = merge_probe_failed_state(None, state);
    assert_eq!(merged.password, ContextProbe::Unavailable);
    assert!(merged.password_probe_failed);
    assert!(!merged.is_sensitive());
    assert_eq!(merged.block_reason(), None);
    assert!(merged.needs_retry());
}

#[test]
fn unavailable_read_keeps_the_same_contexts_known_scope_and_retry_flag() {
    for known in [ContextProbe::Clear, ContextProbe::Restricted] {
        let previous = ContextInputState {
            password: known,
            ..ContextInputState::unrestricted()
        };
        let merged = merge_probe_failed_state(Some(previous), unavailable());
        assert_eq!(merged.password, known);
        assert!(merged.password_probe_failed);
        assert!(merged.needs_retry());
        assert_eq!(merged.is_sensitive(), known == ContextProbe::Restricted);
        assert_eq!(
            merged.block_reason(),
            (known == ContextProbe::Restricted).then_some(InputBlockReason::Sensitive)
        );
    }
}

#[test]
fn unavailable_new_context_does_not_inherit_the_old_password_scope() {
    let restricted = ContextInputState {
        password: ContextProbe::Restricted,
        ..ContextInputState::unrestricted()
    };
    assert!(merge_probe_failed_state(Some(restricted), unavailable()).is_sensitive());

    // None is supplied when the key's TSF context differs from the cached one.
    let new_context = merge_probe_failed_state(None, unavailable());
    assert_eq!(new_context.password, ContextProbe::Unavailable);
    assert!(!new_context.is_sensitive());
    assert!(new_context.password_probe_failed);
    assert!(new_context.needs_retry());
}

#[test]
fn unavailable_read_does_not_reuse_an_unknown_answer() {
    for previous_probe in [
        ContextProbe::Unknown,
        ContextProbe::ProbeFailed,
        ContextProbe::Unavailable,
    ] {
        let previous = ContextInputState {
            password: previous_probe,
            ..ContextInputState::unrestricted()
        };
        let merged = merge_probe_failed_state(Some(previous), unavailable());
        assert_eq!(merged.password, ContextProbe::Unavailable);
        assert!(!merged.is_sensitive());
        assert!(merged.password_probe_failed);
        assert!(merged.needs_retry());
    }
}

#[test]
fn unavailable_scope_never_overrides_disabled_or_unknown_compartments() {
    for restriction in [ContextProbe::Restricted, ContextProbe::Unknown] {
        for state in [
            ContextInputState {
                keyboard_disabled: restriction,
                ..unavailable()
            },
            ContextInputState {
                empty_context: restriction,
                ..unavailable()
            },
        ] {
            let merged = merge_probe_failed_state(None, state);
            assert!(merged.is_sensitive());
            assert!(merged.needs_retry());
            assert_eq!(
                merged.block_reason(),
                Some(if restriction == ContextProbe::Restricted {
                    InputBlockReason::ContextDisabled
                } else {
                    InputBlockReason::ProbeFailed
                })
            );
        }
    }
}

#[test]
fn a_successful_scope_retry_replaces_the_retained_answer_and_clears_failure() {
    let restricted = ContextInputState {
        password: ContextProbe::Restricted,
        ..ContextInputState::unrestricted()
    };
    let failed = merge_probe_failed_state(Some(restricted), unavailable());
    let retried = merge_probe_failed_state(Some(failed), ContextInputState::unrestricted());
    assert_eq!(retried.password, ContextProbe::Clear);
    assert!(!retried.password_probe_failed);
    assert!(!retried.is_sensitive());
    assert!(!retried.needs_retry());
}

#[test]
fn unknown_then_unavailable_never_erase_a_previously_declared_sensitive_scope() {
    let restricted = ContextInputState {
        password: ContextProbe::Restricted,
        ..ContextInputState::unrestricted()
    };
    // A disconnected context, failed selection, or E_ACCESSDENIED has no new
    // declaration. Its Unknown result must not erase the sensitive history.
    let unknown = ContextInputState {
        password: ContextProbe::Unknown,
        ..ContextInputState::unrestricted()
    };
    let failed = merge_probe_failed_state(Some(restricted), unknown);
    assert_eq!(failed.password, ContextProbe::Restricted);
    assert!(failed.password_probe_failed);
    assert!(failed.is_sensitive());
    assert!(failed.needs_retry());

    let still_unavailable = merge_probe_failed_state(Some(failed), unavailable());
    assert_eq!(still_unavailable.password, ContextProbe::Restricted);
    assert_eq!(
        still_unavailable.block_reason(),
        Some(InputBlockReason::Sensitive)
    );
    assert!(still_unavailable.password_probe_failed);
    assert!(still_unavailable.is_sensitive());
    assert!(still_unavailable.needs_retry());

    let clear =
        merge_probe_failed_state(Some(still_unavailable), ContextInputState::unrestricted());
    assert!(!clear.is_sensitive());
    assert!(!clear.password_probe_failed);
    assert!(!clear.needs_retry());

    // Changing context discards the old sensitive history, not its retry policy.
    let other_context = merge_probe_failed_state(None, unavailable());
    assert_eq!(other_context.password, ContextProbe::Unavailable);
    assert!(!other_context.is_sensitive());
    assert!(other_context.needs_retry());
}

#[test]
fn a_known_clear_scope_does_not_turn_a_later_unknown_result_into_clear() {
    let unknown = ContextInputState {
        password: ContextProbe::Unknown,
        ..ContextInputState::unrestricted()
    };
    let failed = merge_probe_failed_state(Some(ContextInputState::unrestricted()), unknown);
    assert_eq!(failed.password, ContextProbe::Unknown);
    assert_eq!(failed.block_reason(), Some(InputBlockReason::ProbeFailed));
    assert!(failed.is_sensitive());
    assert!(failed.needs_retry());
}
