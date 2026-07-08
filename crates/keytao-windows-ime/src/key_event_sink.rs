//! ITfKeyEventSink + ITfCompositionSink — the hot path for all keystrokes.
//!
//! Key handling mirrors keytao-linux-ime/src/wayland_backend.rs:
//!   VK → X11 keysym → ImeRuntimeSession::process_key_result() → ImeState
//!   committed text  → write via TSF edit session (ITfInsertAtSelection)
//!   preedit text    → manage ITfComposition
//!   candidate list  → update CandidateWindow (same tiny-skia panel as Linux)

use std::{
    cell::UnsafeCell,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use keytao_core::{ImeRuntimeSession, ImeState, KeyProcessResult};
use windows::{
    core::{implement, Interface, Result, GUID},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, HWND, LPARAM, RECT, WPARAM},
        UI::TextServices::*,
    },
};

use crate::{
    key_map::{
        candidate_index_for_select_key, current_mod_mask, is_enter_vk, is_shift_vk,
        shift_keysym_for_vk, should_bypass_empty_composition, should_eat_key, vk_to_keysym,
        RIME_RELEASE_MASK,
    },
    state::{
        append_diagnostic, fallback_focus_window, hide_candidate_window, hide_ime_windows,
        start_engine_warmup, start_reload_if_needed, update_ime_windows, SharedState,
    },
};

static KEY_DIAGNOSTIC_COUNT: AtomicUsize = AtomicUsize::new(0);

fn append_key_diagnostic(message: impl AsRef<str>) {
    if KEY_DIAGNOSTIC_COUNT.fetch_add(1, Ordering::Relaxed) < 96 {
        append_diagnostic(message);
    }
}

// ── Edit session helper ───────────────────────────────────────────────────────

type EditFn = Box<dyn FnOnce(u32, &ITfContext) -> Result<()>>;

#[implement(ITfEditSession)]
struct EditSession {
    context: ITfContext,
    f: UnsafeCell<Option<EditFn>>,
}

// SAFETY: STA — called on the same thread that created it.
unsafe impl Send for EditSession {}
unsafe impl Sync for EditSession {}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let f = unsafe { &mut *self.f.get() }.take();
        match f {
            Some(f) => f(ec, &self.context),
            None => Ok(()),
        }
    }
}

fn with_write_session(
    context: &ITfContext,
    client_id: u32,
    f: impl FnOnce(u32, &ITfContext) -> Result<()> + 'static,
) -> Result<()> {
    let session = EditSession {
        context: context.clone(),
        f: UnsafeCell::new(Some(Box::new(f))),
    };
    let iface: ITfEditSession = session.into();
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
    unsafe {
        let hr_session = context.RequestEditSession(client_id, &iface, flags)?;
        hr_session.ok()
    }
}

// ── Composition helpers ───────────────────────────────────────────────────────

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Start an ITfComposition at the current caret and set initial preedit text.
fn start_composition(
    ec: u32,
    context: &ITfContext,
    preedit: &str,
    comp_sink: &ITfCompositionSink,
) -> Result<ITfComposition> {
    unsafe {
        // Get insertion point (query only — don't insert yet)
        let ins: ITfInsertAtSelection = context.cast()?;
        let range = ins.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])?;

        let ctx_comp: ITfContextComposition = context.cast()?;
        let comp = ctx_comp.StartComposition(ec, &range, comp_sink)?;

        // Set preedit text into the composition range
        let wide = to_wide(preedit);
        let comp_range = comp.GetRange()?;
        comp_range.SetText(ec, 0, &wide)?;

        Ok(comp)
    }
}

/// Update preedit text on an existing composition.
fn update_composition_text(ec: u32, composition: &ITfComposition, preedit: &str) -> Result<()> {
    unsafe {
        let range = composition.GetRange()?;
        let wide = to_wide(preedit);
        range.SetText(ec, 0, &wide)?;
    }
    Ok(())
}

