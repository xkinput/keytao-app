//! ITfKeyEventSink + ITfCompositionSink — the hot path for all keystrokes.
//!
//! Key handling mirrors keytao-linux-ime/src/wayland_backend.rs:
//!   VK → X11 keysym → ImeEngineBackend::process_key() → ImeState
//!   committed text  → write via TSF edit session (ITfInsertAtSelection)
//!   preedit text    → manage ITfComposition
//!   candidate list  → update CandidateWindow (same tiny-skia panel as Linux)

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use std::sync::atomic::{AtomicUsize, Ordering};

use keytao_core::{
    key_policy,
    runtime_log::{self, Level},
    utf16_offset_from_chars, ImeState, KeyProcessResult,
};
use windows::{
    core::{implement, Interface, Result, GUID, PCWSTR, VARIANT},
    Win32::{
        Foundation::{
            BOOL, E_FAIL, E_INVALIDARG, E_UNEXPECTED, HWND, LPARAM, POINT, RECT, S_FALSE, WPARAM,
        },
        Graphics::Gdi::{
            ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
        System::Threading::{GetCurrentProcessId, GetCurrentThreadId},
        UI::TextServices::*,
        UI::WindowsAndMessaging::{
            GetAncestor, GetClientRect, GetForegroundWindow, GetGUIThreadInfo,
            GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, GA_ROOT, GUITHREADINFO,
        },
    },
};

use crate::{
    composition_rules::{
        composition_end_retry_delay_ms, composition_end_to_finish_first,
        leftover_to_delete_on_host_termination, may_process_key, may_terminate_composition,
        retain_exhausted_composition_end_for_cleanup, should_backoff_composition_end_failure,
        should_drop_pending_context, should_retry_commit,
    },
    deferred_keys::{DeferredKey, KeyEventSource},
    edit_session::{
        take_selection_range, with_async_dontcare_write_session, with_async_write_session,
        with_read_session, with_write_session,
    },
    engine_backend::ImeEngineBackend,
    globals::DllActivityGuard,
    guard,
    key_map::{
        ascii_mode_rejects_key, chord_keysym_for_vk, current_mod_mask, is_application_shortcut,
        is_caps_lock_vk, is_enter_vk, modifier_tap_is_alone, modifier_tap_keysym,
        rime_mod_mask_for_vk, schema_shortcut_key, should_bypass_empty_composition,
        should_eat_key, vk_to_keysym, RIME_RELEASE_MASK,
    },
    panel_anchor::{start_panel_anchor, PanelAnchor},
    solo_shift::{modifier_tap_matches, modifier_tap_should_apply, record_modifier_tap},
    state::{
        append_diagnostic, apply_pending_session_reset, clear_input_after_composition_terminated,
        clear_input_for_blocked_context, clear_layout_sink, defer_ime_ui_hide, diagnostics_enabled,
        embedded_composition, fallback_focus_window, flush_pending_ime_ui, hide_candidate_window,
        host_is_uiless, input_is_blocked, layout_sink_context, poll_engine_builds,
        post_pending_ime_ui, prime_theme_resolver, refresh_engine_for_focus, refresh_input_context,
        reset_input_for_focus_change, start_engine_warmup, start_reload_if_needed,
        store_layout_sink, sync_input_policy, update_ime_windows, update_language_bar_mode,
        with_current_theme, CaretPosition, LayoutSinkRegistration, SharedState, TsfState,
        WeakState,
    },
};

static KEY_DIAGNOSTIC_COUNT: AtomicUsize = AtomicUsize::new(0);
const KEY_DIAGNOSTIC_LIMIT: usize = 65_536;

macro_rules! append_key_diagnostic {
    ($($arg:tt)*) => {
        if diagnostics_enabled()
            && KEY_DIAGNOSTIC_COUNT.fetch_add(1, Ordering::Relaxed) < KEY_DIAGNOSTIC_LIMIT
        {
            append_diagnostic(format!($($arg)*));
        }
    };
}

fn upgrade_state(state: &WeakState) -> Option<SharedState> {
    state.upgrade()
}

fn modifier_tap_enabled(st: &TsfState, keysym: u32) -> bool {
    matches!(keysym, 0xffe1 | 0xffe2)
        || st
            .ime_state
            .as_ref()
            .is_some_and(|ime| key_policy::is_configured_modifier_tap(keysym, ime))
}

fn record_modifier_press(state: &SharedState, keysym: u32, lparam: isize, mods: u32) {
    let mut st = state.borrow_mut();
    st.modifier_tap_press = if st.keyboard_open
        && modifier_tap_enabled(&st, keysym)
        && modifier_tap_is_alone(keysym, mods)
    {
        record_modifier_tap(st.modifier_tap_press, keysym, lparam, Instant::now())
    } else {
        None
    };
}

fn modifier_tap_pending(st: &TsfState, keysym: u32, mods: u32) -> bool {
    modifier_tap_matches(st.modifier_tap_press, keysym, Instant::now())
        && modifier_tap_enabled(st, keysym)
        && modifier_tap_is_alone(keysym, mods)
}

// ── Composition helpers ───────────────────────────────────────────────────────

fn normalize_commit_newlines(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut previous = '\0';
    for character in text.chars() {
        if character == '\n' && previous != '\r' {
            normalized.push('\r');
        }
        normalized.push(character);
        previous = character;
    }
    normalized
}

#[cfg(test)]
mod commit_text_tests {
    use super::normalize_commit_newlines;

    #[test]
    fn commit_newlines_preserve_existing_carriage_returns_and_unicode() {
        for (input, expected) in [
            ("a\nb", "a\r\nb"),
            ("a\r\nb", "a\r\nb"),
            ("\n\n", "\r\n\r\n"),
            ("a\u{200b}\nb", "a\u{200b}\r\nb"),
            ("a\rb", "a\rb"),
            ("甲😀\n乙", "甲😀\r\n乙"),
        ] {
            assert_eq!(normalize_commit_newlines(input), expected);
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Create the native composition before any text write, so a later failure
/// cannot lose the only handle needed to clean it up.
fn start_composition(
    ec: u32,
    context: &ITfContext,
    comp_sink: &ITfCompositionSink,
) -> Result<ITfComposition> {
    unsafe {
        // Get insertion point (query only — don't insert yet)
        let ins: ITfInsertAtSelection = context.cast()?;
        let mut raw_range = std::ptr::null_mut();
        (ins.vtable().InsertTextAtSelection)(
            ins.as_raw(),
            ec,
            TF_IAS_QUERYONLY,
            PCWSTR::null(),
            0,
            &mut raw_range,
        )
        .ok()?;
        if raw_range.is_null() {
            return Err(E_FAIL.into());
        }
        let range = ITfRange::from_raw(raw_range);

        let ctx_comp: ITfContextComposition = context.cast()?;
        ctx_comp.StartComposition(ec, &range, comp_sink)
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
    input: (&SharedState, u64),
    written: &Cell<bool>,
) -> Result<()> {
    unsafe {
        let range = composition.GetRange()?;
        require_current_input(Some(input))?;
        let wide = to_wide(preedit);
        range.SetText(ec, 0, &wide)?;
        written.set(true);
        let mut st = input.0.borrow_mut();
        st.written_preedits
            .retain(|(identity, _, _)| *identity != composition.as_raw() as usize);
        st.written_preedits.push((
            composition.as_raw() as usize,
            context.clone(),
            preedit.to_owned(),
        ));
        drop(st);
        require_current_input(Some(input))?;
        set_composition_display_attribute(ec, context, &range, display_attribute_atom)?;
        set_composition_cursor(ec, context, &range, preedit, cursor, input)?;
    }
    Ok(())
}

fn set_composition_cursor(
    ec: u32,
    context: &ITfContext,
    composition_range: &ITfRange,
    preedit: &str,
    cursor: usize,
    input: (&SharedState, u64),
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
        require_current_input(Some(input))?;

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
    end_composition_with_commit_tracking(ec, context, composition, committed, None, None)
}

fn end_composition_with_commit_tracking(
    ec: u32,
    context: &ITfContext,
    composition: &ITfComposition,
    committed: Option<&str>,
    written: Option<&Cell<bool>>,
    input: Option<(&SharedState, u64)>,
) -> Result<()> {
    unsafe {
        let range = composition.GetRange()?;
        require_current_input(input)?;
        clear_composition_display_attribute(ec, context, &range);
        require_current_input(input)?;
        let wide = committed
            .map(|text| to_wide(&normalize_commit_newlines(text)))
            .unwrap_or_default();
        range.SetText(ec, 0, &wide)?;
        if let Some((state, _)) = input {
            forget_written_preedit(state, composition.as_raw() as usize);
        }
        if committed.is_some() {
            if let Some(written) = written {
                written.set(true);
            }
        }

        // SetText can leave the document selection spanning the old
        // composition range. Collapse it after the committed text so a
        // top-up result containing both commit and new preedit starts the new
        // composition after the committed character instead of before it.
        if committed.is_some() {
            let caret = range.Clone()?;
            caret.Collapse(ec, TF_ANCHOR_END)?;
            require_current_input(input)?;
            let mut selections = [TF_SELECTION::default()];
            selections[0].range = std::mem::ManuallyDrop::new(Some(caret));
            selections[0].style.ase = TF_AE_NONE;
            selections[0].style.fInterimChar = BOOL::from(false);
            let result = context.SetSelection(ec, &selections);
            std::mem::ManuallyDrop::drop(&mut selections[0].range);
            result?;
            require_current_input(input)?;
        }
        composition.EndComposition(ec)?;
    }
    Ok(())
}

fn require_current_input(input: Option<(&SharedState, u64)>) -> Result<()> {
    if input.is_some_and(|(state, generation)| state.borrow().input_generation != generation) {
        return Err(E_UNEXPECTED.into());
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
#[derive(Clone)]
struct CaretProbe {
    owner_hwnd: HWND,
    position: Option<(i32, i32)>,
    // An editing viewport is a last-resort panel anchor, never a caret.
    viewport: Option<RECT>,
}

#[derive(Clone)]
pub(crate) struct PendingWindowUpdate {
    context: ITfContext,
    input_generation: u64,
    ime_state: ImeState,
    caret: CaretProbe,
    document_mgr: Option<ITfDocumentMgr>,
    show_mode_hint: bool,
    embedded: bool,
    reposition_only: bool,
    key_started: Option<Instant>,
}

pub(crate) struct PendingCommitRetry {
    context: ITfContext,
    client_id: u32,
    input_generation: u64,
    ime_state: ImeState,
}

type ApplyEdit = Box<dyn FnOnce(u32, &ITfContext) -> Result<()>>;

pub(crate) struct PendingImeApply {
    context: ITfContext,
    client_id: u32,
    generation: u64,
    queued: bool,
    apply: ApplyEdit,
}

fn request_pending_ime_apply(state: &SharedState, asynchronous: bool) -> Result<()> {
    {
        let st = state.borrow();
        if st.pending_ime_apply.as_ref().is_some_and(|apply| {
            st.pending_composition_ends
                .iter()
                .any(|end| !end.exhausted && end.context.as_raw() == apply.context.as_raw())
        }) {
            return Ok(());
        }
    }
    let (context, client_id, generation) = {
        let mut st = state.borrow_mut();
        let Some(pending) = st.pending_ime_apply.as_mut() else {
            return Ok(());
        };
        if pending.queued {
            return Ok(());
        }
        pending.queued = true;
        (
            pending.context.clone(),
            pending.client_id,
            pending.generation,
        )
    };
    if discard_disconnected_context(state, &context, false) {
        return Ok(());
    }
    let request_active = Rc::new(Cell::new(true));
    let request_active_for_session = request_active.clone();
    let for_session = state.clone();
    let apply = move |ec, ctx: &ITfContext| {
        if for_session
            .borrow()
            .pending_ime_apply
            .as_ref()
            .is_none_or(|pending| pending.generation != generation)
        {
            return Ok(());
        }
        {
            let _guard = ImeWriteSessionGuard::enter(&for_session);
            if let Err(error) = finish_pending_composition_ends_first(ec, ctx, &for_session) {
                if let Some(pending) = for_session.borrow_mut().pending_ime_apply.as_mut() {
                    pending.queued = false;
                }
                post_pending_ime_ui(&for_session);
                return Err(error);
            }
        }
        let pending = for_session.borrow_mut().pending_ime_apply.take();
        let result = if let Some(pending) = pending {
            (pending.apply)(ec, ctx)
        } else {
            Ok(())
        };
        // Inline callbacks leave flushing to the caller until the request returns.
        if !request_active_for_session.get() {
            post_pending_ime_ui(&for_session);
        }
        result
    };
    let result = if asynchronous {
        with_async_dontcare_write_session(&context, client_id, apply)
    } else {
        with_write_session(&context, client_id, apply)
    };
    request_active.set(false);
    if result.as_ref().is_err_and(|error| {
        discard_disconnected_context(state, &context, error.code() == TF_E_DISCONNECTED)
    }) {
        return result;
    }
    if result.is_err() {
        if let Some(pending) = state.borrow_mut().pending_ime_apply.as_mut() {
            if pending.generation == generation {
                pending.queued = false;
            }
        }
    }
    result
}

pub(crate) fn flush_pending_ime_apply(state: &SharedState) {
    let context = state
        .borrow()
        .pending_ime_apply
        .as_ref()
        .map(|pending| pending.context.clone());
    if context.is_some_and(|context| discard_disconnected_context(state, &context, false)) {
        return;
    }
    let ready = {
        let st = state.borrow();
        !st.ime_write_session_active
            && !st.caret_probe_session_in_progress
            && st
                .pending_ime_apply
                .as_ref()
                .is_some_and(|pending| !pending.queued)
    };
    if ready {
        let _ = request_pending_ime_apply(state, true);
        if state.borrow().pending_ime_apply.is_none() {
            post_pending_ime_ui(state);
        }
    }
}

#[derive(Clone)]
pub(crate) struct PendingCompositionEnd {
    context: ITfContext,
    pub(crate) composition: ITfComposition,
    pub(crate) queued: bool,
    clear_text: bool,
    terminated_range: Option<(Option<ITfRange>, String)>,
    panel_anchor: Option<PanelAnchor>,
    failures: u8,
    pub(crate) retry_after: Option<Instant>,
    pub(crate) exhausted: bool,
}

impl PendingCompositionEnd {
    pub(crate) fn has_terminated_range(&self) -> bool {
        self.terminated_range.is_some()
    }
}

pub(crate) fn has_active_composition_ends(st: &TsfState) -> bool {
    st.pending_composition_ends
        .iter()
        .any(|pending| !pending.exhausted)
}

fn forget_written_preedit(state: &SharedState, identity: usize) {
    state
        .borrow_mut()
        .written_preedits
        .retain(|(written, _, _)| *written != identity);
}

fn discard_disconnected_context(
    state: &SharedState,
    context: &ITfContext,
    session_disconnected: bool,
) -> bool {
    // Keep S_FALSE distinct from unknown errors; the wrapper maps NULL to E_POINTER.
    let mut document = std::ptr::null_mut();
    let status = unsafe { (context.vtable().GetDocumentMgr)(context.as_raw(), &mut document) };
    if !document.is_null() {
        drop(unsafe { ITfDocumentMgr::from_raw(document) });
    }
    let attached = if status == S_FALSE {
        Some(false)
    } else if status.is_ok() {
        Some(true)
    } else {
        None
    };
    if !should_drop_pending_context(
        attached,
        session_disconnected || status == TF_E_DISCONNECTED,
    ) {
        return false;
    }
    let mut st = state.borrow_mut();
    st.pending_composition_ends
        .retain(|pending| pending.context.as_raw() != context.as_raw());
    st.written_preedits
        .retain(|(_, written_context, _)| written_context.as_raw() != context.as_raw());
    if st
        .pending_ime_apply
        .as_ref()
        .is_some_and(|pending| pending.context.as_raw() == context.as_raw())
    {
        st.pending_ime_apply = None;
    }
    if st
        .pending_commit_retry
        .as_ref()
        .is_some_and(|pending| pending.context.as_raw() == context.as_raw())
    {
        st.pending_commit_retry = None;
    }
    if st
        .deferred_context
        .as_ref()
        .is_some_and(|deferred| deferred.as_raw() == context.as_raw())
    {
        st.deferred_keys.reset();
        st.deferred_context = None;
    }
    true
}

pub(crate) fn defer_failed_composition_end(
    shared_state: &SharedState,
    context: &ITfContext,
    composition: &ITfComposition,
    clear_text: bool,
) {
    let mut st = shared_state.borrow_mut();
    if let Some(pending) = st
        .pending_composition_ends
        .iter_mut()
        .find(|pending| pending.composition.as_raw() == composition.as_raw())
    {
        pending.queued = false;
        pending.clear_text = clear_text;
        pending.failures = 0;
        pending.retry_after = None;
        pending.exhausted = false;
    } else {
        st.pending_composition_ends.push(PendingCompositionEnd {
            context: context.clone(),
            composition: composition.clone(),
            queued: false,
            clear_text,
            terminated_range: None,
            panel_anchor: None,
            failures: 0,
            retry_after: None,
            exhausted: false,
        });
    }
}

pub(crate) fn take_pending_composition_end(
    shared_state: &SharedState,
    identity: usize,
) -> Option<PendingCompositionEnd> {
    let mut st = shared_state.borrow_mut();
    let index = st
        .pending_composition_ends
        .iter()
        .position(|pending| pending.composition.as_raw() as usize == identity)?;
    Some(st.pending_composition_ends.remove(index))
}

struct CompositionEndFlushGuard(SharedState);

impl Drop for CompositionEndFlushGuard {
    fn drop(&mut self) {
        self.0.borrow_mut().composition_end_flush_in_progress = false;
    }
}

fn record_composition_end_failure(state: &SharedState, identity: usize) -> bool {
    let mut st = state.borrow_mut();
    let Some(pending) = st
        .pending_composition_ends
        .iter_mut()
        .find(|pending| pending.composition.as_raw() as usize == identity)
    else {
        return true;
    };
    pending.failures = pending.failures.saturating_add(1);
    pending.queued = false;
    if let Some(delay) = composition_end_retry_delay_ms(pending.failures) {
        pending.retry_after = Some(Instant::now() + Duration::from_millis(delay));
        return false;
    }
    let context = pending.context.as_raw();
    if retain_exhausted_composition_end_for_cleanup() {
        st.pending_composition_ends.retain(|entry| {
            entry.context.as_raw() != context
                || !entry
                    .panel_anchor
                    .as_ref()
                    .is_some_and(|anchor| anchor.composition().is_none())
        });
        for entry in &mut st.pending_composition_ends {
            if entry.context.as_raw() == context {
                entry.exhausted = true;
                entry.queued = false;
                entry.retry_after = None;
            }
        }
        const MAX_EXHAUSTED_PER_CONTEXT: usize = 8;
        while st
            .pending_composition_ends
            .iter()
            .filter(|entry| entry.context.as_raw() == context && entry.exhausted)
            .count()
            > MAX_EXHAUSTED_PER_CONTEXT
        {
            if let Some(evict) = st
                .pending_composition_ends
                .iter()
                .position(|entry| entry.context.as_raw() == context && entry.exhausted)
            {
                let removed = st.pending_composition_ends.remove(evict);
                let removed_id = removed.composition.as_raw() as usize;
                st.written_preedits
                    .retain(|(written, _, _)| *written != removed_id);
            }
        }
    } else {
        let identities: Vec<_> = st
            .pending_composition_ends
            .iter()
            .filter(|pending| pending.context.as_raw() == context)
            .map(|pending| pending.composition.as_raw() as usize)
            .collect();
        st.pending_composition_ends
            .retain(|pending| pending.context.as_raw() != context);
        st.written_preedits
            .retain(|(written, _, _)| !identities.contains(written));
    }
    drop(st);
    append_diagnostic(format!(
        "composition end retries exhausted attempts=5 context={context:p}; releasing queued keys"
    ));
    true
}

fn finish_pending_composition_end(ec: u32, state: &SharedState, identity: usize) -> Result<()> {
    let pending = state
        .borrow()
        .pending_composition_ends
        .iter()
        .find(|pending| pending.composition.as_raw() as usize == identity)
        .cloned();
    let Some(pending) = pending else {
        return Ok(());
    };
    if pending
        .retry_after
        .is_some_and(|deadline| Instant::now() < deadline)
    {
        return Err(E_FAIL.into());
    }
    if pending
        .panel_anchor
        .as_ref()
        .is_some_and(|anchor| anchor.composition().is_none())
    {
        drop(take_pending_composition_end(state, identity));
        return Ok(());
    }
    track_composition_in_flight(state, Some(&pending.composition));
    let result = (|| unsafe {
        if let Some((range, last_preedit)) = &pending.terminated_range {
            let range = match range {
                Some(range) => range.clone(),
                None => pending.composition.GetRange()?,
            };
            let mut text = vec![0u16; last_preedit.encode_utf16().count() + 1];
            let mut fetched = 0;
            range.GetText(ec, 0, &mut text, &mut fetched)?;
            let text = String::from_utf16(&text[..fetched as usize]);
            if text.is_ok_and(|text| {
                leftover_to_delete_on_host_termination(&text, last_preedit, false)
            }) {
                range.SetText(ec, 0, &[])?;
            }
        } else {
            if pending.clear_text {
                let range = pending.composition.GetRange()?;
                clear_composition_display_attribute(ec, &pending.context, &range);
                range.SetText(ec, 0, &[])?;
                forget_written_preedit(state, identity);
                if let Some(entry) = state
                    .borrow_mut()
                    .pending_composition_ends
                    .iter_mut()
                    .find(|entry| entry.composition.as_raw() as usize == identity)
                {
                    entry.clear_text = false;
                }
            }
            pending.composition.EndComposition(ec)?;
        }
        Ok(())
    })();
    track_composition_in_flight(state, None);
    if result.is_ok() {
        drop(take_pending_composition_end(state, identity));
        forget_written_preedit(state, identity);
    } else if record_composition_end_failure(state, identity) {
        return Ok(());
    }
    result
}

fn finish_exhausted_composition_end(ec: u32, state: &SharedState, pending: PendingCompositionEnd) {
    let identity = pending.composition.as_raw() as usize;
    if pending
        .panel_anchor
        .as_ref()
        .is_some_and(|anchor| anchor.composition().is_none())
    {
        forget_written_preedit(state, identity);
        return;
    }
    let written_preedit = state
        .borrow()
        .written_preedits
        .iter()
        .find(|(written, _, _)| *written == identity)
        .map(|(_, _, text)| text.clone());
    track_composition_in_flight(state, Some(&pending.composition));
    unsafe {
        if let Some((range, last_preedit)) = &pending.terminated_range {
            if let Ok(range) = range
                .clone()
                .ok_or_else(|| E_FAIL.into())
                .or_else(|_: windows::core::Error| pending.composition.GetRange())
            {
                let mut text = vec![0u16; last_preedit.encode_utf16().count() + 1];
                let mut fetched = 0;
                if range.GetText(ec, 0, &mut text, &mut fetched).is_ok()
                    && String::from_utf16(&text[..fetched as usize]).is_ok_and(|text| {
                        leftover_to_delete_on_host_termination(&text, last_preedit, false)
                    })
                {
                    let _ = range.SetText(ec, 0, &[]);
                }
            }
        } else {
            if pending.clear_text {
                if let (Some(last_preedit), Ok(range)) =
                    (written_preedit.as_deref(), pending.composition.GetRange())
                {
                    let mut text = vec![0u16; last_preedit.encode_utf16().count() + 1];
                    let mut fetched = 0;
                    if range.GetText(ec, 0, &mut text, &mut fetched).is_ok()
                        && String::from_utf16(&text[..fetched as usize]).is_ok_and(|text| {
                            leftover_to_delete_on_host_termination(&text, last_preedit, false)
                        })
                    {
                        clear_composition_display_attribute(ec, &pending.context, &range);
                        let _ = range.SetText(ec, 0, &[]);
                    }
                }
            }
            let _ = pending.composition.EndComposition(ec);
        }
    }
    track_composition_in_flight(state, None);
    forget_written_preedit(state, identity);
}

fn finish_pending_composition_ends_first(
    ec: u32,
    context: &ITfContext,
    state: &SharedState,
) -> Result<()> {
    loop {
        let exhausted = {
            let mut st = state.borrow_mut();
            st.pending_composition_ends
                .iter()
                .position(|pending| {
                    pending.exhausted && pending.context.as_raw() == context.as_raw()
                })
                .map(|index| st.pending_composition_ends.remove(index))
        };
        let Some(pending) = exhausted else {
            break;
        };
        finish_exhausted_composition_end(ec, state, pending);
    }
    loop {
        let pending = {
            let st = state.borrow();
            composition_end_to_finish_first(
                st.pending_composition_ends.iter().map(|pending| {
                    (
                        pending.context.as_raw() as usize,
                        !pending.exhausted && pending.queued,
                    )
                }),
                context.as_raw() as usize,
            )
            .map(|index| st.pending_composition_ends[index].composition.as_raw() as usize)
        };
        let Some(identity) = pending else {
            break;
        };
        finish_pending_composition_end(ec, state, identity)?;
    }
    if state
        .borrow()
        .pending_composition_ends
        .iter()
        .any(|pending| !pending.exhausted && pending.context.as_raw() == context.as_raw())
    {
        return Err(E_FAIL.into());
    }
    Ok(())
}

pub(crate) fn queue_terminated_preedit_cleanup(state: &SharedState, composition: &ITfComposition) {
    let identity = composition.as_raw() as usize;
    let written = {
        let mut st = state.borrow_mut();
        st.written_preedits
            .iter()
            .position(|(written, _, _)| *written == identity)
            .map(|index| st.written_preedits.remove(index))
    };
    if let Some((_, context, text)) = written {
        if discard_disconnected_context(state, &context, false) {
            return;
        }
        let range = unsafe { composition.GetRange().ok() };
        state
            .borrow_mut()
            .pending_composition_ends
            .push(PendingCompositionEnd {
                context,
                composition: composition.clone(),
                queued: false,
                clear_text: true,
                terminated_range: Some((range, text)),
                panel_anchor: None,
                failures: 0,
                retry_after: None,
                exhausted: false,
            });
    }
}

pub(crate) fn queue_panel_anchor_end(state: &SharedState, anchor: PanelAnchor) {
    let Some(composition) = anchor.composition() else {
        return;
    };
    let identity = composition.as_raw() as usize;
    defer_failed_composition_end(state, anchor.context(), composition, false);
    if let Some(pending) = state
        .borrow_mut()
        .pending_composition_ends
        .iter_mut()
        .find(|pending| pending.composition.as_raw() as usize == identity)
    {
        pending.panel_anchor = Some(anchor);
    }
}

fn try_terminate_pending_composition(shared_state: &SharedState, identity: usize) -> bool {
    let current = shared_state
        .borrow()
        .pending_composition_ends
        .iter()
        .find(|entry| entry.composition.as_raw() as usize == identity)
        .cloned();
    let Some(current) = current else {
        return true;
    };
    if current.terminated_range.is_none() && may_terminate_composition(current.clear_text, false) {
        if let (Ok(services), Ok(view)) = (
            current.context.cast::<ITfContextOwnerCompositionServices>(),
            current.composition.cast::<ITfCompositionView>(),
        ) {
            if unsafe { services.TerminateComposition(&view) }.is_ok() {
                drop(take_pending_composition_end(shared_state, identity));
                forget_written_preedit(shared_state, identity);
                return true;
            }
        }
    }
    false
}

/// Clear owned preedit under a write lock before any lock-free termination.
pub(crate) fn flush_pending_composition_ends(shared_state: &SharedState) {
    let pending = {
        let mut st = shared_state.borrow_mut();
        if st.ime_write_session_active
            || st.caret_probe_session_in_progress
            || st.composition_end_flush_in_progress
        {
            return;
        }
        st.composition_end_flush_in_progress = true;
        st.pending_composition_ends
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    };
    let _guard = CompositionEndFlushGuard(shared_state.clone());
    for pending in pending {
        if discard_disconnected_context(shared_state, &pending.context, false) {
            continue;
        }
        if pending.exhausted
            || pending.queued
            || pending
                .retry_after
                .is_some_and(|deadline| Instant::now() < deadline)
        {
            continue;
        }
        let identity = pending.composition.as_raw() as usize;
        let client_id = shared_state.borrow().client_id;
        if client_id == 0 {
            continue;
        }
        if let Some(entry) = shared_state
            .borrow_mut()
            .pending_composition_ends
            .iter_mut()
            .find(|entry| entry.composition.as_raw() as usize == identity)
        {
            entry.queued = true;
        }
        let make_apply = |entered_session: Rc<Cell<bool>>| {
            let state = shared_state.clone();
            move |ec, ctx: &ITfContext| {
                entered_session.set(true);
                let result = {
                    let _guard = ImeWriteSessionGuard::enter(&state);
                    finish_pending_composition_end(ec, &state, identity)
                };
                if result.as_ref().is_err_and(|error| {
                    discard_disconnected_context(&state, ctx, error.code() == TF_E_DISCONNECTED)
                }) {
                    return result;
                }
                if result.is_ok() {
                    post_pending_ime_ui(&state);
                }
                result
            }
        };
        let entered = Rc::new(Cell::new(false));
        let sync_attempt = pending.failures == 0;
        let result = if sync_attempt {
            with_write_session(&pending.context, client_id, make_apply(entered.clone()))
        } else {
            with_async_write_session(&pending.context, client_id, make_apply(entered.clone()))
        };
        if let Err(error) = result {
            if discard_disconnected_context(
                shared_state,
                &pending.context,
                error.code() == TF_E_DISCONNECTED,
            ) {
                continue;
            }
            if !should_backoff_composition_end_failure(sync_attempt, entered.get()) {
                if try_terminate_pending_composition(shared_state, identity) {
                    continue;
                }
                let entered_async = Rc::new(Cell::new(false));
                if let Err(async_error) = with_async_write_session(
                    &pending.context,
                    client_id,
                    make_apply(entered_async.clone()),
                ) {
                    if discard_disconnected_context(
                        shared_state,
                        &pending.context,
                        async_error.code() == TF_E_DISCONNECTED,
                    ) {
                        continue;
                    }
                    if !entered_async.get()
                        && record_composition_end_failure(shared_state, identity)
                    {
                        continue;
                    }
                    let _ = try_terminate_pending_composition(shared_state, identity);
                }
                continue;
            }
            if !entered.get() && record_composition_end_failure(shared_state, identity) {
                continue;
            }
            let _ = try_terminate_pending_composition(shared_state, identity);
        }
    }
    if {
        let st = shared_state.borrow();
        has_active_composition_ends(&st)
            || st.pending_ime_apply.is_some()
            || st.pending_commit_retry.is_some()
    } {
        let weak = Rc::downgrade(shared_state);
        crate::state::with_detached_windows(shared_state, false, |window, _| {
            window.arm_deferred_keys(&weak);
        });
    }
}

fn end_composition_or_defer(
    ec: u32,
    context: &ITfContext,
    shared_state: &SharedState,
    composition: &ITfComposition,
    clear_text: bool,
) {
    let ended = if clear_text {
        end_composition(ec, context, composition, None)
    } else {
        unsafe { composition.EndComposition(ec) }
    };
    if ended.is_err() {
        defer_failed_composition_end(shared_state, context, composition, clear_text);
    } else {
        forget_written_preedit(shared_state, composition.as_raw() as usize);
        drop(take_pending_composition_end(
            shared_state,
            composition.as_raw() as usize,
        ));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaretSource {
    Probe,
    Cache,
    System,
    Accessibility,
    View,
    Window,
}

pub(crate) fn should_arm_caret_reprobe(source: Option<CaretSource>) -> bool {
    source != Some(CaretSource::Probe)
}

fn should_schedule_layout_reposition(
    has_content: bool,
    lcode: TfLayoutCode,
    pending: bool,
) -> bool {
    has_content && lcode == TF_LC_CHANGE && !pending
}

fn caret_probe_extent_is_usable(rect: &RECT, clipped: bool) -> bool {
    !clipped && keytao_core::caret_extent_is_usable(rect.left, rect.top, rect.right, rect.bottom)
}

fn system_caret_extent_is_usable(rect: &RECT) -> bool {
    rect.right >= rect.left
        && keytao_core::caret_extent_is_usable(rect.left, rect.top, rect.right, rect.bottom)
        && (rect.left != 0 || rect.top != 0)
}

fn has_same_root_window(caret_hwnd: HWND, owner_hwnd: HWND) -> bool {
    if caret_hwnd.0.is_null() || owner_hwnd.0.is_null() {
        return false;
    }
    unsafe {
        let caret_root = GetAncestor(caret_hwnd, GA_ROOT);
        let owner_root = GetAncestor(owner_hwnd, GA_ROOT);
        !caret_root.0.is_null() && caret_root == owner_root
    }
}

fn caret_is_inside_monitor(x: i32, y: i32, monitor: &RECT) -> bool {
    x >= monitor.left && x <= monitor.right && y >= monitor.top && y <= monitor.bottom
}

fn caret_is_inside_owner_monitor(owner_hwnd: HWND, x: i32, y: i32) -> bool {
    if owner_hwnd.0.is_null() {
        return true;
    }
    unsafe {
        let monitor = MonitorFromWindow(owner_hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if monitor.0.is_null() || !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return true;
        }
        caret_is_inside_monitor(x, y, &info.rcMonitor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdjacentChar {
    Before,
    After,
}

impl AdjacentChar {
    fn caret_position(self, rect: &RECT) -> (i32, i32) {
        let x = match self {
            Self::Before => rect.right,
            Self::After => rect.left,
        };
        (x, rect.bottom)
    }
}

/// Get caret screen position and the owner HWND from the current context view.
fn probe_caret(
    ec: u32,
    context: &ITfContext,
    composition: Option<&ITfComposition>,
    ime_state: &ImeState,
    embedded: bool,
) -> CaretProbe {
    unsafe {
        let Ok(view) = context.GetActiveView() else {
            return CaretProbe {
                owner_hwnd: fallback_focus_window(),
                position: None,
                viewport: None,
            };
        };
        let owner_hwnd = view
            .GetWnd()
            .ok()
            .filter(|hwnd| !hwnd.0.is_null())
            .unwrap_or_else(fallback_focus_window);

        let log_probe = |kind: &str,
                         hr: u32,
                         rect: &RECT,
                         clipped: i32,
                         usable: bool,
                         shift: Option<(u32, i32)>| {
            if !diagnostics_enabled() {
                return;
            }
            if let Some((shift_hr, shifted)) = shift {
                append_diagnostic(format!(
                    "caret probe range={kind} hr=0x{hr:08X} rect=({},{},{},{}) clipped={clipped} usable={} ctx=0x{:x} owner=0x{:x} shift_hr=0x{shift_hr:08X} shifted={shifted}",
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    u8::from(usable),
                    context.as_raw() as usize,
                    owner_hwnd.0 as usize,
                ));
            } else {
                append_diagnostic(format!(
                    "caret probe range={kind} hr=0x{hr:08X} rect=({},{},{},{}) clipped={clipped} usable={} ctx=0x{:x} owner=0x{:x}",
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom,
                    u8::from(usable),
                    context.as_raw() as usize,
                    owner_hwnd.0 as usize,
                ));
            }
        };

        let try_range =
            |kind: &str, range: &ITfRange, adjacent: AdjacentChar, shift: Option<(u32, i32)>| {
                let mut rect = RECT::default();
                let mut clipped = BOOL::default();
                let result = view.GetTextExt(ec, range, &mut rect, &mut clipped);
                let hr = result
                    .as_ref()
                    .err()
                    .map_or(0, |error| error.code().0 as u32);
                let (x, y) = adjacent.caret_position(&rect);
                let usable = result.is_ok()
                    && caret_probe_extent_is_usable(&rect, clipped.as_bool())
                    && caret_is_inside_owner_monitor(owner_hwnd, x, y);
                log_probe(kind, hr, &rect, clipped.0, usable, shift);
                usable.then_some((x, y))
            };

        let try_adjacent =
            |kind: &str, caret: &ITfRange, adjacent: AdjacentChar| -> Option<(i32, i32)> {
                let range = match caret.Clone() {
                    Ok(range) => range,
                    Err(error) => {
                        log_probe(
                            kind,
                            error.code().0 as u32,
                            &RECT::default(),
                            0,
                            false,
                            None,
                        );
                        return None;
                    }
                };
                let requested = match adjacent {
                    AdjacentChar::Before => -1,
                    AdjacentChar::After => 1,
                };
                let mut shifted = 0;
                let shift_result = match adjacent {
                    AdjacentChar::Before => {
                        range.ShiftStart(ec, requested, &mut shifted, std::ptr::null())
                    }
                    AdjacentChar::After => {
                        range.ShiftEnd(ec, requested, &mut shifted, std::ptr::null())
                    }
                };
                let shift_hr = shift_result
                    .as_ref()
                    .err()
                    .map_or(0, |error| error.code().0 as u32);
                if shift_result.is_err() || shifted != requested {
                    log_probe(
                        kind,
                        shift_hr,
                        &RECT::default(),
                        0,
                        false,
                        Some((shift_hr, shifted)),
                    );
                    return None;
                }
                try_range(kind, &range, adjacent, Some((shift_hr, shifted)))
            };

        let mut selections = [TF_SELECTION::default()];
        let mut count: u32 = 0;
        let selection_result =
            context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selections, &mut count);
        let selection_anchor = selections[0].style.ase;
        let range = take_selection_range(&mut selections[0]);
        if selection_result.is_ok() && count > 0 {
            if let Some(range) = range {
                let caret = range.Clone().and_then(|caret| {
                    let anchor = if selection_anchor == TF_AE_START {
                        TF_ANCHOR_START
                    } else {
                        TF_ANCHOR_END
                    };
                    caret.Collapse(ec, anchor)?;
                    Ok(caret)
                });
                if let Ok(caret) = caret {
                    for (kind, adjacent) in [
                        ("selection-after", AdjacentChar::After),
                        ("selection-before", AdjacentChar::Before),
                    ] {
                        if let Some(position) = try_adjacent(kind, &caret, adjacent) {
                            return CaretProbe {
                                owner_hwnd,
                                position: Some(position),
                                viewport: None,
                            };
                        }
                    }
                    // Empty editors have no adjacent character to expand into.
                    // They can still report a valid, zero-width caret extent.
                    if let Some(position) =
                        try_range("selection-caret", &caret, AdjacentChar::After, None)
                    {
                        return CaretProbe {
                            owner_hwnd,
                            position: Some(position),
                            viewport: None,
                        };
                    }
                }
            }
        }

        if let Some(composition) = composition {
            let cursor_offset = if embedded {
                utf16_offset_from_chars(&ime_state.preedit, ime_state.cursor).min(i32::MAX as usize)
                    as i32
            } else {
                0
            };
            let caret = (|| -> Result<ITfRange> {
                let range = composition.GetRange()?;
                let range = range.Clone()?;
                range.Collapse(ec, TF_ANCHOR_START)?;
                if cursor_offset > 0 {
                    let mut shifted = 0;
                    range.ShiftEnd(ec, cursor_offset, &mut shifted, std::ptr::null())?;
                    if shifted != cursor_offset {
                        return Err(E_FAIL.into());
                    }
                    range.Collapse(ec, TF_ANCHOR_END)?;
                }
                Ok(range)
            })();
            match caret {
                Ok(caret) => {
                    let attempts = if cursor_offset == 0 {
                        [
                            ("composition-after", AdjacentChar::After),
                            ("composition-before", AdjacentChar::Before),
                        ]
                    } else {
                        [
                            ("composition-before", AdjacentChar::Before),
                            ("composition-after", AdjacentChar::After),
                        ]
                    };
                    for (kind, adjacent) in attempts {
                        if let Some(position) = try_adjacent(kind, &caret, adjacent) {
                            return CaretProbe {
                                owner_hwnd,
                                position: Some(position),
                                viewport: None,
                            };
                        }
                    }
                    if let Some(position) =
                        try_range("composition-caret", &caret, AdjacentChar::After, None)
                    {
                        return CaretProbe {
                            owner_hwnd,
                            position: Some(position),
                            viewport: None,
                        };
                    }
                }
                Err(error) => log_probe(
                    "composition-cursor",
                    error.code().0 as u32,
                    &RECT::default(),
                    0,
                    false,
                    None,
                ),
            }
        }

        CaretProbe {
            owner_hwnd,
            position: None,
            viewport: view.GetScreenExt().ok().filter(valid_panel_area),
        }
    }
}

/// Turn a probe into a position to draw at, remembering the last good caret.
fn resolve_caret(
    shared_state: &SharedState,
    probe: CaretProbe,
    embedded: bool,
    allow_panel_fallback: bool,
) -> Option<(CaretPosition, CaretSource)> {
    if let Some((x, y)) = probe.position {
        let caret = CaretPosition {
            x,
            y,
            owner_hwnd: probe.owner_hwnd,
        };
        shared_state.borrow_mut().last_caret = Some(caret);
        if diagnostics_enabled() {
            append_diagnostic(format!("caret resolved via=probe x={x} y={y}"));
        }
        return Some((caret, CaretSource::Probe));
    }
    if !embedded {
        if let Some(caret) = resolved_system_caret(probe.owner_hwnd) {
            return Some(caret);
        }
    }
    let cached = shared_state.borrow().last_caret;
    if let Some(mut caret) = cached {
        if !probe.owner_hwnd.0.is_null() {
            caret.owner_hwnd = probe.owner_hwnd;
        }
        if caret_is_inside_owner_monitor(caret.owner_hwnd, caret.x, caret.y) {
            if diagnostics_enabled() {
                append_diagnostic(format!(
                    "caret resolved via=cache x={} y={}",
                    caret.x, caret.y
                ));
            }
            return Some((caret, CaretSource::Cache));
        }
    }
    if embedded {
        if let Some(caret) = resolved_system_caret(probe.owner_hwnd) {
            return Some(caret);
        }
    }
    if let Some(rect) = crate::accessible_caret::accessible_caret_rect(probe.owner_hwnd) {
        if rect.right >= rect.left
            && keytao_core::caret_extent_is_usable(rect.left, rect.top, rect.right, rect.bottom)
            && caret_is_inside_owner_monitor(probe.owner_hwnd, rect.left, rect.bottom)
        {
            if diagnostics_enabled() {
                append_diagnostic(format!(
                    "caret resolved via=accessibility x={} y={}",
                    rect.left, rect.bottom
                ));
            }
            return Some((
                CaretPosition {
                    x: rect.left,
                    y: rect.bottom,
                    owner_hwnd: probe.owner_hwnd,
                },
                CaretSource::Accessibility,
            ));
        }
    }
    if allow_panel_fallback {
        let visible_client = visible_owner_client(probe.owner_hwnd);
        if let Some(((x, y), source)) =
            panel_fallback_position(probe.viewport, visible_client, allow_panel_fallback)
        {
            if diagnostics_enabled() {
                append_diagnostic(format!(
                    "caret resolved via={} x={x} y={y} owner=0x{:x}",
                    if source == CaretSource::View {
                        "view"
                    } else {
                        "window"
                    },
                    probe.owner_hwnd.0 as usize,
                ));
            }
            // Do not cache this position or label it Probe: layout callbacks
            // and the bounded retry ladder must immediately prefer a real caret.
            return Some((
                CaretPosition {
                    x,
                    y,
                    owner_hwnd: probe.owner_hwnd,
                },
                source,
            ));
        }
    }
    if diagnostics_enabled() {
        append_diagnostic("caret resolved via=none x=0 y=0");
    }
    None
}

fn valid_panel_area(rect: &RECT) -> bool {
    rect.right > rect.left && rect.bottom > rect.top
}

fn intersect_panel_areas(first: RECT, second: RECT) -> Option<RECT> {
    if !valid_panel_area(&first) || !valid_panel_area(&second) {
        return None;
    }
    let intersection = RECT {
        left: first.left.max(second.left),
        top: first.top.max(second.top),
        right: first.right.min(second.right),
        bottom: first.bottom.min(second.bottom),
    };
    valid_panel_area(&intersection).then_some(intersection)
}

/// Geometry only. `visible_client` is supplied only after ownership, focus,
/// visibility and clipping checks; an arbitrary TSF viewport is not sufficient.
fn panel_fallback_position(
    viewport: Option<RECT>,
    visible_client: Option<RECT>,
    has_panel_content: bool,
) -> Option<((i32, i32), CaretSource)> {
    if !has_panel_content {
        return None;
    }
    let client = visible_client.filter(valid_panel_area)?;
    let (area, source) = match viewport.and_then(|view| intersect_panel_areas(view, client)) {
        Some(area) => (area, CaretSource::View),
        None => (client, CaretSource::Window),
    };
    let width = i64::from(area.right) - i64::from(area.left);
    let height = i64::from(area.bottom) - i64::from(area.top);
    let x = i64::from(area.left) + 8.min(width - 1);
    let y = i64::from(area.top) + 8.min(height - 1);
    Some(((x as i32, y as i32), source))
}

/// Never place a fallback panel over an unrelated or hidden application. TSF
/// can hand back a stale HWND, so check its current owner rather than trusting it.
fn visible_owner_client(owner: HWND) -> Option<RECT> {
    unsafe {
        if owner.0.is_null() || !IsWindow(owner).as_bool() || !IsWindowVisible(owner).as_bool() {
            return None;
        }
        let mut process = 0;
        if GetWindowThreadProcessId(owner, Some(&mut process)) != GetCurrentThreadId()
            || process != GetCurrentProcessId()
        {
            return None;
        }
        let focus = fallback_focus_window();
        if !has_same_root_window(focus, owner)
            || !has_same_root_window(GetForegroundWindow(), owner)
        {
            return None;
        }
        let root = GetAncestor(owner, GA_ROOT);
        if root.0.is_null()
            || !IsWindowVisible(root).as_bool()
            || IsIconic(owner).as_bool()
            || IsIconic(root).as_bool()
        {
            return None;
        }
        let owner_client = client_screen_rect(owner)?;
        let root_client = client_screen_rect(root)?;
        let visible = intersect_panel_areas(owner_client, root_client)?;
        let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if monitor.0.is_null() || !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        intersect_panel_areas(visible, info.rcWork)
    }
}

unsafe fn client_screen_rect(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    GetClientRect(window, &mut rect).ok()?;
    if !valid_panel_area(&rect) {
        return None;
    }
    let mut start = POINT {
        x: rect.left,
        y: rect.top,
    };
    let mut end = POINT {
        x: rect.right,
        y: rect.bottom,
    };
    if !ClientToScreen(window, &mut start).as_bool() || !ClientToScreen(window, &mut end).as_bool()
    {
        return None;
    }
    let rect = RECT {
        left: start.x,
        top: start.y,
        right: end.x,
        bottom: end.y,
    };
    valid_panel_area(&rect).then_some(rect)
}

fn resolved_system_caret(owner_hwnd: HWND) -> Option<(CaretPosition, CaretSource)> {
    let caret = system_caret(owner_hwnd)?;
    if diagnostics_enabled() {
        append_diagnostic(format!(
            "caret resolved via=system x={} y={}",
            caret.x, caret.y
        ));
    }
    Some((caret, CaretSource::System))
}

/// Use a validated system caret when the host cannot report a text extent.
fn system_caret(owner_hwnd: HWND) -> Option<CaretPosition> {
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
        if GetGUIThreadInfo(GetCurrentThreadId(), &mut info).is_ok()
            && !info.hwndCaret.0.is_null()
            && has_same_root_window(info.hwndCaret, owner_hwnd)
            && system_caret_extent_is_usable(&info.rcCaret)
        {
            let mut point = POINT {
                x: info.rcCaret.left,
                y: info.rcCaret.bottom,
            };
            if ClientToScreen(info.hwndCaret, &mut point).as_bool() {
                if !caret_is_inside_owner_monitor(owner_hwnd, point.x, point.y) {
                    return None;
                }
                return Some(CaretPosition {
                    x: point.x,
                    y: point.y,
                    owner_hwnd,
                });
            }
        }
    }
    None
}

struct ImeWriteSessionGuard {
    state: SharedState,
    previous: Option<(Option<usize>, bool)>,
    active: bool,
}

impl ImeWriteSessionGuard {
    fn enter(state: &SharedState) -> Self {
        let previous = {
            let mut st = state.borrow_mut();
            let previous = st
                .ime_write_session_active
                .then_some((st.composition_in_flight, st.composition_clear_pending));
            st.ime_write_session_active = true;
            st.composition_in_flight = None;
            st.composition_clear_pending = false;
            previous
        };
        Self {
            state: Rc::clone(state),
            previous,
            active: true,
        }
    }

    fn finish(&mut self) {
        if !self.active {
            return;
        }
        let mut st = self.state.borrow_mut();
        st.ime_write_session_active = self.previous.is_some();
        (st.composition_in_flight, st.composition_clear_pending) =
            self.previous.unwrap_or((None, false));
        self.active = false;
    }
}

impl Drop for ImeWriteSessionGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

struct CaretProbeSessionGuard {
    state: SharedState,
}

impl CaretProbeSessionGuard {
    fn enter(state: &SharedState, context: &ITfContext) -> Option<Self> {
        let mut st = state.borrow_mut();
        let context_mismatch = st
            .panel_context
            .as_ref()
            .is_some_and(|active| active.as_raw() != context.as_raw());
        if context_mismatch || st.ime_write_session_active || st.caret_probe_session_in_progress {
            return None;
        }
        st.caret_probe_session_in_progress = true;
        drop(st);
        Some(Self {
            state: Rc::clone(state),
        })
    }
}

impl Drop for CaretProbeSessionGuard {
    fn drop(&mut self) {
        self.state.borrow_mut().caret_probe_session_in_progress = false;
    }
}

fn track_composition_in_flight(shared_state: &SharedState, composition: Option<&ITfComposition>) {
    shared_state.borrow_mut().composition_in_flight =
        composition.map(|composition| composition.as_raw() as usize);
}

fn queue_lost_commit_once(
    context: &ITfContext,
    client_id: u32,
    shared_state: &SharedState,
    ime_state: &ImeState,
    scheduled: &Cell<bool>,
    input_generation: u64,
    committed_written: &Cell<bool>,
) {
    if !should_retry_commit(committed_written.get()) || scheduled.replace(true) {
        return;
    }
    let Some(committed) = ime_state
        .committed
        .as_deref()
        .filter(|text| !text.is_empty())
    else {
        return;
    };
    let committed_len = committed.chars().count();
    keytao_core::rt_log!(Level::Info, "error", "commit_lost", len = committed_len);
    let mut st = shared_state.borrow_mut();
    if st.input_generation != input_generation || st.client_id != client_id {
        return;
    }
    st.pending_commit_retry = Some(PendingCommitRetry {
        context: context.clone(),
        client_id,
        input_generation,
        ime_state: ImeState {
            committed: Some(committed.to_owned()),
            ascii_mode: ime_state.ascii_mode,
            schema_id: ime_state.schema_id.clone(),
            schema_name: ime_state.schema_name.clone(),
            ..ImeState::empty()
        },
    });
}

pub(crate) fn flush_pending_commit_retry(shared_state: &SharedState) {
    if shared_state.borrow().pending_ime_apply.is_some() {
        return;
    }
    let Some(retry) = shared_state.borrow_mut().pending_commit_retry.take() else {
        return;
    };
    if !commit_retry_is_current(shared_state, &retry) {
        return;
    }
    if shared_state
        .borrow()
        .pending_composition_ends
        .iter()
        .any(|pending| !pending.exhausted && pending.context.as_raw() == retry.context.as_raw())
    {
        shared_state.borrow_mut().pending_commit_retry = Some(retry);
        return;
    }
    let _ = apply_ime_state(
        &retry.context,
        retry.client_id,
        shared_state,
        retry.ime_state,
        false,
        true,
        None,
    );
}

fn commit_retry_is_current(shared_state: &SharedState, retry: &PendingCommitRetry) -> bool {
    let st = shared_state.borrow();
    st.client_id != 0
        && st.client_id == retry.client_id
        && st.input_generation == retry.input_generation
}

fn window_update_context_matches(
    has_ime_state: bool,
    expected: usize,
    key_context: Option<usize>,
    panel_context: Option<usize>,
) -> bool {
    has_ime_state && key_context.or(panel_context) == Some(expected)
}

thread_local! {
    // A provider may synchronously flush a newer update for the same context.
    // Context identity and an empty pending slot alone cannot detect that case.
    static WINDOW_UPDATE_GENERATION: Cell<u64> = const { Cell::new(0) };
}

fn next_window_update_generation() -> u64 {
    WINDOW_UPDATE_GENERATION.with(|current| {
        let next = current.get().wrapping_add(1);
        current.set(next);
        next
    })
}

fn window_update_generation_is_current(generation: u64) -> bool {
    WINDOW_UPDATE_GENERATION.with(|current| current.get() == generation)
}

fn window_update_is_current(
    shared_state: &SharedState,
    update: &PendingWindowUpdate,
    generation: u64,
) -> bool {
    if !window_update_generation_is_current(generation) {
        return false;
    }
    let st = shared_state.borrow();
    window_update_context_matches(
        st.ime_state.is_some() || st.deployment_hint.is_some(),
        update.context.as_raw() as usize,
        st.key_context
            .as_ref()
            .map(|context| context.as_raw() as usize),
        st.panel_context
            .as_ref()
            .map(|context| context.as_raw() as usize),
    ) && st.input_generation == update.input_generation
        && st.pending_window_update.is_none()
}

pub(crate) fn flush_pending_window_update(shared_state: &SharedState) {
    let Some(mut update) = shared_state.borrow_mut().pending_window_update.take() else {
        return;
    };
    let generation = next_window_update_generation();
    if !window_update_is_current(shared_state, &update, generation) {
        return;
    }

    if !update.reposition_only {
        if has_visible_state(&update.ime_state) {
            ensure_layout_sink(shared_state, &update.context);
        } else {
            clear_layout_sink(shared_state);
        }
    }
    if !window_update_is_current(shared_state, &update, generation) {
        return;
    }
    let has_visible_panel =
        has_visible_state(&update.ime_state) || shared_state.borrow().deployment_hint.is_some();
    let caret = resolve_caret(
        shared_state,
        update.caret.clone(),
        update.embedded,
        has_visible_panel,
    );
    // Accessibility providers can synchronously reenter TSF and move focus,
    // even to another document in the same HWND. Never resurrect the old panel
    // after such a callback or overwrite a newer pending update.
    if !window_update_is_current(shared_state, &update, generation) {
        return;
    }
    let caret_source = caret.map(|(_, source)| source);
    {
        let mut st = shared_state.borrow_mut();
        if st.ime_state.is_none() && st.deployment_hint.is_none() {
            return;
        }
        if caret_source == Some(CaretSource::Probe) {
            st.caret_retry_armed = true;
            st.caret_retry_active = false;
        }
        st.caret_retry_mode_hint = update.show_mode_hint && should_arm_caret_reprobe(caret_source);
        st.panel_context = (has_visible_state(&update.ime_state) || st.deployment_hint.is_some())
            .then(|| update.context.clone());
    }
    if caret_source == Some(CaretSource::Probe) {
        crate::state::stop_caret_reprobe(shared_state);
    }
    if update.reposition_only
        && !update.show_mode_hint
        && shared_state.borrow().deployment_hint.is_none()
        && caret.map(|(position, _)| position) == shared_state.borrow().last_shown_caret
    {
        return;
    }
    if !update.reposition_only {
        update_language_bar_mode(shared_state, update.ime_state.is_english_mode());
    }
    if !window_update_is_current(shared_state, &update, generation) {
        return;
    }
    let updated = update_ime_windows(
        shared_state,
        &update.ime_state,
        update.document_mgr.as_ref(),
        caret,
        update.show_mode_hint,
        update.embedded,
    );
    if let Some(started) = update.key_started.take() {
        runtime_log::record_keystroke(started.elapsed().as_secs_f64() * 1000.0);
    }
    if !updated {
        if !window_update_is_current(shared_state, &update, generation) {
            return;
        }
        let mut st = shared_state.borrow_mut();
        if st.pending_window_update.is_none() {
            st.pending_window_update = Some(update);
        }
    }
}

fn finish_panel_anchor(ec: u32, anchor: Option<PanelAnchor>, client_id: u32, state: &SharedState) {
    if let Some(anchor) = anchor {
        if let Err(error) = anchor.end(ec) {
            append_key_diagnostic!("panel anchor end hr=0x{:08X}", error.code().0 as u32);
            crate::state::request_panel_anchor_end(state, anchor, client_id);
        }
    }
}

fn apply_ime_state(
    context: &ITfContext,
    client_id: u32,
    shared_state: &SharedState,
    ime_state: ImeState,
    show_mode_hint_on_change: bool,
    async_dontcare: bool,
    key_started: Option<Instant>,
) -> Result<()> {
    if shared_state.borrow().pending_ime_apply.is_some() {
        return Err(E_UNEXPECTED.into());
    }
    let update_generation = {
        let mut st = shared_state.borrow_mut();
        st.ime_update_generation = st.ime_update_generation.wrapping_add(1);
        st.ime_update_generation
    };
    let state_arc = Rc::clone(shared_state);
    let state_arc_for_session = Rc::clone(&state_arc);
    let ime_state_clone = ime_state.clone();
    let failed_commit_retry_scheduled = Rc::new(Cell::new(false));
    let failed_commit_retry_for_session = Rc::clone(&failed_commit_retry_scheduled);
    let committed_written = Rc::new(Cell::new(false));
    let committed_written_for_session = Rc::clone(&committed_written);
    let committed_for_retry = ime_state
        .committed
        .as_deref()
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let comp_sink_obj = CompositionSink {
        state: Rc::downgrade(shared_state),
        _dll_guard: DllActivityGuard::new(),
    };
    let comp_sink_iface: ITfCompositionSink = comp_sink_obj.into();
    let hide_candidate_panel = with_current_theme(|theme| theme.ui.hide_candidate_panel);
    let (display_attribute_atom, uiless) = {
        let st = shared_state.borrow();
        (
            if hide_candidate_panel {
                st.converted_display_attribute_atom
            } else {
                st.display_attribute_atom
            },
            host_is_uiless(&st),
        )
    };
    // `BeginUIElement(pbShow=FALSE)` is only known after this apply. If UIless
    // coverage expands, cache CandidateUiManager::host_allows_window in
    // TsfState and include it in this effective-mode decision.
    let embedded = embedded_composition() || uiless;
    {
        let mut st = shared_state.borrow_mut();
        st.embedded_mode_cached = embedded;
        st.caret_retry_attempts = 0;
        st.caret_retry_armed = false;
        st.caret_retry_active = false;
        st.caret_retry_mode_hint = false;
    }
    crate::state::stop_caret_reprobe(shared_state);
    let display_attribute_atom = if embedded {
        display_attribute_atom
    } else {
        None
    };
    let plan = plan_composition(&ime_state, embedded, hide_candidate_panel);
    if diagnostics_enabled() {
        let mode = if embedded { "embedded" } else { "panel" };
        let target = match plan.target {
            CompositionTarget::None => "none",
            CompositionTarget::Preedit => "preedit",
        };
        append_diagnostic(format!(
            "ime apply mode={mode} uiless={} preedit_len={} candidates={} commit={} target={target}",
            u8::from(uiless),
            ime_state.preedit.chars().count(),
            ime_state.candidates.len(),
            u8::from(plan.commit),
        ));
    }
    let input_generation = state_arc_for_session.borrow().input_generation;
    let apply_session = move |ec: u32, ctx: &ITfContext| {
        // A queued mouse edit can run after this input session has ended.
        if {
            let st = state_arc_for_session.borrow();
            st.input_generation != input_generation
                || st.client_id == 0
                || st.client_id != client_id
        } {
            return Ok(());
        }
        if !plan.commit && state_arc_for_session.borrow().ime_update_generation != update_generation
        {
            return Ok(());
        }
        let _write_session_guard = ImeWriteSessionGuard::enter(&state_arc_for_session);
        let committed = ime_state_clone
            .committed
            .as_deref()
            .filter(|text| !text.is_empty());

        let mut composition = {
            let mut st = state_arc_for_session.borrow_mut();
            st.composition_context = None;
            st.composition.take()
        };
        track_composition_in_flight(&state_arc_for_session, composition.as_ref());
        let mut panel_anchor = state_arc_for_session.borrow_mut().panel_anchor.take();
        if panel_anchor
            .as_ref()
            .is_some_and(|anchor| anchor.context().as_raw() != ctx.as_raw())
        {
            if let Some(anchor) = panel_anchor.take() {
                crate::state::request_panel_anchor_end(&state_arc_for_session, anchor, client_id);
            }
        }
        // Anchors own no text. End them before the original selection-based
        // commit/inline path, and never run the preedit range-clearing helper.
        if plan.target != CompositionTarget::None
            || committed.is_some()
            || !has_visible_state(&ime_state_clone)
        {
            finish_panel_anchor(ec, panel_anchor.take(), client_id, &state_arc_for_session);
        }
        panel_anchor = panel_anchor.filter(|anchor| anchor.composition().is_some());
        if state_arc_for_session.borrow().input_generation != input_generation {
            finish_panel_anchor(ec, panel_anchor.take(), client_id, &state_arc_for_session);
            if let Some(comp) = composition.as_ref() {
                track_composition_in_flight(&state_arc_for_session, Some(comp));
                end_composition_or_defer(ec, ctx, &state_arc_for_session, comp, true);
                track_composition_in_flight(&state_arc_for_session, None);
            }
            return Ok(());
        }

        let mut committing_composition = false;
        let composition_owns_text = Cell::new(composition.is_some());
        let apply_result = (|| -> Result<()> {
            if let Some(committed) = committed {
                let committed_len = committed.chars().count();
                if composition.is_some() {
                    append_key_diagnostic!("commit path=composition len={committed_len}");
                } else {
                    append_key_diagnostic!("commit path=fresh-composition len={committed_len}");
                    finish_pending_composition_ends_first(ec, ctx, &state_arc_for_session)?;
                    composition = Some(start_composition(ec, ctx, &comp_sink_iface)?);
                }
                committing_composition = true;
                let comp = composition.as_ref().unwrap();
                track_composition_in_flight(&state_arc_for_session, Some(&comp));
                end_composition_with_commit_tracking(
                    ec,
                    ctx,
                    comp,
                    Some(committed),
                    Some(&committed_written_for_session),
                    Some((&state_arc_for_session, input_generation)),
                )?;
                track_composition_in_flight(&state_arc_for_session, None);
                composition = None;
                committing_composition = false;
                composition_owns_text.set(false);
            }

            match plan.target {
                CompositionTarget::None => {
                    if let Some(comp) = composition.as_ref() {
                        append_key_diagnostic!("composition end reason=clear");
                        track_composition_in_flight(&state_arc_for_session, Some(&comp));
                        end_composition(ec, ctx, &comp, None)?;
                        track_composition_in_flight(&state_arc_for_session, None);
                        forget_written_preedit(&state_arc_for_session, comp.as_raw() as usize);
                        composition = None;
                    }
                }
                CompositionTarget::Preedit => {
                    let (preedit, cursor) =
                        composition_text(&ime_state_clone, hide_candidate_panel);
                    if let Some(comp) = composition.as_ref() {
                        append_key_diagnostic!(
                            "composition update mode=embedded text_len={}",
                            preedit.chars().count()
                        );
                        update_composition_text(
                            ec,
                            ctx,
                            comp,
                            preedit,
                            cursor,
                            display_attribute_atom,
                            (&state_arc_for_session, input_generation),
                            &composition_owns_text,
                        )?;
                    } else {
                        append_key_diagnostic!(
                            "composition start mode=embedded text_len={}",
                            preedit.chars().count()
                        );
                        finish_pending_composition_ends_first(ec, ctx, &state_arc_for_session)?;
                        composition = Some(start_composition(ec, ctx, &comp_sink_iface)?);
                        track_composition_in_flight(&state_arc_for_session, composition.as_ref());
                        update_composition_text(
                            ec,
                            ctx,
                            composition.as_ref().unwrap(),
                            preedit,
                            cursor,
                            display_attribute_atom,
                            (&state_arc_for_session, input_generation),
                            &composition_owns_text,
                        )?;
                    }
                }
            }
            Ok(())
        })();

        if let Err(error) = apply_result {
            finish_panel_anchor(ec, panel_anchor.take(), client_id, &state_arc_for_session);
            if let Some(comp) = composition.as_ref() {
                track_composition_in_flight(&state_arc_for_session, Some(comp));
                if (committing_composition && committed_written_for_session.get())
                    || !composition_owns_text.get()
                {
                    // SetText already inserted the commit. Cleanup must neither
                    // erase it nor schedule a second insertion after a later
                    // selection/EndComposition/preedit failure. A fresh range
                    // whose SetText failed still contains the user's selection.
                    end_composition_or_defer(ec, ctx, &state_arc_for_session, comp, false);
                } else {
                    end_composition_or_defer(ec, ctx, &state_arc_for_session, comp, true);
                }
            }
            track_composition_in_flight(&state_arc_for_session, None);
            if state_arc_for_session.borrow().input_generation != input_generation {
                return Err(error);
            }
            let mut st = state_arc_for_session.borrow_mut();
            st.composition = None;
            st.composition_context = None;
            st.panel_context = None;
            st.ime_state = None;
            st.pending_window_update = None;
            drop(st);
            keytao_core::rt_log!(
                Level::Info,
                "error",
                "edit_session_failed",
                stage = "apply",
                hr = error.code().0,
            );
            if committed.is_some() {
                queue_lost_commit_once(
                    ctx,
                    client_id,
                    &state_arc_for_session,
                    &ime_state_clone,
                    &failed_commit_retry_for_session,
                    input_generation,
                    &committed_written_for_session,
                );
            }
            if async_dontcare {
                defer_ime_ui_hide(&state_arc_for_session);
                post_pending_ime_ui(&state_arc_for_session);
            }
            return Err(error);
        }

        if std::mem::take(&mut state_arc_for_session.borrow_mut().composition_clear_pending) {
            finish_panel_anchor(ec, panel_anchor.take(), client_id, &state_arc_for_session);
            if let Some(comp) = composition.as_ref() {
                track_composition_in_flight(&state_arc_for_session, Some(comp));
                end_composition_or_defer(ec, ctx, &state_arc_for_session, comp, true);
                track_composition_in_flight(&state_arc_for_session, None);
            }
            if async_dontcare {
                post_pending_ime_ui(&state_arc_for_session);
            }
            return Ok(());
        }
        let mut caret = probe_caret(
            ec,
            ctx,
            composition
                .as_ref()
                .or_else(|| panel_anchor.as_ref().and_then(PanelAnchor::composition)),
            &ime_state_clone,
            embedded,
        );
        if should_hold_panel_anchor(plan.target, &ime_state_clone, caret.position.is_some())
            && panel_anchor
                .as_ref()
                .and_then(PanelAnchor::composition)
                .is_none()
            && state_arc_for_session.borrow().input_generation == input_generation
        {
            // Some TSF/IMM hosts supply geometry only after WM_IME_STARTCOMPOSITION,
            // and Chromium/Electron/IMM hosts also gate `isComposing` on an active
            // TSF composition when candidate-only menus (such as F4) are open.
            // A native empty range activates that protocol without writing into the
            // document or moving the original selection.
            panel_anchor = match start_panel_anchor(ec, ctx) {
                Ok(anchor) => anchor,
                Err(error) => {
                    append_key_diagnostic!("panel anchor start hr=0x{:08X}", error.code().0 as u32);
                    None
                }
            };
            if caret.position.is_none()
                && state_arc_for_session.borrow().input_generation == input_generation
            {
                caret = probe_caret(
                    ec,
                    ctx,
                    panel_anchor.as_ref().and_then(PanelAnchor::composition),
                    &ime_state_clone,
                    false,
                );
            }
        }
        let document_mgr = unsafe { ctx.GetDocumentMgr().ok() };
        // Start/GetRange/GetTextExt/GetDocumentMgr may synchronously reset focus,
        // including leave+reenter of the same context. Never restore that state.
        if state_arc_for_session.borrow().input_generation != input_generation {
            finish_panel_anchor(ec, panel_anchor.take(), client_id, &state_arc_for_session);
            if let Some(comp) = composition.as_ref() {
                track_composition_in_flight(&state_arc_for_session, Some(comp));
                end_composition_or_defer(ec, ctx, &state_arc_for_session, comp, true);
                track_composition_in_flight(&state_arc_for_session, None);
            }
            return Ok(());
        }
        let live_panel_anchor = panel_anchor.filter(|anchor| anchor.composition().is_some());
        let mode_changed = {
            let mut st = state_arc_for_session.borrow_mut();
            let mode_changed = ime_state_clone.is_english_mode() != st.english_mode;
            // Retain the caret only for a mode-only hint without a commit.
            // Normal panel applies must discard the previous caret cache.
            let mode_only_update = (mode_changed || ime_state_clone.switch_hint.is_some())
                && !has_visible_state(&ime_state_clone);
            if committed.is_some() || (composition.is_none() && !mode_only_update) {
                st.last_caret = None;
            }
            st.composition_context = composition.as_ref().map(|_| ctx.clone());
            st.composition_in_flight = None;
            st.composition = composition;
            st.panel_anchor = live_panel_anchor;
            st.ascii_mode = ime_state_clone.ascii_mode;
            st.english_mode = ime_state_clone.is_english_mode();
            st.ime_state = Some(ime_state_clone.clone());
            mode_changed
        };
        let show_mode_hint =
            (show_mode_hint_on_change && mode_changed) || ime_state_clone.switch_hint.is_some();
        {
            let mut st = state_arc_for_session.borrow_mut();
            st.pending_window_update = Some(PendingWindowUpdate {
                context: ctx.clone(),
                input_generation,
                ime_state: ime_state_clone.clone(),
                caret,
                document_mgr,
                show_mode_hint,
                embedded,
                reposition_only: false,
                key_started,
            });
        }
        if async_dontcare {
            post_pending_ime_ui(&state_arc_for_session);
        }
        Ok(())
    };
    state_arc.borrow_mut().pending_ime_apply = Some(PendingImeApply {
        context: context.clone(),
        client_id,
        generation: update_generation,
        queued: false,
        apply: Box::new(apply_session),
    });
    let mut session_result = request_pending_ime_apply(&state_arc, async_dontcare);
    if session_result.is_err() && state_arc.borrow().pending_ime_apply.is_some() {
        // A refused lock has not executed this frame; retain it and stop engine replay.
        if !async_dontcare {
            session_result = request_pending_ime_apply(&state_arc, true);
        }
        flush_pending_ime_ui(&state_arc);
        return session_result;
    }
    if let Err(error) = &session_result {
        keytao_core::rt_log!(
            Level::Info,
            "error",
            "edit_session_failed",
            stage = "request",
            hr = error.code().0,
        );
        if committed_for_retry.is_some() {
            queue_lost_commit_once(
                context,
                client_id,
                &state_arc,
                &ime_state,
                &failed_commit_retry_scheduled,
                input_generation,
                &committed_written,
            );
        }
        // COM calls may have switched focus while the failed frame was being
        // applied. Do not clear that newer input session. A retry from this
        // frame alone survives our own cleanup, then expires on any later reset.
        if state_arc.borrow().input_generation == input_generation {
            let retry = state_arc.borrow_mut().pending_commit_retry.take();
            reset_input_for_focus_change(&state_arc);
            let mut st = state_arc.borrow_mut();
            if st.input_generation == input_generation.wrapping_add(1) {
                st.pending_commit_retry = retry.map(|mut retry| {
                    retry.input_generation = st.input_generation;
                    retry
                });
            }
        }
        flush_pending_ime_ui(&state_arc);
    } else {
        flush_pending_ime_ui(&state_arc);
    }
    session_result
}

/// Re-run the caret query for the context the panel is anchored to.
///
/// Hosts that answered `TF_E_NOLAYOUT` earlier call `OnLayoutChange` once the
/// text is laid out; scrolling and window moves report the same way.
pub(crate) fn reposition_ime_windows(
    shared_state: &SharedState,
    context: &ITfContext,
    show_mode_hint: bool,
) -> bool {
    let Some(_probe_session_guard) = CaretProbeSessionGuard::enter(shared_state, context) else {
        return false;
    };
    let (client_id, ime_state, composition, embedded, input_generation) = {
        let st = shared_state.borrow();
        (
            st.client_id,
            st.ime_state
                .clone()
                .or_else(|| st.deployment_hint.as_ref().map(|_| ImeState::empty())),
            st.composition.clone().or_else(|| {
                st.panel_anchor
                    .as_ref()
                    .and_then(PanelAnchor::composition)
                    .cloned()
            }),
            st.embedded_mode_cached,
            st.input_generation,
        )
    };
    let Some(ime_state) = ime_state else {
        return true;
    };
    if client_id == 0
        || (!has_visible_state(&ime_state)
            && !show_mode_hint
            && shared_state.borrow().deployment_hint.is_none())
    {
        return true;
    }
    let owner_hwnd = unsafe {
        context
            .GetActiveView()
            .ok()
            .and_then(|view| view.GetWnd().ok())
            .filter(|hwnd| !hwnd.0.is_null())
            .unwrap_or_else(fallback_focus_window)
    };
    let document_mgr = unsafe { context.GetDocumentMgr().ok() };

    let probe = Rc::new(RefCell::new(None));
    let probe_for_session = Rc::clone(&probe);
    let ime_state_for_probe = ime_state.clone();
    let session_result = with_read_session(context, client_id, move |ec, ctx| {
        *probe_for_session.borrow_mut() = Some(probe_caret(
            ec,
            ctx,
            composition.as_ref(),
            &ime_state_for_probe,
            embedded,
        ));
        Ok(())
    });
    let session_probe = probe.borrow_mut().take();
    let probe = match session_result {
        Ok(()) => session_probe.unwrap_or_else(|| CaretProbe {
            owner_hwnd,
            position: None,
            viewport: unsafe {
                context
                    .GetActiveView()
                    .and_then(|view| view.GetScreenExt())
                    .ok()
            }
            .filter(valid_panel_area),
        }),
        Err(error) => {
            if diagnostics_enabled() {
                append_diagnostic(format!(
                    "reposition read session hr=0x{:08X}",
                    error.code().0 as u32
                ));
            }
            CaretProbe {
                owner_hwnd,
                position: None,
                viewport: unsafe {
                    context
                        .GetActiveView()
                        .and_then(|view| view.GetScreenExt())
                        .ok()
                }
                .filter(valid_panel_area),
            }
        }
    };

    if shared_state.borrow().input_generation != input_generation {
        return true;
    }
    shared_state.borrow_mut().pending_window_update = Some(PendingWindowUpdate {
        context: context.clone(),
        input_generation,
        ime_state,
        caret: probe,
        document_mgr,
        show_mode_hint,
        embedded,
        reposition_only: true,
        key_started: None,
    });
    // The synchronous TSF read session has returned. Let the deferred UI flush
    // run now without carrying a document-lock guard into accessibility calls.
    drop(_probe_session_guard);
    flush_pending_ime_ui(shared_state);
    true
}

pub(crate) fn retry_caret_probe(shared_state: &SharedState) {
    let (attempt, context, show_mode_hint) = {
        let mut st = shared_state.borrow_mut();
        if !st.caret_retry_active
            || crate::candidate_win::caret_retry_delay_ms(st.caret_retry_attempts).is_none()
        {
            st.caret_retry_active = false;
            return;
        }
        st.caret_retry_attempts = st.caret_retry_attempts.saturating_add(1);
        (
            st.caret_retry_attempts,
            st.panel_context.clone().or_else(|| st.key_context.clone()),
            st.caret_retry_mode_hint,
        )
    };
    if diagnostics_enabled() {
        append_diagnostic(format!("caret retry attempt={attempt} fired"));
    }
    if let Some(context) = context {
        if !reposition_ime_windows(shared_state, &context, show_mode_hint) {
            // A reentrant callback did not probe; it must not spend a step.
            let mut st = shared_state.borrow_mut();
            st.caret_retry_attempts = st.caret_retry_attempts.saturating_sub(1);
        }
        crate::state::request_caret_reprobe_rearm(shared_state);
    }
}

pub(crate) fn flush_layout_reposition(shared_state: &SharedState) {
    let context = {
        let mut st = shared_state.borrow_mut();
        if !std::mem::take(&mut st.layout_reposition_pending)
            || !st.ime_state.as_ref().is_some_and(has_visible_state)
        {
            return;
        }
        st.panel_context.clone()
    };
    if let Some(context) = context {
        reposition_ime_windows(shared_state, &context, false);
    }
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

fn runtime_session(shared_state: &SharedState) -> Option<std::sync::Arc<dyn ImeEngineBackend>> {
    shared_state.borrow().session()
}

fn cached_ime_state(shared_state: &SharedState) -> Option<ImeState> {
    shared_state.borrow().ime_state.clone()
}

fn input_engine_paused(shared_state: &SharedState) -> bool {
    let (has_input, should_reload) = {
        let mut state = shared_state.borrow_mut();
        if state.input_engine_enabled_cached() {
            return false;
        }
        state.modifier_tap_press = None;
        state.claimed_key = None;
        let has_input = state.composition.is_some()
            || state.panel_anchor.is_some()
            || state.deferred_context.is_some()
            || state.pending_ime_apply.is_some()
            || state.ime_state.as_ref().is_some_and(has_visible_state);
        (has_input, state.reload_needed_cached())
    };
    if has_input {
        reset_input_for_focus_change(shared_state);
    }
    if should_reload {
        start_reload_if_needed(shared_state);
    }
    true
}

fn poll_pending_key_state(shared_state: &SharedState) {
    let pending = {
        let st = shared_state.borrow();
        st.engine_building || st.reload_in_progress || (st.ime_state.is_none() && st.engine_ready())
    };
    if pending {
        poll_engine_builds(shared_state);
    }
}

pub(crate) fn has_visible_state(state: &ImeState) -> bool {
    !state.preedit.is_empty() || !state.candidates.is_empty()
}

fn has_commit(state: &ImeState) -> bool {
    state
        .committed
        .as_deref()
        .map(|text| !text.is_empty())
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompositionTarget {
    None,
    Preedit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompositionPlan {
    pub commit: bool,
    pub target: CompositionTarget,
}

fn composition_text(state: &ImeState, hide_candidate_panel: bool) -> (&str, usize) {
    if hide_candidate_panel {
        let text = if state.commit_preview.is_empty() {
            &state.preedit
        } else {
            &state.commit_preview
        };
        (text, text.chars().count())
    } else {
        (&state.preedit, state.cursor)
    }
}

fn plan_composition(
    state: &ImeState,
    embedded: bool,
    hide_candidate_panel: bool,
) -> CompositionPlan {
    let target = if embedded {
        if composition_text(state, hide_candidate_panel && !state.is_package_switcher())
            .0
            .is_empty()
        {
            CompositionTarget::None
        } else {
            CompositionTarget::Preedit
        }
    } else {
        CompositionTarget::None
    };
    CompositionPlan {
        commit: has_commit(state),
        target,
    }
}

fn should_hold_panel_anchor(
    target: CompositionTarget,
    state: &ImeState,
    has_caret: bool,
) -> bool {
    target == CompositionTarget::None
        && has_visible_state(state)
        && (state.is_package_switcher() || !has_caret)
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

fn should_apply_processed_state(consumed: bool, before: &ImeState, after: &ImeState) -> bool {
    consumed || before.ascii_mode != after.ascii_mode
}

/// Whether `OnKeyDown` should report the key as eaten.
///
/// `OnTestKeyDown` deliberately claims more than librime will take (every
/// printable key has to be offered to the punctuator). Declined printable keys
/// declined by Rime are completed by `complete_unhandled_key` before this check;
/// other declined keys are handed back to the host. Anything librime actually did
/// — accepting the key, committing text, or changing the composition — is
/// eaten so the host never sees it twice.
fn should_consume_processed_state(accepted: bool, before: &ImeState, after: &ImeState) -> bool {
    accepted || has_commit(after) || visible_state_changed(before, after)
}

/// Complete a printable key that Rime left to the application.
/// IMM32/CUAS hosts (including Flutter) may already have replaced the physical
/// key with VK_PROCESSKEY after OnTestKeyDown claimed it. Returning false from
/// OnKeyDown then loses the character. Use the normal TSF commit path instead,
/// but only after Rime has had the chance to handle schema bindings, digit
/// codes and full-width punctuation itself. ExpressEditor can commit the old
/// composition and still reject the triggering character; append that character
/// to the commit so neither half of the input disappears.
fn complete_unhandled_key(keysym: u32, mods: u32, result: &mut KeyProcessResult) {
    if result.accepted
        || has_visible_state(&result.state)
        || mods & !(keytao_core::RIME_MOD_SHIFT | keytao_core::RIME_MOD_LOCK) != 0
    {
        return;
    }
    if let Some(ch) = key_policy::char_for_keysym(keysym) {
        result
            .state
            .committed
            .get_or_insert_with(String::new)
            .push(ch);
    }
}

fn process_key_result(
    shared_state: &SharedState,
    keysym: u32,
    mods: u32,
) -> Option<KeyProcessResult> {
    runtime_session(shared_state)?.process_key(keysym, mods)
}

fn process_deferred_key(
    state: &SharedState,
    key: DeferredKey,
    before: &ImeState,
) -> Option<KeyProcessResult> {
    if key.modifier_tap {
        return runtime_session(state)?
            .process_keys(&[(key.keysym, 0), (key.keysym, RIME_RELEASE_MASK)]);
    }
    if key.enter {
        return runtime_session(state)?.process_enter();
    }
    let panel_layout = with_current_theme(|theme| {
        if theme.ui.hide_candidate_panel && !before.is_package_switcher() {
            key_policy::CandidatePanelLayout::Hidden
        } else {
            match theme.panel.orientation {
                keytao_theme::PanelOrientation::Horizontal => {
                    key_policy::CandidatePanelLayout::Horizontal
                }
                keytao_theme::PanelOrientation::Vertical => {
                    key_policy::CandidatePanelLayout::Vertical
                }
            }
        }
    });
    let keysym = key_policy::candidate_arrow_key(key.keysym, key.mods, before, panel_layout);
    process_key_result(state, keysym, key.mods)
}

fn defer_key(
    state: &SharedState,
    context: &ITfContext,
    key: DeferredKey,
    already_written: bool,
) -> bool {
    let consumed = {
        let mut st = state.borrow_mut();
        if st
            .deferred_context
            .as_ref()
            .is_some_and(|old| old.as_raw() != context.as_raw())
        {
            st.deferred_keys.reset();
        }
        let consumed = st
            .deferred_keys
            .defer_failed(key, already_written, Instant::now());
        if !already_written {
            st.deferred_context = consumed.then(|| context.clone());
        }
        consumed
    };
    if consumed && !already_written {
        let weak = Rc::downgrade(state);
        crate::state::with_detached_windows(state, false, |window, _| {
            window.arm_deferred_keys(&weak);
        });
    }
    consumed
}

fn may_process_context_key(st: &TsfState, context: &ITfContext) -> bool {
    may_process_key(
        st.pending_ime_apply.is_some(),
        st.pending_composition_ends
            .iter()
            .filter(|pending| !pending.exhausted)
            .map(|pending| pending.context.as_raw() as usize),
        context.as_raw() as usize,
    )
}

fn deferred_input_pending(state: &SharedState, context: Option<&ITfContext>) -> bool {
    let st = state.borrow();
    st.deferred_keys.front().is_some()
        || st.deferred_replaying
        || st.pending_commit_retry.is_some()
        || context.is_some_and(|context| !may_process_context_key(&st, context))
}

pub(crate) fn flush_deferred_keys(state: &SharedState) {
    flush_pending_ime_ui(state);
    let (context, generation) = {
        let Ok(mut st) = state.try_borrow_mut() else {
            return;
        };
        st.deferred_keys.expire(Instant::now());
        if st.deferred_keys.front().is_none() {
            st.deferred_context = None;
            return;
        }
        let Some(context) = st.deferred_context.clone() else {
            return;
        };
        if st.deferred_replaying
            || st.ime_write_session_active
            || st.caret_probe_session_in_progress
            || st.pending_commit_retry.is_some()
            || !may_process_context_key(&st, &context)
        {
            return;
        }
        st.deferred_replaying = true;
        (context, st.input_generation)
    };
    let _ = (|| -> Result<()> {
        if input_is_blocked(state, Some(&context)) {
            reset_input_for_focus_change(state);
            return Ok(());
        }
        let Some(client_id) = prepare_engine_for_key(&context, state, KeyEventSource::DeferredKey)?
        else {
            return Ok(());
        };
        loop {
            let key = {
                let mut st = state.borrow_mut();
                if st.input_generation != generation
                    || !may_process_context_key(&st, &context)
                    || st
                        .deferred_context
                        .as_ref()
                        .is_none_or(|bound| bound.as_raw() != context.as_raw())
                {
                    break;
                }
                st.deferred_keys.expire(Instant::now());
                let Some(key) = st.deferred_keys.front() else {
                    break;
                };
                key
            };
            let before = cached_ime_state(state).unwrap_or_else(ImeState::empty);
            let result = process_deferred_key(state, key, &before);
            if state.borrow().input_generation != generation {
                break;
            }
            let Some(mut result) = result else {
                if runtime_session(state).is_some_and(|session| session.consume_failed_key()) {
                    state.borrow_mut().deferred_keys.pop();
                    continue;
                }
                start_reload_if_needed(state);
                break;
            };
            state.borrow_mut().deferred_keys.pop();
            if key.modifier_tap {
                if !modifier_tap_should_apply(
                    result.accepted,
                    before.ascii_mode,
                    result.state.ascii_mode,
                ) {
                    continue;
                }
            } else {
                complete_unhandled_key(key.keysym, key.mods, &mut result);
            }
            // Timer replay runs outside TSF key callbacks, where synchronous writes may be rejected.
            apply_ime_state(&context, client_id, state, result.state, true, true, None)?;
            if state.borrow().input_generation != generation {
                break;
            }
        }
        Ok(())
    })();
    let mut st = state.borrow_mut();
    st.deferred_replaying = false;
    if st.deferred_keys.front().is_none() {
        st.deferred_context = None;
    }
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
            st.panel_context.clone().or_else(|| st.key_context.clone()),
            st.client_id,
        )
    };
    let Some(context) = context else {
        return;
    };
    if client_id == 0 || input_is_blocked(shared_state, Some(&context)) {
        return;
    }
    if deferred_input_pending(shared_state, Some(&context)) {
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
        let _ = apply_ime_state(
            &context,
            client_id,
            shared_state,
            ime_state,
            false,
            true,
            None,
        );
    }
}

fn prepare_engine_for_key(
    context: &ITfContext,
    shared_state: &SharedState,
    callback: KeyEventSource,
) -> Result<Option<u32>> {
    {
        let st = shared_state.borrow();
        if st.ime_write_session_active || !may_process_context_key(&st, context) {
            return Ok(None);
        }
    }
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
            "{callback:?} engine not ready building={engine_building} error={engine_error:?}"
        );
        return Ok(None);
    }

    let reset_acknowledged = apply_pending_session_reset(shared_state);
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
        apply_ime_state(
            context,
            client_id,
            shared_state,
            ImeState::empty(),
            false,
            callback.async_dontcare(),
            None,
        )?;
    }

    if !reset_acknowledged || reload_started || reload_in_progress {
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
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(());
            };
            reset_input_for_focus_change(&state);
            refresh_input_context(&state, None);
            refresh_engine_for_focus(&state);
            Ok(())
        })
    }

    fn OnTestKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(BOOL::from(false));
            };
            if input_engine_paused(&state) {
                return Ok(BOOL::from(false));
            }
            let vk = (wparam.0 & 0xFFFF) as u16;
            let mods = current_mod_mask();
            state.borrow_mut().claimed_key = None;
            // Intervening application shortcuts may never reach OnKeyDown.
            let modifier = modifier_tap_keysym(vk, lparam.0);
            if modifier.is_none() {
                state.borrow_mut().modifier_tap_press = None;
            }
            poll_pending_key_state(&state);
            if let Some(keysym) = modifier {
                record_modifier_press(&state, keysym, lparam.0, mods);
                return Ok(BOOL::from(false));
            }
            let (composing, schema_shortcut) = {
                let st = state.borrow();
                let ime_state = st.ime_state.as_ref();
                (
                    ime_state.is_some_and(has_visible_state) || deferred_input_pending(&state, pic),
                    ime_state.and_then(|ime| schema_shortcut_key(vk, lparam.0, mods, ime)),
                )
            };
            if schema_shortcut.is_none() && is_application_shortcut(vk, mods, composing) {
                return Ok(BOOL::from(false));
            }
            prime_theme_resolver();
            poll_engine_builds(&state);
            let composing = state
                .borrow()
                .ime_state
                .as_ref()
                .is_some_and(has_visible_state)
                || deferred_input_pending(&state, pic);
            if schema_shortcut.is_none()
                && !should_eat_key(vk, composing, mods)
                && !state
                    .borrow()
                    .ime_state
                    .as_ref()
                    .is_some_and(|ime| chord_keysym_for_vk(vk, lparam.0, mods, ime).is_some())
            {
                return Ok(BOOL::from(false));
            }
            if input_is_blocked(&state, pic) {
                // The host may have revoked the keyboard while a composition was up
                // and `OnKeyDown` will not run to clean it away.
                clear_input_for_blocked_context(&state);
                return Ok(BOOL::from(false));
            }
            if !crate::state::deployment_key_pressed(&state, pic) {
                return Ok(BOOL::from(false));
            }
            let (engine_ready, reload_needed, visible, schema_shortcut, ascii_mode) = {
                let mut st = state.borrow_mut();
                let reload_needed = st.reload_needed_cached();
                let ime_state = st.ime_state.as_ref();
                (
                    st.engine_ready(),
                    reload_needed,
                    ime_state.is_some_and(has_visible_state),
                    ime_state.and_then(|ime| schema_shortcut_key(vk, lparam.0, mods, ime)),
                    ime_state.map_or(st.ascii_mode, |ime| ime.ascii_mode),
                )
            };
            let is_composing = visible || deferred_input_pending(&state, pic);
            let ascii_rejects = schema_shortcut.is_none()
                && !is_composing
                && ascii_mode
                && ascii_mode_rejects_key(vk, mods);
            if ascii_rejects {
                return Ok(BOOL::from(false));
            }
            if !engine_ready || reload_needed {
                if !engine_ready {
                    start_engine_warmup(&state);
                }
                if reload_needed {
                    start_reload_if_needed(&state);
                }
                crate::state::refresh_deployment_hint(&state);
                let eat = !is_caps_lock_vk(vk)
                    && state.borrow_mut().deferred_keys.can_queue(Instant::now());
                state.borrow_mut().claimed_key = eat.then_some((vk, lparam.0));
                return Ok(BOOL::from(eat));
            }
            let chord_key = state
                .borrow()
                .ime_state
                .as_ref()
                .is_some_and(|ime| chord_keysym_for_vk(vk, lparam.0, mods, ime).is_some());
            let eat = schema_shortcut.is_some() || chord_key || should_eat_key(vk, is_composing, mods);
            state.borrow_mut().claimed_key = eat.then_some((vk, lparam.0));
            append_key_diagnostic!(
                "OnTestKeyDown ready={engine_ready} reload={reload_needed} composing={is_composing} eat={eat}"
            );
            Ok(BOOL::from(eat))
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnTestKeyUp(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(BOOL::from(false));
            };
            if input_engine_paused(&state) {
                return Ok(BOOL::from(false));
            }
            let vk = (wparam.0 & 0xFFFF) as u16;
            let modifier = modifier_tap_keysym(vk, lparam.0);
            if let Some(keysym) = modifier {
                if !modifier_tap_pending(&state.borrow(), keysym, current_mod_mask()) {
                    state.borrow_mut().modifier_tap_press = None;
                    return Ok(BOOL::from(false));
                }
            } else if !state.borrow().ime_state.as_ref().is_some_and(|st| {
                st.chord_composer
                    && chord_keysym_for_vk(vk, lparam.0, current_mod_mask(), st).is_some()
            }) {
                return Ok(BOOL::from(false));
            }
            poll_engine_builds(&state);
            let is_modifier_tap = modifier.is_some();
            if input_is_blocked(&state, pic) {
                append_key_diagnostic!("OnTestKeyUp modifier_tap={is_modifier_tap} blocked=true eat=false");
                return Ok(BOOL::from(false));
            }
            let input_generation = state.borrow().input_generation;
            crate::state::refresh_deployment_hint(&state);
            if state.borrow().input_generation != input_generation {
                return Ok(BOOL::from(false));
            }
            let (should_eat, should_retry, should_reload) = {
                let mut st = state.borrow_mut();
                let reload_needed = st.reload_needed_cached();
                (
                    modifier.is_none_or(|keysym| modifier_tap_pending(&st, keysym, current_mod_mask()))
                        && st.engine_ready()
                        && !reload_needed,
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
            append_key_diagnostic!(
                "OnTestKeyUp modifier_tap={is_modifier_tap} pending={} ready={} reload={} eat={should_eat}",
                state.borrow().modifier_tap_press.is_some(),
                !should_retry,
                should_reload,
            );
            Ok(BOOL::from(should_eat))
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        let key_started = runtime_log::enabled(Level::Info).then(Instant::now);
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(BOOL::from(false));
            };
            if input_engine_paused(&state) {
                return Ok(BOOL::from(false));
            }
            let vk = (wparam.0 & 0xFFFF) as u16;
            let mods = current_mod_mask();
            let claimed = state.borrow_mut().claimed_key.take() == Some((vk, lparam.0));
            let modifier = modifier_tap_keysym(vk, lparam.0);
            if modifier.is_none() {
                state.borrow_mut().modifier_tap_press = None;
            }
            poll_pending_key_state(&state);
            if let Some(keysym) = modifier {
                record_modifier_press(&state, keysym, lparam.0, mods);
                return Ok(BOOL::from(false));
            }
            // Some hosts call OnKeyDown without OnTestKeyDown. Apply the same
            // shortcut policy before context inspection, warmup, or Enter's
            // modifier-free process_enter path.
            let (composing, schema_shortcut) = {
                let st = state.borrow();
                let ime_state = st.ime_state.as_ref();
                (
                    ime_state.is_some_and(has_visible_state) || deferred_input_pending(&state, pic),
                    ime_state.and_then(|ime| schema_shortcut_key(vk, lparam.0, mods, ime)),
                )
            };
            if !claimed && schema_shortcut.is_none() && is_application_shortcut(vk, mods, composing) {
                return Ok(BOOL::from(false));
            }
            poll_engine_builds(&state);
            let (schema_shortcut, composing, ascii_mode) = {
                let st = state.borrow();
                let ime_state = st.ime_state.as_ref();
                (
                    ime_state.and_then(|ime| schema_shortcut_key(vk, lparam.0, mods, ime)),
                    ime_state.is_some_and(has_visible_state) || deferred_input_pending(&state, pic),
                    ime_state.map_or(st.ascii_mode, |ime| ime.ascii_mode),
                )
            };
            if !claimed
                && schema_shortcut.is_none()
                && !composing
                && ascii_mode
                && ascii_mode_rejects_key(vk, mods)
            {
                return Ok(BOOL::from(false));
            }
            if !claimed
                && schema_shortcut.is_none()
                && !should_eat_key(vk, composing, mods)
                && !state
                    .borrow()
                    .ime_state
                    .as_ref()
                    .is_some_and(|ime| chord_keysym_for_vk(vk, lparam.0, mods, ime).is_some())
            {
                return Ok(BOOL::from(false));
            }
            let context = pic.ok_or(windows::core::Error::from(E_INVALIDARG))?.clone();

            if input_is_blocked(&state, Some(&context)) {
                reset_input_for_focus_change(&state);
                return Ok(BOOL::from(false));
            }
            if !crate::state::deployment_key_pressed(&state, Some(&context)) {
                return Ok(BOOL::from(false));
            }

            let Some((keysym, mods)) = schema_shortcut.or_else(|| {
                vk_to_keysym(vk, lparam.0, mods).map(|key| (key, rime_mod_mask_for_vk(vk, mods)))
            }) else {
                return Ok(BOOL::from(false));
            };
            let key = DeferredKey {
                keysym,
                mods,
                enter: schema_shortcut.is_none() && is_enter_vk(vk),
                modifier_tap: false,
            };
            if deferred_input_pending(&state, Some(&context)) {
                let consumed = !is_caps_lock_vk(vk) && defer_key(&state, &context, key, false);
                flush_deferred_keys(&state);
                return Ok(BOOL::from(consumed));
            }

            let prepared_engine = {
                let mut st = state.borrow_mut();
                st.modifier_tap_press = None;
                st.key_context = Some(context.clone());
                drop(st);
                prepare_engine_for_key(&context, &state, KeyEventSource::OnKeyDown)?
            };
            let client_id = match prepared_engine {
                Some(client_id) => client_id,
                None => {
                    return Ok(BOOL::from(
                        !is_caps_lock_vk(vk) && defer_key(&state, &context, key, false),
                    ))
                }
            };

            let before_state = cached_ime_state(&state).unwrap_or_else(ImeState::empty);

            if !claimed
                && schema_shortcut.is_none()
                && should_bypass_empty_composition(vk, mods, &before_state)
                && chord_keysym_for_vk(vk, lparam.0, mods, &before_state).is_none()
            {
                hide_candidate_window(&state);
                return Ok(BOOL::from(false));
            }

            let mods = key.mods;
            // Enter has exactly one meaning across all platforms: hand XK_Return to
            // Rime and let core fall back to committing the raw input (D5).
            let result = process_deferred_key(&state, key, &before_state);
            let mut result = match result {
                Some(r) => r,
                None => {
                    let already_written =
                        runtime_session(&state).is_some_and(|session| session.consume_failed_key());
                    let consumed =
                        !is_caps_lock_vk(vk) && defer_key(&state, &context, key, already_written);
                    append_key_diagnostic!("OnKeyDown result=false eat={consumed}");
                    // Another text service in this host may be deploying a
                    // new public snapshot. Core declines without waiting for
                    // its reload lock; do not leave obsolete candidates up.
                    hide_candidate_window(&state);
                    crate::state::refresh_deployment_hint(&state);
                    return Ok(BOOL::from(consumed));
                }
            };
            state.borrow_mut().deferred_keys.reset();
            state.borrow_mut().deployment_loaded_cached();
            complete_unhandled_key(keysym, mods, &mut result);
            let ime_state = result.state;

            let processed =
                should_consume_processed_state(result.accepted, &before_state, &ime_state);
            // Apply Caps Lock's commit/mode change while preserving its native toggle.
            let consumed = processed && !is_caps_lock_vk(vk);
            append_key_diagnostic!(
                "OnKeyDown accepted={} consumed={} preedit_len={} candidates={} commit={}",
                result.accepted,
                consumed,
                ime_state.preedit.chars().count(),
                ime_state.candidates.len(),
                has_commit(&ime_state),
            );

            if !should_apply_processed_state(processed, &before_state, &ime_state) {
                return Ok(BOOL::from(false));
            }

            if apply_ime_state(
                &context,
                client_id,
                &state,
                ime_state,
                true,
                false,
                key_started,
            )
            .is_err()
            {
                return Ok(BOOL::from(consumed));
            }

            Ok(BOOL::from(consumed))
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnKeyUp(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(BOOL::from(false));
            };
            if input_engine_paused(&state) {
                return Ok(BOOL::from(false));
            }
            let vk = (wparam.0 & 0xFFFF) as u16;
            let modifier = modifier_tap_keysym(vk, lparam.0);
            let is_modifier_tap = modifier.is_some();
            let (keysym, mods) = if let Some(keysym) = modifier {
                if !modifier_tap_pending(&state.borrow(), keysym, current_mod_mask()) {
                    state.borrow_mut().modifier_tap_press = None;
                    return Ok(BOOL::from(false));
                }
                (keysym, 0)
            } else {
                let st = state.borrow();
                let Some(before_state) = st
                    .ime_state
                    .as_ref()
                    .filter(|ime| ime.chord_composer && !ime.ascii_mode)
                else {
                    return Ok(BOOL::from(false));
                };
                let mods = current_mod_mask();
                let Some(keysym) = chord_keysym_for_vk(vk, lparam.0, mods, before_state) else {
                    return Ok(BOOL::from(false));
                };
                (keysym, mods)
            };
            let context = pic.ok_or(windows::core::Error::from(E_INVALIDARG))?.clone();
            if input_is_blocked(&state, Some(&context)) {
                return Ok(BOOL::from(false));
            }
            let input_generation = state.borrow().input_generation;
            crate::state::refresh_deployment_hint(&state);
            if state.borrow().input_generation != input_generation {
                return Ok(BOOL::from(false));
            }
            if is_modifier_tap
                && !state
                    .borrow_mut()
                    .modifier_tap_press
                    .take()
                    .is_some_and(|(key, _)| key == keysym)
            {
                return Ok(BOOL::from(false));
            }
            let key = DeferredKey {
                keysym,
                mods: if is_modifier_tap {
                    0
                } else {
                    mods | RIME_RELEASE_MASK
                },
                enter: false,
                modifier_tap: is_modifier_tap,
            };
            if deferred_input_pending(&state, Some(&context)) {
                state.borrow_mut().modifier_tap_press = None;
                let consumed = defer_key(&state, &context, key, false);
                flush_deferred_keys(&state);
                return Ok(BOOL::from(consumed));
            }
            let prepared_engine = {
                let mut st = state.borrow_mut();
                st.key_context = Some(context.clone());
                drop(st);
                prepare_engine_for_key(&context, &state, KeyEventSource::OnKeyUp)?
            };
            let client_id = match prepared_engine {
                Some(client_id) => client_id,
                None => return Ok(BOOL::from(defer_key(&state, &context, key, false))),
            };

            let before_state = cached_ime_state(&state).unwrap_or_else(ImeState::empty);
            if !is_modifier_tap
                && !key_policy::should_forward_chord_release(keysym, mods, &before_state)
            {
                return Ok(BOOL::from(false));
            }
            // ascii_composer only recognizes a release after seeing its press.
            // Both run here so test callbacks remain entirely outside librime.
            let result = process_deferred_key(&state, key, &before_state);
            let Some(result) = result else {
                let already_written =
                    runtime_session(&state).is_some_and(|session| session.consume_failed_key());
                let consumed = defer_key(&state, &context, key, already_written);
                append_key_diagnostic!("OnKeyUp modifier_tap={is_modifier_tap} result=false");
                hide_candidate_window(&state);
                crate::state::refresh_deployment_hint(&state);
                return Ok(BOOL::from(consumed));
            };
            state.borrow_mut().deployment_loaded_cached();
            append_key_diagnostic!(
                "OnKeyUp modifier_tap={is_modifier_tap} accepted={} ascii={}->{}",
                result.accepted,
                before_state.ascii_mode,
                result.state.ascii_mode,
            );
            let processed = if is_modifier_tap {
                modifier_tap_should_apply(
                    result.accepted,
                    before_state.ascii_mode,
                    result.state.ascii_mode,
                )
            } else {
                should_consume_processed_state(result.accepted, &before_state, &result.state)
            };
            let consumed = result.accepted || (is_modifier_tap && processed);
            if (if is_modifier_tap {
                processed
            } else {
                should_apply_processed_state(processed, &before_state, &result.state)
            }) && apply_ime_state(&context, client_id, &state, result.state, true, false, None)
                .is_err()
            {
                return Ok(BOOL::from(consumed));
            }
            Ok(BOOL::from(consumed))
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        guard(|| {
            if let Some(state) = upgrade_state(&self.state) {
                state.borrow_mut().modifier_tap_press = None;
            }
            Ok(BOOL::from(false))
        })
        .or(Ok(BOOL::from(false)))
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
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(());
            };
            clear_input_after_composition_terminated(&state, pcomposition);
            Ok(())
        })
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
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(());
            };
            let Some(context) = pic else {
                return Ok(());
            };
            let schedule = {
                let mut st = state.borrow_mut();
                // A first GetTextExt can return TF_E_NOLAYOUT before a popup
                // has ever been shown. Its later layout event must recover it.
                let has_content = st.ime_state.as_ref().is_some_and(has_visible_state)
                    && st
                        .panel_context
                        .as_ref()
                        .is_some_and(|active| active.as_raw() == context.as_raw());
                let schedule = should_schedule_layout_reposition(
                    has_content,
                    lcode,
                    st.layout_reposition_pending,
                );
                if schedule {
                    st.layout_reposition_pending = true;
                }
                schedule
            };
            if schedule {
                append_key_diagnostic!(
                    "layout change scheduled ctx=0x{:x}",
                    context.as_raw() as usize
                );
                crate::state::schedule_layout_reposition(&state);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "caret_tests.rs"]
mod caret_tests;

#[cfg(test)]
mod tests {
    use keytao_core::{Candidate, ImeState};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::TextServices::{TF_LC_CHANGE, TF_LC_CREATE, TF_LC_DESTROY};

    use super::{
        caret_is_inside_monitor, caret_probe_extent_is_usable, composition_text, plan_composition,
        should_apply_processed_state, should_arm_caret_reprobe, should_consume_processed_state,
        should_hold_panel_anchor, should_schedule_layout_reposition,
        system_caret_extent_is_usable, AdjacentChar, CaretSource, CompositionPlan,
        CompositionTarget, ImeWriteSessionGuard,
    };

    fn empty_state() -> ImeState {
        ImeState::empty()
    }

    #[test]
    fn mode_change_applies_even_when_the_key_is_not_consumed() {
        let before = empty_state();
        let mut after = before.clone();
        after.is_last_page = false;
        after.ascii_mode = !before.ascii_mode;
        let consumed = should_consume_processed_state(false, &before, &after);
        assert!(!consumed);
        assert!(should_apply_processed_state(consumed, &before, &after));
        assert!(should_apply_processed_state(true, &before, &before));
        assert!(!should_apply_processed_state(false, &before, &before));
    }

    #[test]
    fn layout_changes_coalesce_while_candidates_are_waiting_for_layout() {
        assert!(should_schedule_layout_reposition(true, TF_LC_CHANGE, false));
        assert!(!should_schedule_layout_reposition(true, TF_LC_CHANGE, true));
        assert!(!should_schedule_layout_reposition(
            false,
            TF_LC_CHANGE,
            false
        ));
        for lcode in [TF_LC_CREATE, TF_LC_DESTROY] {
            assert!(!should_schedule_layout_reposition(true, lcode, false));
        }
    }

    #[test]
    fn ui_metadata_changes_do_not_consume_declined_keys() {
        let before = state_with_candidate("candidate");
        let changes: [fn(&mut ImeState); 7] = [
            |state| state.cursor += 1,
            |state| state.sel_start += 1,
            |state| state.sel_end += 1,
            |state| state.page_size += 1,
            |state| state.is_last_page = !state.is_last_page,
            |state| state.select_keys = Some("asdf".into()),
            |state| state.candidates[0].comment = Some("comment".into()),
        ];
        for change in changes {
            let mut after = before.clone();
            change(&mut after);
            assert!(!should_consume_processed_state(false, &before, &after));
        }
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

    fn state_with_commit_and_preedit(committed: &str, preedit: &str) -> ImeState {
        let mut state = state_with_commit(committed);
        state.preedit = preedit.to_string();
        state
    }

    #[test]
    fn write_session_guard_clears_flags_during_unwind() {
        let state = crate::state::new_shared_state();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = ImeWriteSessionGuard::enter(&state);
            let mut st = state.borrow_mut();
            st.composition_in_flight = Some(0x1234);
            st.composition_clear_pending = true;
            drop(st);
            panic!("deferred edit session panic");
        }));

        assert!(result.is_err());
        let st = state.borrow();
        assert!(!st.ime_write_session_active);
        assert_eq!(st.composition_in_flight, None);
        assert!(!st.composition_clear_pending);
    }

    #[test]
    fn nested_write_session_guard_preserves_the_outer_lock_state() {
        let state = crate::state::new_shared_state();
        let outer = ImeWriteSessionGuard::enter(&state);
        state.borrow_mut().composition_in_flight = Some(7);
        state.borrow_mut().composition_clear_pending = true;
        drop(ImeWriteSessionGuard::enter(&state));
        assert!(state.borrow().ime_write_session_active);
        assert_eq!(state.borrow().composition_in_flight, Some(7));
        assert!(state.borrow().composition_clear_pending);
        drop(outer);
        assert!(!state.borrow().ime_write_session_active);
    }

    #[test]
    fn only_a_fresh_probe_suppresses_caret_reprobe() {
        assert!(!should_arm_caret_reprobe(Some(CaretSource::Probe)));
        assert!(should_arm_caret_reprobe(Some(CaretSource::Cache)));
        assert!(should_arm_caret_reprobe(Some(CaretSource::System)));
        assert!(should_arm_caret_reprobe(None));
    }

    #[test]
    fn clipped_caret_extent_is_never_usable() {
        let rect = RECT {
            left: 100,
            top: 200,
            right: 100,
            bottom: 220,
        };
        assert!(caret_probe_extent_is_usable(&rect, false));
        assert!(!caret_probe_extent_is_usable(&rect, true));
    }

    #[test]
    fn system_caret_rejects_degenerate_and_client_origin_rects() {
        assert!(system_caret_extent_is_usable(&RECT {
            left: 0,
            top: 10,
            right: 2,
            bottom: 30,
        }));
        assert!(!system_caret_extent_is_usable(&RECT {
            left: 10,
            top: 20,
            right: 10,
            bottom: 20,
        }));
        assert!(!system_caret_extent_is_usable(&RECT {
            left: 0,
            top: 0,
            right: 2,
            bottom: 20,
        }));
    }

    #[test]
    fn adjacent_character_edge_selects_the_caret_x() {
        let rect = RECT {
            left: 120,
            top: 200,
            right: 140,
            bottom: 220,
        };
        assert_eq!(AdjacentChar::After.caret_position(&rect), (120, 220));
        assert_eq!(AdjacentChar::Before.caret_position(&rect), (140, 220));
    }

    #[test]
    fn caret_monitor_sanity_rejects_other_monitor_coordinates() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(caret_is_inside_monitor(-100, 500, &monitor));
        assert!(caret_is_inside_monitor(-1920, 0, &monitor));
        assert!(caret_is_inside_monitor(0, 500, &monitor));
        assert!(caret_is_inside_monitor(-100, 1080, &monitor));
        assert!(!caret_is_inside_monitor(1, 500, &monitor));
        assert!(!caret_is_inside_monitor(-100, 1081, &monitor));
        assert!(!caret_is_inside_monitor(100, 500, &monitor));
    }

    #[test]
    fn hidden_composition_uses_commit_preview_with_a_trailing_caret() {
        let mut state = state_with_preedit("prefix f");
        state.cursor = 0;
        state.commit_preview = "converted这😀".into();
        assert_eq!(composition_text(&state, true), ("converted这😀", 11));
        assert_eq!(composition_text(&state, false), ("prefix f", 0));

        state.commit_preview.clear();
        assert_eq!(composition_text(&state, true), ("prefix f", 8));
    }

    #[test]
    fn embedded_preedit_goes_into_document() {
        assert_eq!(
            plan_composition(&state_with_preedit("ni"), true, false),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::Preedit,
            }
        );
    }

    #[test]
    fn embedded_without_preedit_ends_composition() {
        assert_eq!(
            plan_composition(&empty_state(), true, false),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::None,
            }
        );
    }

    #[test]
    fn panel_mode_never_creates_a_composition_for_preedit() {
        assert_eq!(
            plan_composition(&state_with_preedit("ni"), false, false),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::None,
            }
        );
    }

    #[test]
    fn panel_mode_never_creates_a_composition_for_candidates_only() {
        assert_eq!(
            plan_composition(&state_with_candidate("你"), false, false),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::None,
            }
        );
    }

    #[test]
    fn commit_only_ends_composition_in_both_modes() {
        let state = state_with_commit("，");
        let expected = CompositionPlan {
            commit: true,
            target: CompositionTarget::None,
        };

        assert_eq!(plan_composition(&state, true, false), expected);
        assert_eq!(plan_composition(&state, false, false), expected);
    }

    #[test]
    fn top_up_restarts_composition_in_both_modes() {
        let state = state_with_commit_and_preedit("缤", "c");

        assert_eq!(
            plan_composition(&state, true, false),
            CompositionPlan {
                commit: true,
                target: CompositionTarget::Preedit,
            }
        );
        assert_eq!(
            plan_composition(&state, false, false),
            CompositionPlan {
                commit: true,
                target: CompositionTarget::None,
            }
        );
    }

    #[test]
    fn embedded_plan_matches_legacy_behaviour() {
        for state in [
            empty_state(),
            state_with_candidate("你"),
            state_with_preedit("ni"),
            state_with_commit_and_preedit("你", "hao"),
        ] {
            let plan = plan_composition(&state, true, false);
            assert_eq!(
                plan.target,
                if state.preedit.is_empty() {
                    CompositionTarget::None
                } else {
                    CompositionTarget::Preedit
                }
            );
        }
    }

    #[test]
    fn package_switcher_holds_panel_anchor_even_with_embedded_mode_and_resolved_caret() {
        let mut switcher = state_with_candidate("键道6");
        switcher.schema_id = keytao_core::switcher_model::SCHEMA_ID.to_owned();
        let plan = plan_composition(&switcher, true, true);
        assert_eq!(plan.target, CompositionTarget::None);
        assert!(should_hold_panel_anchor(plan.target, &switcher, true));
        assert!(should_hold_panel_anchor(plan.target, &switcher, false));

        let inline = state_with_preedit("ni");
        let inline_plan = plan_composition(&inline, true, false);
        assert_eq!(inline_plan.target, CompositionTarget::Preedit);
        assert!(!should_hold_panel_anchor(inline_plan.target, &inline, false));
        assert!(!should_hold_panel_anchor(
            CompositionTarget::None,
            &empty_state(),
            false
        ));
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
    fn passes_first_key_after_focus_reset_when_only_no_menu_metadata_changes() {
        let no_menu_state = ImeState {
            is_last_page: false,
            ..ImeState::empty()
        };
        assert!(!should_consume_processed_state(
            false,
            &ImeState::empty(),
            &no_menu_state
        ));
    }

    #[test]
    fn passes_shortcuts_rime_declined_while_composing() {
        // Any remaining schema key that leaves the composition untouched and
        // is declined must be released too. Application shortcuts bypass the
        // engine before reaching this decision.
        assert!(!should_consume_processed_state(
            false,
            &state_with_preedit("ni"),
            &state_with_preedit("ni")
        ));
    }
}

#[cfg(test)]
#[path = "shortcut_passthrough_tests.rs"]
mod shortcut_passthrough_tests;

#[cfg(test)]
#[path = "commit_retry_tests.rs"]
mod commit_retry_tests;

#[cfg(test)]
#[path = "unhandled_key_tests.rs"]
mod unhandled_key_tests;
