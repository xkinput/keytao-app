//! ITfKeyEventSink + ITfCompositionSink — the hot path for all keystrokes.
//!
//! Key handling mirrors keytao-linux-ime/src/wayland_backend.rs:
//!   VK → X11 keysym → ImeRuntimeSession::process_key_result() → ImeState
//!   committed text  → write via TSF edit session (ITfInsertAtSelection)
//!   preedit text    → manage ITfComposition
//!   candidate list  → update CandidateWindow (same tiny-skia panel as Linux)

use std::{cell::RefCell, rc::Rc};

#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicUsize, Ordering};

use keytao_core::{utf16_offset_from_chars, ImeRuntimeSession, ImeState, KeyProcessResult};
use windows::{
    core::{implement, Interface, Result, GUID, VARIANT},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        UI::TextServices::*,
        UI::WindowsAndMessaging::{GetGUIThreadInfo, GetWindowRect, GUITHREADINFO},
    },
};

use crate::{
    edit_session::{take_selection_range, with_read_session, with_write_session},
    globals::DllActivityGuard,
    key_map::{
        current_mod_mask, is_enter_vk, is_shift_vk, shift_keysym_for_vk,
        shift_pending_after_key_down, should_bypass_empty_composition, should_eat_key,
        vk_to_keysym, RIME_RELEASE_MASK,
    },
    state::{
        apply_pending_session_reset, clear_input_after_composition_terminated,
        clear_input_for_blocked_context, clear_layout_sink, fallback_focus_window,
        hide_candidate_window, input_is_blocked, layout_sink_context, poll_engine_builds,
        refresh_engine_for_focus, refresh_input_context, reset_input_for_focus_change,
        retry_input_context_if_unknown, start_engine_warmup, start_reload_if_needed,
        store_layout_sink, sync_input_policy, update_ime_windows, update_language_bar_mode,
        CaretPosition, LayoutSinkRegistration, SharedState, WeakState,
    },
};

#[cfg(debug_assertions)]
use crate::state::append_diagnostic;

#[cfg(debug_assertions)]
static KEY_DIAGNOSTIC_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(debug_assertions)]
macro_rules! append_key_diagnostic {
    ($($arg:tt)*) => {
        if KEY_DIAGNOSTIC_COUNT.fetch_add(1, Ordering::Relaxed) < 96 {
            append_diagnostic(format!($($arg)*));
        }
    };
}

#[cfg(not(debug_assertions))]
macro_rules! append_key_diagnostic {
    ($($arg:tt)*) => {
        if false {
            let _ = format_args!($($arg)*);
        }
    };
}

fn upgrade_state(state: &WeakState) -> Option<SharedState> {
    state.upgrade()
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
    cursor: usize,
    comp_sink: &ITfCompositionSink,
    display_attribute_atom: Option<u32>,
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
        set_composition_display_attribute(ec, context, &comp_range, display_attribute_atom)?;
        set_composition_cursor(ec, context, &comp_range, preedit, cursor)?;

        Ok(comp)
    }
}

/// Update preedit text on an existing composition.
fn update_composition_text(
    ec: u32,
    context: &ITfContext,
    composition: &ITfComposition,
    preedit: &str,
    cursor: usize,
    display_attribute_atom: Option<u32>,
) -> Result<()> {
    unsafe {
        let range = composition.GetRange()?;
        let wide = to_wide(preedit);
        range.SetText(ec, 0, &wide)?;
        set_composition_display_attribute(ec, context, &range, display_attribute_atom)?;
        set_composition_cursor(ec, context, &range, preedit, cursor)?;
    }
    Ok(())
}

fn set_composition_cursor(
    ec: u32,
    context: &ITfContext,
    composition_range: &ITfRange,
    preedit: &str,
    cursor: usize,
) -> Result<()> {
    unsafe {
        let caret = composition_range.Clone()?;
        caret.Collapse(ec, TF_ANCHOR_START)?;
        // `ImeState::cursor` counts Unicode scalars; TSF ranges count UTF-16
        // code units.
        let requested = utf16_offset_from_chars(preedit, cursor) as i32;
        let mut shifted = 0;
        caret.ShiftEnd(ec, requested, &mut shifted, std::ptr::null())?;
        caret.Collapse(ec, TF_ANCHOR_END)?;

        let mut selections = [TF_SELECTION::default()];
        selections[0].range = std::mem::ManuallyDrop::new(Some(caret));
        selections[0].style.ase = TF_AE_END;
        selections[0].style.fInterimChar = BOOL::from(false);
        let result = context.SetSelection(ec, &selections);
        std::mem::ManuallyDrop::drop(&mut selections[0].range);
        result?;
    }
    Ok(())
}

