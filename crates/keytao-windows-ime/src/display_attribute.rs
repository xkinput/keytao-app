//! TSF display attribute provider for active composition text.

use std::cell::{Cell, RefCell};

use windows::{
    core::{implement, Error, Result, BSTR, GUID},
    Win32::{
        Foundation::{BOOL, COLORREF, E_INVALIDARG, S_FALSE},
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::TextServices::{
            CLSID_TF_CategoryMgr, IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl,
            ITfCategoryMgr, ITfDisplayAttributeInfo, ITfDisplayAttributeInfo_Impl, TF_ATTR_INPUT,
            TF_CT_COLORREF, TF_CT_NONE, TF_DA_COLOR, TF_DA_COLOR_0, TF_DISPLAYATTRIBUTE, TF_LS_DOT,
        },
    },
};

use crate::{globals::DllActivityGuard, GUID_DISPLAY_ATTRIBUTE_INPUT};

fn default_attribute() -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        crText: TF_DA_COLOR {
            r#type: TF_CT_NONE,
            Anonymous: TF_DA_COLOR_0 { cr: COLORREF(0) },
        },
        crBk: TF_DA_COLOR {
            r#type: TF_CT_NONE,
            Anonymous: TF_DA_COLOR_0 { cr: COLORREF(0) },
        },
        lsStyle: TF_LS_DOT,
        fBoldLine: BOOL::from(false),
        crLine: TF_DA_COLOR {
            r#type: TF_CT_COLORREF,
            Anonymous: TF_DA_COLOR_0 {
                cr: COLORREF(0x00ce_6700),
            },
        },
        bAttr: TF_ATTR_INPUT,
    }
}

#[implement(ITfDisplayAttributeInfo)]
struct DisplayAttributeInfo {
    attribute: RefCell<TF_DISPLAYATTRIBUTE>,
    _dll_guard: DllActivityGuard,
}

impl DisplayAttributeInfo {
    fn new() -> Self {
        Self {
            attribute: RefCell::new(default_attribute()),
            _dll_guard: DllActivityGuard::new(),
        }
    }
}

impl ITfDisplayAttributeInfo_Impl for DisplayAttributeInfo_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID_DISPLAY_ATTRIBUTE_INPUT)
    }

    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("KeyTao composition input"))
    }

    fn GetAttributeInfo(&self, attribute: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if attribute.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            *attribute = *self.attribute.borrow();
        }
        Ok(())
    }

    fn SetAttributeInfo(&self, attribute: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        if attribute.is_null() {
            return Err(E_INVALIDARG.into());
        }
        *self.attribute.borrow_mut() = unsafe { *attribute };
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        *self.attribute.borrow_mut() = default_attribute();
        Ok(())
    }
}

#[implement(IEnumTfDisplayAttributeInfo)]
struct DisplayAttributeEnumerator {
    yielded: Cell<bool>,
    _dll_guard: DllActivityGuard,
}

impl DisplayAttributeEnumerator {
    fn new(yielded: bool) -> Self {
        Self {
            yielded: Cell::new(yielded),
            _dll_guard: DllActivityGuard::new(),
        }
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for DisplayAttributeEnumerator_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(DisplayAttributeEnumerator::new(self.yielded.get()).into())
    }

    fn Next(
        &self,
        count: u32,
        info: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        if count == 0 || info.is_null() || (count != 1 && fetched.is_null()) {
            return Err(E_INVALIDARG.into());
        }

        let actual = u32::from(!self.yielded.get());
        unsafe {
            if !fetched.is_null() {
                *fetched = actual;
            }
            if actual == 1 {
                info.write(Some(DisplayAttributeInfo::new().into()));
            }
        }
        self.yielded.set(true);

        if actual == count {
            Ok(())
        } else {
            Err(Error::from(S_FALSE))
        }
    }

    fn Reset(&self) -> Result<()> {
        self.yielded.set(false);
        Ok(())
    }

    fn Skip(&self, count: u32) -> Result<()> {
        if count == 0 {
            return Ok(());
        }
        if self.yielded.get() {
            Err(Error::from(S_FALSE))
        } else {
            self.yielded.set(true);
            if count == 1 {
                Ok(())
            } else {
                Err(Error::from(S_FALSE))
            }
        }
    }
}

pub(crate) fn register_atom() -> Result<u32> {
    unsafe {
        let manager: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        manager.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE_INPUT)
    }
}

pub(crate) fn new_enumerator() -> IEnumTfDisplayAttributeInfo {
    DisplayAttributeEnumerator::new(false).into()
}

pub(crate) fn get_info(guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
    if guid.is_null() || unsafe { *guid } != GUID_DISPLAY_ATTRIBUTE_INPUT {
        return Err(E_INVALIDARG.into());
    }
    Ok(DisplayAttributeInfo::new().into())
}
