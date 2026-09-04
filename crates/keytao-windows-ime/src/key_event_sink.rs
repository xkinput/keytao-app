//! ITfKeyEventSink + ITfCompositionSink — the hot path for all keystrokes.
//!
//! Key handling mirrors keytao-linux-ime/src/wayland_backend.rs:
//!   VK → X11 keysym → ImeRuntimeSession::process_key_result() → ImeState
//!   committed text  → write via TSF edit session (ITfInsertAtSelection)
//!   preedit text    → manage ITfComposition
//!   candidate list  → update CandidateWindow (same tiny-skia panel as Linux)

use std::{cell::RefCell, rc::Rc};

use std::sync::atomic::{AtomicUsize, Ordering};

use keytao_core::{utf16_offset_from_chars, ImeRuntimeSession, ImeState, KeyProcessResult};
use windows::{
    core::{implement, Interface, Result, GUID, PCWSTR, VARIANT},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
        System::Threading::GetCurrentThreadId,
        UI::TextServices::*,
        UI::WindowsAndMessaging::{GetAncestor, GetGUIThreadInfo, GA_ROOT, GUITHREADINFO},
    },
};

use crate::{
    edit_session::{
        take_selection_range, with_async_dontcare_write_session, with_read_session,
        with_write_session,
    },
    globals::DllActivityGuard,
    guard,
    key_map::{
        current_mod_mask, is_enter_vk, is_shift_vk, shift_keysym_for_vk,
        shift_pending_after_key_down, should_bypass_empty_composition, should_eat_key,
        vk_to_keysym, RIME_RELEASE_MASK,
    },
    state::{
        append_diagnostic, apply_pending_session_reset, clear_input_after_composition_terminated,
        clear_input_for_blocked_context, clear_layout_sink, diagnostics_enabled,
        embedded_composition, fallback_focus_window, hide_candidate_window, host_is_uiless,
        input_is_blocked, layout_sink_context, poll_engine_builds, prime_theme_resolver,
        refresh_engine_for_focus, refresh_input_context, reset_input_for_focus_change,
        retry_input_context_if_unknown, start_engine_warmup, start_reload_if_needed,
        store_layout_sink, sync_input_policy, update_ime_windows, update_language_bar_mode,
        CaretPosition, LayoutSinkRegistration, SharedState, WeakState,
    },
};

static KEY_DIAGNOSTIC_COUNT: AtomicUsize = AtomicUsize::new(0);

macro_rules! append_key_diagnostic {
    ($($arg:tt)*) => {
        if diagnostics_enabled()
            && KEY_DIAGNOSTIC_COUNT.fetch_add(1, Ordering::Relaxed) < 96
        {
            append_diagnostic(format!($($arg)*));
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

        // SetText can leave the document selection spanning the old
        // composition range. Collapse it after the committed text before
        // EndComposition, matching Weasel's TSF ordering, so a
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
        composition.EndComposition(ec)?;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaretSource {
    Probe,
    Cache,
    System,
}

pub(crate) fn should_arm_caret_reprobe(source: Option<CaretSource>) -> bool {
    source != Some(CaretSource::Probe)
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
                            };
                        }
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
                            };
                        }
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
        }
    }
}

/// Turn a probe into a position to draw at, remembering the last good caret.
fn resolve_caret(
    shared_state: &SharedState,
    probe: CaretProbe,
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
    if let Some(caret) = system_caret(probe.owner_hwnd) {
        if diagnostics_enabled() {
            append_diagnostic(format!(
                "caret resolved via=system x={} y={}",
                caret.x, caret.y
            ));
        }
        return Some((caret, CaretSource::System));
    }
    if diagnostics_enabled() {
        append_diagnostic("caret resolved via=none x=0 y=0");
    }
    None
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
    active: bool,
}

impl ImeWriteSessionGuard {
    fn enter(state: &SharedState) -> Self {
        {
            let mut st = state.borrow_mut();
            st.ime_write_session_active = true;
            st.composition_in_flight = None;
            st.composition_terminated_in_session = false;
        }
        Self {
            state: Rc::clone(state),
            active: true,
        }
    }

