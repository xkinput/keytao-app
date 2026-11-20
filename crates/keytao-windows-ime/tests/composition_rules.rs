#[path = "../src/composition_rules.rs"]
mod composition_rules;

use composition_rules::{
    composition_end_to_finish_first, leftover_to_delete_on_host_termination, may_process_key,
    may_terminate_composition, should_drop_pending_context, should_retry_commit,
};
use std::collections::VecDeque;

#[test]
fn finish_only_a_queued_end_in_the_same_context() {
    assert_eq!(composition_end_to_finish_first([], 1), None);
    assert_eq!(composition_end_to_finish_first([(2, true)], 1), None);
    assert_eq!(composition_end_to_finish_first([(1, false)], 1), None);
    assert_eq!(
        composition_end_to_finish_first([(2, true), (1, false), (1, true), (1, true)], 1),
        Some(2)
    );
}

#[test]
fn preedit_must_be_cleared_before_termination() {
    assert!(!may_terminate_composition(true, false));
    assert!(may_terminate_composition(true, true));
    assert!(may_terminate_composition(false, false));
    assert!(may_terminate_composition(false, true));
}

#[test]
fn host_termination_deletes_only_the_exact_uncommitted_preedit() {
    assert!(leftover_to_delete_on_host_termination("yu", "yu", false));
    assert!(!leftover_to_delete_on_host_termination("词", "yu", false));
    assert!(!leftover_to_delete_on_host_termination("yu!", "yu", false));
    assert!(!leftover_to_delete_on_host_termination("yu", "yu", true));
    assert!(!leftover_to_delete_on_host_termination("", "", false));
}

#[test]
fn pending_ends_hold_only_their_context_while_apply_serializes_the_engine() {
    assert!(may_process_key(false, [], 1));
    assert!(!may_process_key(true, [], 1));
    assert!(!may_process_key(false, [1], 1));
    assert!(!may_process_key(true, [1], 1));
    assert!(may_process_key(false, [2], 1));
    assert!(!may_process_key(false, [2, 1], 1));
    assert!(!may_process_key(true, [2], 1));
}

#[test]
fn only_confirmed_disconnected_contexts_drop_pending_work() {
    assert!(should_drop_pending_context(Some(false), false));
    assert!(should_drop_pending_context(None, true));
    assert!(should_drop_pending_context(Some(true), true));
    assert!(!should_drop_pending_context(Some(true), false));
    assert!(!should_drop_pending_context(None, false));
}

#[test]
fn an_already_written_commit_is_never_retried() {
    assert!(should_retry_commit(false));
    assert!(!should_retry_commit(true));
}

struct Composition {
    context: usize,
    start: usize,
    text: String,
    committed: bool,
}

struct FakeHost {
    document: String,
    compositions: Vec<Composition>,
    pending_ends: Vec<(usize, bool)>,
    pending_letter: Option<char>,
    queued_keys: VecDeque<char>,
    engine_keys: Vec<char>,
    commit_written: bool,
    toggle_written: bool,
    commit_writes: Vec<String>,
    max_live: usize,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            document: "x".into(),
            compositions: vec![Composition {
                context: 1,
                start: 0,
                text: "x".into(),
                committed: false,
            }],
            pending_ends: Vec::new(),
            pending_letter: None,
            queued_keys: VecDeque::new(),
            engine_keys: vec!['x'],
            commit_written: false,
            toggle_written: false,
            commit_writes: Vec::new(),
            max_live: 1,
        }
    }

    fn write_word_once(&mut self) {
        if should_retry_commit(self.commit_written) {
            let composition = &mut self.compositions[0];
            self.document.replace_range(composition.start.., "词");
            composition.text = "词".into();
            composition.committed = true;
            self.commit_written = true;
            self.commit_writes.push("词".into());
        }
    }

    fn top_up(&mut self, sync_refused: bool, commit_already_written: bool) {
        self.engine_keys.push('y');
        self.pending_letter = Some('y');
        self.pending_ends.push((1, true));
        if commit_already_written {
            self.write_word_once();
        }
        if !sync_refused {
            self.run_edit_session();
        }
    }

    fn key(&mut self, key: char) {
        if may_process_key(
            self.pending_letter.is_some(),
            self.pending_ends.iter().map(|(context, _)| *context),
            1,
        ) {
            self.engine_keys.push(key);
            if key == '⇧' {
                self.commit_shift_toggle();
            } else {
                self.write_preedit(key);
            }
        } else {
            self.queued_keys.push_back(key);
        }
    }

    fn write_preedit(&mut self, letter: char) {
        if self.compositions.is_empty() || self.compositions[0].committed {
            self.compositions.push(Composition {
                context: 1,
                start: self.document.len(),
                text: String::new(),
                committed: false,
            });
            self.max_live = self.max_live.max(self.compositions.len());
        }
        let composition = self.compositions.last_mut().unwrap();
        composition.text.push(letter);
        self.document
            .replace_range(composition.start.., &composition.text);
    }

    fn run_edit_session(&mut self) {
        if let Some(index) = composition_end_to_finish_first(self.pending_ends.clone(), 1) {
            self.write_word_once();
            self.pending_ends.remove(index);
            self.compositions.remove(0);
        }
        if let Some(letter) = self.pending_letter.take() {
            self.write_preedit(letter);
        }
        while let Some(key) = self.queued_keys.pop_front() {
            self.key(key);
        }
    }

    fn host_terminates_newest(&mut self) {
        let composition = self.compositions.pop().unwrap();
        assert_eq!(composition.context, 1);
        let range_text = &self.document[composition.start..];
        if leftover_to_delete_on_host_termination(
            range_text,
            &composition.text,
            composition.committed,
        ) {
            self.document.truncate(composition.start);
        }
    }

    fn shift_toggle(&mut self) {
        self.key('⇧');
    }

    fn commit_shift_toggle(&mut self) {
        if should_retry_commit(self.toggle_written) {
            let composition = self.compositions.last_mut().unwrap();
            composition.committed = true;
            self.toggle_written = true;
            self.commit_writes.push(composition.text.clone());
        }
    }
}

#[test]
fn refused_top_up_then_next_key_and_host_termination_leave_only_one_word() {
    for commit_already_written in [false, true] {
        let mut host = FakeHost::new();
        host.top_up(true, commit_already_written);
        host.key('u');
        assert_eq!(host.engine_keys, vec!['x', 'y']);
        assert_eq!(host.compositions.len(), 1);
        host.run_edit_session();
        assert_eq!(host.engine_keys, vec!['x', 'y', 'u']);
        host.host_terminates_newest();
        assert_eq!(host.document, "词");
        assert_eq!(host.commit_writes, vec!["词"]);
        assert_eq!(host.max_live, 1);
    }
}

#[test]
fn shift_toggle_waits_for_top_up_and_commits_once() {
    let mut host = FakeHost::new();
    host.top_up(true, false);
    host.shift_toggle();
    assert_eq!(host.engine_keys, vec!['x', 'y']);
    host.run_edit_session();
    assert_eq!(host.engine_keys, vec!['x', 'y', '⇧']);
    host.commit_shift_toggle();
    host.host_terminates_newest();
    assert_eq!(host.document, "词y");
    assert_eq!(host.commit_writes, vec!["词", "y"]);
    assert_eq!(host.max_live, 1);
}
