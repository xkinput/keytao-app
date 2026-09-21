//! Exercise registration retries and synchronous TSF callbacks without loading
//! Rime or registering a text service in the user's Windows session.

use std::{
    cell::Cell,
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};

use windows::{
    core::{implement, Result, GUID},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_NOTIMPL, HWND, RECT},
        UI::TextServices::*,
    },
};

use super::{new_shared_state, refresh_language_bar, SharedState, WeakState};

// Unexpected COM calls fail instead of consulting the real Windows thread
// manager. In particular, this mock does not expose ITfCompartmentMgr.
macro_rules! unimplemented_methods {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> {
            Err(E_NOTIMPL.into())
        })*
    };
}

struct RegistrationTrace {
    state: WeakState,
    fail_first_add: Cell<bool>,
    reenter_once: Cell<bool>,
    deactivate_during_add: Cell<bool>,
    reenter_during_remove: Cell<bool>,
    add_attempts: Cell<usize>,
    successful_adds: Cell<usize>,
    remove_calls: Cell<usize>,
    reentry_calls: Cell<usize>,
    reentry_panicked: Cell<bool>,
}

#[implement(ITfThreadMgr, ITfLangBarItemMgr)]
struct MockThreadMgr {
    trace: Rc<RegistrationTrace>,
}

impl ITfThreadMgr_Impl for MockThreadMgr_Impl {
    unimplemented_methods! {
        fn Activate(&self) -> u32;
        fn Deactivate(&self) -> ();
        fn CreateDocumentMgr(&self) -> ITfDocumentMgr;
        fn EnumDocumentMgrs(&self) -> IEnumTfDocumentMgrs;
        fn GetFocus(&self) -> ITfDocumentMgr;
        fn SetFocus(&self, focus: Option<&ITfDocumentMgr>) -> ();
        fn AssociateFocus(&self, window: HWND, focus: Option<&ITfDocumentMgr>) -> ITfDocumentMgr;
        fn IsThreadFocus(&self) -> BOOL;
        fn GetFunctionProvider(&self, clsid: *const GUID) -> ITfFunctionProvider;
        fn EnumFunctionProviders(&self) -> IEnumTfFunctionProviders;
        fn GetGlobalCompartment(&self) -> ITfCompartmentMgr;
    }
}

impl ITfLangBarItemMgr_Impl for MockThreadMgr_Impl {
    fn AddItem(&self, item: Option<&ITfLangBarItem>) -> Result<()> {
        self.trace
            .add_attempts
            .set(self.trace.add_attempts.get() + 1);
        if item.is_none() || self.trace.fail_first_add.replace(false) {
            return Err(E_FAIL.into());
        }

        if self.trace.deactivate_during_add.replace(false) {
            if let Some(state) = self.trace.state.upgrade() {
                if catch_unwind(AssertUnwindSafe(|| {
                    let mut st = state.borrow_mut();
                    st.language_bar_enabled = false;
                    st.thread_mgr = None;
                    st.client_id = 0;
                }))
                .is_err()
                {
                    self.trace.reentry_panicked.set(true);
                    return Err(E_FAIL.into());
                }
            }
        }

        if self.trace.reenter_once.replace(false) {
            if let Some(state) = self.trace.state.upgrade() {
                self.trace
                    .reentry_calls
                    .set(self.trace.reentry_calls.get() + 1);
                // Keep any regression panic on the Rust side of this COM ABI
                // boundary, so the test reports the failure rather than aborts.
                if catch_unwind(AssertUnwindSafe(|| refresh_language_bar(&state))).is_err() {
                    self.trace.reentry_panicked.set(true);
                    return Err(E_FAIL.into());
                }
            }
        }

        self.trace
            .successful_adds
            .set(self.trace.successful_adds.get() + 1);
        Ok(())
    }

    fn RemoveItem(&self, item: Option<&ITfLangBarItem>) -> Result<()> {
        self.trace
            .remove_calls
            .set(self.trace.remove_calls.get() + 1);
        if item.is_none() {
            return Err(E_FAIL.into());
        }
        if self.trace.reenter_during_remove.replace(false) {
            if let Some(state) = self.trace.state.upgrade() {
                self.trace
                    .reentry_calls
                    .set(self.trace.reentry_calls.get() + 1);
                if catch_unwind(AssertUnwindSafe(|| refresh_language_bar(&state))).is_err() {
                    self.trace.reentry_panicked.set(true);
                    return Err(E_FAIL.into());
                }
            }
        }
        Ok(())
    }

