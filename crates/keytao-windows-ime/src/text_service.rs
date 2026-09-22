//! TSF TextService — implements ITfTextInputProcessor + IClassFactory.
//!
//! Activate sequence:
//!   1. IClassFactory::CreateInstance  → TextService
//!   2. ITfTextInputProcessor::Activate → init runtime session, advise sinks
//!   3. ITfKeyEventSink::OnKeyDown (via KeyEventSink) → process keystrokes
//!   4. ITfTextInputProcessor::Deactivate → unadvise, cleanup

use windows::{
    core::{implement, IUnknown, Interface, Result, GUID},
    Win32::{
        Foundation::{BOOL, CLASS_E_NOAGGREGATION, E_POINTER},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::*,
    },
};

use crate::{
    display_attribute,
    globals::{lock_server, pin_module, DllActivityGuard},
    guard,
    input_context::CONTEXT_SENSITIVITY_COMPARTMENTS,
    key_event_sink::KeyEventSink,
    state::{
        append_diagnostic, apply_context_compartment_change, apply_conversion_mode_change,
        apply_open_close_change, clear_compartment_sinks, clear_context_compartment_sinks,
        context_compartment_sinks_cover, hide_ime_windows, input_context_matches,
        input_document_matches, new_shared_state, publish_initial_compartments,
        refresh_engine_for_focus, refresh_input_context, refresh_language_bar,
        reset_input_for_focus_change, same_com_object, start_engine_warmup, store_compartment_sink,
        store_context_compartment_sinks, terminate_input_now, CompartmentSinkRegistration,
        SharedState, WeakState,
    },
};

// ── IClassFactory ─────────────────────────────────────────────────────────────

#[implement(IClassFactory)]
pub(crate) struct ClassFactory {
    _dll_guard: DllActivityGuard,
}

impl ClassFactory {
    pub(crate) fn new() -> Self {
        Self {
            _dll_guard: DllActivityGuard::new(),
        }
    }
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut std::ffi::c_void,
    ) -> Result<()> {
        guard(|| {
            if riid.is_null() || ppvobject.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe {
                *ppvobject = std::ptr::null_mut();
            }
            if punkouter.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let state = new_shared_state();
            let ts: ITfTextInputProcessorEx = TextService {
                state,
                _dll_guard: DllActivityGuard::new(),
            }
            .into();
            unsafe {
                ts.query(riid, ppvobject).ok()?;
            }
            Ok(())
        })
    }

    fn LockServer(&self, flock: BOOL) -> Result<()> {
        guard(|| {
            lock_server(flock.as_bool());
            Ok(())
        })
    }
}

// ── ITfTextInputProcessor ─────────────────────────────────────────────────────

#[implement(
    ITfTextInputProcessor,
    ITfTextInputProcessorEx,
    ITfDisplayAttributeProvider
)]
pub(crate) struct TextService {
    state: SharedState,
    _dll_guard: DllActivityGuard,
}

