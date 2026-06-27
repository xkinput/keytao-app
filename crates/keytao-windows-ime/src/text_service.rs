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
    state::{new_shared_state, SharedState},
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
        let ts: ITfTextInputProcessor = TextService { state }.into();
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

#[implement(ITfTextInputProcessor)]
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

        let mut st = self.state.lock().unwrap();

        st.thread_mgr = Some(thread_mgr.clone());
        st.client_id = tid;

        // Create KeyEventSink COM object (shares Arc<Mutex<State>>)
        let key_sink = KeyEventSink {
            state: std::sync::Arc::clone(&self.state),
        };
        let key_sink_iface: ITfKeyEventSink = key_sink.into();

        // Advise KeyEventSink via ITfKeystrokeMgr
        let keystroke_mgr: ITfKeystrokeMgr = thread_mgr.cast()?;
        unsafe {
            keystroke_mgr.AdviseKeyEventSink(tid, &key_sink_iface, BOOL::from(true))?;
        }
        st.key_sink = Some(key_sink_iface);

        // Advise ThreadMgrEventSink (optional but good practice for focus tracking)
        // We skip this for now to keep the implementation minimal.

        tracing::info!("KeyTao TSF activated (client_id={})", tid);
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        let mut st = self.state.lock().unwrap();

        if let (Some(thread_mgr), client_id) = (&st.thread_mgr, st.client_id) {
            if let Ok(km) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                unsafe {
                    let _ = km.UnadviseKeyEventSink(client_id);
                }
            }
        }

        // End any active composition
        // (The edit session needed for this runs on the UI thread; at Deactivate
        // there may be no active context, so we just drop the handle.)
        st.composition = None;
        st.candidate_win.hide();
        st.mode_hint_win.hide();
        st.key_sink = None;
        st.thread_mgr = None;
        st.ime_state = None;

        tracing::info!("KeyTao TSF deactivated");
        Ok(())
    }
}
