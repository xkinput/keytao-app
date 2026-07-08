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
        Foundation::{BOOL, CLASS_E_NOAGGREGATION},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::*,
    },
};

use crate::{
    globals::{lock_server, obj_add, obj_release},
    key_event_sink::KeyEventSink,
    state::{
        append_diagnostic, hide_ime_windows, new_shared_state, start_engine_warmup, SharedState,
    },
};

// ── IClassFactory ─────────────────────────────────────────────────────────────

#[implement(IClassFactory)]
pub(crate) struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut std::ffi::c_void,
    ) -> Result<()> {
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let state = new_shared_state();
        obj_add();
        let ts: ITfTextInputProcessorEx = TextService { state }.into();
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

#[implement(ITfTextInputProcessor, ITfTextInputProcessorEx)]
pub(crate) struct TextService {
    state: SharedState,
}

impl Drop for TextService {
    fn drop(&mut self) {
        obj_release();
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        let thread_mgr = ptim.ok_or(windows::core::Error::from(
            windows::Win32::Foundation::E_INVALIDARG,
        ))?;

        {
            let mut st = self.state.lock().unwrap();
            st.thread_mgr = Some(thread_mgr.clone());
            st.client_id = tid;
        }

        // Create KeyEventSink COM object (shares Arc<Mutex<State>>)
        let key_sink = KeyEventSink {
            state: std::sync::Arc::clone(&self.state),
        };
        let key_sink_iface: ITfKeyEventSink = key_sink.into();

        // Advise KeyEventSink via ITfKeystrokeMgr
        let keystroke_mgr: ITfKeystrokeMgr = thread_mgr.cast()?;
        let advise_result =
            unsafe { keystroke_mgr.AdviseKeyEventSink(tid, &key_sink_iface, BOOL::from(true)) };
        if let Err(e) = advise_result {
            let mut st = self.state.lock().unwrap();
            st.thread_mgr = None;
            st.client_id = 0;
            append_diagnostic(format!("AdviseKeyEventSink failed: {e}"));
            return Err(e);
        }

        let thread_sink = ThreadMgrEventSink {
            state: std::sync::Arc::clone(&self.state),
        };
        let thread_sink_iface: ITfThreadMgrEventSink = thread_sink.into();
        let source: ITfSource = match thread_mgr.cast() {
            Ok(source) => source,
            Err(e) => {
                unsafe {
                    let _ = keystroke_mgr.UnadviseKeyEventSink(tid);
                }
                let mut st = self.state.lock().unwrap();
                st.thread_mgr = None;
                st.client_id = 0;
                append_diagnostic(format!("Query ITfSource failed: {e}"));
                return Err(e);
            }
        };
        let thread_sink_cookie =
            match unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, &thread_sink_iface) } {
                Ok(cookie) => cookie,
                Err(e) => {
                    unsafe {
                        let _ = keystroke_mgr.UnadviseKeyEventSink(tid);
                    }
                    let mut st = self.state.lock().unwrap();
                    st.thread_mgr = None;
                    st.client_id = 0;
                    append_diagnostic(format!("Advise ThreadMgrEventSink failed: {e}"));
                    return Err(e);
                }
            };

        let mut st = self.state.lock().unwrap();
        st.key_sink = Some(key_sink_iface);
        st.thread_mgr_sink = Some(thread_sink_iface);
        st.thread_mgr_sink_cookie = Some(thread_sink_cookie);
        drop(st);

        start_engine_warmup(&self.state);

        tracing::info!("KeyTao TSF activated (client_id={})", tid);
        append_diagnostic(format!("TSF activated client_id={tid}"));
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        let (thread_mgr, client_id, thread_sink_cookie) = {
            let st = self.state.lock().unwrap();
            (
                st.thread_mgr.clone(),
                st.client_id,
                st.thread_mgr_sink_cookie,
            )
        };

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
        }

        let mut st = self.state.lock().unwrap();

        // End any active composition
        // (The edit session needed for this runs on the UI thread; at Deactivate
        // there may be no active context, so we just drop the handle.)
        st.composition = None;
        st.key_sink = None;
        st.thread_mgr_sink = None;
        st.thread_mgr_sink_cookie = None;
        st.thread_mgr = None;
        st.ime_state = None;
        st.client_id = 0;
        drop(st);
        hide_ime_windows(&self.state);

        tracing::info!("KeyTao TSF deactivated");
        append_diagnostic("TSF deactivated");
        Ok(())
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, _flags: u32) -> Result<()> {
        self.Activate(ptim, tid)
    }
}

#[implement(ITfThreadMgrEventSink)]
struct ThreadMgrEventSink {
    state: SharedState,
}

impl ITfThreadMgrEventSink_Impl for ThreadMgrEventSink_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        append_diagnostic("ThreadMgrEventSink OnInitDocumentMgr");
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        append_diagnostic("ThreadMgrEventSink OnUninitDocumentMgr");
        hide_ime_windows(&self.state);
        Ok(())
    }

    fn OnSetFocus(
        &self,
        pdimfocus: Option<&ITfDocumentMgr>,
        _pdimprevfocus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        {
            let mut st = self.state.lock().unwrap();
            st.shift_pressed_without_key = false;
            if pdimfocus.is_none() {
                st.ime_state = None;
                st.composition = None;
            }
        }
        hide_ime_windows(&self.state);
        append_diagnostic(format!(
            "ThreadMgrEventSink OnSetFocus focus={}",
            pdimfocus.is_some()
        ));
        Ok(())
    }

    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        append_diagnostic("ThreadMgrEventSink OnPushContext");
        Ok(())
    }

    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        {
            let mut st = self.state.lock().unwrap();
            st.shift_pressed_without_key = false;
        }
        hide_ime_windows(&self.state);
        append_diagnostic("ThreadMgrEventSink OnPopContext");
        Ok(())
    }
}