fn activate_service(
    state: &SharedState,
    thread_mgr: Option<&ITfThreadMgr>,
    client_id: u32,
    activation_flags: u32,
) -> Result<()> {
    let thread_mgr = thread_mgr.ok_or(windows::core::Error::from(
        windows::Win32::Foundation::E_INVALIDARG,
    ))?;
    pin_module()?;
    let thread_mgr_flags = thread_mgr
        .cast::<ITfThreadMgrEx>()
        .ok()
        .and_then(|manager| unsafe { manager.GetActiveFlags().ok() })
        .unwrap_or(0);
    let display_attribute_atom = display_attribute::register_atom()?;

    {
        let mut st = state.borrow_mut();
        if st.thread_mgr.is_some() {
            drop(st);
            refresh_language_bar(state);
            return Ok(());
        }
        st.thread_mgr = Some(thread_mgr.clone());
        st.client_id = client_id;
        st.activation_flags = activation_flags;
        st.thread_mgr_flags = thread_mgr_flags;
        st.display_attribute_atom = Some(display_attribute_atom);
    }

    let key_sink = KeyEventSink {
        state: std::rc::Rc::downgrade(state),
        _dll_guard: DllActivityGuard::new(),
    };
    let key_sink_iface: ITfKeyEventSink = key_sink.into();

    let keystroke_mgr: ITfKeystrokeMgr = thread_mgr.cast()?;
    let advise_result =
        unsafe { keystroke_mgr.AdviseKeyEventSink(client_id, &key_sink_iface, BOOL::from(true)) };
    if let Err(error) = advise_result {
        let mut st = state.borrow_mut();
        st.thread_mgr = None;
        st.client_id = 0;
        st.activation_flags = 0;
        st.thread_mgr_flags = 0;
        st.display_attribute_atom = None;
        drop(st);
        append_diagnostic(format!("AdviseKeyEventSink failed: {error}"));
        return Err(error);
    }

    let thread_sink = ThreadMgrEventSink {
        state: std::rc::Rc::downgrade(state),
        _dll_guard: DllActivityGuard::new(),
    };
    let thread_sink_iface: ITfThreadMgrEventSink = thread_sink.into();
    let source: ITfSource = match thread_mgr.cast() {
        Ok(source) => source,
        Err(error) => {
            unsafe {
                let _ = keystroke_mgr.UnadviseKeyEventSink(client_id);
            }
            let mut st = state.borrow_mut();
            st.thread_mgr = None;
            st.client_id = 0;
            st.activation_flags = 0;
            st.thread_mgr_flags = 0;
            drop(st);
            append_diagnostic(format!("Query ITfSource failed: {error}"));
            return Err(error);
        }
    };
    let thread_sink_cookie =
        match unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, &thread_sink_iface) } {
            Ok(cookie) => cookie,
            Err(error) => {
                unsafe {
                    let _ = keystroke_mgr.UnadviseKeyEventSink(client_id);
                }
                let mut st = state.borrow_mut();
                st.thread_mgr = None;
                st.client_id = 0;
                st.activation_flags = 0;
                st.thread_mgr_flags = 0;
                drop(st);
                append_diagnostic(format!("Advise ThreadMgrEventSink failed: {error}"));
                return Err(error);
            }
        };

    let thread_focus_sink = ThreadFocusSink {
        state: std::rc::Rc::downgrade(state),
        _dll_guard: DllActivityGuard::new(),
    };
    let thread_focus_sink_iface: ITfThreadFocusSink = thread_focus_sink.into();
    let thread_focus_sink_cookie =
        match unsafe { source.AdviseSink(&ITfThreadFocusSink::IID, &thread_focus_sink_iface) } {
            Ok(cookie) => cookie,
            Err(error) => {
                unsafe {
                    let _ = source.UnadviseSink(thread_sink_cookie);
                    let _ = keystroke_mgr.UnadviseKeyEventSink(client_id);
                }
                let mut st = state.borrow_mut();
                st.thread_mgr = None;
                st.client_id = 0;
                st.activation_flags = 0;
                st.thread_mgr_flags = 0;
                st.display_attribute_atom = None;
                drop(st);
                append_diagnostic(format!("Advise ThreadFocusSink failed: {error}"));
                return Err(error);
            }
        };

    let mut st = state.borrow_mut();
    st.key_sink = Some(key_sink_iface);
    st.thread_mgr_sink = Some(thread_sink_iface);
    st.thread_mgr_sink_cookie = Some(thread_sink_cookie);
    st.thread_focus_sink = Some(thread_focus_sink_iface);
    st.thread_focus_sink_cookie = Some(thread_focus_sink_cookie);
    // Sinks can reenter during activation; register UI only after activation
    // has installed every required sink successfully.
    st.language_bar_enabled = true;
    drop(st);

    advise_compartment_sinks(state, thread_mgr);
    publish_initial_compartments(state);

    tracing::info!("KeyTao TSF activated (client_id={})", client_id);
    keytao_core::rt_log!(
        keytao_core::runtime_log::Level::Info,
        "lifecycle",
        "tsf_activated",
        activation_flags = activation_flags,
        thread_manager_flags = thread_mgr_flags
    );
    append_diagnostic(format!(
        "TSF activated client_id={client_id} activation_flags=0x{activation_flags:08x} thread_mgr_flags=0x{thread_mgr_flags:08x}"
    ));
    if unsafe { thread_mgr.GetFocus() }.is_ok() {
        start_engine_warmup(state);
    }
    refresh_input_context(state, None);
    Ok(())
}

