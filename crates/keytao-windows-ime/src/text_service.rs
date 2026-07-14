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
    key_event_sink::KeyEventSink,
    language_bar::LanguageBarItem,
    state::{
        append_diagnostic, hide_ime_windows, new_shared_state, refresh_engine_for_focus,
        reset_input_for_focus_change, start_engine_warmup, SharedState, WeakState,
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
            Ok(())
        }
    }

    fn LockServer(&self, flock: BOOL) -> Result<()> {
        lock_server(flock.as_bool());
        Ok(())
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

    let language_bar =
        match LanguageBarItem::add(thread_mgr, client_id, std::rc::Rc::downgrade(state)) {
            Ok(item) => Some(item),
            Err(error) => {
                append_diagnostic(format!("Add TSF language bar item failed: {error}"));
                None
            }
        };

    let mut st = state.borrow_mut();
    st.key_sink = Some(key_sink_iface);
    st.thread_mgr_sink = Some(thread_sink_iface);
    st.thread_mgr_sink_cookie = Some(thread_sink_cookie);
    st.thread_focus_sink = Some(thread_focus_sink_iface);
    st.thread_focus_sink_cookie = Some(thread_focus_sink_cookie);
    st.language_bar = language_bar;
    drop(st);

    tracing::info!("KeyTao TSF activated (client_id={})", client_id);
    append_diagnostic(format!(
        "TSF activated client_id={client_id} activation_flags=0x{activation_flags:08x} thread_mgr_flags=0x{thread_mgr_flags:08x}"
    ));
    if unsafe { thread_mgr.GetFocus() }.is_ok() {
        start_engine_warmup(state);
    }
    Ok(())
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        activate_service(&self.state, ptim, tid, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        reset_input_for_focus_change(&self.state);
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

        // reset_input_for_focus_change requested an asynchronous end before the
        // sinks and client id are released; clear any disconnected handles too.
        st.composition = None;
        st.composition_context = None;
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
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, flags: u32) -> Result<()> {
        activate_service(&self.state, ptim, tid, flags)
    }
}

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(display_attribute::new_enumerator())
    }

    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        display_attribute::get_info(guid)
    }
}

#[implement(ITfThreadMgrEventSink)]
struct ThreadMgrEventSink {
    state: WeakState,
    _dll_guard: DllActivityGuard,
}

impl ITfThreadMgrEventSink_Impl for ThreadMgrEventSink_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        append_diagnostic("ThreadMgrEventSink OnInitDocumentMgr");
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        append_diagnostic("ThreadMgrEventSink OnUninitDocumentMgr");
        if let Some(state) = self.state.upgrade() {
            reset_input_for_focus_change(&state);
        }
        Ok(())
    }

    fn OnSetFocus(
        &self,
        pdimfocus: Option<&ITfDocumentMgr>,
        _pdimprevfocus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        if let Some(state) = self.state.upgrade() {
            reset_input_for_focus_change(&state);
            refresh_engine_for_focus(&state);
        }
        append_diagnostic(format!(
            "ThreadMgrEventSink OnSetFocus focus={}",
            pdimfocus.is_some()
        ));
        Ok(())
    }

    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        if let Some(state) = self.state.upgrade() {
            reset_input_for_focus_change(&state);
            refresh_engine_for_focus(&state);
        }
        append_diagnostic("ThreadMgrEventSink OnPushContext");
        Ok(())
    }

    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        if let Some(state) = self.state.upgrade() {
            reset_input_for_focus_change(&state);
        }
        append_diagnostic("ThreadMgrEventSink OnPopContext");
        Ok(())
    }
}

#[implement(ITfThreadFocusSink)]
struct ThreadFocusSink {
    state: WeakState,
    _dll_guard: DllActivityGuard,
}

impl ITfThreadFocusSink_Impl for ThreadFocusSink_Impl {
    fn OnSetThreadFocus(&self) -> Result<()> {
        if let Some(state) = self.state.upgrade() {
            refresh_engine_for_focus(&state);
        }
        append_diagnostic("ThreadFocusSink OnSetThreadFocus");
        Ok(())
    }

    fn OnKillThreadFocus(&self) -> Result<()> {
        if let Some(state) = self.state.upgrade() {
            reset_input_for_focus_change(&state);
        }
        append_diagnostic("ThreadFocusSink OnKillThreadFocus");
        Ok(())
    }
}