    unimplemented_methods! {
        fn EnumItems(&self) -> IEnumTfLangBarItems;
        fn GetItem(&self, guid: *const GUID) -> ITfLangBarItem;
        fn AdviseItemSink(&self, sink: Option<&ITfLangBarItemSink>, cookie: *mut u32, guid: *const GUID) -> ();
        fn UnadviseItemSink(&self, cookie: u32) -> ();
        fn GetItemFloatingRect(&self, thread: u32, guid: *const GUID) -> RECT;
        fn GetItemsStatus(&self, count: u32, guids: *const GUID, status: *mut u32) -> ();
        fn GetItemNum(&self) -> u32;
        fn GetItems(&self, count: u32, items: *mut Option<ITfLangBarItem>, info: *mut TF_LANGBARITEMINFO, status: *mut u32, fetched: *mut u32) -> ();
        fn AdviseItemsSink(&self, count: u32, sinks: *const Option<ITfLangBarItemSink>, guids: *const GUID, cookies: *mut u32) -> ();
        fn UnadviseItemsSink(&self, count: u32, cookies: *const u32) -> ();
    }
}

fn mock_state(fail_first_add: bool, reenter_once: bool) -> (SharedState, Rc<RegistrationTrace>) {
    let state = new_shared_state();
    let trace = Rc::new(RegistrationTrace {
        state: Rc::downgrade(&state),
        fail_first_add: Cell::new(fail_first_add),
        reenter_once: Cell::new(reenter_once),
        deactivate_during_add: Cell::new(false),
        reenter_during_remove: Cell::new(false),
        add_attempts: Cell::new(0),
        successful_adds: Cell::new(0),
        remove_calls: Cell::new(0),
        reentry_calls: Cell::new(0),
        reentry_panicked: Cell::new(false),
    });
    let manager: ITfThreadMgr = MockThreadMgr {
        trace: Rc::clone(&trace),
    }
    .into();
    {
        let mut st = state.borrow_mut();
        st.thread_mgr = Some(manager);
        st.client_id = 7;
        st.language_bar_enabled = true;
    }
    (state, trace)
}

fn assert_engine_is_unloaded(state: &SharedState) {
    let st = state.borrow();
    assert!(st.runtime.is_none());
    assert!(st.session.is_none());
    assert!(!st.engine_building);
}

#[test]
fn failed_language_bar_registration_retries_without_duplicating_a_successful_item() {
    let (state, trace) = mock_state(true, false);

    refresh_language_bar(&state);
    assert_eq!(trace.add_attempts.get(), 1);
    assert_eq!(trace.successful_adds.get(), 0);
    assert!(state.borrow().language_bar.is_none());
    assert!(!state.borrow().language_bar_refreshing);

    refresh_language_bar(&state);
    assert_eq!(trace.add_attempts.get(), 2);
    assert_eq!(trace.successful_adds.get(), 1);
    assert!(state.borrow().language_bar.is_some());
    assert!(!state.borrow().language_bar_refreshing);

    refresh_language_bar(&state);
    assert_eq!(trace.add_attempts.get(), 2);
    assert_eq!(trace.successful_adds.get(), 1);
    assert!(state.borrow().language_bar.is_some());
    assert_engine_is_unloaded(&state);
}

#[test]
fn synchronous_focus_callback_during_add_does_not_register_twice_or_borrow_panic() {
    let (state, trace) = mock_state(false, true);

    refresh_language_bar(&state);

    assert_eq!(trace.reentry_calls.get(), 1);
    assert!(!trace.reentry_panicked.get());
    assert_eq!(trace.add_attempts.get(), 1);
    assert_eq!(trace.successful_adds.get(), 1);
    assert!(state.borrow().language_bar.is_some());
    assert!(!state.borrow().language_bar_refreshing);
    assert_engine_is_unloaded(&state);
}

#[test]
fn deactivation_during_add_removes_the_completed_registration() {
    let (state, trace) = mock_state(false, false);
    trace.deactivate_during_add.set(true);

    refresh_language_bar(&state);

    assert!(!trace.reentry_panicked.get());
    assert_eq!(trace.add_attempts.get(), 1);
    assert_eq!(trace.successful_adds.get(), 1);
    assert_eq!(trace.remove_calls.get(), 1);
    {
        let st = state.borrow();
        assert!(!st.language_bar_enabled);
        assert!(st.thread_mgr.is_none());
        assert_eq!(st.client_id, 0);
        assert!(st.language_bar.is_none());
        assert!(!st.language_bar_refreshing);
    }
    assert_engine_is_unloaded(&state);
}

#[test]
fn focus_callback_during_deactivation_does_not_recreate_the_removed_item() {
    let (state, trace) = mock_state(false, false);
    refresh_language_bar(&state);
    assert_eq!(trace.successful_adds.get(), 1);

    let item = {
        let mut st = state.borrow_mut();
        // Deactivation disables registration before calling TSF. The manager
        // and client id remain present until those external calls complete.
        st.language_bar_enabled = false;
        st.language_bar.take().unwrap()
    };
    trace.reenter_during_remove.set(true);
    item.remove();

    assert_eq!(trace.remove_calls.get(), 1);
    assert_eq!(trace.reentry_calls.get(), 1);
    assert!(!trace.reentry_panicked.get());
    assert_eq!(trace.add_attempts.get(), 1);
    assert_eq!(trace.successful_adds.get(), 1);
    assert!(state.borrow().language_bar.is_none());
    assert!(!state.borrow().language_bar_refreshing);
    assert_engine_is_unloaded(&state);
}