/// A keyboard TIP must follow `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE` and should
/// follow `..._INPUTMODE_CONVERSION`; both live on the thread manager.
fn advise_compartment_sinks(state: &SharedState, thread_mgr: &ITfThreadMgr) {
    let Ok(compartment_mgr) = thread_mgr.cast::<ITfCompartmentMgr>() else {
        return;
    };
    for guid in [
        GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
        GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
    ] {
        let Some(registration) = advise_compartment_sink(state, &compartment_mgr, &guid) else {
            continue;
        };
        store_compartment_sink(state, registration);
    }
}

/// Follow the focused context's `KEYBOARD_DISABLED` / `EMPTYCONTEXT`
/// compartments.
///
/// These live on the context rather than on the thread manager, so unlike the
/// sinks above they have to move with the focus: the previous context's
/// registrations are unadvised here. Without them a host that disables a
/// context mid-composition is not heard at all — the key path would keep
/// passing keys through while the old preedit, candidate window and native
/// composition stayed on screen.
pub(crate) fn advise_context_compartment_sinks(state: &SharedState, context: Option<&ITfContext>) {
    if context_compartment_sinks_cover(state, context) {
        return;
    }
    let Some(context) = context else {
        clear_context_compartment_sinks(state);
        return;
    };
    let Ok(compartment_mgr) = context.cast::<ITfCompartmentMgr>() else {
        // Nothing to listen on. Drop the old registrations anyway; they belong
        // to a context that no longer has the focus.
        clear_context_compartment_sinks(state);
        return;
    };
    let registrations = CONTEXT_SENSITIVITY_COMPARTMENTS
        .iter()
        .filter_map(|guid| advise_compartment_sink(state, &compartment_mgr, guid))
        .collect();
    store_context_compartment_sinks(state, Some(context.clone()), registrations);
}

fn advise_compartment_sink(
    state: &SharedState,
    compartment_mgr: &ITfCompartmentMgr,
    guid: &GUID,
) -> Option<CompartmentSinkRegistration> {
    let compartment = unsafe { compartment_mgr.GetCompartment(guid) }.ok()?;
    let source = compartment.cast::<ITfSource>().ok()?;
    let sink: ITfCompartmentEventSink = CompartmentSink {
        state: std::rc::Rc::downgrade(state),
        _dll_guard: DllActivityGuard::new(),
    }
    .into();
    // AdviseSink holds the only reference we need; the cookie is what has to
    // survive until the registration is dropped.
    match unsafe { source.AdviseSink(&ITfCompartmentEventSink::IID, &sink) } {
        Ok(cookie) => Some(CompartmentSinkRegistration { source, cookie }),
        Err(error) => {
            append_diagnostic(format!("Advise compartment sink failed: {error}"));
            None
        }
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        guard(|| activate_service(&self.state, ptim, tid, 0))
    }

    fn Deactivate(&self) -> Result<()> {
        guard(|| {
            // Cleanup calls into TSF and may synchronously deliver focus events.
            // Close the recovery gate before removing any registrations.
            self.state.borrow_mut().language_bar_enabled = false;
            // TSF releases its last reference right after this returns, so a queued
            // edit session would never run — end the composition synchronously.
            terminate_input_now(&self.state);
            clear_compartment_sinks(&self.state);
            clear_context_compartment_sinks(&self.state);
            let (thread_mgr, client_id, thread_sink_cookie, thread_focus_sink_cookie, language_bar) = {
                let mut st = self.state.borrow_mut();
                (
                    st.thread_mgr.clone(),
                    st.client_id,
                    st.thread_mgr_sink_cookie,
                    st.thread_focus_sink_cookie,
                    st.language_bar.take(),
                )
            };

            if let Some(language_bar) = language_bar {
                language_bar.remove();
            }

            if let Some(thread_mgr) = thread_mgr {
                if let Ok(km) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                    unsafe {
                        let _ = km.UnadviseKeyEventSink(client_id);
                    }
                }
                if let Some(cookie) = thread_sink_cookie {
                    if let Ok(source) = thread_mgr.cast::<ITfSource>() {
                        unsafe {
                            let _ = source.UnadviseSink(cookie);
                        }
                    }
                }
                if let Some(cookie) = thread_focus_sink_cookie {
                    if let Ok(source) = thread_mgr.cast::<ITfSource>() {
                        unsafe {
                            let _ = source.UnadviseSink(cookie);
                        }
                    }
                }
            }

            let mut st = self.state.borrow_mut();

            // The composition is already gone; clear any disconnected handles too.
            st.composition = None;
            st.composition_context = None;
            st.key_context = None;
            st.key_sink = None;
            st.thread_mgr_sink = None;
            st.thread_mgr_sink_cookie = None;
            st.thread_focus_sink = None;
            st.thread_focus_sink_cookie = None;
            st.language_bar = None;
            st.thread_mgr = None;
            st.ime_state = None;
            st.client_id = 0;
            st.activation_flags = 0;
            st.thread_mgr_flags = 0;
            st.display_attribute_atom = None;
            drop(st);
            hide_ime_windows(&self.state);

            tracing::info!("KeyTao TSF deactivated");
            append_diagnostic("TSF deactivated");
            Ok(())
        })
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, flags: u32) -> Result<()> {
        guard(|| activate_service(&self.state, ptim, tid, flags))
    }
}

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        guard(|| Ok(display_attribute::new_enumerator()))
    }

    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        guard(|| display_attribute::get_info(guid))
    }
}

