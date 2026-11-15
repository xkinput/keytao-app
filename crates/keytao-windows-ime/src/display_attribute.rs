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
            TF_ATTR_TARGET_CONVERTED, TF_CT_COLORREF, TF_CT_NONE, TF_DA_COLOR, TF_DA_COLOR_0,
            TF_DISPLAYATTRIBUTE, TF_LS_DOT, TF_LS_SOLID,
        },
    },
};

use crate::{
    globals::DllActivityGuard, guard, GUID_DISPLAY_ATTRIBUTE_CONVERTED,
    GUID_DISPLAY_ATTRIBUTE_INPUT,
};

fn default_attribute(converted: bool) -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        crText: TF_DA_COLOR {
            r#type: TF_CT_NONE,
            Anonymous: TF_DA_COLOR_0 { cr: COLORREF(0) },
        },
        crBk: TF_DA_COLOR {
            r#type: TF_CT_NONE,
            Anonymous: TF_DA_COLOR_0 { cr: COLORREF(0) },
        },
        lsStyle: if converted { TF_LS_SOLID } else { TF_LS_DOT },
        fBoldLine: BOOL::from(false),
        crLine: TF_DA_COLOR {
            r#type: TF_CT_COLORREF,
            Anonymous: TF_DA_COLOR_0 {
                cr: COLORREF(0x00ce_6700),
            },
        },
        bAttr: if converted {
            TF_ATTR_TARGET_CONVERTED
        } else {
            TF_ATTR_INPUT
        },
    }
}

#[implement(ITfDisplayAttributeInfo)]
struct DisplayAttributeInfo {
    converted: bool,
    attribute: RefCell<TF_DISPLAYATTRIBUTE>,
    _dll_guard: DllActivityGuard,
}

impl DisplayAttributeInfo {
    fn new(converted: bool) -> Self {
        Self {
            converted,
            attribute: RefCell::new(default_attribute(converted)),
            _dll_guard: DllActivityGuard::new(),
        }
    }
}

impl ITfDisplayAttributeInfo_Impl for DisplayAttributeInfo_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        guard(|| {
            Ok(if self.converted {
                GUID_DISPLAY_ATTRIBUTE_CONVERTED
            } else {
                GUID_DISPLAY_ATTRIBUTE_INPUT
            })
        })
    }

    fn GetDescription(&self) -> Result<BSTR> {
        guard(|| {
            Ok(BSTR::from(if self.converted {
                "KeyTao candidate preview"
            } else {
                "KeyTao composition input"
            }))
        })
    }

    fn GetAttributeInfo(&self, attribute: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        guard(|| {
            if attribute.is_null() {
                return Err(E_INVALIDARG.into());
            }
            unsafe {
                *attribute = *self.attribute.borrow();
            }
            Ok(())
        })
    }

    fn SetAttributeInfo(&self, attribute: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        guard(|| {
            if attribute.is_null() {
                return Err(E_INVALIDARG.into());
            }
            *self.attribute.borrow_mut() = unsafe { *attribute };
            Ok(())
        })
    }

    fn Reset(&self) -> Result<()> {
        guard(|| {
            *self.attribute.borrow_mut() = default_attribute(self.converted);
            Ok(())
        })
    }
}

#[implement(IEnumTfDisplayAttributeInfo)]
struct DisplayAttributeEnumerator {
    position: Cell<u32>,
    _dll_guard: DllActivityGuard,
}

impl DisplayAttributeEnumerator {
    fn new(position: u32) -> Self {
        Self {
            position: Cell::new(position),
            _dll_guard: DllActivityGuard::new(),
        }
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for DisplayAttributeEnumerator_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        guard(|| Ok(DisplayAttributeEnumerator::new(self.position.get()).into()))
    }

    fn Next(
        &self,
        count: u32,
        info: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        guard(|| {
            if count == 0 || info.is_null() || (count != 1 && fetched.is_null()) {
                return Err(E_INVALIDARG.into());
            }

            let position = self.position.get();
            let actual = count.min(2 - position);
            unsafe {
                if !fetched.is_null() {
                    *fetched = actual;
                }
                for index in 0..actual {
                    info.add(index as usize).write(Some(
                        DisplayAttributeInfo::new(position + index == 1).into(),
                    ));
                }
            }
            self.position.set(position + actual);

            if actual == count {
                Ok(())
            } else {
                Err(Error::from(S_FALSE))
            }
        })
    }

    fn Reset(&self) -> Result<()> {
        guard(|| {
            self.position.set(0);
            Ok(())
        })
    }

    fn Skip(&self, count: u32) -> Result<()> {
        guard(|| {
            if count == 0 {
                return Ok(());
            }
            let actual = count.min(2 - self.position.get());
            self.position.set(self.position.get() + actual);
            if actual == count {
                Ok(())
            } else {
                Err(Error::from(S_FALSE))
            }
        })
    }
}

pub(crate) fn register_atoms() -> Result<(u32, u32)> {
    unsafe {
        let manager: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        Ok((
            manager.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE_INPUT)?,
            manager.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE_CONVERTED)?,
        ))
    }
}

pub(crate) fn new_enumerator() -> IEnumTfDisplayAttributeInfo {
    DisplayAttributeEnumerator::new(0).into()
}

pub(crate) fn get_info(guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
    if guid.is_null() {
        return Err(E_INVALIDARG.into());
    }
    let guid = unsafe { *guid };
    if guid != GUID_DISPLAY_ATTRIBUTE_INPUT && guid != GUID_DISPLAY_ATTRIBUTE_CONVERTED {
        return Err(E_INVALIDARG.into());
    }
    Ok(DisplayAttributeInfo::new(guid == GUID_DISPLAY_ATTRIBUTE_CONVERTED).into())
}
