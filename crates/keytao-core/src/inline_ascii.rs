//! Session-layer inline ASCII composition for backends without native support.
//!
//! Pure decoding backends (`libjd`, and future `libpinyin` / `libime` backends)
//! own dictionary lookup and candidate generation rather than modifier-driven
//! temporary English editing. Backends that already own a processor pipeline
//! (`librime`) declare [`ImeBackend::supports_native_inline_ascii`] = `true`
//! and pass through unwrapped without any behavioral change.

use crate::{
    english_mode::EnglishBackend,
    global_input::InputState,
    key_policy::{
        is_enter_key, is_space_key, XK_BACK_SPACE, XK_DELETE, XK_DOWN, XK_END, XK_ESCAPE, XK_HOME,
        XK_LEFT, XK_PAGE_DOWN, XK_PAGE_UP, XK_RIGHT, XK_UP,
    },
    lock_ignore_poison, Candidate, EngineCapabilities, ImeBackend, ImeState, KeyProcessResult,
    SchemaInfo, SchemaShortcut, SchemaSwitch, SchemaSwitchRequest, SchemaSwitchSetting,
    RIME_MOD_LOCK, RIME_MOD_SHIFT, RIME_RELEASE_MASK,
};
use std::{
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

const MAX_INLINE_ASCII_BYTES: usize = 256;
const SOLO_SHIFT_TIMEOUT: Duration = Duration::from_millis(500);
const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;
const XK_CAPS_LOCK: u32 = 0xffe5;
const XK_INSERT: u32 = 0xff63;

/// Wrap `backend` with session-layer inline ASCII composition unless it already
/// handles temporary English composition natively (such as `librime`).
pub(crate) fn wrap_if_needed(backend: Box<dyn ImeBackend>) -> Box<dyn ImeBackend> {
    if backend.supports_native_inline_ascii() {
        backend
    } else {
        Box::new(InlineAsciiBackend::new(backend))
    }
}

/// An `A..=Z` keypress without CapsLock or non-Shift modifiers starts temporary
/// English composition when Chinese mode is idle. Accepting both `RIME_MOD_SHIFT`
/// (desktop and hardware keyboards) and `0` (mobile soft-keyboard uppercase text)
/// matches `librime`'s `recognizer/patterns/uppercase` across all platforms.
fn is_uppercase_starter(keycode: u32, mask: u32) -> bool {
    mask & !RIME_MOD_SHIFT == 0 && (u32::from(b'A')..=u32::from(b'Z')).contains(&keycode)
}

/// Matches `librime`'s `recognizer/patterns/uppercase: "[A-Z][-_+.'0-9A-Za-z]*$"`.
fn is_continuation_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'+' | b'.' | b'\'')
}

fn printable_ascii(keycode: u32) -> Option<u8> {
    (0x21..=0x7e).contains(&keycode).then_some(keycode as u8)
}

fn prepend_committed(state: &mut ImeState, prefix: Option<String>) {
    let Some(mut text) = prefix else {
        return;
    };
    if let Some(rest) = state.committed.take() {
        text.push_str(&rest);
    }
    state.committed = Some(text);
}

#[derive(Default)]
struct InlineAsciiState {
    buffer: String,
    cursor: usize,
    solo_shift: Option<(u32, Instant)>,
    /// Shift key pressed down while inactive; if the next key starts inline
    /// ASCII, its release is suppressed so the inner backend does not mistake
    /// `Shift+A` for a solo Shift language toggle.
    pending_outer_shift: Option<u32>,
    suppress_outer_shift_release: Option<u32>,
}

impl InlineAsciiState {
    fn is_active(&self) -> bool {
        !self.buffer.is_empty()
    }

    fn start(&mut self, byte: u8) {
        self.buffer.clear();
        self.buffer.push(char::from(byte));
        self.cursor = 1;
        self.solo_shift = None;
        self.suppress_outer_shift_release = self.pending_outer_shift.take();
    }

    fn insert(&mut self, byte: u8) {
        if self.buffer.len() < MAX_INLINE_ASCII_BYTES {
            self.buffer.insert(self.cursor, char::from(byte));
            self.cursor += 1;
        }
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
        }
        if self.buffer.is_empty() {
            self.solo_shift = None;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
        if self.buffer.is_empty() {
            self.solo_shift = None;
        }
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.buffer.len());
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    fn take_commit(&mut self) -> Option<String> {
        self.cursor = 0;
        self.solo_shift = None;
        let text = std::mem::take(&mut self.buffer);
        (!text.is_empty()).then_some(text)
    }

    fn clear_composition(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.solo_shift = None;
    }