/// Commit (end) the composition, writing the final committed text.
fn end_composition(
    ec: u32,
    context: &ITfContext,
    composition: &ITfComposition,
    committed: Option<&str>,
) -> Result<()> {
    unsafe {
        let range = composition.GetRange()?;
        clear_composition_display_attribute(ec, context, &range);
        let wide = committed.map(to_wide).unwrap_or_default();
        range.SetText(ec, 0, &wide)?;
        composition.EndComposition(ec)?;

        // SetText can leave the document selection spanning the old
        // composition range. Collapse it after the committed text so a
        // top-up result containing both commit and new preedit starts the new
        // composition after the committed character instead of before it.
        if committed.is_some() {
            let caret = range.Clone()?;
            caret.Collapse(ec, TF_ANCHOR_END)?;
            let mut selections = [TF_SELECTION::default()];
            selections[0].range = std::mem::ManuallyDrop::new(Some(caret));
            selections[0].style.ase = TF_AE_END;
            selections[0].style.fInterimChar = BOOL::from(false);
            let result = context.SetSelection(ec, &selections);
            std::mem::ManuallyDrop::drop(&mut selections[0].range);
            result?;
        }
    }
    Ok(())
}

fn set_composition_display_attribute(
    ec: u32,
    context: &ITfContext,
    range: &ITfRange,
    atom: Option<u32>,
) -> Result<()> {
    let Some(atom) = atom else {
        return Ok(());
    };
    unsafe {
        let property = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
        let value = VARIANT::from(atom as i32);
        property.SetValue(ec, range, &value)?;
    }
    Ok(())
}

fn clear_composition_display_attribute(ec: u32, context: &ITfContext, range: &ITfRange) {
    unsafe {
        if let Ok(property) = context.GetProperty(&GUID_PROP_ATTRIBUTE) {
            let _ = property.Clear(ec, range);
        }
    }
}

/// What one `GetTextExt` attempt yielded.
///
/// `position` stays `None` when the host has not laid the text out yet
/// (`TF_E_NOLAYOUT`) or refuses the query; the owner window is still usable in
/// that case, so the two answers are kept apart.
struct CaretProbe {
    owner_hwnd: HWND,
    position: Option<(i32, i32)>,
}

/// Get caret screen position and the owner HWND from the current context view.
fn probe_caret(ec: u32, context: &ITfContext) -> CaretProbe {
    unsafe {
        let Ok(view) = context.GetActiveView() else {
            return CaretProbe {
                owner_hwnd: fallback_focus_window(),
                position: None,
            };
        };
        let owner_hwnd = view
            .GetWnd()
            .ok()
            .filter(|hwnd| !hwnd.0.is_null())
            .unwrap_or_else(fallback_focus_window);

        let mut selections = [TF_SELECTION::default()];
        let mut count: u32 = 0;
        let selection_result =
            context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selections, &mut count);
        let range = take_selection_range(&mut selections[0]);
        if selection_result.is_err() {
            return CaretProbe {
                owner_hwnd,
                position: None,
            };
        }
        let Some(range) = range else {
            return CaretProbe {
                owner_hwnd,
                position: None,
            };
        };

        let mut rect = RECT::default();
        let mut clipped = BOOL::default();
        // TF_E_NOLAYOUT means "ask again after OnLayoutChange", never "put the
        // panel in the corner of the primary monitor".
        if view
            .GetTextExt(ec, &range, &mut rect, &mut clipped)
            .is_err()
        {
            return CaretProbe {
                owner_hwnd,
                position: None,
            };
        }
        CaretProbe {
            owner_hwnd,
            position: Some((rect.left, rect.bottom)),
        }
    }
}