#[implement(ITfThreadMgrEventSink)]
struct ThreadMgrEventSink {
    state: WeakState,
    _dll_guard: DllActivityGuard,
}

fn focused_document(state: &SharedState) -> Option<ITfDocumentMgr> {
    let manager = state.borrow().thread_mgr.clone()?;
    unsafe { manager.GetFocus() }.ok()
}

fn is_focused_top_context(state: &SharedState, context: Option<&ITfContext>) -> bool {
    let Some(context) = context else {
        return false;
    };
    focused_document(state)
        .and_then(|document| unsafe { document.GetTop() }.ok())
        .is_some_and(|top| same_com_object(&top, context))
}

fn document_event_affects_input(state: &SharedState, document: Option<&ITfDocumentMgr>) -> bool {
    let Some(document) = document else {
        return false;
    };
    input_document_matches(state, document)
        || focused_document(state).is_some_and(|focused| same_com_object(&focused, document))
}

fn pop_event_affects_input(state: &SharedState, context: Option<&ITfContext>) -> bool {
    let Some(context) = context else {
        return false;
    };
    // GetDocumentMgr may already return S_FALSE after Pop, so consult the
    // tracked context before asking TSF about a still-attached context.
    input_context_matches(state, Some(context))
        || unsafe { context.GetDocumentMgr() }
            .ok()
            .is_some_and(|document| document_event_affects_input(state, Some(&document)))
}

