//! TSF language-bar item for the persistent Chinese/English input mode.

use std::{cell::RefCell, rc::Rc};

use windows::{
    core::{implement, Error, IUnknown, Interface, Result, BSTR, HRESULT, PCWSTR},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, HINSTANCE, POINT, RECT},
        UI::{
            TextServices::{
                ITfCompartmentMgr, ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemButton_Impl,
                ITfLangBarItemMgr, ITfLangBarItemSink, ITfLangBarItem_Impl, ITfMenu, ITfSource,
                ITfSource_Impl, ITfThreadMgr, TfLBIClick,
                GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION, TF_CONVERSIONMODE_NATIVE,
                TF_LANGBARITEMINFO, TF_LBI_CLK_LEFT, TF_LBI_ICON, TF_LBI_STATUS,
                TF_LBI_STATUS_HIDDEN, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_SHOWNINTRAY,
                TF_LBI_STYLE_TEXTCOLORICON, TF_LBI_TEXT, TF_LBI_TOOLTIP,
            },
            WindowsAndMessaging::{LoadImageW, HICON, IMAGE_ICON, LR_SHARED},
        },
    },
};

use crate::{
    globals::{DllActivityGuard, DLL_INSTANCE},
    state::{set_ascii_mode_from_language_bar, WeakState},
    CLSID_TEXT_SERVICE, GUID_LANG_BAR_INPUT_MODE, MODE_ICON_CHINESE_RESOURCE_ID,
    MODE_ICON_ENGLISH_RESOURCE_ID,
};

const LANG_BAR_SINK_COOKIE: u32 = 0x4B54_4C42;
const CONNECT_E_NOCONNECTION: HRESULT = HRESULT(0x8004_0200_u32 as i32);
const CONNECT_E_ADVISELIMIT: HRESULT = HRESULT(0x8004_0201_u32 as i32);
const CONNECT_E_CANNOTCONNECT: HRESULT = HRESULT(0x8004_0202_u32 as i32);

struct LanguageBarModel {
    ascii_mode: bool,
    status: u32,
    sink: Option<ITfLangBarItemSink>,
}

fn notify_model(model: &Rc<RefCell<LanguageBarModel>>) {
    let sink = model.borrow().sink.clone();
    if let Some(sink) = sink {
        unsafe {
            let _ = sink.OnUpdate(TF_LBI_STATUS | TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP);
        }
    }
}

#[implement(ITfLangBarItemButton, ITfSource)]
struct LanguageBarButton {
    model: Rc<RefCell<LanguageBarModel>>,
    state: WeakState,
    _dll_guard: DllActivityGuard,
}

impl ITfLangBarItem_Impl for LanguageBarButton_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if info.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut value = TF_LANGBARITEMINFO {
            clsidService: CLSID_TEXT_SERVICE,
            guidItem: GUID_LANG_BAR_INPUT_MODE,
            dwStyle: TF_LBI_STYLE_BTN_BUTTON
                | TF_LBI_STYLE_SHOWNINTRAY
                | TF_LBI_STYLE_TEXTCOLORICON,
            ulSort: 1,
            ..Default::default()
        };
        let description: Vec<u16> = "KeyTao input mode".encode_utf16().collect();
        let count = description.len().min(value.szDescription.len() - 1);
        value.szDescription[..count].copy_from_slice(&description[..count]);
        unsafe {
            *info = value;
        }
        Ok(())
    }

    fn GetStatus(&self) -> Result<u32> {
        Ok(self.model.borrow().status)
    }

    fn Show(&self, show: BOOL) -> Result<()> {
        {
            let mut model = self.model.borrow_mut();
            if show.as_bool() {
                model.status &= !TF_LBI_STATUS_HIDDEN;
            } else {
                model.status |= TF_LBI_STATUS_HIDDEN;
            }
        }
        notify_model(&self.model);
        Ok(())
    }

    fn GetTooltipString(&self) -> Result<BSTR> {
        let label = if self.model.borrow().ascii_mode {
            "KeyTao English input"
        } else {
            "KeyTao Chinese input"
        };
        Ok(BSTR::from(label))
    }
}

impl ITfLangBarItemButton_Impl for LanguageBarButton_Impl {
    fn OnClick(&self, click: TfLBIClick, _point: &POINT, _area: *const RECT) -> Result<()> {
        if click == TF_LBI_CLK_LEFT {
            let ascii_mode = !self.model.borrow().ascii_mode;
            if let Some(state) = self.state.upgrade() {
                set_ascii_mode_from_language_bar(&state, ascii_mode);
            }
        }
        Ok(())
    }

