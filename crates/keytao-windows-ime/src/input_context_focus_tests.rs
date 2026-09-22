//! Context identity and background-document regressions through COM mocks.
//! No profile activation, real edit control, or Rime initialization is involved.
use super::ThreadMgrEventSink;
use crate::{
    globals::DllActivityGuard,
    input_context::ContextProbe,
    state::{self, SharedState},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::{
    core::{implement, Error, IUnknown, Result, GUID, HRESULT, VARIANT},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_NOTIMPL, HWND, S_FALSE, S_OK},
        System::Com::IEnumGUID,
        UI::TextServices::*,
    },
};

macro_rules! unused_methods {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> { Err(E_NOTIMPL.into()) })*
    };
}

#[implement(ITfCompartment)]
struct Compartment {
    value: i32,
}
impl ITfCompartment_Impl for Compartment_Impl {
    fn GetValue(&self) -> Result<VARIANT> {
        Ok(VARIANT::from(self.value))
    }
    unused_methods! { fn SetValue(&self, tid: u32, value: *const VARIANT) -> (); }
}

struct ContextData {
    disabled: bool,
    property_result: Cell<HRESULT>,
    reads: Cell<usize>,
    document: RefCell<Option<ITfDocumentMgr>>,
}
#[implement(ITfContext, ITfCompartmentMgr)]
struct Context {
    data: Rc<ContextData>,
}
impl ITfContext_Impl for Context_Impl {
    fn RequestEditSession(
        &self,
        _tid: u32,
        session: Option<&ITfEditSession>,
        _flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    ) -> Result<HRESULT> {
        self.data.reads.set(self.data.reads.get() + 1);
        let session = session.ok_or_else(|| Error::from(E_FAIL))?;
        Ok(match unsafe { session.DoEditSession(7) } {
            Ok(()) => S_OK,
            Err(error) => error.code(),
        })
    }
    fn GetAppProperty(&self, _guid: *const GUID) -> Result<ITfReadOnlyProperty> {
        Err(Error::from(self.data.property_result.get()))
    }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.data
            .document
            .borrow()
            .clone()
            .ok_or_else(|| Error::from(S_FALSE))
    }
    unused_methods! {
        fn InWriteSession(&self, tid: u32) -> BOOL;
        fn GetSelection(&self, ec: u32, index: u32, count: u32, selection: *mut TF_SELECTION, fetched: *mut u32) -> ();
        fn SetSelection(&self, ec: u32, count: u32, selection: *const TF_SELECTION) -> ();
        fn GetStart(&self, ec: u32) -> ITfRange;
        fn GetEnd(&self, ec: u32) -> ITfRange;
        fn GetActiveView(&self) -> ITfContextView;
        fn EnumViews(&self) -> IEnumTfContextViews;
        fn GetStatus(&self) -> TS_STATUS;
        fn GetProperty(&self, guid: *const GUID) -> ITfProperty;
        fn TrackProperties(&self, props: *const *const GUID, count: u32, app_props: *const *const GUID, app_count: u32) -> ITfReadOnlyProperty;
        fn EnumProperties(&self) -> IEnumTfProperties;
        fn CreateRangeBackup(&self, ec: u32, range: Option<&ITfRange>) -> ITfRangeBackup;
    }
}
impl ITfCompartmentMgr_Impl for Context_Impl {
    fn GetCompartment(&self, guid: *const GUID) -> Result<ITfCompartment> {
        if guid.is_null() {
            return Err(E_FAIL.into());
        }
        let disabled = unsafe { *guid } == GUID_COMPARTMENT_KEYBOARD_DISABLED && self.data.disabled;
        Ok(Compartment {
            value: i32::from(disabled),
        }
        .into())
    }
    unused_methods! {
        fn ClearCompartment(&self, tid: u32, guid: *const GUID) -> ();
        fn EnumCompartments(&self) -> IEnumGUID;
    }
}

#[implement(ITfDocumentMgr)]
struct Document {
    top: Rc<RefCell<Option<ITfContext>>>,
}
impl ITfDocumentMgr_Impl for Document_Impl {
    fn GetTop(&self) -> Result<ITfContext> {
        self.top.borrow().clone().ok_or_else(|| E_FAIL.into())
    }
    fn GetBase(&self) -> Result<ITfContext> {
        self.GetTop()
    }
    unused_methods! {
        fn CreateContext(&self, tid: u32, flags: u32, owner: Option<&IUnknown>, context: *mut Option<ITfContext>, cookie: *mut u32) -> ();
        fn Push(&self, context: Option<&ITfContext>) -> ();
        fn Pop(&self, flags: u32) -> ();
        fn EnumContexts(&self) -> IEnumTfContexts;
    }
}

struct Editor {
    context: ITfContext,
    document: ITfDocumentMgr,
    data: Rc<ContextData>,
    top: Rc<RefCell<Option<ITfContext>>>,
}
impl Editor {
    fn new(disabled: bool) -> Self {
        let top = Rc::new(RefCell::new(None));
        let document: ITfDocumentMgr = Document { top: top.clone() }.into();
        let data = Rc::new(ContextData {
            disabled,
            property_result: Cell::new(S_FALSE),
            reads: Cell::new(0),
            document: RefCell::new(Some(document.clone())),
        });
        let context: ITfContext = Context { data: data.clone() }.into();
        top.replace(Some(context.clone()));
        Self {
            context,
            document,
            data,
            top,
        }
    }
    fn detach(&self) {
        self.top.replace(None);
        self.data.document.replace(None);
    }
}
impl Drop for Editor {
    fn drop(&mut self) {
        self.detach();
    }
}