/// Turn a probe into a position to draw at, remembering the last good caret.
fn resolve_caret(shared_state: &SharedState, probe: CaretProbe) -> CaretPosition {
    if let Some((x, y)) = probe.position {
        let caret = CaretPosition {
            x,
            y,
            owner_hwnd: probe.owner_hwnd,
        };
        shared_state.borrow_mut().last_caret = Some(caret);
        return caret;
    }
    let cached = shared_state.borrow().last_caret;
    if let Some(mut caret) = cached {
        if !probe.owner_hwnd.0.is_null() {
            caret.owner_hwnd = probe.owner_hwnd;
        }
        return caret;
    }
    fallback_caret(probe.owner_hwnd)
}

/// Used before the host ever reported a caret: the system caret of the
/// foreground thread, else the top-left corner of the owner window.
fn fallback_caret(owner_hwnd: HWND) -> CaretPosition {
    let owner_hwnd = if owner_hwnd.0.is_null() {
        fallback_focus_window()
    } else {
        owner_hwnd
    };
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(0, &mut info).is_ok() && !info.hwndCaret.0.is_null() {
            let mut point = POINT {
                x: info.rcCaret.left,
                y: info.rcCaret.bottom,
            };
            if ClientToScreen(info.hwndCaret, &mut point).as_bool() {
                return CaretPosition {
                    x: point.x,
                    y: point.y,
                    owner_hwnd,
                };
            }
        }
        let mut rect = RECT::default();
        if !owner_hwnd.0.is_null() && GetWindowRect(owner_hwnd, &mut rect).is_ok() {
            return CaretPosition {
                x: rect.left,
                y: rect.top,
                owner_hwnd,
            };
        }
    }
    CaretPosition {
        x: 0,
        y: 0,
        owner_hwnd,
    }
}

fn apply_ime_state(
    context: &ITfContext,
    client_id: u32,
    shared_state: &SharedState,
    ime_state: ImeState,
    show_mode_hint_on_change: bool,
) -> Result<()> {
    let state_arc = Rc::clone(shared_state);
    let state_arc_for_session = Rc::clone(&state_arc);
    let ime_state_clone = ime_state.clone();
    let window_update = Rc::new(RefCell::new(None));
    let window_update_for_session = Rc::clone(&window_update);
    let comp_sink_obj = CompositionSink {
        state: Rc::downgrade(shared_state),
        _dll_guard: DllActivityGuard::new(),
    };
    let comp_sink_iface: ITfCompositionSink = comp_sink_obj.into();
    let display_attribute_atom = shared_state.borrow().display_attribute_atom;

    let session_result = with_write_session(context, client_id, move |ec, ctx| {
        let committed = ime_state_clone
            .committed
            .as_deref()
            .filter(|text| !text.is_empty());
        let has_commit = committed.is_some();

        let mut composition = {
            let mut st = state_arc_for_session.borrow_mut();
            st.composition_context = None;
            st.composition.take()
        };
        let original_composition = composition.clone();

        let apply_result = (|| -> Result<()> {
            if let Some(committed) = committed {
                if let Some(comp) = composition.take() {
                    end_composition(ec, ctx, &comp, Some(committed))?;
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
                    update_composition_text(
                        ec,
                        ctx,
                        comp,
                        &ime_state_clone.preedit,
                        ime_state_clone.cursor,
                        display_attribute_atom,
                    )?;
                } else {
                    let comp = start_composition(
                        ec,
                        ctx,
                        &ime_state_clone.preedit,
                        ime_state_clone.cursor,
                        &comp_sink_iface,
                        display_attribute_atom,
                    )?;
                    composition = Some(comp);
                }
            } else if composition.is_some() && !has_commit {
                if let Some(comp) = composition.take() {
                    end_composition(ec, ctx, &comp, None)?;
                }
            }
            Ok(())
        })();

        if let Err(error) = apply_result {
            if let Some(comp) = composition.as_ref().or(original_composition.as_ref()) {
                let _ = end_composition(ec, ctx, comp, None);
            }
            let mut st = state_arc_for_session.borrow_mut();
            st.composition = None;
            st.composition_context = None;
            st.ime_state = None;
            return Err(error);
        }

        let caret = probe_caret(ec, ctx);
        let document_mgr = unsafe { ctx.GetDocumentMgr().ok() };
        let mode_changed = {
            let mut st = state_arc_for_session.borrow_mut();
            st.composition_context = composition.as_ref().map(|_| ctx.clone());
            st.composition = composition;
            let mode_changed = ime_state_clone.ascii_mode != st.ascii_mode;
            st.ascii_mode = ime_state_clone.ascii_mode;
            st.ime_state = Some(ime_state_clone.clone());
            mode_changed
        };
        *window_update_for_session.borrow_mut() = Some((
            ime_state_clone.clone(),
            caret,
            document_mgr,
            show_mode_hint_on_change && mode_changed,
        ));

        Ok(())
    });
    if let Err(error) = session_result {
        reset_input_for_focus_change(&state_arc);
        append_key_diagnostic!("TSF edit session failed: {error}");
        return Err(error);
    }

    if let Some((ime_state, probe, document_mgr, show_mode_hint)) =
        window_update.borrow_mut().take()
    {
        let caret = resolve_caret(&state_arc, probe);
        if has_visible_state(&ime_state) {
            ensure_layout_sink(&state_arc, context);
        } else {
            clear_layout_sink(&state_arc);
        }
        update_language_bar_mode(&state_arc, ime_state.ascii_mode);
        update_ime_windows(
            &state_arc,
            &ime_state,
            document_mgr.as_ref(),
            caret.x,
            caret.y,
            caret.owner_hwnd,
            show_mode_hint,
        );
    }

    Ok(())
}

