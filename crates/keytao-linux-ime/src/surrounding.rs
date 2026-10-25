//! Validated, single-use surrounding text for Wayland's text-only fallback.
//! Protocol offsets and deletion lengths count UTF-8 bytes, not characters.

#[derive(Default)]
pub struct SurroundingText {
    current: Option<Snapshot>,
    pending: Option<Snapshot>,
}

struct Snapshot {
    text: String,
    cursor: usize,
}

impl SurroundingText {
    /// A received snapshot becomes usable only at the following `done`.
    /// Selections are left to physical forwarding; the text-only protocol
    /// cannot safely reproduce their native deletion behavior here.
    pub fn stage(&mut self, text: String, cursor: u32, anchor: u32) {
        self.current = None;
        let cursor = cursor as usize;
        self.pending =
            (cursor == anchor as usize && cursor <= text.len() && text.is_char_boundary(cursor))
                .then_some(Snapshot { text, cursor });
    }

    pub fn done(&mut self) {
        self.current = self.pending.take();
    }

    /// Focus changes and our own edits invalidate both buffers. Never guess
    /// the cursor after an edit, or replay a late snapshot optimistically.
    pub fn invalidate(&mut self) {
        self.current = None;
        self.pending = None;
    }

    /// Consume one authoritative snapshot. Repeats must wait for a new one;
    /// otherwise a delayed reply could make us cut into a multibyte scalar.
    pub fn take_delete(&mut self, backward: bool) -> Option<(u32, u32)> {
        let snapshot = self.current.take()?;
        if backward {
            let ch = snapshot.text[..snapshot.cursor].chars().next_back()?;
            Some((ch.len_utf8() as u32, 0))
        } else {
            let ch = snapshot.text[snapshot.cursor..].chars().next()?;
            Some((0, ch.len_utf8() as u32))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SurroundingText;

    fn snapshot(text: &str, cursor: u32) -> SurroundingText {
        let mut state = SurroundingText::default();
        state.stage(text.into(), cursor, cursor);
        state.done();
        state
    }

    #[test]
    fn deletion_counts_adjacent_unicode_scalar_bytes() {
        for (text, cursor, backward, expected) in [
            ("a中😀z", 1, true, (1, 0)),
            ("a中😀z", 4, true, (3, 0)),
            ("a中😀z", 8, true, (4, 0)),
            ("a中😀z", 1, false, (0, 3)),
            ("a中😀z", 4, false, (0, 4)),
        ] {
            assert_eq!(snapshot(text, cursor).take_delete(backward), Some(expected));
        }
    }

    #[test]
    fn repeat_waits_for_each_new_done_snapshot() {
        let mut state = snapshot("a中😀", 8);
        assert_eq!(state.take_delete(true), Some((4, 0)));
        assert_eq!(state.take_delete(true), None);
        state.stage("a中".into(), 4, 4);
        assert_eq!(state.take_delete(true), None);
        state.done();
        assert_eq!(state.take_delete(true), Some((3, 0)));
        assert_eq!(state.take_delete(true), None);
        state.stage("a".into(), 1, 1);
        state.done();
        assert_eq!(state.take_delete(true), Some((1, 0)));
    }

    #[test]
    fn selection_and_invalid_byte_offsets_do_not_delete_neighbors() {
        for (cursor, anchor) in [(1, 4), (4, 1), (2, 2), (20, 20)] {
            let mut state = SurroundingText::default();
            state.stage("a中😀".into(), cursor, anchor);
            state.done();
            assert_eq!(state.take_delete(true), None);
            assert_eq!(state.take_delete(false), None);
        }
    }

    #[test]
    fn pending_selection_replaces_an_older_collapsed_snapshot() {
        let mut state = snapshot("a中", 4);
        state.stage("a中".into(), 4, 1);
        assert_eq!(state.take_delete(true), None);
        state.done();
        assert_eq!(state.take_delete(true), None);
    }

    #[test]
    fn missing_or_truncated_surrounding_text_never_guesses_a_byte() {
        assert_eq!(SurroundingText::default().take_delete(true), None);
        assert_eq!(snapshot("中", 0).take_delete(true), None);
        assert_eq!(snapshot("中", 3).take_delete(false), None);
    }

    #[test]
    fn focus_and_own_edits_discard_current_and_pending_snapshots() {
        let mut state = snapshot("中", 3);
        state.invalidate();
        assert_eq!(state.take_delete(true), None);
        state.stage("😀".into(), 4, 4);
        state.invalidate();
        state.done();
        assert_eq!(state.take_delete(true), None);
    }

    #[test]
    fn done_without_surrounding_text_drops_the_previous_snapshot() {
        let mut state = snapshot("中", 3);
        state.done();
        assert_eq!(state.take_delete(true), None);
    }
}