    fn InitMenu(&self, _menu: Option<&ITfMenu>) -> Result<()> {
        Ok(())
    }

    fn OnMenuSelect(&self, _id: u32) -> Result<()> {
        Ok(())
    }

    fn GetIcon(&self) -> Result<HICON> {
        let resource_id = if self.model.borrow().ascii_mode {
            MODE_ICON_ENGLISH_RESOURCE_ID
        } else {
            MODE_ICON_CHINESE_RESOURCE_ID
        };
        let instance = DLL_INSTANCE.get().copied().unwrap_or_default();
        let handle = unsafe {
            LoadImageW(
                HINSTANCE(instance as *mut _),
                PCWSTR(resource_id as usize as *const u16),
                IMAGE_ICON,
                0,
                0,
                LR_SHARED,
            )?
        };
        Ok(HICON(handle.0))
    }

    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(if self.model.borrow().ascii_mode {
            "\u{82f1}"
        } else {
            "\u{4e2d}"
        }))
    }
}

impl ITfSource_Impl for LanguageBarButton_Impl {
    fn AdviseSink(
        &self,
        riid: *const windows::core::GUID,
        source: Option<&IUnknown>,
    ) -> Result<u32> {
        if riid.is_null() || unsafe { *riid } != ITfLangBarItemSink::IID {
            return Err(Error::from(CONNECT_E_CANNOTCONNECT));
        }
        let source = source.ok_or_else(|| Error::from(E_INVALIDARG))?;
        let sink: ITfLangBarItemSink = source.cast()?;
        let mut model = self.model.borrow_mut();
        if model.sink.is_some() {
            return Err(Error::from(CONNECT_E_ADVISELIMIT));
        }
        model.sink = Some(sink);
        Ok(LANG_BAR_SINK_COOKIE)
    }

    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        let mut model = self.model.borrow_mut();
        if cookie != LANG_BAR_SINK_COOKIE || model.sink.is_none() {
            return Err(Error::from(CONNECT_E_NOCONNECTION));
        }
        model.sink = None;
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct LanguageBarItem {
    manager: ITfLangBarItemMgr,
    item: ITfLangBarItem,
    model: Rc<RefCell<LanguageBarModel>>,
    thread_mgr: ITfThreadMgr,
    client_id: u32,
}

impl LanguageBarItem {
    pub(crate) fn add(thread_mgr: &ITfThreadMgr, client_id: u32, state: WeakState) -> Result<Self> {
        let manager: ITfLangBarItemMgr = thread_mgr.cast()?;
        let model = Rc::new(RefCell::new(LanguageBarModel {
            ascii_mode: false,
            status: 0,
            sink: None,
        }));
        let button: ITfLangBarItemButton = LanguageBarButton {
            model: Rc::clone(&model),
            state,
            _dll_guard: DllActivityGuard::new(),
        }
        .into();
        let item: ITfLangBarItem = button.cast()?;
        unsafe {
            manager.AddItem(&item)?;
            item.Show(BOOL::from(true))?;
        }
        Ok(Self {
            manager,
            item,
            model,
            thread_mgr: thread_mgr.clone(),
            client_id,
        })
    }

    pub(crate) fn update_mode(&self, ascii_mode: bool) {
        let changed = {
            let mut model = self.model.borrow_mut();
            if model.ascii_mode == ascii_mode {
                false
            } else {
                model.ascii_mode = ascii_mode;
                true
            }
        };
        self.update_input_mode_compartment(ascii_mode);
        if changed {
            notify_model(&self.model);
        }
    }

    fn update_input_mode_compartment(&self, ascii_mode: bool) {
        let Ok(manager) = self.thread_mgr.cast::<ITfCompartmentMgr>() else {
            return;
        };
        let Ok(compartment) =
            (unsafe { manager.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) })
        else {
            return;
        };
        let flags = if ascii_mode {
            0
        } else {
            TF_CONVERSIONMODE_NATIVE as i32
        };
        let value = windows::core::VARIANT::from(flags);
        unsafe {
            let _ = compartment.SetValue(self.client_id, &value);
        }
    }

    pub(crate) fn remove(&self) {
        unsafe {
            let _ = self.manager.RemoveItem(&self.item);
        }
    }
}