/// Re-run the caret query for the context the panel is anchored to.
///
/// Hosts that answered `TF_E_NOLAYOUT` earlier call `OnLayoutChange` once the
/// text is laid out; scrolling and window moves report the same way.
fn reposition_ime_windows(shared_state: &SharedState, context: &ITfContext) {
    let (client_id, ime_state) = {
        let st = shared_state.borrow();
        (st.client_id, st.ime_state.clone())
    };
    let Some(ime_state) = ime_state else {
        return;
    };
    if client_id == 0 || !has_visible_state(&ime_state) {
        return;
    }

    let probe = Rc::new(RefCell::new(None));
    let probe_for_session = Rc::clone(&probe);
    if with_read_session(context, client_id, move |ec, ctx| {
        *probe_for_session.borrow_mut() = Some(probe_caret(ec, ctx));
        Ok(())
    })
    .is_err()
    {
        return;
    }
    let Some(probe) = probe.borrow_mut().take() else {
        return;
    };
    if probe.position.is_none() {
        return;
    }

    let caret = resolve_caret(shared_state, probe);
    let document_mgr = unsafe { context.GetDocumentMgr().ok() };
    update_ime_windows(
        shared_state,
        &ime_state,
        document_mgr.as_ref(),
        caret.x,
        caret.y,
        caret.owner_hwnd,
        false,
    );
}

fn ensure_layout_sink(shared_state: &SharedState, context: &ITfContext) {
    let already_advised = layout_sink_context(shared_state)
        .is_some_and(|advised| advised.as_raw() == context.as_raw());
    if already_advised {
        return;
    }
    let Ok(source) = context.cast::<ITfSource>() else {
        return;
    };
    let sink: ITfTextLayoutSink = TextLayoutSink {
        state: Rc::downgrade(shared_state),
        _dll_guard: DllActivityGuard::new(),
    }
    .into();
    let Ok(cookie) = (unsafe { source.AdviseSink(&ITfTextLayoutSink::IID, &sink) }) else {
        return;
    };
    store_layout_sink(
        shared_state,
        LayoutSinkRegistration {
            source,
            cookie,
            sink,
            context: context.clone(),
        },
    );
}

fn runtime_session(shared_state: &SharedState) -> Option<ImeRuntimeSession> {
    shared_state.borrow().session()
}

