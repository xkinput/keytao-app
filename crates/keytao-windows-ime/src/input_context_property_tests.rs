//! Exercise the actual COM HRESULT boundary: the Windows projection otherwise
//! hides S_FALSE + null as E_POINTER for an absent application property.

use super::{input_scope_value_probe, read_password_scope, ContextProbe};
use std::{cell::Cell, mem::ManuallyDrop, rc::Rc};
use windows::{
    core::{
        implement, Error, IUnknown, Interface, Result, BSTR, GUID, HRESULT, PCWSTR, PWSTR, VARIANT,
    },
    Win32::{
        Foundation::{BOOL, E_ACCESSDENIED, E_FAIL, E_NOTIMPL, S_FALSE, S_OK},
        System::Com::{CoTaskMemAlloc, IDataObject},
        UI::TextServices::*,
    },
};

#[implement(ITfContext)]
struct PropertylessContext {
    result: HRESULT,
}

#[implement(ITfInputScope)]
struct DeclaredScope(InputScope);

impl ITfInputScope_Impl for DeclaredScope_Impl {
    fn GetInputScopes(&self, buffer: *mut *mut InputScope, count: *mut u32) -> Result<()> {
        unsafe {
            let allocated = CoTaskMemAlloc(std::mem::size_of::<InputScope>()).cast::<InputScope>();
            assert!(!allocated.is_null());
            allocated.write(self.0);
            *buffer = allocated;
            *count = 1;
        }
        Ok(())
    }

    fn GetPhrase(&self, _phrases: *mut *mut BSTR, _count: *mut u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetRegularExpression(&self) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetSRGS(&self) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetXML(&self) -> Result<BSTR> {
        Err(E_NOTIMPL.into())
    }
}

#[test]
fn input_scope_com_values_keep_declared_sensitive_fields_blocked() {
    for (scope, expected) in [
        (IS_DEFAULT, ContextProbe::Clear),
        (IS_PASSWORD, ContextProbe::Restricted),
        (IS_NUMERIC_PIN, ContextProbe::Restricted),
        (IS_PRIVATE, ContextProbe::Restricted),
    ] {
        let object: ITfInputScope = DeclaredScope(scope).into();
        let value = VARIANT::from(object.cast::<windows::core::IUnknown>().unwrap());
        assert_eq!(unsafe { input_scope_value_probe(&value) }, expected);
    }
}

#[test]
fn absent_scope_values_and_malformed_values_stay_distinct() {
    assert_eq!(
        unsafe { input_scope_value_probe(&VARIANT::default()) },
        ContextProbe::Clear
    );
    assert_eq!(
        unsafe { input_scope_value_probe(&VARIANT::from(42_i32)) },
        ContextProbe::Unknown
    );
    let wrong_interface: ITfContext = PropertylessContext { result: E_FAIL }.into();
    let value = VARIANT::from(wrong_interface.cast::<windows::core::IUnknown>().unwrap());
    assert_eq!(
        unsafe { input_scope_value_probe(&value) },
        ContextProbe::Unknown
    );
}

macro_rules! unimplemented_methods {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> { Err(E_NOTIMPL.into()) })*
    };
}

impl ITfContext_Impl for PropertylessContext_Impl {
    fn GetAppProperty(&self, _guid: *const GUID) -> Result<ITfReadOnlyProperty> {
        Err(Error::from(self.result))
    }
    unimplemented_methods! {
        fn RequestEditSession(&self, tid: u32, session: Option<&ITfEditSession>, flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS) -> HRESULT;
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
        fn GetDocumentMgr(&self) -> ITfDocumentMgr;
        fn CreateRangeBackup(&self, ec: u32, range: Option<&ITfRange>) -> ITfRangeBackup;
    }
}

#[test]
fn explicitly_absent_input_scope_is_not_a_password_probe_failure() {
    for result in [S_FALSE, E_NOTIMPL] {
        let context: ITfContext = PropertylessContext { result }.into();
        assert_eq!(read_password_scope(1, &context), ContextProbe::Clear);
    }
}

#[test]
fn failed_input_scope_queries_remain_restricted() {
    for result in [E_FAIL, TF_E_DISCONNECTED] {
        let context: ITfContext = PropertylessContext { result }.into();
        assert_eq!(read_password_scope(1, &context), ContextProbe::Unknown);
    }
}

// These mocks expose no native view, window, document text, or TSF manager.
// The complete read_password_scope path must distinguish a host's failed
// optional GetValue from an explicitly declared sensitive scope.
const PROBE_COOKIE: u32 = 73;

#[implement(ITfRange)]
struct SelectionRange;