    fn overlay(&self, mut state: ImeState) -> ImeState {
        if !self.is_active() {
            return state;
        }
        state.preedit = self.buffer.clone();
        // Keep commit_preview empty when there are no candidates, matching
        // librime so every frontend uses `preedit` and preserves mid-string
        // caret positions.
        state.commit_preview.clear();
        state.cursor = self.cursor;
        state.sel_start = self.cursor;
        state.sel_end = self.cursor;
        state.candidates.clear();
        state.highlighted_candidate_index = 0;
        state.page = 0;
        state.is_last_page = true;
        state.select_keys = None;
        state
    }
}

pub(crate) struct InlineAsciiBackend {
    inner: Box<dyn ImeBackend>,
    inline: Mutex<InlineAsciiState>,
}

impl InlineAsciiBackend {
    pub(crate) fn new(inner: Box<dyn ImeBackend>) -> Self {
        Self {
            inner,
            inline: Mutex::new(InlineAsciiState::default()),
        }
    }

    fn process_active_key(
        &self,
        inline: &mut InlineAsciiState,
        keycode: u32,
        mask: u32,
    ) -> KeyProcessResult {
        if matches!(keycode, XK_SHIFT_L | XK_SHIFT_R) {
            return self.process_active_shift(inline, keycode, mask);
        }
        if mask & RIME_RELEASE_MASK != 0 {
            return KeyProcessResult {
                state: inline.overlay(self.inner.state()),
                accepted: false,
            };
        }
        inline.solo_shift = None;
        if mask & !(RIME_MOD_SHIFT | RIME_MOD_LOCK) != 0 {
            return KeyProcessResult {
                state: inline.overlay(self.inner.state()),
                accepted: false,
            };
        }
        if keycode == XK_CAPS_LOCK {
            let committed = inline.take_commit();
            let mut result = self.inner.process_key_result(keycode, mask);
            prepend_committed(&mut result.state, committed);
            return result;
        }
        match keycode {
            XK_BACK_SPACE => {
                inline.backspace();
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: true,
                }
            }
            XK_DELETE => {
                inline.delete();
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: true,
                }
            }
            XK_LEFT => {
                inline.move_left();
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: true,
                }
            }
            XK_RIGHT => {
                inline.move_right();
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: true,
                }
            }
            XK_HOME => {
                inline.move_home();
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: true,
                }
            }
            XK_END => {
                inline.move_end();
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: true,
                }
            }
            XK_UP | XK_DOWN | XK_PAGE_UP | XK_PAGE_DOWN | XK_INSERT => KeyProcessResult {
                state: inline.overlay(self.inner.state()),
                accepted: true,
            },
            XK_ESCAPE => {
                inline.clear_composition();
                KeyProcessResult {
                    state: self.inner.state(),
                    accepted: true,
                }
            }
            key if is_enter_key(key) || is_space_key(key) => {
                let committed = inline.take_commit();
                let mut state = self.inner.state();
                state.committed = committed;
                KeyProcessResult {
                    state,
                    accepted: true,
                }
            }
            _ => {
                let Some(byte) = printable_ascii(keycode) else {
                    return KeyProcessResult {
                        state: inline.overlay(self.inner.state()),
                        accepted: false,
                    };
                };
                if is_continuation_byte(byte) {
                    inline.insert(byte);
                    return KeyProcessResult {
                        state: inline.overlay(self.inner.state()),
                        accepted: true,
                    };
                }
                // Commit the inline ASCII segment and immediately hand the
                // punctuation/delimiter to the inner backend in one atomic step.
                let committed = inline.take_commit();
                let mut result = self.inner.process_key_result(keycode, mask);
                prepend_committed(&mut result.state, committed);
                result
            }
        }
    }

    fn process_active_shift(
        &self,
        inline: &mut InlineAsciiState,
        keycode: u32,
        mask: u32,
    ) -> KeyProcessResult {
        let armed = inline.solo_shift.take();
        match mask & !RIME_MOD_LOCK {
            0 => {
                inline.suppress_outer_shift_release = None;
                inline.solo_shift = Some((keycode, Instant::now()));
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: false,
                }
            }
            RIME_RELEASE_MASK => {
                if inline.suppress_outer_shift_release == Some(keycode) {
                    inline.suppress_outer_shift_release = None;
                }
                if armed.is_some_and(|(key, started)| {
                    key == keycode && started.elapsed() < SOLO_SHIFT_TIMEOUT
                }) {
                    let committed = inline.take_commit();
                    let mut state = self.inner.set_ascii_mode(!self.inner.is_ascii_mode());
                    prepend_committed(&mut state, committed);
                    return KeyProcessResult {
                        state,
                        accepted: true,
                    };
                }
                KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: false,
                }
            }
            _ => KeyProcessResult {
                state: inline.overlay(self.inner.state()),
                accepted: false,
            },
        }
    }
}

impl EnglishBackend for InlineAsciiBackend {
    fn current_schema(&self) -> Option<SchemaInfo> {
        self.inner.current_schema()
    }