fn cached_ime_state(shared_state: &SharedState) -> Option<ImeState> {
    shared_state.borrow().ime_state.clone()
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

fn visible_state_changed(before: &ImeState, after: &ImeState) -> bool {
    before.preedit != after.preedit
        || before.highlighted_candidate_index != after.highlighted_candidate_index
        || before.page != after.page
        || before.candidates.len() != after.candidates.len()
        || before
            .candidates
            .iter()
            .zip(after.candidates.iter())
            .any(|(before, after)| before.text != after.text)
}

/// Whether `OnKeyDown` should report the key as eaten.
///
/// `OnTestKeyDown` deliberately claims more than librime will take (every
/// printable key has to be offered to the punctuator), so this is where a key
/// librime declined is handed back to the host. Anything librime actually did
/// — accepting the key, committing text, or changing the composition — is
/// eaten so the host never sees it twice.
fn should_consume_processed_state(accepted: bool, before: &ImeState, after: &ImeState) -> bool {
    accepted || has_commit(after) || visible_state_changed(before, after)
}

fn process_key_result(
    shared_state: &SharedState,
    keysym: u32,
    mods: u32,
) -> Option<KeyProcessResult> {
    runtime_session(shared_state)?.process_key_result(keysym, mods)
}

/// What a click on the candidate panel maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelClick {
    Candidate(usize),
    PreviousPage,
    NextPage,
}

/// Apply a candidate-panel click. Runs from the popup's `wnd_proc`, i.e. on the
/// same STA thread as the key callbacks but outside any TSF callback.
pub(crate) fn handle_panel_click(shared_state: &SharedState, click: PanelClick) {
    let (context, client_id) = {
        let st = shared_state.borrow();
        (
            st.composition_context
                .clone()
                .or_else(|| st.key_context.clone()),
            st.client_id,
        )
    };
    let Some(context) = context else {
        return;
    };
    if client_id == 0 || input_is_blocked(shared_state, Some(&context)) {
        return;
    }
    let Some(session) = runtime_session(shared_state) else {
        return;
    };
    let ime_state = match click {
        PanelClick::Candidate(index) => session.select_candidate_on_page(index),
        PanelClick::PreviousPage => session.change_page(true),
        PanelClick::NextPage => session.change_page(false),
    };
    if let Some(ime_state) = ime_state {
        let _ = apply_ime_state(&context, client_id, shared_state, ime_state, false);
    }
}