impl ITfRange_Impl for SelectionRange_Impl {
    unimplemented_methods! {
        fn GetText(&self, ec: u32, flags: u32, text: PWSTR, max: u32, count: *mut u32) -> ();
        fn SetText(&self, ec: u32, flags: u32, text: &PCWSTR, count: i32) -> ();
        fn GetFormattedText(&self, ec: u32) -> IDataObject;
        fn GetEmbedded(&self, ec: u32, service: *const GUID, iid: *const GUID) -> IUnknown;
        fn InsertEmbedded(&self, ec: u32, flags: u32, data: Option<&IDataObject>) -> ();
        fn ShiftStart(&self, ec: u32, requested: i32, shifted: *mut i32, halt: *const TF_HALTCOND) -> ();
        fn ShiftEnd(&self, ec: u32, requested: i32, shifted: *mut i32, halt: *const TF_HALTCOND) -> ();
        fn ShiftStartToRange(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> ();
        fn ShiftEndToRange(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> ();
        fn ShiftStartRegion(&self, ec: u32, direction: TfShiftDir) -> BOOL;
        fn ShiftEndRegion(&self, ec: u32, direction: TfShiftDir) -> BOOL;
        fn IsEmpty(&self, ec: u32) -> BOOL;
        fn Collapse(&self, ec: u32, anchor: TfAnchor) -> ();
        fn IsEqualStart(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> BOOL;
        fn IsEqualEnd(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> BOOL;
        fn CompareStart(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> i32;
        fn CompareEnd(&self, ec: u32, range: Option<&ITfRange>, anchor: TfAnchor) -> i32;
        fn AdjustForInsert(&self, ec: u32, count: u32) -> BOOL;
        fn GetGravity(&self, start: *mut TfGravity, end: *mut TfGravity) -> ();
        fn SetGravity(&self, ec: u32, start: TfGravity, end: TfGravity) -> ();
        fn Clone(&self) -> ITfRange;
        fn GetContext(&self) -> ITfContext;
    }
}

#[derive(Clone, Copy)]
enum PropertyAnswer {
    Failure(HRESULT),
    Scope(InputScope),
    Empty,
    Integer,
    WrongInterface,
    MissingScopeBuffer,
}

#[implement(ITfInputScope)]
struct MissingScopeBuffer;

impl ITfInputScope_Impl for MissingScopeBuffer_Impl {
    fn GetInputScopes(&self, buffer: *mut *mut InputScope, count: *mut u32) -> Result<()> {
        unsafe {
            *buffer = std::ptr::null_mut();
            *count = 1;
        }
        Ok(())
    }
    unimplemented_methods! {
        fn GetPhrase(&self, phrases: *mut *mut BSTR, count: *mut u32) -> ();
        fn GetRegularExpression(&self) -> BSTR;
        fn GetSRGS(&self) -> BSTR;
        fn GetXML(&self) -> BSTR;
    }
}

#[implement(ITfReadOnlyProperty)]
struct ScopeProperty {
    answer: PropertyAnswer,
    selection: ITfRange,
    reads: Rc<Cell<usize>>,
}

impl ITfReadOnlyProperty_Impl for ScopeProperty_Impl {
    fn GetValue(&self, ec: u32, range: Option<&ITfRange>) -> Result<VARIANT> {
        assert_eq!(ec, PROBE_COOKIE);
        assert_eq!(range.map(Interface::as_raw), Some(self.selection.as_raw()));
        self.reads.set(self.reads.get() + 1);
        match self.answer {
            PropertyAnswer::Failure(hr) => Err(Error::from(hr)),
            PropertyAnswer::Scope(scope) => {
                let object: ITfInputScope = DeclaredScope(scope).into();
                Ok(VARIANT::from(object.cast::<IUnknown>().unwrap()))
            }
            PropertyAnswer::Empty => Ok(VARIANT::default()),
            PropertyAnswer::Integer => Ok(VARIANT::from(42_i32)),
            PropertyAnswer::WrongInterface => {
                Ok(VARIANT::from(self.selection.cast::<IUnknown>().unwrap()))
            }
            PropertyAnswer::MissingScopeBuffer => {
                let object: ITfInputScope = MissingScopeBuffer.into();
                Ok(VARIANT::from(object.cast::<IUnknown>().unwrap()))
            }
        }
    }
    unimplemented_methods! {
        fn GetType(&self) -> GUID;
        fn EnumRanges(&self, ec: u32, ranges: *mut Option<IEnumTfRanges>, target: Option<&ITfRange>) -> ();
        fn GetContext(&self) -> ITfContext;
    }
}

#[implement(ITfContext)]
struct ScopeContext {
    property: ITfReadOnlyProperty,
    selection: Option<ITfRange>,
    selection_result: HRESULT,
    selection_count: u32,
}

impl ITfContext_Impl for ScopeContext_Impl {
    fn GetAppProperty(&self, guid: *const GUID) -> Result<ITfReadOnlyProperty> {
        assert!(!guid.is_null());
        assert_eq!(unsafe { *guid }, GUID_PROP_INPUTSCOPE);
        Ok(self.property.clone())
    }
    fn GetSelection(
        &self,
        ec: u32,
        index: u32,
        count: u32,
        selection: *mut TF_SELECTION,
        fetched: *mut u32,
    ) -> Result<()> {
        assert_eq!(ec, PROBE_COOKIE);
        assert_eq!(index, TF_DEFAULT_SELECTION);
        assert_eq!(count, 1);
        unsafe {
            *selection = TF_SELECTION {
                range: ManuallyDrop::new(self.selection.clone()),
                style: TF_SELECTIONSTYLE {
                    ase: TF_AE_END,
                    fInterimChar: BOOL(0),
                },
            };
            *fetched = self.selection_count;
        }
        self.selection_result.ok()
    }
    unimplemented_methods! {
        fn RequestEditSession(&self, tid: u32, session: Option<&ITfEditSession>, flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS) -> HRESULT;
        fn InWriteSession(&self, tid: u32) -> BOOL;
        fn SetSelection(&self, ec: u32, count: u32, selection: *const TF_SELECTION) -> ();
        fn GetStart(&self, ec: u32) -> ITfRange;
        fn GetEnd(&self, ec: u32) -> ITfRange;
        fn GetActiveView(&self) -> ITfContextView;
        fn EnumViews(&self) -> IEnumTfContextViews;
        fn GetStatus(&self) -> TS_STATUS;
        fn GetProperty(&self, guid: *const GUID) -> ITfProperty;
        fn TrackProperties(&self, props: *const *const GUID, count: u32, app_props: *const *const GUID, app_count: u32) -> ITfReadOnlyProperty;
        fn EnumProperties(&self) -> IEnumTfProperties;
        fn GetDocumentMgr(&self) -> ITfDocumentMgr;
        fn CreateRangeBackup(&self, ec: u32, range: Option<&ITfRange>) -> ITfRangeBackup;
    }
}

fn scope_context(answer: PropertyAnswer) -> (ScopeContext, Rc<Cell<usize>>) {
    let selection: ITfRange = SelectionRange.into();
    let reads = Rc::new(Cell::new(0));
    let property: ITfReadOnlyProperty = ScopeProperty {
        answer,
        selection: selection.clone(),
        reads: reads.clone(),
    }
    .into();
    (
        ScopeContext {
            property,
            selection: Some(selection),
            selection_result: S_OK,
            selection_count: 1,
        },
        reads,
    )
}

fn probe_property(answer: PropertyAnswer) -> ContextProbe {
    let (context, reads) = scope_context(answer);
    let context: ITfContext = context.into();
    let result = read_password_scope(PROBE_COOKIE, &context);
    assert_eq!(
        reads.get(),
        1,
        "probe must read the selected range's property"
    );
    result
}

#[test]
fn optional_scope_value_e_fail_is_unavailable_without_a_native_view() {
    assert_eq!(
        probe_property(PropertyAnswer::Failure(E_FAIL)),
        ContextProbe::Unavailable
    );
}

#[test]
fn optional_scope_value_other_failures_remain_unknown() {
    for hr in [TF_E_NOLOCK, TF_E_DISCONNECTED, E_NOTIMPL, E_ACCESSDENIED] {
        assert_eq!(
            probe_property(PropertyAnswer::Failure(hr)),
            ContextProbe::Unknown,
            "HRESULT {hr:?} must not be treated as optional metadata"
        );
    }
}

#[test]
fn full_property_probe_preserves_every_declared_sensitive_scope() {
    for scope in [
        IS_PASSWORD,
        IS_NUMERIC_PASSWORD,
        IS_NUMERIC_PIN,
        IS_ALPHANUMERIC_PIN,
        IS_ALPHANUMERIC_PIN_SET,
        IS_PRIVATE,
    ] {
        assert_eq!(
            probe_property(PropertyAnswer::Scope(scope)),
            ContextProbe::Restricted,
            "scope {scope:?}"
        );
    }
    assert_eq!(
        probe_property(PropertyAnswer::Scope(IS_DEFAULT)),
        ContextProbe::Clear
    );
    assert_eq!(probe_property(PropertyAnswer::Empty), ContextProbe::Clear);
}

#[test]
fn full_property_probe_does_not_open_malformed_scope_values() {
    for answer in [
        PropertyAnswer::Integer,
        PropertyAnswer::WrongInterface,
        PropertyAnswer::MissingScopeBuffer,
    ] {
        assert_eq!(probe_property(answer), ContextProbe::Unknown);
    }
}

#[test]
fn unavailable_metadata_requires_a_valid_selection() {
    for (result, count, has_range) in [
        (E_FAIL, 1, true),
        (TF_E_NOLOCK, 1, true),
        (S_OK, 0, false),
        (S_OK, 1, false),
    ] {
        let (mut context, reads) = scope_context(PropertyAnswer::Failure(E_FAIL));
        context.selection_result = result;
        context.selection_count = count;
        if !has_range {
            context.selection = None;
        }
        let context: ITfContext = context.into();
        assert_eq!(
            read_password_scope(PROBE_COOKIE, &context),
            ContextProbe::Unknown
        );
        assert_eq!(
            reads.get(),
            0,
            "invalid selection must stop before GetValue"
        );
    }
}