/// Commit (end) the composition, writing the final committed text.
fn end_composition(ec: u32, composition: &ITfComposition, committed: Option<&str>) -> Result<()> {
    unsafe {
        let range = composition.GetRange()?;
        let wide = committed.map(to_wide).unwrap_or_default();
        range.SetText(ec, 0, &wide)?;
        composition.EndComposition(ec)?;
    }
    Ok(())
}

struct CaretScreenInfo {
    x: i32,
    y: i32,
    owner_hwnd: HWND,
}

/// Get caret screen position and the owner HWND from the current context view.
fn caret_screen_info(ec: u32, context: &ITfContext) -> CaretScreenInfo {
    unsafe {
        let view = context.GetActiveView().ok();
        let view = match view {
            Some(v) => v,
            None => {
                return CaretScreenInfo {
                    x: 100,
                    y: 100,
                    owner_hwnd: fallback_focus_window(),
                }
            }
        };
        let owner_hwnd = view
            .GetWnd()
            .ok()
            .filter(|hwnd| !hwnd.0.is_null())
            .unwrap_or_else(fallback_focus_window);

        // Get default selection range
        let mut selections = [TF_SELECTION::default()];
        let mut count: u32 = 0;
        // TF_DEFAULT_SELECTION = 0xFFFFFFFF
        if context
            .GetSelection(ec, 0xFFFFFFFF, &mut selections, &mut count)
            .is_err()
        {
            return CaretScreenInfo {
                x: 100,
                y: 100,
                owner_hwnd,
            };
        }
        let range = match selections[0].range.as_ref() {
            Some(r) => r.clone(),
            None => {
                return CaretScreenInfo {
                    x: 100,
                    y: 100,
                    owner_hwnd,
                }
            }
        };

        let mut rect = RECT::default();
        let mut clipped = BOOL::default();
        if view
            .GetTextExt(ec, &range, &mut rect, &mut clipped)
            .is_err()
        {
            return CaretScreenInfo {
                x: 100,
                y: 100,
                owner_hwnd,
            };
        }
        CaretScreenInfo {
            x: rect.left,
            y: rect.bottom,
            owner_hwnd,
        }
    }
}

fn apply_ime_state(
    context: &ITfContext,
    client_id: u32,
    shared_state: &SharedState,
    ime_state: ImeState,
    show_mode_hint_on_change: bool,
) -> Result<()> {
    let state_arc = Arc::clone(shared_state);
    let state_arc_for_session = Arc::clone(&state_arc);
    let ime_state_clone = ime_state.clone();
    let window_update = Arc::new(Mutex::new(None));
    let window_update_for_session = Arc::clone(&window_update);
    let comp_sink_obj = CompositionSink {
        state: Arc::clone(shared_state),
    };
    let comp_sink_iface: ITfCompositionSink = comp_sink_obj.into();

    with_write_session(context, client_id, move |ec, ctx| {
        let committed = ime_state_clone
            .committed
            .as_deref()
            .filter(|text| !text.is_empty());
        let has_commit = committed.is_some();

        let mut composition = {
            let mut st = state_arc_for_session.lock().unwrap();
            st.composition.take()
        };

        if let Some(committed) = committed {
            if let Some(comp) = composition.take() {
                end_composition(ec, &comp, Some(committed))?;
            } else {
                unsafe {
                    let ins: ITfInsertAtSelection = ctx.cast()?;
                    let wide = to_wide(committed);
                    ins.InsertTextAtSelection(ec, TF_IAS_NOQUERY, &wide)?;
                }
            }
        }

        if !ime_state_clone.preedit.is_empty() {
            if let Some(comp) = &composition {
                update_composition_text(ec, comp, &ime_state_clone.preedit)?;
            } else {
                let comp = start_composition(ec, ctx, &ime_state_clone.preedit, &comp_sink_iface)?;
                composition = Some(comp);
            }
        } else if composition.is_some() && !has_commit {
            if let Some(comp) = composition.take() {
                end_composition(ec, &comp, None)?;
            }
        }

        let caret = caret_screen_info(ec, ctx);
        let mode_changed = {
            let mut st = state_arc_for_session.lock().unwrap();
            st.composition = composition;
            let mode_changed = ime_state_clone.ascii_mode != st.ascii_mode;
            st.ascii_mode = ime_state_clone.ascii_mode;
            st.ime_state = Some(ime_state_clone.clone());
            mode_changed
        };
        *window_update_for_session.lock().unwrap() = Some((
            ime_state_clone.clone(),
            caret.x,
            caret.y,
            caret.owner_hwnd,
            show_mode_hint_on_change && mode_changed,
        ));

        Ok(())
    })?;

    if let Some((ime_state, cx, cy, owner_hwnd, show_mode_hint)) =
        window_update.lock().unwrap().take()
    {
        update_ime_windows(&state_arc, &ime_state, cx, cy, owner_hwnd, show_mode_hint);
    }

    Ok(())
}

