//! ITfKeyEventSink + ITfCompositionSink — the hot path for all keystrokes.
//!
//! Key handling mirrors keytao-linux-ime/src/wayland_backend.rs:
//!   VK → X11 keysym → ImeRuntimeSession::process_key_result() → ImeState
//!   committed text  → write via TSF edit session (ITfInsertAtSelection)
//!   preedit text    → manage ITfComposition
//!   candidate list  → update CandidateWindow (same tiny-skia panel as Linux)

use std::{cell::UnsafeCell, sync::Arc};

use keytao_core::ImeState;
use windows::{
    core::{implement, Interface, Result, GUID},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, LPARAM, RECT, WPARAM},
        UI::TextServices::*,
    },
};

use crate::{
    key_map::{
        candidate_index_for_select_key, current_mod_mask, is_enter_vk, is_shift_vk,
        shift_keysym_for_vk, should_bypass_empty_composition, should_eat_key, vk_to_keysym,
        RIME_RELEASE_MASK,
    },
    state::SharedState,
};

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

/// Get caret screen position from the current context view + selection.
fn caret_screen_pos(ec: u32, context: &ITfContext) -> (i32, i32) {
    unsafe {
        let view = context.GetActiveView().ok();
        let view = match view {
            Some(v) => v,
            None => return (100, 100),
        };

        // Get default selection range
        let mut selections = [TF_SELECTION::default()];
        let mut count: u32 = 0;
        // TF_DEFAULT_SELECTION = 0xFFFFFFFF
        if context
            .GetSelection(ec, 0xFFFFFFFF, &mut selections, &mut count)
            .is_err()
        {
            return (100, 100);
        }
        let range = match selections[0].range.as_ref() {
            Some(r) => r.clone(),
            None => return (100, 100),
        };

        let mut rect = RECT::default();
        let mut clipped = BOOL::default();
        if view
            .GetTextExt(ec, &range, &mut rect, &mut clipped)
            .is_err()
        {
            return (100, 100);
        }
        (rect.left, rect.bottom)
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
    let ime_state_clone = ime_state.clone();
    let comp_sink_obj = CompositionSink {
        state: Arc::clone(shared_state),
    };
    let comp_sink_iface: ITfCompositionSink = comp_sink_obj.into();

    with_write_session(context, client_id, move |ec, ctx| {
        let mut st = state_arc.lock().unwrap();

        let committed = ime_state_clone
            .committed
            .as_deref()
            .filter(|text| !text.is_empty());
        let has_commit = committed.is_some();
        if let Some(committed) = committed {
            if let Some(comp) = st.composition.take() {
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
            if let Some(comp) = &st.composition {
                update_composition_text(ec, comp, &ime_state_clone.preedit)?;
            } else {
                let comp = start_composition(ec, ctx, &ime_state_clone.preedit, &comp_sink_iface)?;
                st.composition = Some(comp);
            }
        } else if st.composition.is_some() && !has_commit {
            if let Some(comp) = st.composition.take() {
                end_composition(ec, &comp, None)?;
            }
        }

        let (cx, cy) = caret_screen_pos(ec, ctx);
        let mode_changed = ime_state_clone.ascii_mode != st.ascii_mode;
        st.ascii_mode = ime_state_clone.ascii_mode;
        st.ime_state = Some(ime_state_clone.clone());

        let show = !ime_state_clone.candidates.is_empty() || !ime_state_clone.preedit.is_empty();
        if show {
            st.candidate_win.show(&ime_state_clone, cx, cy);
        } else {
            st.candidate_win.hide();
        }
        if show_mode_hint_on_change && mode_changed {
            st.mode_hint_win
                .show_mode_hint(ime_state_clone.ascii_mode, cx, cy);
        }

        Ok(())
    })
}

fn clear_after_reload(
    context: &ITfContext,
    client_id: u32,
    shared_state: &SharedState,
) -> Result<()> {
    apply_ime_state(context, client_id, shared_state, ImeState::empty(), false)
}

// ── KeyEventSink + CompositionSink (one COM object, shared state) ─────────────

#[implement(ITfKeyEventSink)]
pub(crate) struct KeyEventSink {
    pub(crate) state: SharedState,
}

impl ITfKeyEventSink_Impl for KeyEventSink_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.check_reload_stamp() {
            st.ime_state = None;
            st.candidate_win.hide();
            st.mode_hint_win.hide();
        }
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
        let mut st = self.state.lock().unwrap();
        if is_shift_vk(vk) {
            st.shift_pressed_without_key = true;
            return Ok(BOOL::from(false));
        }
        if st.shift_pressed_without_key {
            st.shift_pressed_without_key = false;
        }
        let state = st.ime_state.clone().or_else(|| st.current_state());
        let is_composing = state
            .as_ref()
            .map(|s| !s.preedit.is_empty() || !s.candidates.is_empty())
            .unwrap_or(false);
        Ok(BOOL::from(should_eat_key(vk, is_composing, mods)))
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

        let client_id = {
            let mut st = self.state.lock().unwrap();
            st.shift_pressed_without_key = false;
            if !st.ensure_engine() {
                return Ok(BOOL::from(false));
            }
            let _ = st.check_reload_stamp();
            let should_clear_reload = st.take_reload_clear_pending();
            let client_id = st.client_id;
            drop(st);
            if should_clear_reload {
                clear_after_reload(&context, client_id, &self.state)?;
            }
            client_id
        };

        let before_state = self
            .state
            .lock()
            .unwrap()
            .current_state()
            .unwrap_or_else(ImeState::empty);

        if should_bypass_empty_composition(vk, mods, &before_state) {
            self.state.lock().unwrap().candidate_win.hide();
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
            let _ = self.state.lock().unwrap().reset_session();
            apply_ime_state(&context, client_id, &self.state, commit_state, true)?;
            return Ok(BOOL::from(true));
        }

        if let Some(index) = candidate_index_for_select_key(vk, &before_state) {
            let ime_state = self.state.lock().unwrap().select_candidate(index);
            if let Some(ime_state) = ime_state {
                apply_ime_state(&context, client_id, &self.state, ime_state, true)?;
                return Ok(BOOL::from(true));
            }
        }

        let keysym = match vk_to_keysym(vk, mods) {
            Some(k) => k,
            None => return Ok(BOOL::from(false)),
        };

        // Call the shared runtime session. Use Rime's accepted flag as the
        // source of truth, matching the Linux frontends.
        let result = {
            let st = self.state.lock().unwrap();
            st.process_key_result(keysym, mods)
        };
        let result = match result {
            Some(r) => r,
            None => return Ok(BOOL::from(false)),
        };
        let ime_state = result.state;

        let consumed = result.accepted;

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
        let client_id = {
            let mut st = self.state.lock().unwrap();
            if !st.shift_pressed_without_key {
                return Ok(BOOL::from(false));
            }
            st.shift_pressed_without_key = false;
            if !st.ensure_engine() {
                return Ok(BOOL::from(false));
            }
            let _ = st.check_reload_stamp();
            let should_clear_reload = st.take_reload_clear_pending();
            let client_id = st.client_id;
            drop(st);
            if should_clear_reload {
                clear_after_reload(&context, client_id, &self.state)?;
            }
            client_id
        };

        let result = {
            let st = self.state.lock().unwrap();
            st.process_key_result(keysym, RIME_RELEASE_MASK)
        };
        let Some(result) = result else {
            return Ok(BOOL::from(false));
        };
        if result.accepted {
            apply_ime_state(&context, client_id, &self.state, result.state, true)?;
        }
        Ok(BOOL::from(result.accepted))
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
        let mut st = self.state.lock().unwrap();
        st.composition = None;
        st.ime_state = None;
        st.candidate_win.hide();
        st.mode_hint_win.hide();
        // Reset the runtime session so the next keypress starts fresh.
        let _ = st.reset_session();
        Ok(())
    }
}