#[implement(ITfThreadMgr)]
struct ThreadManager {
    focus: Rc<RefCell<Option<ITfDocumentMgr>>>,
}
impl ITfThreadMgr_Impl for ThreadManager_Impl {
    fn GetFocus(&self) -> Result<ITfDocumentMgr> {
        self.focus.borrow().clone().ok_or_else(|| E_FAIL.into())
    }
    unused_methods! {
        fn Activate(&self) -> u32;
        fn Deactivate(&self) -> ();
        fn CreateDocumentMgr(&self) -> ITfDocumentMgr;
        fn EnumDocumentMgrs(&self) -> IEnumTfDocumentMgrs;
        fn SetFocus(&self, focus: Option<&ITfDocumentMgr>) -> ();
        fn AssociateFocus(&self, window: HWND, focus: Option<&ITfDocumentMgr>) -> ITfDocumentMgr;
        fn IsThreadFocus(&self) -> BOOL;
        fn GetFunctionProvider(&self, clsid: *const GUID) -> ITfFunctionProvider;
        fn EnumFunctionProviders(&self) -> IEnumTfFunctionProviders;
        fn GetGlobalCompartment(&self) -> ITfCompartmentMgr;
    }
}

fn fixture(editor: &Editor) -> (SharedState, ITfThreadMgrEventSink) {
    let state = state::new_shared_state();
    let manager: ITfThreadMgr = ThreadManager {
        focus: Rc::new(RefCell::new(Some(editor.document.clone()))),
    }
    .into();
    {
        let mut data = state.borrow_mut();
        data.client_id = 1;
        data.thread_mgr = Some(manager);
        // Event tests must never start an actual background Rime build.
        data.engine_building = true;
    }
    state::refresh_input_context(&state, Some(&editor.context));
    let sink: ITfThreadMgrEventSink = ThreadMgrEventSink {
        state: Rc::downgrade(&state),
        _dll_guard: DllActivityGuard::new(),
    }
    .into();
    (state, sink)
}

#[test]
fn supplied_context_replaces_both_clear_and_restricted_cached_policy() {
    for (old_disabled, new_disabled) in [(true, false), (false, true)] {
        let old = Editor::new(old_disabled);
        let new = Editor::new(new_disabled);
        let (state, _sink) = fixture(&old);
        // No focus notification: this is the context delivered with the key.
        assert_eq!(
            state::input_is_blocked(&state, Some(&new.context)),
            new_disabled
        );
        assert!(state::input_context_matches(&state, Some(&new.context)));
        assert_eq!(new.data.reads.get(), 1);
        assert_eq!(
            state.borrow().input_context.keyboard_disabled,
            if new_disabled {
                ContextProbe::Restricted
            } else {
                ContextProbe::Clear
            }
        );
    }
}

#[test]
fn new_unknown_context_never_inherits_the_previous_clear_scope() {
    let old = Editor::new(false);
    let unknown = Editor::new(false);
    unknown.data.property_result.set(E_FAIL);
    let (state, _sink) = fixture(&old);
    assert!(state::input_is_blocked(&state, Some(&unknown.context)));
    assert_eq!(state.borrow().input_context.password, ContextProbe::Unknown);
    assert_eq!(unknown.data.reads.get(), 1);
    // The duplicate safety check in the same key callback remains throttled.
    assert!(state::input_is_blocked(&state, Some(&unknown.context)));
    assert_eq!(unknown.data.reads.get(), 1);
}

#[test]
fn background_push_pop_and_uninit_preserve_the_active_editor_and_preedit() {
    let focused = Editor::new(false);
    let background = Editor::new(true);
    let (state, sink) = fixture(&focused);
    let mut preedit = keytao_core::ImeState::empty();
    preedit.preedit = "pending".into();
    state.borrow_mut().ime_state = Some(preedit);
    unsafe {
        sink.OnPushContext(&background.context).unwrap();
        background.detach();
        sink.OnPopContext(&background.context).unwrap();
        sink.OnUninitDocumentMgr(&background.document).unwrap();
    }
    assert!(state::input_context_matches(&state, Some(&focused.context)));
    assert_eq!(
        state.borrow().ime_state.as_ref().unwrap().preedit,
        "pending"
    );
    assert_eq!(background.data.reads.get(), 0);
    assert!(!state::input_is_blocked(&state, Some(&focused.context)));
}

#[test]
fn focused_push_refreshes_policy_and_detached_pop_clears_it() {
    let editor = Editor::new(false);
    let (state, sink) = fixture(&editor);
    editor.data.property_result.set(E_FAIL);
    unsafe {
        sink.OnPushContext(&editor.context).unwrap();
    }
    assert_eq!(state.borrow().input_context.password, ContextProbe::Unknown);
    editor.detach();
    unsafe {
        sink.OnPopContext(&editor.context).unwrap();
    }
    assert!(state::input_context_matches(&state, None));
    assert!(state.borrow().input_context.is_sensitive());
}

#[test]
fn focused_document_uninit_still_cleans_up_after_its_context_detaches() {
    let editor = Editor::new(false);
    let (state, sink) = fixture(&editor);
    let mut preedit = keytao_core::ImeState::empty();
    preedit.preedit = "pending".into();
    state.borrow_mut().ime_state = Some(preedit);
    editor.detach();
    unsafe {
        sink.OnUninitDocumentMgr(&editor.document).unwrap();
    }
    assert!(state.borrow().ime_state.is_none());
    assert!(state::input_context_matches(&state, None));
}
