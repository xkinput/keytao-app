//! Real COM calls against a synthetic UILess host; no registered TIP or engine.
use std::{cell::Cell, cell::RefCell, rc::Rc};

use keytao_core::{Candidate, ImeState};
use windows::{
    core::{implement, Interface, Result, GUID},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, E_NOTIMPL, HWND},
        UI::TextServices::*,
    },
};

use super::CandidateUiManager;

macro_rules! unimplemented_methods {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> {
            Err(E_NOTIMPL.into())
        })*
    };
}

#[derive(Default)]
struct Host {
    show_own_window: Cell<bool>,
    fail_begin: Cell<bool>,
    element: RefCell<Option<ITfUIElement>>,
    snapshots: RefCell<Vec<Vec<String>>>,
    ended: Cell<usize>,
}

#[implement(ITfThreadMgr, ITfUIElementMgr)]
struct ThreadManager {
    host: Rc<Host>,
}

impl ITfThreadMgr_Impl for ThreadManager_Impl {
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

impl ITfUIElementMgr_Impl for ThreadManager_Impl {
    fn BeginUIElement(
        &self,
        element: Option<&ITfUIElement>,
        show: *mut BOOL,
        id: *mut u32,
    ) -> Result<()> {
        if self.host.fail_begin.get() {
            return Err(E_FAIL.into());
        }
        if show.is_null() || id.is_null() || element.is_none() {
            return Err(E_INVALIDARG.into());
        }
        self.host.element.replace(element.cloned());
        unsafe {
            *show = BOOL::from(self.host.show_own_window.get());
            *id = 123;
        }
        Ok(())
    }

    fn UpdateUIElement(&self, id: u32) -> Result<()> {
        let candidates = self.GetUIElement(id)?.cast::<ITfCandidateListUIElement>()?;
        let mut snapshot = Vec::new();
        unsafe {
            for index in 0..candidates.GetCount()? {
                snapshot.push(candidates.GetString(index)?.to_string());
            }
        }
        self.host.snapshots.borrow_mut().push(snapshot);
        Ok(())
    }

    fn EndUIElement(&self, id: u32) -> Result<()> {
        self.GetUIElement(id)?;
        self.host.element.replace(None);
        self.host.ended.set(self.host.ended.get() + 1);
        Ok(())
    }

    fn GetUIElement(&self, id: u32) -> Result<ITfUIElement> {
        if id != 123 {
            return Err(E_INVALIDARG.into());
        }
        self.host
            .element
            .borrow()
            .clone()
            .ok_or_else(|| E_FAIL.into())
    }

    unimplemented_methods! {
        fn EnumUIElements(&self) -> IEnumTfUIElements;
    }
}

fn fixture() -> (CandidateUiManager, ITfThreadMgr, Rc<Host>, ImeState) {
    let host = Rc::new(Host::default());
    let thread: ITfThreadMgr = ThreadManager { host: host.clone() }.into();
    let mut state = ImeState::empty();
    state.preedit = "ni".into();
    state.candidates.push(Candidate {
        text: "你".into(),
        comment: None,
    });
    (CandidateUiManager::new(), thread, host, state)
}

#[test]
fn uiless_host_receives_candidates_and_updates_without_tip_window() {
    let (mut manager, thread, host, mut state) = fixture();
    assert!(!manager.update(Some(&thread), None, &state, false));
    assert_eq!(*host.snapshots.borrow(), vec![vec!["你"]]);
    state.candidates[0].text = "拟".into();
    assert!(!manager.update(Some(&thread), None, &state, false));
    assert_eq!(host.snapshots.borrow().last().unwrap(), &vec!["拟"]);
    assert!(!manager.update(Some(&thread), None, &ImeState::empty(), false));
    assert_eq!(host.ended.get(), 1);
    assert!(host.element.borrow().is_none());
}

#[test]
fn uiless_activation_still_honors_hosts_explicit_permission_to_draw() {
    let (mut manager, thread, host, state) = fixture();
    host.show_own_window.set(true);
    assert!(manager.update(Some(&thread), None, &state, false));
    unsafe {
        host.element.borrow().as_ref().unwrap().Show(false).unwrap();
    }
    assert!(!manager.update(Some(&thread), None, &state, false));
}

#[test]
fn uiless_host_failure_never_falls_back_to_an_unapproved_window() {
    let (mut manager, thread, host, state) = fixture();
    host.fail_begin.set(true);
    assert!(!manager.update(Some(&thread), None, &state, false));
    assert!(!manager.update(None, None, &state, false));
    assert!(host.snapshots.borrow().is_empty());
    host.fail_begin.set(false);
    assert!(!manager.update(Some(&thread), None, &state, false));
    assert_eq!(host.snapshots.borrow().len(), 1);
}