fn runtime_session(shared_state: &SharedState) -> Option<ImeRuntimeSession> {
    shared_state.lock().unwrap().session()
}

fn cached_ime_state(shared_state: &SharedState) -> Option<ImeState> {
    shared_state.lock().unwrap().ime_state.clone()
}

fn has_visible_state(state: &ImeState) -> bool {
    !state.preedit.is_empty() || !state.candidates.is_empty()
}

fn has_commit(state: &ImeState) -> bool {
    state
        .committed
        .as_deref()
        .map(|text| !text.is_empty())
        .unwrap_or(false)
}

fn should_consume_processed_state(accepted: bool, before: &ImeState, after: &ImeState) -> bool {
    accepted || has_visible_state(before) || has_visible_state(after) || has_commit(after)
}

fn process_key_result(
    shared_state: &SharedState,
    keysym: u32,
    mods: u32,
) -> Option<KeyProcessResult> {
    runtime_session(shared_state)?.process_key_result(keysym, mods)
}

fn select_candidate(shared_state: &SharedState, index: usize) -> Option<ImeState> {
    runtime_session(shared_state)?.select_candidate(index)
}

fn reset_session(shared_state: &SharedState) -> Option<ImeState> {
    runtime_session(shared_state)?.reset()
}

fn prepare_engine_for_key(context: &ITfContext, shared_state: &SharedState) -> Result<Option<u32>> {
    let (engine_ready, engine_building, engine_error) = {
        let st = shared_state.lock().unwrap();
        (
            st.engine_ready(),
            st.engine_building,
            st.engine_error.clone(),
        )
    };
    if !engine_ready {
        start_engine_warmup(shared_state);
        append_key_diagnostic(format!(
            "OnKeyDown engine not ready building={engine_building} error={engine_error:?}"
        ));
        return Ok(None);
    }

    let reload_started = start_reload_if_needed(shared_state);
    let (client_id, reload_in_progress, should_clear_reload) = {
        let mut st = shared_state.lock().unwrap();
        (
            st.client_id,
            st.reload_in_progress,
            st.take_reload_clear_pending(),
        )
    };

    if should_clear_reload {
        apply_ime_state(context, client_id, shared_state, ImeState::empty(), false)?;
    }

    if reload_started || reload_in_progress {
        return Ok(None);
    }

    Ok(Some(client_id))
}

// ── KeyEventSink + CompositionSink (one COM object, shared state) ─────────────

#[implement(ITfKeyEventSink)]
pub(crate) struct KeyEventSink {
    pub(crate) state: SharedState,
}