impl ITfThreadMgrEventSink_Impl for ThreadMgrEventSink_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        guard(|| {
            append_diagnostic("ThreadMgrEventSink OnInitDocumentMgr");
            Ok(())
        })
    }

    fn OnUninitDocumentMgr(&self, pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        guard(|| {
            append_diagnostic("ThreadMgrEventSink OnUninitDocumentMgr");
            if let Some(state) = self.state.upgrade() {
                if document_event_affects_input(&state, pdim) {
                    reset_input_for_focus_change(&state);
                    refresh_input_context(&state, None);
                }
            }
            Ok(())
        })
    }

    fn OnSetFocus(
        &self,
        pdimfocus: Option<&ITfDocumentMgr>,
        _pdimprevfocus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        guard(|| {
            if let Some(state) = self.state.upgrade() {
                reset_input_for_focus_change(&state);
                refresh_engine_for_focus(&state);
                let context = pdimfocus.and_then(|manager| unsafe { manager.GetTop() }.ok());
                refresh_input_context(&state, context.as_ref());
                if pdimfocus.is_some() {
                    refresh_language_bar(&state);
                }
            }
            append_diagnostic(format!(
                "ThreadMgrEventSink OnSetFocus focus={}",
                pdimfocus.is_some()
            ));
            Ok(())
        })
    }

    fn OnPushContext(&self, pic: Option<&ITfContext>) -> Result<()> {
        guard(|| {
            if let Some(state) = self.state.upgrade() {
                // Push is broadcast for every document in the thread, including
                // background tabs and controls. Only the focused top may replace
                // the active editor's policy and composition.
                if !is_focused_top_context(&state, pic) {
                    return Ok(());
                }
                reset_input_for_focus_change(&state);
                refresh_engine_for_focus(&state);
                refresh_input_context(&state, pic);
                if pic.is_some() {
                    refresh_language_bar(&state);
                }
            }
            append_diagnostic("ThreadMgrEventSink OnPushContext");
            Ok(())
        })
    }

    fn OnPopContext(&self, pic: Option<&ITfContext>) -> Result<()> {
        guard(|| {
            if let Some(state) = self.state.upgrade() {
                if !pop_event_affects_input(&state, pic) {
                    return Ok(());
                }
                reset_input_for_focus_change(&state);
                refresh_input_context(&state, None);
            }
            append_diagnostic("ThreadMgrEventSink OnPopContext");
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "input_context_focus_tests.rs"]
mod input_context_focus_tests;

// ── ITfCompartmentEventSink ───────────────────────────────────────────────────

/// Keeps the system on/off state and the conversion-mode indicator in step with
/// Rime: Ctrl+Space and the input indicator write these compartments.
///
/// The same object serves the focused context's `KEYBOARD_DISABLED` /
/// `EMPTYCONTEXT` compartments; `OnChange` tells them apart by GUID.
#[implement(ITfCompartmentEventSink)]
struct CompartmentSink {
    state: WeakState,
    _dll_guard: DllActivityGuard,
}

impl ITfCompartmentEventSink_Impl for CompartmentSink_Impl {
    fn OnChange(&self, rguid: *const GUID) -> Result<()> {
        guard(|| {
            if rguid.is_null() {
                return Ok(());
            }
            let Some(state) = self.state.upgrade() else {
                return Ok(());
            };
            let guid = unsafe { *rguid };
            if guid == GUID_COMPARTMENT_KEYBOARD_OPENCLOSE {
                apply_open_close_change(&state);
            } else if guid == GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION {
                apply_conversion_mode_change(&state);
            } else if CONTEXT_SENSITIVITY_COMPARTMENTS.contains(&guid) {
                // Re-inspects the context but leaves the registrations alone: this
                // sink must not be unadvised from inside its own callback.
                apply_context_compartment_change(&state);
            }
            Ok(())
        })
    }
}

#[implement(ITfThreadFocusSink)]
struct ThreadFocusSink {
    state: WeakState,
    _dll_guard: DllActivityGuard,
}

impl ITfThreadFocusSink_Impl for ThreadFocusSink_Impl {
    fn OnSetThreadFocus(&self) -> Result<()> {
        guard(|| {
            if let Some(state) = self.state.upgrade() {
                refresh_engine_for_focus(&state);
                refresh_input_context(&state, None);
                refresh_language_bar(&state);
            }
            append_diagnostic("ThreadFocusSink OnSetThreadFocus");
            Ok(())
        })
    }

    fn OnKillThreadFocus(&self) -> Result<()> {
        guard(|| {
            if let Some(state) = self.state.upgrade() {
                // The thread stops pumping our edit sessions once focus is gone, so
                // a queued composition end would hang around in the document.
                terminate_input_now(&state);
            }
            append_diagnostic("ThreadFocusSink OnKillThreadFocus");
            Ok(())
        })
    }
}
