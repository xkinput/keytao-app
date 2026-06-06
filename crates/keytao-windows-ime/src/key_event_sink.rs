//! ITfKeyEventSink + ITfCompositionSink — the hot path for all keystrokes.
//!
//! Key handling mirrors keytao-linux-ime/src/wayland_backend.rs:
//!   VK → X11 keysym → engine.process_key() → ImeState
//!   committed text  → write via TSF edit session (ITfInsertAtSelection)
//!   preedit text    → manage ITfComposition
//!   candidate list  → update CandidateWindow (same tiny-skia panel as Linux)

use std::{cell::UnsafeCell, sync::Arc};

use windows::{
    core::{implement, Interface, Result, GUID},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, LPARAM, RECT, WPARAM},
        UI::TextServices::*,
    },
};

use crate::{
    key_map::{current_mod_mask, should_eat_key, vk_to_keysym},
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

// ── KeyEventSink + CompositionSink (one COM object, shared state) ─────────────

#[implement(ITfKeyEventSink)]
pub(crate) struct KeyEventSink {
    pub(crate) state: SharedState,
}

impl ITfKeyEventSink_Impl for KeyEventSink_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
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
        let is_composing = self
            .state
            .lock()
            .unwrap()
            .ime_state
            .as_ref()
            .map(|s| !s.preedit.is_empty() || !s.candidates.is_empty())
            .unwrap_or(false);
        Ok(BOOL::from(should_eat_key(vk, is_composing, mods)))
    }

    fn OnTestKeyUp(
        &self,
        _pic: Option<&ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        Ok(BOOL::from(false))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let context = pic.ok_or(windows::core::Error::from(E_INVALIDARG))?.clone();
        let vk = (wparam.0 & 0xFFFF) as u16;
        let mods = current_mod_mask();

        let keysym = match vk_to_keysym(vk) {
            Some(k) => k,
            None => return Ok(BOOL::from(false)),
        };

        // Call librime engine. Use Rime's accepted flag as the source of truth,
        // matching the Linux frontends.
        let result = {
            let st = self.state.lock().unwrap();
            st.engine().map(|e| e.process_key_result(keysym, mods))
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

        // Write committed / preedit to the document via edit session
        let client_id = self.state.lock().unwrap().client_id;
        let state_arc = Arc::clone(&self.state);
        let ime_state_clone = ime_state.clone();

        // Obtain ITfCompositionSink interface pointer for StartComposition
        // We need to pass it to ctx_comp.StartComposition(ec, range, sink).
        // The sink is *this* COM object (KeyEventSink also implements ITfCompositionSink).
        // We create a throw-away CompositionSink object that shares state.
        let comp_sink_obj = CompositionSink {
            state: Arc::clone(&self.state),
        };
        let comp_sink_iface: ITfCompositionSink = comp_sink_obj.into();

        with_write_session(&context, client_id, move |ec, ctx| {
            let mut st = state_arc.lock().unwrap();

            if let Some(committed) = &ime_state_clone.committed {
                // End existing composition and commit text
                if let Some(comp) = st.composition.take() {
                    end_composition(ec, &comp, Some(committed))?;
                } else {
                    // No composition — insert text directly at selection
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
                    let comp =
                        start_composition(ec, ctx, &ime_state_clone.preedit, &comp_sink_iface)?;
                    st.composition = Some(comp);
                }
            } else if st.composition.is_some() && ime_state_clone.committed.is_none() {
                // Preedit cleared without commit (e.g. Escape)
                if let Some(comp) = st.composition.take() {
                    end_composition(ec, &comp, None)?;
                }
            }

            // Update caret position for candidate window
            let (cx, cy) = caret_screen_pos(ec, ctx);
            st.ime_state = Some(ime_state_clone.clone());

            let show =
                !ime_state_clone.candidates.is_empty() || !ime_state_clone.preedit.is_empty();
            if show {
                st.candidate_win.show(&ime_state_clone, cx, cy);
            } else {
                st.candidate_win.hide();
            }

            Ok(())
        })?;

        Ok(BOOL::from(consumed))
    }

    fn OnKeyUp(&self, _pic: Option<&ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL::from(false))
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
        // Reset librime so next keypress starts fresh
        if let Some(e) = &st.engine {
            e.reset();
        }
        Ok(())
    }
}