    fn finish(&mut self) {
        if !self.active {
            return;
        }
        let mut st = self.state.borrow_mut();
        st.ime_write_session_active = false;
        st.composition_in_flight = None;
        st.composition_terminated_in_session = false;
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
            .composition_context
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

fn apply_ime_state(
    context: &ITfContext,
    client_id: u32,
    shared_state: &SharedState,
    ime_state: ImeState,
    show_mode_hint_on_change: bool,
    async_dontcare: bool,
) -> Result<()> {
    let state_arc = Rc::clone(shared_state);
    let state_arc_for_session = Rc::clone(&state_arc);
    let ime_state_clone = ime_state.clone();
    let comp_sink_obj = CompositionSink {
        state: Rc::downgrade(shared_state),
        _dll_guard: DllActivityGuard::new(),
    };
    let comp_sink_iface: ITfCompositionSink = comp_sink_obj.into();
    let (display_attribute_atom, uiless) = {
        let st = shared_state.borrow();
        (st.display_attribute_atom, host_is_uiless(&st))
    };
    // `BeginUIElement(pbShow=FALSE)` is only known after this apply. If UIless
    // coverage expands, cache CandidateUiManager::host_allows_window in
    // TsfState and include it in this effective-mode decision.
    let embedded = embedded_composition() || uiless;
    let display_attribute_atom = if embedded {
        display_attribute_atom
    } else {
        None
    };
    let plan = plan_composition(&ime_state, embedded);
    if diagnostics_enabled() {
        let mode = if embedded { "embedded" } else { "panel" };
        let target = match plan.target {
            CompositionTarget::None => "none",
            CompositionTarget::Empty => "empty",
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
    if has_visible_state(&ime_state) {
        ensure_layout_sink(&state_arc, context);
    }

    let apply_session = move |ec: u32, ctx: &ITfContext| {
        let mut write_session_guard = ImeWriteSessionGuard::enter(&state_arc_for_session);
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
        let original_composition = composition.clone();

        let apply_result = (|| -> Result<()> {
            if let Some(committed) = committed {
                let committed_len = committed.chars().count();
                let comp = if let Some(comp) = composition.take() {
                    append_key_diagnostic!("commit path=composition len={committed_len}");
                    comp
                } else {
                    append_key_diagnostic!("commit path=fresh-composition len={committed_len}");
                    start_composition(ec, ctx, "", 0, &comp_sink_iface, None)?
                };
                // Drop local ownership before our own EndComposition. A host may
                // notify the sink synchronously from that call; it is benign.
                track_composition_in_flight(&state_arc_for_session, composition.as_ref());
                end_composition(ec, ctx, &comp, Some(committed))?;
            }

            match plan.target {
                CompositionTarget::None => {
                    if let Some(comp) = composition.take() {
                        append_key_diagnostic!("composition end reason=clear");
                        track_composition_in_flight(&state_arc_for_session, composition.as_ref());
                        end_composition(ec, ctx, &comp, None)?;
                    }
                }
                CompositionTarget::Empty => {
                    if composition.is_none() {
                        append_key_diagnostic!("composition start mode=panel text_len=0");
                        composition =
                            Some(start_composition(ec, ctx, "", 0, &comp_sink_iface, None)?);
                        track_composition_in_flight(&state_arc_for_session, composition.as_ref());
                    }
                }
                CompositionTarget::Preedit => {
                    if let Some(comp) = composition.as_ref() {
                        append_key_diagnostic!(
                            "composition update mode=embedded text_len={}",
                            ime_state_clone.preedit.chars().count()
                        );
                        update_composition_text(
                            ec,
                            ctx,
                            comp,
                            &ime_state_clone.preedit,
                            ime_state_clone.cursor,
                            display_attribute_atom,
                        )?;
                    } else {
                        append_key_diagnostic!(
                            "composition start mode=embedded text_len={}",
                            ime_state_clone.preedit.chars().count()
                        );
                        composition = Some(start_composition(
                            ec,
                            ctx,
                            &ime_state_clone.preedit,
                            ime_state_clone.cursor,
                            &comp_sink_iface,
                            display_attribute_atom,
                        )?);
                        track_composition_in_flight(&state_arc_for_session, composition.as_ref());
                    }
                }
            }
            Ok(())
        })();

        if let Err(error) = apply_result {
            track_composition_in_flight(&state_arc_for_session, None);
            if let Some(comp) = composition.as_ref().or(original_composition.as_ref()) {
                let _ = end_composition(ec, ctx, comp, None);
            }
            let mut st = state_arc_for_session.borrow_mut();
            st.composition = None;
            st.composition_context = None;
            st.ime_state = None;
            drop(st);
            write_session_guard.finish();
            if async_dontcare {
                reset_input_for_focus_change(&state_arc_for_session);
                if diagnostics_enabled() {
                    append_diagnostic(format!("TSF edit session failed: {error}"));
                }
            }
            return Err(error);
        }

        let caret = probe_caret(ec, ctx, composition.as_ref(), &ime_state_clone, embedded);
        let document_mgr = unsafe { ctx.GetDocumentMgr().ok() };
        let mode_changed = {
            let mut st = state_arc_for_session.borrow_mut();
            if composition.is_none() || committed.is_some() {
                st.last_caret = None;
            }
            st.composition_context = composition.as_ref().map(|_| ctx.clone());
            st.composition_in_flight = None;
            st.composition = composition;
            let mode_changed = ime_state_clone.ascii_mode != st.ascii_mode;
            st.ascii_mode = ime_state_clone.ascii_mode;
            st.ime_state = Some(ime_state_clone.clone());
            mode_changed
        };
        let show_mode_hint = show_mode_hint_on_change && mode_changed;

        let terminated_in_session = {
            let mut st = state_arc_for_session.borrow_mut();
            std::mem::take(&mut st.composition_terminated_in_session)
        };
        write_session_guard.finish();
        if terminated_in_session {
            let mut st = state_arc_for_session.borrow_mut();
            st.composition = None;
            st.composition_context = None;
            drop(st);
            append_diagnostic("composition dropped reason=terminated-in-session");
        }

        let owner_hwnd = caret.owner_hwnd;
        let caret = resolve_caret(&state_arc_for_session, caret);
        let caret_source = caret.map(|(_, source)| source);
        {
            let mut st = state_arc_for_session.borrow_mut();
            if caret_source == Some(CaretSource::Probe) {
                st.caret_retry_attempts = 0;
            }
            st.caret_retry_mode_hint = show_mode_hint && should_arm_caret_reprobe(caret_source);
        }
        if !has_visible_state(&ime_state_clone) {
            clear_layout_sink(&state_arc_for_session);
        }
        update_language_bar_mode(&state_arc_for_session, ime_state_clone.ascii_mode);
        update_ime_windows(
            &state_arc_for_session,
            &ime_state_clone,
            document_mgr.as_ref(),
            caret,
            owner_hwnd,
            show_mode_hint,
            embedded,
        );

        Ok(())
    };
    let session_result = if async_dontcare {
        with_async_dontcare_write_session(context, client_id, apply_session)
    } else {
        with_write_session(context, client_id, apply_session)
    };
    if let Err(error) = &session_result {
        {
            let mut st = state_arc.borrow_mut();
            st.ime_write_session_active = false;
            st.composition_in_flight = None;
            st.composition_terminated_in_session = false;
        }
        reset_input_for_focus_change(&state_arc);
        if diagnostics_enabled() {
            append_diagnostic(format!("TSF edit session failed: {error}"));
        }
    }
    session_result
}

/// Re-run the caret query for the context the panel is anchored to.
///
/// Hosts that answered `TF_E_NOLAYOUT` earlier call `OnLayoutChange` once the
/// text is laid out; scrolling and window moves report the same way.
fn reposition_ime_windows(
    shared_state: &SharedState,
    context: &ITfContext,
    show_mode_hint: bool,
) -> bool {
    let Some(_probe_session_guard) = CaretProbeSessionGuard::enter(shared_state, context) else {
        return false;
    };
    let (client_id, ime_state, composition, uiless) = {
        let st = shared_state.borrow();
        (
            st.client_id,
            st.ime_state.clone(),
            st.composition.clone(),
            host_is_uiless(&st),
        )
    };
    let Some(ime_state) = ime_state else {
        return true;
    };
    if client_id == 0 || (!has_visible_state(&ime_state) && !show_mode_hint) {
        return true;
    }
    let embedded = embedded_composition() || uiless;
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
        Ok(()) => session_probe.unwrap_or(CaretProbe {
            owner_hwnd,
            position: None,
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
            }
        }
    };

    let owner_hwnd = probe.owner_hwnd;
    let caret = resolve_caret(shared_state, probe);
    let caret_source = caret.map(|(_, source)| source);
    if caret_source == Some(CaretSource::Probe) {
        let mut st = shared_state.borrow_mut();
        st.caret_retry_attempts = 0;
        if show_mode_hint {
            st.caret_retry_mode_hint = false;
        }
    }
    update_ime_windows(
        shared_state,
        &ime_state,
        document_mgr.as_ref(),
        caret,
        owner_hwnd,
        show_mode_hint,
        embedded,
    );
    true
}

pub(crate) fn retry_caret_probe(shared_state: &SharedState) {
    let (attempt, context, show_mode_hint) = {
        let mut st = shared_state.borrow_mut();
        st.caret_retry_attempts = st.caret_retry_attempts.saturating_add(1);
        (
            st.caret_retry_attempts,
            st.composition_context
                .clone()
                .or_else(|| st.key_context.clone()),
            st.caret_retry_mode_hint,
        )
    };
    if crate::candidate_win::caret_retry_delay_ms(attempt.saturating_sub(1)).is_none() {
        if diagnostics_enabled() {
            append_diagnostic(format!("caret retry attempt={attempt} gave_up"));
        }
        return;
    }
    if diagnostics_enabled() {
        append_diagnostic(format!("caret retry attempt={attempt} fired"));
    }
    if let Some(context) = context {
        if !reposition_ime_windows(shared_state, &context, show_mode_hint) {
            let mut st = shared_state.borrow_mut();
            st.caret_retry_attempts = st.caret_retry_attempts.saturating_sub(1);
        }
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

fn runtime_session(shared_state: &SharedState) -> Option<ImeRuntimeSession> {
    shared_state.borrow().session()
}

fn cached_ime_state(shared_state: &SharedState) -> Option<ImeState> {
    shared_state.borrow().ime_state.clone()
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
    Empty,
    Preedit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompositionPlan {
    pub commit: bool,
    pub target: CompositionTarget,
}

fn plan_composition(state: &ImeState, embedded: bool) -> CompositionPlan {
    let target = if embedded {
        if state.preedit.is_empty() {
            CompositionTarget::None
        } else {
            CompositionTarget::Preedit
        }
    } else if has_visible_state(state) {
        CompositionTarget::Empty
    } else {
        CompositionTarget::None
    };
    CompositionPlan {
        commit: has_commit(state),
        target,
    }
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
        let _ = apply_ime_state(&context, client_id, shared_state, ime_state, false, true);
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
        apply_ime_state(
            context,
            client_id,
            shared_state,
            ImeState::empty(),
            false,
            false,
        )?;
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
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(());
            };
            reset_input_for_focus_change(&state);
            refresh_engine_for_focus(&state);
            refresh_input_context(&state, None);
            Ok(())
        })
    }

    fn OnTestKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        guard(|| {
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(BOOL::from(false));
            };
            prime_theme_resolver();
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
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnTestKeyUp(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        guard(|| {
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
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        guard(|| {
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

        if apply_ime_state(&context, client_id, &state, ime_state, true, false).is_err() {
            return Ok(BOOL::from(consumed));
        }

            Ok(BOOL::from(consumed))
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnKeyUp(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        guard(|| {
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
            if consumed
                && apply_ime_state(&context, client_id, &state, result.state, true, false).is_err()
            {
                return Ok(BOOL::from(consumed));
            }
            Ok(BOOL::from(consumed))
        })
        .or(Ok(BOOL::from(false)))
    }

    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        guard(|| Ok(BOOL::from(false))).or(Ok(BOOL::from(false)))
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
            if diagnostics_enabled() {
                append_diagnostic(format!(
                    "layout change lcode={} ctx=0x{:x}",
                    lcode.0,
                    pic.map_or(0, |context| context.as_raw() as usize)
                ));
            }
            if lcode == TF_LC_DESTROY {
                return Ok(());
            }
            let Some(state) = upgrade_state(&self.state) else {
                return Ok(());
            };
            let Some(context) = pic else {
                return Ok(());
            };
            reposition_ime_windows(&state, context, false);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use keytao_core::{Candidate, ImeState};
    use windows::Win32::Foundation::RECT;

    use super::{
        caret_is_inside_monitor, caret_probe_extent_is_usable, plan_composition,
        should_arm_caret_reprobe, should_consume_processed_state, system_caret_extent_is_usable,
        AdjacentChar, CaretSource, CompositionPlan, CompositionTarget, ImeWriteSessionGuard,
    };

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
            st.composition_terminated_in_session = true;
            drop(st);
            panic!("deferred edit session panic");
        }));

        assert!(result.is_err());
        let st = state.borrow();
        assert!(!st.ime_write_session_active);
        assert_eq!(st.composition_in_flight, None);
        assert!(!st.composition_terminated_in_session);
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
    fn embedded_preedit_goes_into_document() {
        assert_eq!(
            plan_composition(&state_with_preedit("ni"), true),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::Preedit,
            }
        );
    }

    #[test]
    fn embedded_without_preedit_ends_composition() {
        assert_eq!(
            plan_composition(&empty_state(), true),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::None,
            }
        );
    }

    #[test]
    fn panel_mode_keeps_empty_anchor() {
        assert_eq!(
            plan_composition(&state_with_preedit("ni"), false),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::Empty,
            }
        );
    }

    #[test]
    fn panel_mode_anchors_on_candidates_only() {
        assert_eq!(
            plan_composition(&state_with_candidate("你"), false),
            CompositionPlan {
                commit: false,
                target: CompositionTarget::Empty,
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

        assert_eq!(plan_composition(&state, true), expected);
        assert_eq!(plan_composition(&state, false), expected);
    }

    #[test]
    fn top_up_restarts_composition_in_both_modes() {
        let state = state_with_commit_and_preedit("缤", "c");

        assert_eq!(
            plan_composition(&state, true),
            CompositionPlan {
                commit: true,
                target: CompositionTarget::Preedit,
            }
        );
        assert_eq!(
            plan_composition(&state, false),
            CompositionPlan {
                commit: true,
                target: CompositionTarget::Empty,
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
            let plan = plan_composition(&state, true);
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