impl ITfKeyEventSink_Impl for KeyEventSink_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        self.state.lock().unwrap().shift_pressed_without_key = false;
        hide_ime_windows(&self.state);
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = (wparam.0 & 0xFFFF) as u16;
        let mods = current_mod_mask();
        if is_shift_vk(vk) {
            let mut st = self.state.lock().unwrap();
            st.shift_pressed_without_key = true;
            return Ok(BOOL::from(false));
        }
        let (engine_ready, reload_in_progress, state) = {
            let mut st = self.state.lock().unwrap();
            if st.shift_pressed_without_key {
                st.shift_pressed_without_key = false;
            }
            (
                st.engine_ready(),
                st.reload_in_progress,
                st.ime_state.clone(),
            )
        };
        if !engine_ready || reload_in_progress {
            let eat = should_eat_key(vk, false, mods);
            append_key_diagnostic(format!(
                "OnTestKeyDown vk=0x{vk:02x} mods=0x{mods:x} ready={engine_ready} reload={reload_in_progress} composing=false eat={eat}"
            ));
            return Ok(BOOL::from(eat));
        }
        let is_composing = state.as_ref().map(has_visible_state).unwrap_or(false);
        let eat = should_eat_key(vk, is_composing, mods);
        append_key_diagnostic(format!(
            "OnTestKeyDown vk=0x{vk:02x} mods=0x{mods:x} ready={engine_ready} reload={reload_in_progress} composing={is_composing} eat={eat}"
        ));
        Ok(BOOL::from(eat))
    }

    fn OnTestKeyUp(
        &self,
        _pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = (wparam.0 & 0xFFFF) as u16;
        if !is_shift_vk(vk) {
            return Ok(BOOL::from(false));
        }
        let st = self.state.lock().unwrap();
        Ok(BOOL::from(st.shift_pressed_without_key))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let context = pic.ok_or(windows::core::Error::from(E_INVALIDARG))?.clone();
        let vk = (wparam.0 & 0xFFFF) as u16;
        let mods = current_mod_mask();

        if is_shift_vk(vk) {
            return Ok(BOOL::from(false));
        }

        let client_id = match {
            let mut st = self.state.lock().unwrap();
            st.shift_pressed_without_key = false;
            drop(st);
            prepare_engine_for_key(&context, &self.state)?
        } {
            Some(client_id) => client_id,
            None => return Ok(BOOL::from(false)),
        };

        let before_state = cached_ime_state(&self.state).unwrap_or_else(ImeState::empty);

        if should_bypass_empty_composition(vk, mods, &before_state) {
            hide_candidate_window(&self.state);
            return Ok(BOOL::from(false));
        }

        if is_enter_vk(vk) && !before_state.preedit.is_empty() {
            let mut commit_state = before_state.clone();
            commit_state.committed = Some(before_state.preedit.clone());
            commit_state.preedit.clear();
            commit_state.cursor = 0;
            commit_state.candidates.clear();
            commit_state.highlighted_candidate_index = 0;
            commit_state.page = 0;
            commit_state.is_last_page = true;
            let _ = reset_session(&self.state);
            apply_ime_state(&context, client_id, &self.state, commit_state, true)?;
            return Ok(BOOL::from(true));
        }

        if let Some(index) = candidate_index_for_select_key(vk, &before_state) {
            let ime_state = select_candidate(&self.state, index);
            if let Some(ime_state) = ime_state {
                apply_ime_state(&context, client_id, &self.state, ime_state, true)?;
                return Ok(BOOL::from(true));
            }
        }

        let keysym = match vk_to_keysym(vk, mods) {
            Some(k) => k,
            None => {
                append_key_diagnostic(format!("OnKeyDown vk=0x{vk:02x} mods=0x{mods:x} no keysym"));
                return Ok(BOOL::from(false));
            }
        };

        let result = process_key_result(&self.state, keysym, mods);
        let result = match result {
            Some(r) => r,
            None => {
                append_key_diagnostic(format!(
                    "OnKeyDown vk=0x{vk:02x} keysym=0x{keysym:x} mods=0x{mods:x} no result"
                ));
                return Ok(BOOL::from(false));
            }
        };
        let ime_state = result.state;

        let consumed = should_consume_processed_state(result.accepted, &before_state, &ime_state);
        append_key_diagnostic(format!(
            "OnKeyDown vk=0x{vk:02x} keysym=0x{keysym:x} mods=0x{mods:x} accepted={} consumed={} preedit_len={} candidates={} commit={}",
            result.accepted,
            consumed,
            ime_state.preedit.chars().count(),
            ime_state.candidates.len(),
            has_commit(&ime_state),
        ));

        if !consumed {
            return Ok(BOOL::from(false));
        }

        apply_ime_state(&context, client_id, &self.state, ime_state, true)?;

        Ok(BOOL::from(consumed))
    }

    fn OnKeyUp(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let vk = (wparam.0 & 0xFFFF) as u16;
        let Some(keysym) = shift_keysym_for_vk(vk) else {
            return Ok(BOOL::from(false));
        };

        let context = pic.ok_or(windows::core::Error::from(E_INVALIDARG))?.clone();
        let client_id = match {
            let mut st = self.state.lock().unwrap();
            if !st.shift_pressed_without_key {
                return Ok(BOOL::from(false));
            }
            st.shift_pressed_without_key = false;
            drop(st);
            prepare_engine_for_key(&context, &self.state)?
        } {
            Some(client_id) => client_id,
            None => return Ok(BOOL::from(false)),
        };

        let before_state = cached_ime_state(&self.state).unwrap_or_else(ImeState::empty);
        let result = process_key_result(&self.state, keysym, RIME_RELEASE_MASK);
        let Some(result) = result else {
            return Ok(BOOL::from(false));
        };
        let consumed =
            should_consume_processed_state(result.accepted, &before_state, &result.state);
        if consumed {
            apply_ime_state(&context, client_id, &self.state, result.state, true)?;
        }
        Ok(BOOL::from(consumed))
    }

    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }
}

