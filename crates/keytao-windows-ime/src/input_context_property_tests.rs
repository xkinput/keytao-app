//! Exercise the actual COM HRESULT boundary: the Windows projection otherwise
//! hides S_FALSE + null as E_POINTER for an absent application property.

use super::{input_scope_value_probe, read_password_scope, ContextProbe};
use windows::{
    core::{implement, Error, Interface, Result, BSTR, GUID, HRESULT, VARIANT},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_NOTIMPL, S_FALSE},
        System::Com::CoTaskMemAlloc,
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