fn prepare_engine_for_key(context: &ITfContext, shared_state: &SharedState) -> Result<Option<u32>> {
    poll_engine_builds(shared_state);
    let (engine_ready, engine_building, engine_error) = {
        let st = shared_state.borrow();
        (
            st.engine_ready(),
            st.engine_building,
            st.engine_error.clone(),
        )
    };
    if !engine_ready {
        start_engine_warmup(shared_state);
        append_key_diagnostic!(
            "OnKeyDown engine not ready building={engine_building} error={engine_error:?}"
        );
        return Ok(None);
    }

    apply_pending_session_reset(shared_state);
    // The context policy is decided on focus changes, but the engine is built
    // in the background and may only have arrived since.
    sync_input_policy(shared_state);

    let reload_started = start_reload_if_needed(shared_state);
    let (client_id, reload_in_progress, should_clear_reload) = {
        let mut st = shared_state.borrow_mut();
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
    pub(crate) state: WeakState,
    pub(crate) _dll_guard: DllActivityGuard,
}

impl ITfKeyEventSink_Impl for KeyEventSink_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
        let Some(state) = upgrade_state(&self.state) else {
            return Ok(());
        };
        reset_input_for_focus_change(&state);
        refresh_engine_for_focus(&state);
        refresh_input_context(&state, None);
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let Some(state) = upgrade_state(&self.state) else {
            return Ok(BOOL::from(false));
        };
        poll_engine_builds(&state);
        let vk = (wparam.0 & 0xFFFF) as u16;
        let mods = current_mod_mask();
        // TSF only calls OnKeyDown for keys claimed here, so solo-Shift state
        // has to be maintained in the test callback (windows-1).
        state.borrow_mut().shift_pressed_without_key = shift_pending_after_key_down(vk);
        if is_shift_vk(vk) {
            return Ok(BOOL::from(false));
        }
        // An unanswered probe counts as blocked, so the retry has to run before
        // the check that consumes it: TSF skips `OnKeyDown` for a key this
        // callback declines, and a stale `Unknown` would never be revisited.
        retry_input_context_if_unknown(&state, pic);
        if input_is_blocked(&state, pic) {
            // The host may have revoked the keyboard while a composition was up
            // and `OnKeyDown` will not run to clean it away.
            clear_input_for_blocked_context(&state);
            return Ok(BOOL::from(false));
        }
        let (engine_ready, reload_needed, cached_state) = {
            let mut st = state.borrow_mut();
            let reload_needed = st.reload_needed_cached();
            (st.engine_ready(), reload_needed, st.ime_state.clone())
        };
        if !engine_ready || reload_needed {
            if !engine_ready {
                start_engine_warmup(&state);
            }
            if reload_needed {
                start_reload_if_needed(&state);
            }
            append_key_diagnostic!(
                "OnTestKeyDown vk=0x{vk:02x} mods=0x{mods:x} ready={engine_ready} reload={reload_needed} composing=false eat=false"
            );
            return Ok(BOOL::from(false));
        }
        let is_composing = cached_state
            .as_ref()
            .map(has_visible_state)
            .unwrap_or(false);
        let eat = should_eat_key(vk, is_composing, mods);
        append_key_diagnostic!(
            "OnTestKeyDown vk=0x{vk:02x} mods=0x{mods:x} ready={engine_ready} reload={reload_needed} composing={is_composing} eat={eat}"
        );
        Ok(BOOL::from(eat))
    }

    fn OnTestKeyUp(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        let Some(state) = upgrade_state(&self.state) else {
            return Ok(BOOL::from(false));
        };
        poll_engine_builds(&state);
        let vk = (wparam.0 & 0xFFFF) as u16;
        if !is_shift_vk(vk) {
            return Ok(BOOL::from(false));
        }
        if input_is_blocked(&state, pic) {
            return Ok(BOOL::from(false));
        }
        let (should_eat, should_retry, should_reload) = {
            let mut st = state.borrow_mut();
            let reload_needed = st.reload_needed_cached();
            (
                st.shift_pressed_without_key && st.engine_ready() && !reload_needed,
                !st.engine_ready(),
                reload_needed,
            )
        };
        if should_retry {
            start_engine_warmup(&state);
        }
        if should_reload {
            start_reload_if_needed(&state);
        }
        Ok(BOOL::from(should_eat))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        let Some(state) = upgrade_state(&self.state) else {
            return Ok(BOOL::from(false));
        };
        let context = pic.ok_or(windows::core::Error::from(E_INVALIDARG))?.clone();
        let vk = (wparam.0 & 0xFFFF) as u16;
        let mods = current_mod_mask();

        if is_shift_vk(vk) {
            state.borrow_mut().shift_pressed_without_key = true;
            return Ok(BOOL::from(false));
        }

        retry_input_context_if_unknown(&state, Some(&context));
        if input_is_blocked(&state, Some(&context)) {
            reset_input_for_focus_change(&state);
            return Ok(BOOL::from(false));
        }

        let prepared_engine = {
            let mut st = state.borrow_mut();
            st.shift_pressed_without_key = false;
            st.key_context = Some(context.clone());
            drop(st);
            prepare_engine_for_key(&context, &state)?
        };
        let client_id = match prepared_engine {
            Some(client_id) => client_id,
            None => return Ok(BOOL::from(false)),
        };

        let before_state = cached_ime_state(&state).unwrap_or_else(ImeState::empty);

        if should_bypass_empty_composition(vk, mods, &before_state) {
            hide_candidate_window(&state);
            return Ok(BOOL::from(false));
        }

        // Enter has exactly one meaning across all platforms: hand XK_Return to
        // Rime and let core fall back to committing the raw input (D5).
        let (result, keysym) = if is_enter_vk(vk) {
            let result = runtime_session(&state).and_then(|session| session.process_enter());
            (result, 0xff0du32)
        } else {
            let keysym = match vk_to_keysym(vk, lparam.0, mods) {
                Some(keysym) => keysym,
                None => {
                    append_key_diagnostic!("OnKeyDown vk=0x{vk:02x} mods=0x{mods:x} no keysym");
                    return Ok(BOOL::from(false));
                }
            };
            (process_key_result(&state, keysym, mods), keysym)
        };
        let result = match result {
            Some(r) => r,
            None => {
                append_key_diagnostic!(
                    "OnKeyDown vk=0x{vk:02x} keysym=0x{keysym:x} mods=0x{mods:x} no result"
                );
                return Ok(BOOL::from(false));
            }
        };
        let ime_state = result.state;

        let consumed = should_consume_processed_state(result.accepted, &before_state, &ime_state);
        append_key_diagnostic!(
            "OnKeyDown vk=0x{vk:02x} keysym=0x{keysym:x} mods=0x{mods:x} accepted={} consumed={} preedit_len={} candidates={} commit={}",
            result.accepted,
            consumed,
            ime_state.preedit.chars().count(),
            ime_state.candidates.len(),
            has_commit(&ime_state),
        );

        if !consumed {
            return Ok(BOOL::from(false));
        }

        apply_ime_state(&context, client_id, &state, ime_state, true)?;

        Ok(BOOL::from(consumed))
    }

    fn OnKeyUp(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        let Some(state) = upgrade_state(&self.state) else {
            return Ok(BOOL::from(false));
        };
        let vk = (wparam.0 & 0xFFFF) as u16;
        let Some(keysym) = shift_keysym_for_vk(vk) else {
            return Ok(BOOL::from(false));
        };

        let context = pic.ok_or(windows::core::Error::from(E_INVALIDARG))?.clone();
        if input_is_blocked(&state, Some(&context)) {
            return Ok(BOOL::from(false));
        }
        let prepared_engine = {
            let mut st = state.borrow_mut();
            if !st.shift_pressed_without_key {
                return Ok(BOOL::from(false));
            }
            st.shift_pressed_without_key = false;
            st.key_context = Some(context.clone());
            drop(st);
            prepare_engine_for_key(&context, &state)?
        };
        let client_id = match prepared_engine {
            Some(client_id) => client_id,
            None => return Ok(BOOL::from(false)),
        };

        let before_state = cached_ime_state(&state).unwrap_or_else(ImeState::empty);
        let result = process_key_result(&state, keysym, RIME_RELEASE_MASK);
        let Some(result) = result else {
            return Ok(BOOL::from(false));
        };
        let consumed =
            should_consume_processed_state(result.accepted, &before_state, &result.state);
        if consumed {
            apply_ime_state(&context, client_id, &state, result.state, true)?;
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
    pub(crate) state: WeakState,
    pub(crate) _dll_guard: DllActivityGuard,
}

impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        pcomposition: Option<&ITfComposition>,
    ) -> Result<()> {
        let Some(state) = upgrade_state(&self.state) else {
            return Ok(());
        };
        clear_input_after_composition_terminated(&state, pcomposition);
        Ok(())
    }
}