// ── ITfCompositionSink ────────────────────────────────────────────────────────

/// Separate COM object for ITfCompositionSink (passed to StartComposition).
/// TSF calls OnCompositionTerminated when the application externally ends
/// our composition (e.g. user clicks somewhere else).
#[implement(ITfCompositionSink)]
pub(crate) struct CompositionSink {
    pub(crate) state: SharedState,
}

impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Option<&ITfComposition>,
    ) -> Result<()> {
        {
            let mut st = self.state.lock().unwrap();
            st.composition = None;
            st.ime_state = None;
        }
        hide_ime_windows(&self.state);
        // Reset the runtime session so the next keypress starts fresh.
        let _ = reset_session(&self.state);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use keytao_core::{Candidate, ImeState};

    use super::should_consume_processed_state;

    fn empty_state() -> ImeState {
        ImeState::empty()
    }

    fn state_with_preedit(text: &str) -> ImeState {
        let mut state = ImeState::empty();
        state.preedit = text.to_string();
        state
    }

    fn state_with_candidate(text: &str) -> ImeState {
        let mut state = ImeState::empty();
        state.candidates.push(Candidate {
            text: text.to_string(),
            comment: None,
        });
        state
    }

    fn state_with_commit(text: &str) -> ImeState {
        let mut state = ImeState::empty();
        state.committed = Some(text.to_string());
        state
    }

    #[test]
    fn consumes_when_rime_accepts_key() {
        assert!(should_consume_processed_state(
            true,
            &empty_state(),
            &empty_state()
        ));
    }

    #[test]
    fn consumes_when_candidates_become_visible_even_if_rime_passes() {
        assert!(should_consume_processed_state(
            false,
            &empty_state(),
            &state_with_candidate("candidate")
        ));
    }

    #[test]
    fn consumes_when_existing_composition_is_cleared() {
        assert!(should_consume_processed_state(
            false,
            &state_with_preedit("ni"),
            &empty_state()
        ));
    }

    #[test]
    fn consumes_when_commit_is_available_even_if_rime_passes() {
        assert!(should_consume_processed_state(
            false,
            &empty_state(),
            &state_with_commit("commit")
        ));
    }

    #[test]
    fn passes_plain_key_when_rime_passes_without_ime_state() {
        assert!(!should_consume_processed_state(
            false,
            &empty_state(),
            &empty_state()
        ));
    }
}