    fn schema_switches(&self) -> Vec<SchemaSwitch> {
        self.inner.schema_switches()
    }

    fn get_option(&self, name: &str) -> bool {
        self.inner.get_option(name)
    }

    fn set_option(&self, name: &str, enabled: bool) -> ImeState {
        let mut inline = lock_ignore_poison(&self.inline);
        if name == "ascii_mode" && enabled {
            let committed = inline.take_commit();
            let mut state = self.inner.set_option(name, enabled);
            prepend_committed(&mut state, committed);
            return state;
        }
        inline.overlay(self.inner.set_option(name, enabled))
    }

    fn select_schema(&self, id: &str) -> Result<ImeState, String> {
        let mut inline = lock_ignore_poison(&self.inline);
        inline.clear_composition();
        self.inner.select_schema(id)
    }

    fn discard_composition(&self) {
        let mut inline = lock_ignore_poison(&self.inline);
        inline.clear_composition();
        self.inner.discard_composition();
    }

    fn state(&self) -> ImeState {
        let inline = lock_ignore_poison(&self.inline);
        inline.overlay(self.inner.state())
    }

    fn apply_ascii_mode(&self, enabled: bool) {
        let mut inline = lock_ignore_poison(&self.inline);
        if enabled {
            inline.clear_composition();
        }
        self.inner.apply_ascii_mode(enabled);
    }

    fn set_ascii_mode(&self, enabled: bool) -> ImeState {
        let mut inline = lock_ignore_poison(&self.inline);
        if enabled {
            let committed = inline.take_commit();
            let mut state = self.inner.set_ascii_mode(enabled);
            prepend_committed(&mut state, committed);
            return state;
        }
        inline.overlay(self.inner.set_ascii_mode(enabled))
    }
}

impl ImeBackend for InlineAsciiBackend {
    fn input_state(&self) -> InputState {
        self.inner.input_state()
    }

    fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult {
        let mut inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            return self.process_active_key(&mut inline, keycode, mask);
        }
        if matches!(keycode, XK_SHIFT_L | XK_SHIFT_R) {
            if mask & RIME_RELEASE_MASK == 0 {
                inline.suppress_outer_shift_release = None;
                inline.pending_outer_shift = (mask & !RIME_MOD_LOCK == 0).then_some(keycode);
                return self.inner.process_key_result(keycode, mask);
            }
            if inline.suppress_outer_shift_release == Some(keycode) {
                inline.suppress_outer_shift_release = None;
                inline.pending_outer_shift = None;
                return KeyProcessResult {
                    state: self.inner.state(),
                    accepted: false,
                };
            }
            inline.pending_outer_shift = None;
            return self.inner.process_key_result(keycode, mask);
        }
        if mask & RIME_RELEASE_MASK == 0 {
            if is_uppercase_starter(keycode, mask)
                && !self.inner.is_ascii_mode()
                && !self.inner.is_composing()
            {
                inline.start(keycode as u8);
                // Clear any armed modifier tap inside the inner backend while it
                // is idle so releasing Shift after `Shift+A` cannot toggle mode.
                self.inner.discard_composition();
                return KeyProcessResult {
                    state: inline.overlay(self.inner.state()),
                    accepted: true,
                };
            }
            inline.pending_outer_shift = None;
            inline.suppress_outer_shift_release = None;
        }
        self.inner.process_key_result(keycode, mask)
    }

    fn switcher_hotkeys(&self) -> &[SchemaShortcut] {
        self.inner.switcher_hotkeys()
    }

    fn has_declared_shortcut(&self, keycode: u32, mask: u32) -> bool {
        self.inner.has_declared_shortcut(keycode, mask)
    }

    fn shift_switch_disabled(&self, schema: &str, keycode: u32) -> bool {
        self.inner.shift_switch_disabled(schema, keycode)
    }

    fn process_enter(&self) -> KeyProcessResult {
        let mut inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            let committed = inline.take_commit();
            let mut state = self.inner.state();
            state.committed = committed;
            return KeyProcessResult {
                state,
                accepted: true,
            };
        }
        self.inner.process_enter()
    }

    fn select_candidate_on_page(&self, index: usize) -> ImeState {
        let inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            return inline.overlay(self.inner.state());
        }
        self.inner.select_candidate_on_page(index)
    }

    fn select_candidate_global(&self, index: usize) -> ImeState {
        let inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            return inline.overlay(self.inner.state());
        }
        self.inner.select_candidate_global(index)
    }

    fn highlight_candidate_on_page(&self, index: usize) -> ImeState {
        let inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            return inline.overlay(self.inner.state());
        }
        self.inner.highlight_candidate_on_page(index)
    }

    fn candidate_is_user_phrase_on_page(&self, index: usize) -> bool {
        if lock_ignore_poison(&self.inline).is_active() {
            return false;
        }
        self.inner.candidate_is_user_phrase_on_page(index)
    }

    fn delete_candidate_on_page_result(&self, index: usize) -> (ImeState, bool) {
        let inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            return (inline.overlay(self.inner.state()), false);
        }
        self.inner.delete_candidate_on_page_result(index)
    }

    fn all_candidates_limited(&self, max_count: usize) -> Vec<Candidate> {
        if lock_ignore_poison(&self.inline).is_active() {
            return Vec::new();
        }
        self.inner.all_candidates_limited(max_count)
    }

    fn change_page(&self, backward: bool) -> ImeState {
        let inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            return inline.overlay(self.inner.state());
        }
        self.inner.change_page(backward)
    }

    fn reset(&self) -> ImeState {
        let mut inline = lock_ignore_poison(&self.inline);
        inline.clear_composition();
        self.inner.reset()
    }

    fn clear_composition(&self) -> ImeState {
        let mut inline = lock_ignore_poison(&self.inline);
        inline.clear_composition();
        self.inner.clear_composition()
    }

    fn commit_composition(&self) -> ImeState {
        let mut inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            let committed = inline.take_commit();
            let mut state = self.inner.state();
            state.committed = committed;
            return state;
        }
        self.inner.commit_composition()
    }

    fn raw_input(&self) -> Option<String> {
        let inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            return Some(inline.buffer.clone());
        }
        self.inner.raw_input()
    }

    fn commit_raw_input(&self) -> ImeState {
        let mut inline = lock_ignore_poison(&self.inline);
        if inline.is_active() {
            let committed = inline.take_commit();
            let mut state = self.inner.state();
            state.committed = committed;
            return state;
        }
        self.inner.commit_raw_input()
    }

    fn list_schemas(&self) -> Vec<SchemaInfo> {
        self.inner.list_schemas()
    }

    fn active_switches(&self) -> Vec<SchemaSwitchSetting> {
        self.inner.active_switches()
    }

    fn persist_switch_options(&self, options: &[(String, bool)]) {
        self.inner.persist_switch_options(options);
    }

    fn apply_switch_request(
        &self,
        request: &SchemaSwitchRequest,
        reset_only: bool,
    ) -> Result<(), String> {
        self.inner.apply_switch_request(request, reset_only)
    }

    fn apply_reset_switch_request(&self, root: &Path) -> Result<(), String> {
        self.inner.apply_reset_switch_request(root)
    }

    fn is_ascii_mode(&self) -> bool {
        self.inner.is_ascii_mode()
    }

    fn is_composing(&self) -> bool {
        lock_ignore_poison(&self.inline).is_active() || self.inner.is_composing()
    }

    fn supports_native_inline_ascii(&self) -> bool {
        true
    }

    fn capabilities(&self) -> EngineCapabilities {
        self.inner.capabilities()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SUPER};

    #[derive(Default)]
    struct FakeInnerState {
        ascii_mode: bool,
        preedit: String,
        candidates: Vec<Candidate>,
        solo_shift: Option<u32>,
        received_keys: Vec<(u32, u32)>,
        /// Simulates a backend (like libpinyin) that rejects unknown symbols
        /// instead of committing them literally.
        reject_unknown_printable: bool,
    }

    struct FakeBackend {
        native_inline_ascii: bool,
        state: Mutex<FakeInnerState>,
    }

    impl FakeBackend {
        fn new(native_inline_ascii: bool) -> Self {
            Self {
                native_inline_ascii,
                state: Mutex::new(FakeInnerState::default()),
            }
        }

        fn with_rejected_unknowns() -> Self {
            let backend = Self::new(false);
            backend.state.lock().unwrap().reject_unknown_printable = true;
            backend
        }

        fn snapshot(inner: &FakeInnerState, committed: Option<String>) -> ImeState {
            let caret = inner.preedit.len();
            ImeState {
                preedit: inner.preedit.clone(),
                commit_preview: inner
                    .candidates
                    .first()
                    .map(|c| c.text.clone())
                    .unwrap_or_default(),
                cursor: caret,
                sel_start: caret,
                sel_end: caret,
                candidates: inner.candidates.clone(),
                highlighted_candidate_index: 0,
                page_size: 5,
                page: 0,
                is_last_page: true,
                committed,
                select_keys: (!inner.candidates.is_empty()).then(|| "12345".into()),
                ascii_mode: inner.ascii_mode,
                switch_hint: None,
                chord_composer: false,
                schema_shortcuts: Vec::new(),
                modifier_taps: Vec::new(),
                schema_id: "fake".into(),
                schema_name: "Fake".into(),
            }
        }
    }

    impl EnglishBackend for FakeBackend {
        fn current_schema(&self) -> Option<SchemaInfo> {
            Some(SchemaInfo {
                id: "fake".into(),
                name: "Fake".into(),
            })
        }

        fn schema_switches(&self) -> Vec<SchemaSwitch> {
            Vec::new()
        }

        fn get_option(&self, name: &str) -> bool {
            name == "ascii_mode" && self.state.lock().unwrap().ascii_mode
        }

        fn set_option(&self, name: &str, enabled: bool) -> ImeState {
            if name == "ascii_mode" {
                self.set_ascii_mode(enabled)
            } else {
                self.state()
            }
        }

        fn select_schema(&self, _id: &str) -> Result<ImeState, String> {
            Ok(self.state())
        }

        fn discard_composition(&self) {
            let mut inner = self.state.lock().unwrap();
            inner.preedit.clear();
            inner.candidates.clear();
            inner.solo_shift = None;
        }

        fn state(&self) -> ImeState {
            let inner = self.state.lock().unwrap();
            Self::snapshot(&inner, None)
        }

        fn apply_ascii_mode(&self, enabled: bool) {
            self.state.lock().unwrap().ascii_mode = enabled;
        }

        fn set_ascii_mode(&self, enabled: bool) -> ImeState {
            let mut inner = self.state.lock().unwrap();
            inner.ascii_mode = enabled;
            Self::snapshot(&inner, None)
        }
    }

    impl ImeBackend for FakeBackend {
        fn input_state(&self) -> InputState {
            InputState {
                schema_id: "fake".into(),
                ascii_mode: self.state.lock().unwrap().ascii_mode,
                ..Default::default()
            }
        }

        fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult {
            let mut inner = self.state.lock().unwrap();
            inner.received_keys.push((keycode, mask));
            if matches!(keycode, XK_SHIFT_L | XK_SHIFT_R) {
                if mask & RIME_RELEASE_MASK == 0 {
                    inner.solo_shift = Some(keycode);
                    return KeyProcessResult {
                        state: Self::snapshot(&inner, None),
                        accepted: false,
                    };
                }
                let toggled = inner.solo_shift.take() == Some(keycode);
                if toggled {
                    inner.ascii_mode = !inner.ascii_mode;
                }
                return KeyProcessResult {
                    state: Self::snapshot(&inner, None),
                    accepted: toggled,
                };
            }
            if mask & RIME_RELEASE_MASK == 0 {
                inner.solo_shift = None;
            }
            if keycode == XK_CAPS_LOCK {
                inner.ascii_mode = mask & RIME_MOD_LOCK == 0;
                return KeyProcessResult {
                    state: Self::snapshot(&inner, None),
                    accepted: false,
                };
            }
            if inner.ascii_mode {
                return KeyProcessResult {
                    state: Self::snapshot(&inner, None),
                    accepted: false,
                };
            }
            match keycode {
                k if k == u32::from(b'a') => {
                    inner.preedit.push('a');
                    inner.candidates = vec![Candidate {
                        text: "甲".into(),
                        comment: None,
                    }];
                    KeyProcessResult {
                        state: Self::snapshot(&inner, None),
                        accepted: true,
                    }
                }
                k if k == u32::from(b',') => KeyProcessResult {
                    state: Self::snapshot(&inner, Some("，".into())),
                    accepted: true,
                },
                k if k == u32::from(b';') => {
                    inner.preedit = ";".into();
                    inner.candidates = vec![
                        Candidate {
                            text: "；".into(),
                            comment: None,
                        },
                        Candidate {
                            text: "：".into(),
                            comment: None,
                        },
                    ];
                    KeyProcessResult {
                        state: Self::snapshot(&inner, None),
                        accepted: true,
                    }
                }
                k if (u32::from(b'A')..=u32::from(b'Z')).contains(&k) => {
                    let ch = char::from(k as u8);
                    let mut committed = String::new();
                    if let Some(first) = inner.candidates.first() {
                        committed.push_str(&first.text);
                    }
                    committed.push(ch);
                    inner.preedit.clear();
                    inner.candidates.clear();
                    KeyProcessResult {
                        state: Self::snapshot(&inner, Some(committed)),
                        accepted: true,
                    }
                }
                k if (0x21..=0x7e).contains(&k) => {
                    if inner.reject_unknown_printable {
                        return KeyProcessResult {
                            state: Self::snapshot(&inner, None),
                            accepted: false,
                        };
                    }
                    KeyProcessResult {
                        state: Self::snapshot(&inner, Some(char::from(k as u8).to_string())),
                        accepted: true,
                    }
                }
                _ => KeyProcessResult {
                    state: Self::snapshot(&inner, None),
                    accepted: false,
                },
            }
        }

        fn process_enter(&self) -> KeyProcessResult {
            self.process_key_result(crate::key_policy::XK_RETURN, 0)
        }

        fn select_candidate_on_page(&self, _index: usize) -> ImeState {
            self.state()
        }

        fn select_candidate_global(&self, _index: usize) -> ImeState {
            self.state()
        }

        fn highlight_candidate_on_page(&self, _index: usize) -> ImeState {
            self.state()
        }

        fn candidate_is_user_phrase_on_page(&self, _index: usize) -> bool {
            false
        }

        fn delete_candidate_on_page_result(&self, _index: usize) -> (ImeState, bool) {
            (self.state(), false)
        }

        fn all_candidates_limited(&self, _max_count: usize) -> Vec<Candidate> {
            self.state.lock().unwrap().candidates.clone()
        }

        fn change_page(&self, _backward: bool) -> ImeState {
            self.state()
        }

        fn reset(&self) -> ImeState {
            self.clear_composition()
        }

        fn clear_composition(&self) -> ImeState {
            self.discard_composition();
            self.state()
        }

        fn commit_composition(&self) -> ImeState {
            self.state()
        }

        fn raw_input(&self) -> Option<String> {
            let inner = self.state.lock().unwrap();
            (!inner.preedit.is_empty()).then(|| inner.preedit.clone())
        }

        fn commit_raw_input(&self) -> ImeState {
            self.state()
        }

        fn list_schemas(&self) -> Vec<SchemaInfo> {
            self.current_schema().into_iter().collect()
        }

        fn active_switches(&self) -> Vec<SchemaSwitchSetting> {
            Vec::new()
        }

        fn persist_switch_options(&self, _options: &[(String, bool)]) {}

        fn apply_switch_request(
            &self,
            _request: &SchemaSwitchRequest,
            _reset_only: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn apply_reset_switch_request(&self, _root: &Path) -> Result<(), String> {
            Ok(())
        }

        fn is_ascii_mode(&self) -> bool {
            self.state.lock().unwrap().ascii_mode
        }

        fn is_composing(&self) -> bool {
            !self.state.lock().unwrap().candidates.is_empty()
        }

        fn supports_native_inline_ascii(&self) -> bool {
            self.native_inline_ascii
        }

        fn capabilities(&self) -> EngineCapabilities {
            EngineCapabilities::none()
        }
    }

    #[test]
    fn native_inline_ascii_backends_pass_through_unwrapped() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(true)));
        let res = backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        // FakeBackend with native_inline_ascii = true receives 'A' directly and
        // commits it rather than entering the decorator's inline ASCII buffer.
        assert_eq!(res.state.committed.as_deref(), Some("A"));
        assert!(res.state.preedit.is_empty());
        assert!(!backend.is_composing());
    }

    #[test]
    fn desktop_shift_uppercase_composes_without_spurious_shift_toggle_and_commits_on_enter() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(false)));

        // Shift down -> 'A' down -> Shift up -> 'b' down -> Enter
        assert!(!backend.process_key_result(XK_SHIFT_L, 0).accepted);
        let started = backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        assert!(started.accepted);
        assert_eq!(started.state.preedit, "A");
        assert!(started.state.commit_preview.is_empty());
        assert!(started.state.candidates.is_empty());
        assert!(backend.is_composing());
        assert_eq!(backend.raw_input().as_deref(), Some("A"));

        let release = backend.process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK);
        assert!(!release.accepted);
        assert!(!release.state.ascii_mode);
        assert_eq!(release.state.preedit, "A");

        let cont = backend.process_key_result(b'b'.into(), 0);
        assert!(cont.accepted);
        assert_eq!(cont.state.preedit, "Ab");
        assert_eq!(cont.state.cursor, 2);

        let committed = backend.process_enter();
        assert!(committed.accepted);
        assert_eq!(committed.state.committed.as_deref(), Some("Ab"));
        assert!(committed.state.preedit.is_empty());
        assert!(!backend.is_composing());

        // Chinese typing resumes immediately afterward.
        let zh = backend.process_key_result(b'a'.into(), 0);
        assert!(zh.accepted);
        assert_eq!(zh.state.candidates[0].text, "甲");
    }

    #[test]
    fn mobile_soft_keyboard_uppercase_and_space_commit_match_desktop() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(false)));

        // Mobile soft keyboards emit uppercase keysym with mask = 0.
        let started = backend.process_key_result(b'K'.into(), 0);
        assert!(started.accepted);
        assert_eq!(started.state.preedit, "K");

        for &ch in b"eyTao-1.2_a+'" {
            let mask = if ch.is_ascii_uppercase() {
                RIME_MOD_SHIFT
            } else {
                0
            };
            assert!(backend.process_key_result(ch.into(), mask).accepted);
        }
        assert_eq!(backend.state().preedit, "KeyTao-1.2_a+'");

        let committed = backend.process_key_result(crate::key_policy::XK_SPACE, 0);
        assert!(committed.accepted);
        assert_eq!(committed.state.committed.as_deref(), Some("KeyTao-1.2_a+'"));
        assert!(!backend.is_composing());
    }

    #[test]
    fn camel_case_shift_mid_composition_does_not_toggle_ascii_mode() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(false)));

        backend.process_key_result(XK_SHIFT_L, 0);
        backend.process_key_result(b'K'.into(), RIME_MOD_SHIFT);
        backend.process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK);
        backend.process_key_result(b'e'.into(), 0);
        backend.process_key_result(b'y'.into(), 0);

        // Hold Shift for 'T' mid-composition.
        backend.process_key_result(XK_SHIFT_L, 0);
        backend.process_key_result(b'T'.into(), RIME_MOD_SHIFT);
        let rel = backend.process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK);
        assert!(!rel.accepted);
        assert!(!rel.state.ascii_mode);
        assert_eq!(rel.state.preedit, "KeyT");

        backend.process_key_result(b'a'.into(), 0);
        backend.process_key_result(b'o'.into(), 0);
        assert_eq!(backend.commit_composition().committed.as_deref(), Some("KeyTao"));
    }

    #[test]
    fn cursor_navigation_backspace_delete_and_escape_work_inside_inline_ascii() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(false)));

        backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        backend.process_key_result(b'b'.into(), 0);
        backend.process_key_result(b'd'.into(), 0);
        assert_eq!(backend.state().cursor, 3);

        // Move left once, insert 'c' -> "Abcd" with cursor at 3.
        assert!(backend.process_key_result(XK_LEFT, 0).accepted);
        assert_eq!(backend.state().cursor, 2);
        backend.process_key_result(b'c'.into(), 0);
        assert_eq!(backend.state().preedit, "Abcd");
        assert_eq!(backend.state().cursor, 3);

        // Home + Delete removes 'A' -> "bcd" with cursor at 0.
        assert!(backend.process_key_result(XK_HOME, 0).accepted);
        assert_eq!(backend.state().cursor, 0);
        assert!(backend.process_key_result(XK_DELETE, 0).accepted);
        assert_eq!(backend.state().preedit, "bcd");
        assert_eq!(backend.state().cursor, 0);

        // End + BackSpace deletes from end down to empty and exits inline ASCII.
        assert!(backend.process_key_result(XK_END, 0).accepted);
        assert_eq!(backend.state().cursor, 3);
        for expected in ["bc", "b", ""] {
            assert!(backend.process_key_result(XK_BACK_SPACE, 0).accepted);
            assert_eq!(backend.state().preedit, expected);
        }
        assert!(!backend.is_composing());

        // Shift+A -> Escape while still holding Shift -> Shift release must NOT toggle mode.
        backend.process_key_result(XK_SHIFT_L, 0);
        backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        let esc = backend.process_key_result(XK_ESCAPE, RIME_MOD_SHIFT);
        assert!(esc.accepted);
        assert!(esc.state.committed.is_none());
        assert!(esc.state.preedit.is_empty());
        let rel = backend.process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK);
        assert!(!rel.accepted);
        assert!(!rel.state.ascii_mode);
    }

    #[test]
    fn punctuation_commits_inline_ascii_and_delegates_atomically() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(false)));

        backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        backend.process_key_result(b'b'.into(), 0);
        let comma = backend.process_key_result(b','.into(), 0);
        assert!(comma.accepted);
        assert_eq!(comma.state.committed.as_deref(), Some("Ab，"));
        assert!(comma.state.preedit.is_empty());
        assert!(!backend.is_composing());

        // Delimiter opening a symbol candidate window preserves both commit and candidates.
        backend.process_key_result(b'G'.into(), RIME_MOD_SHIFT);
        backend.process_key_result(b'o'.into(), 0);
        let semi = backend.process_key_result(b';'.into(), 0);
        assert!(semi.accepted);
        assert_eq!(semi.state.committed.as_deref(), Some("Go"));
        assert_eq!(semi.state.preedit, ";");
        assert_eq!(semi.state.candidates[0].text, "；");
        assert!(backend.is_composing());
    }

    #[test]
    fn rejected_unknown_printable_still_commits_inline_buffer_for_host_fallback() {
        let backend = wrap_if_needed(Box::new(FakeBackend::with_rejected_unknowns()));

        backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        backend.process_key_result(b'b'.into(), 0);
        let at = backend.process_key_result(b'@'.into(), RIME_MOD_SHIFT);
        assert!(!at.accepted);
        assert_eq!(at.state.committed.as_deref(), Some("Ab"));
        assert!(!backend.is_composing());
    }

    #[test]
    fn solo_shift_and_caps_lock_commit_inline_ascii_and_preserve_existing_rules() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(false)));

        // CapsLock uppercase never starts inline ASCII.
        let locked = backend.process_key_result(b'A'.into(), RIME_MOD_LOCK);
        assert_eq!(locked.state.committed.as_deref(), Some("A"));
        assert!(!backend.is_composing());

        // Solo Shift tap mid-inline-ASCII commits the buffer and switches to ASCII mode.
        backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        backend.process_key_result(b'b'.into(), 0);
        backend.process_key_result(XK_SHIFT_L, 0);
        let toggled = backend.process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK);
        assert!(toggled.accepted);
        assert!(toggled.state.ascii_mode);
        assert_eq!(toggled.state.committed.as_deref(), Some("Ab"));
        assert!(!backend.is_composing());

        // Switch back to Chinese mode.
        backend.process_key_result(XK_SHIFT_L, 0);
        let zh = backend.process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK);
        assert!(!zh.state.ascii_mode);

        // CapsLock mid-inline-ASCII commits buffer and toggles ascii_mode.
        backend.process_key_result(b'C'.into(), RIME_MOD_SHIFT);
        backend.process_key_result(b'd'.into(), 0);
        let caps = backend.process_key_result(XK_CAPS_LOCK, 0);
        assert!(!caps.accepted);
        assert!(caps.state.ascii_mode);
        assert_eq!(caps.state.committed.as_deref(), Some("Cd"));
        assert!(!backend.is_composing());
    }

    #[test]
    fn active_chinese_composition_shortcuts_and_buffer_limit_are_respected() {
        let backend = wrap_if_needed(Box::new(FakeBackend::new(false)));

        // Active Chinese candidates: Shift+B delegates to inner backend ("甲B").
        backend.process_key_result(b'a'.into(), 0);
        let topped = backend.process_key_result(b'B'.into(), RIME_MOD_SHIFT);
        assert_eq!(topped.state.committed.as_deref(), Some("甲B"));
        assert!(!backend.is_composing());

        // Shortcuts pass through without disturbing inline ASCII buffer.
        backend.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        for mods in [RIME_MOD_CONTROL, RIME_MOD_ALT, RIME_MOD_SUPER] {
            let shortcut = backend.process_key_result(b'c'.into(), mods);
            assert!(!shortcut.accepted);
            assert_eq!(shortcut.state.preedit, "A");
        }

        // Buffer enforces MAX_INLINE_ASCII_BYTES without panicking or leaking.
        for _ in 0..(MAX_INLINE_ASCII_BYTES + 20) {
            assert!(backend.process_key_result(b'x'.into(), 0).accepted);
        }
        assert_eq!(backend.state().preedit.len(), MAX_INLINE_ASCII_BYTES);
        assert_eq!(
            backend.commit_raw_input().committed.unwrap().len(),
            MAX_INLINE_ASCII_BYTES
        );
        assert!(!backend.is_composing());
    }

    #[test]
    fn lifecycle_mode_switches_and_session_isolation_clear_or_commit_cleanly() {
        let a = wrap_if_needed(Box::new(FakeBackend::new(false)));
        let b = wrap_if_needed(Box::new(FakeBackend::new(false)));

        a.process_key_result(b'A'.into(), RIME_MOD_SHIFT);
        a.process_key_result(b'p'.into(), 0);
        assert_eq!(a.state().preedit, "Ap");
        assert!(b.state().preedit.is_empty());

        // set_ascii_mode(true) commits the active inline ASCII buffer.
        let switched = a.set_ascii_mode(true);
        assert!(switched.ascii_mode);
        assert_eq!(switched.committed.as_deref(), Some("Ap"));
        assert!(!a.is_composing());
        a.set_ascii_mode(false);

        // set_option("ascii_mode", true) commits as well.
        a.process_key_result(b'B'.into(), RIME_MOD_SHIFT);
        let opt = a.set_option("ascii_mode", true);
        assert!(opt.ascii_mode);
        assert_eq!(opt.committed.as_deref(), Some("B"));
        a.set_option("ascii_mode", false);

        // apply_ascii_mode(true), reset, clear_composition, discard_composition, and select_schema drop preedit.
        a.process_key_result(b'C'.into(), RIME_MOD_SHIFT);
        a.apply_ascii_mode(true);
        assert!(!a.is_composing());
        a.apply_ascii_mode(false);

        a.process_key_result(b'D'.into(), RIME_MOD_SHIFT);
        assert!(a.reset().preedit.is_empty());
        assert!(!a.is_composing());

        a.process_key_result(b'E'.into(), RIME_MOD_SHIFT);
        assert!(a.clear_composition().preedit.is_empty());
        assert!(!a.is_composing());

        a.process_key_result(b'F'.into(), RIME_MOD_SHIFT);
        a.discard_composition();
        assert!(!a.is_composing());

        a.process_key_result(b'G'.into(), RIME_MOD_SHIFT);
        assert!(a.select_schema("fake").unwrap().preedit.is_empty());
        assert!(!a.is_composing());
    }
}