// ── ITfTextLayoutSink ─────────────────────────────────────────────────────────

/// Tells us when a host that answered `TF_E_NOLAYOUT` has laid the text out,
/// and when the caret moved because the document scrolled or the window moved.
#[implement(ITfTextLayoutSink)]
struct TextLayoutSink {
    state: WeakState,
    _dll_guard: DllActivityGuard,
}

impl ITfTextLayoutSink_Impl for TextLayoutSink_Impl {
    fn OnLayoutChange(
        &self,
        pic: Option<&ITfContext>,
        lcode: TfLayoutCode,
        _pview: Option<&ITfContextView>,
    ) -> Result<()> {
        if lcode == TF_LC_DESTROY {
            return Ok(());
        }
        let Some(state) = upgrade_state(&self.state) else {
            return Ok(());
        };
        let Some(context) = pic else {
            return Ok(());
        };
        reposition_ime_windows(&state, context);
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

    #[test]
    fn passes_shortcuts_rime_declined_while_composing() {
        // OnTestKeyDown claims Ctrl chords so key_binder gets a chance at them;
        // when librime leaves the composition untouched the host must still get
        // its Ctrl+C.
        assert!(!should_consume_processed_state(
            false,
            &state_with_preedit("ni"),
            &state_with_preedit("ni")
        ));
    }
}
