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
                TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_SHOWNINTRAY, TF_LBI_TEXT, TF_LBI_TOOLTIP,
            },
            WindowsAndMessaging::{
                GetSystemMetrics, LoadImageW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXSMICON,
                SM_CYSMICON,
            },
        },
    },
};

use crate::{
    globals::{DllActivityGuard, DLL_INSTANCE},
    guard,
    state::{set_ascii_mode_from_language_bar, WeakState},
    CLSID_TEXT_SERVICE, GUID_LANG_BAR_INPUT_MODE, MODE_ICON_CHINESE_RESOURCE_ID,
    MODE_ICON_ENGLISH_RESOURCE_ID,
};

const LANG_BAR_SINK_COOKIE: u32 = 0x4B54_4C42;
const CONNECT_E_NOCONNECTION: HRESULT = HRESULT(0x8004_0200_u32 as i32);
const CONNECT_E_ADVISELIMIT: HRESULT = HRESULT(0x8004_0201_u32 as i32);
const CONNECT_E_CANNOTCONNECT: HRESULT = HRESULT(0x8004_0202_u32 as i32);

#[derive(Default)]
struct LanguageBarModel {
    // The icon must already reflect the current mode when AddItem queries it.
    ascii_mode: Option<bool>,
    sink: Option<ITfLangBarItemSink>,
    notification_pending: bool,
    notifying: bool,
    notification_error: Option<HRESULT>,
    compartment_error: Option<HRESULT>,
}

fn log_language_bar_error(event: &str, error: &Error) {
    keytao_core::rt_log!(
        keytao_core::runtime_log::Level::Info,
        "error",
        "language_bar_operation_failed",
        operation = event,
        hresult = format!("0x{:08X}", error.code().0 as u32),
        msg = error.to_string()
    );
}

fn update_cached_mode(current: &mut Option<bool>, next: bool) -> bool {
    std::mem::replace(current, Some(next)) != Some(next)
}

fn load_mode_icon(instance: HINSTANCE, ascii_mode: bool) -> Result<HICON> {
    let resource_id = if ascii_mode {
        MODE_ICON_ENGLISH_RESOURCE_ID
    } else {
        MODE_ICON_CHINESE_RESOURCE_ID
    };
    // GetIcon transfers ownership to TSF, which calls DestroyIcon. LR_SHARED
    // would return a cached handle that the caller must not destroy.
    let handle = unsafe {
        LoadImageW(
            instance,
            PCWSTR(resource_id as usize as *const u16),
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        )?
    };
    Ok(HICON(handle.0))
}

fn notify_model(model: &Rc<RefCell<LanguageBarModel>>) {
    let sink = {
        let mut model = model.borrow_mut();
        model.notification_pending = true;
        if model.notifying {
            return;
        }
        let Some(sink) = model.sink.clone() else {
            return;
        };
        model.notification_pending = false;
        model.notifying = true;
        sink
    };
    // OnUpdate may immediately query the button or reenter a focus callback.
    // Keep the model unborrowed, and leave any nested update pending instead
    // of recursively calling the shell again.
    let result =
        unsafe { sink.OnUpdate(TF_LBI_STATUS | TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP) };
    let mut model = model.borrow_mut();
    model.notifying = false;
    if let Err(error) = result {
        model.notification_pending = true;
        let changed = model.notification_error.replace(error.code()) != Some(error.code());
        drop(model);
        if changed {
            log_language_bar_error("language_bar_notify", &error);
        }
    } else {
        model.notification_error = None;
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
        guard(|| {
            if info.is_null() {
                return Err(E_INVALIDARG.into());
            }
            let mut value = TF_LANGBARITEMINFO {
                clsidService: CLSID_TEXT_SERVICE,
                guidItem: GUID_LANG_BAR_INPUT_MODE,
                // Keep the resource's white glyph on a transparent background;
                // TEXTCOLORICON would allow the shell to recolor it. The system
                // GUID above exposes this button in the modern Input Indicator.
                dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
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
        })
    }

    fn GetStatus(&self) -> Result<u32> {
        guard(|| Ok(0))
    }

    fn Show(&self, _show: BOOL) -> Result<()> {
        // Without HIDDENSTATUSCONTROL, TSF owns visibility. Like SampleIME,
        // do not retain a temporary Show(false) as the item's own hidden state.
        // There is no model change to notify (and OnUpdate can call Show again).
        guard(|| Ok(()))
    }

    fn GetTooltipString(&self) -> Result<BSTR> {
        guard(|| {
            let label = if self.model.borrow().ascii_mode.unwrap_or(false) {
                "KeyTao English input"
            } else {
                "KeyTao Chinese input"
            };
            Ok(BSTR::from(label))
        })
    }
}

impl ITfLangBarItemButton_Impl for LanguageBarButton_Impl {
    fn OnClick(&self, click: TfLBIClick, _point: &POINT, _area: *const RECT) -> Result<()> {
        guard(|| {
            if click == TF_LBI_CLK_LEFT {
                let ascii_mode = !self.model.borrow().ascii_mode.unwrap_or(false);
                if let Some(state) = self.state.upgrade() {
                    set_ascii_mode_from_language_bar(&state, ascii_mode);
                }
            }
            Ok(())
        })
    }

    fn InitMenu(&self, _menu: Option<&ITfMenu>) -> Result<()> {
        guard(|| Ok(()))
    }

    fn OnMenuSelect(&self, _id: u32) -> Result<()> {
        guard(|| Ok(()))
    }

    fn GetIcon(&self) -> Result<HICON> {
        guard(|| {
            let ascii_mode = self.model.borrow().ascii_mode.unwrap_or(false);
            let instance = DLL_INSTANCE.get().copied().unwrap_or_default();
            load_mode_icon(HINSTANCE(instance as *mut _), ascii_mode)
        })
    }

    fn GetText(&self) -> Result<BSTR> {
        guard(|| {
            Ok(BSTR::from(
                if self.model.borrow().ascii_mode.unwrap_or(false) {
                    "\u{82f1}"
                } else {
                    "\u{4e2d}"
                },
            ))
        })
    }
}

impl ITfSource_Impl for LanguageBarButton_Impl {
    fn AdviseSink(
        &self,
        riid: *const windows::core::GUID,
        source: Option<&IUnknown>,
    ) -> Result<u32> {
        guard(|| {
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
            // The caller has not received its cookie yet. Replay on the next
            // update/focus refresh, not from inside the subscription call.
            model.notification_pending = true;
            Ok(LANG_BAR_SINK_COOKIE)
        })
    }

    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        guard(|| {
            let previous = {
                let mut model = self.model.borrow_mut();
                if cookie != LANG_BAR_SINK_COOKIE || model.sink.is_none() {
                    return Err(Error::from(CONNECT_E_NOCONNECTION));
                }
                model.notification_pending = true;
                model.sink.take()
            };
            // Release is also an external COM call.
            drop(previous);
            Ok(())
        })
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
    pub(crate) fn add(
        thread_mgr: &ITfThreadMgr,
        client_id: u32,
        state: WeakState,
        ascii_mode: bool,
    ) -> Result<Self> {
        let manager: ITfLangBarItemMgr = thread_mgr.cast()?;
        let model = Rc::new(RefCell::new(LanguageBarModel {
            // AddItem can query the icon before it returns.
            ascii_mode: Some(ascii_mode),
            sink: None,
            notification_pending: true,
            ..Default::default()
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
            update_cached_mode(&mut model.ascii_mode, ascii_mode)
        };
        if changed {
            self.update_input_mode_compartment(ascii_mode);
        }
        if changed || self.model.borrow().notification_pending {
            notify_model(&self.model);
        }
    }

    pub(crate) fn refresh_mode(&self, ascii_mode: bool) {
        update_cached_mode(&mut self.model.borrow_mut().ascii_mode, ascii_mode);
        // A focus change or a delayed sink attachment needs a fresh snapshot
        // even when Rime stayed in the same Chinese/English mode.
        self.update_input_mode_compartment(ascii_mode);
        notify_model(&self.model);
    }

    fn update_input_mode_compartment(&self, ascii_mode: bool) {
        let result = (|| -> Result<()> {
            let manager = self.thread_mgr.cast::<ITfCompartmentMgr>()?;
            let compartment =
                unsafe { manager.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)? };
            let flags = if ascii_mode {
                0
            } else {
                TF_CONVERSIONMODE_NATIVE as i32
            };
            let value = windows::core::VARIANT::from(flags);
            unsafe { compartment.SetValue(self.client_id, &value) }
        })();
        if let Err(error) = result {
            let changed = self
                .model
                .borrow_mut()
                .compartment_error
                .replace(error.code())
                != Some(error.code());
            if changed {
                log_language_bar_error("language_bar_conversion", &error);
            }
        } else {
            self.model.borrow_mut().compartment_error = None;
        }
    }

    pub(crate) fn remove(&self) {
        unsafe {
            let _ = self.manager.RemoveItem(&self.item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        load_mode_icon, notify_model, update_cached_mode, LanguageBarButton, LanguageBarModel,
    };
    use crate::{globals::DllActivityGuard, CLSID_TEXT_SERVICE};
    use std::{
        cell::{Cell, RefCell},
        rc::{Rc, Weak},
    };
    use windows::{
        core::{implement, IUnknown, Interface, Result},
        Win32::{
            Foundation::BOOL,
            System::LibraryLoader::GetModuleHandleW,
            UI::{
                TextServices::{
                    ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemSink,
                    ITfLangBarItemSink_Impl, ITfSource, GUID_LBI_INPUTMODE, TF_LANGBARITEMINFO,
                    TF_LBI_ICON, TF_LBI_STATUS, TF_LBI_STATUS_HIDDEN, TF_LBI_STYLE_BTN_BUTTON,
                    TF_LBI_TEXT, TF_LBI_TOOLTIP,
                },
                WindowsAndMessaging::DestroyIcon,
            },
        },
    };

    fn button() -> (Rc<RefCell<LanguageBarModel>>, ITfLangBarItemButton) {
        let model = Rc::new(RefCell::new(LanguageBarModel::default()));
        let button = LanguageBarButton {
            model: Rc::clone(&model),
            state: Weak::new(),
            _dll_guard: DllActivityGuard::new(),
        }
        .into();
        (model, button)
    }

    #[implement(ITfLangBarItemSink)]
    struct RecordingSink {
        model: Weak<RefCell<LanguageBarModel>>,
        observations: Rc<RefCell<Vec<(u32, bool)>>>,
        reenter_once: Cell<bool>,
    }

    impl ITfLangBarItemSink_Impl for RecordingSink_Impl {
        fn OnUpdate(&self, flags: u32) -> Result<()> {
            if let Some(model) = self.model.upgrade() {
                self.observations
                    .borrow_mut()
                    .push((flags, model.borrow().ascii_mode.unwrap_or(false)));
                if self.reenter_once.replace(false) {
                    notify_model(&model);
                }
            }
            Ok(())
        }
    }

    fn advise_recording_sink(
        source: &ITfSource,
        model: &Rc<RefCell<LanguageBarModel>>,
        observations: &Rc<RefCell<Vec<(u32, bool)>>>,
        reenter: bool,
    ) -> u32 {
        let sink: ITfLangBarItemSink = RecordingSink {
            model: Rc::downgrade(model),
            observations: Rc::clone(observations),
            reenter_once: Cell::new(reenter),
        }
        .into();
        let unknown: IUnknown = sink.cast().unwrap();
        unsafe {
            source
                .AdviseSink(&ITfLangBarItemSink::IID, &unknown)
                .unwrap()
        }
    }

    #[test]
    fn input_mode_publishes_initial_chinese_then_only_changes() {
        let mut ascii_mode = None;
        assert!(update_cached_mode(&mut ascii_mode, false));
        assert!(!update_cached_mode(&mut ascii_mode, false));
        assert!(update_cached_mode(&mut ascii_mode, true));
        assert_eq!(ascii_mode, Some(true));
        assert!(!update_cached_mode(&mut ascii_mode, true));
        assert!(update_cached_mode(&mut ascii_mode, false));
        assert_eq!(ascii_mode, Some(false));
    }

    #[test]
    fn input_indicator_finds_the_mode_button_by_the_system_guid() {
        let (model, button) = button();
        let item: ITfLangBarItem = button.cast().unwrap();
        unsafe {
            let mut info = TF_LANGBARITEMINFO::default();
            item.GetInfo(&mut info).unwrap();
            assert_eq!(info.guidItem, GUID_LBI_INPUTMODE);
            assert_eq!(info.clsidService, CLSID_TEXT_SERVICE);
            assert_ne!(info.dwStyle & TF_LBI_STYLE_BTN_BUTTON, 0);
            assert_eq!(item.GetStatus().unwrap() & TF_LBI_STATUS_HIDDEN, 0);
            assert_eq!(button.GetText().unwrap().to_string(), "中");
            model.borrow_mut().ascii_mode = Some(true);
            assert_eq!(button.GetText().unwrap().to_string(), "英");
        }
    }

    #[test]
    fn system_show_requests_do_not_leave_the_input_indicator_hidden() {
        let (model, button) = button();
        let item: ITfLangBarItem = button.cast().unwrap();
        let source: ITfSource = button.cast().unwrap();
        let observations = Rc::new(RefCell::new(Vec::new()));
        let cookie = advise_recording_sink(&source, &model, &observations, false);
        unsafe {
            item.Show(BOOL::from(false)).unwrap();
            item.Show(BOOL::from(false)).unwrap();
            assert_eq!(item.GetStatus().unwrap() & TF_LBI_STATUS_HIDDEN, 0);
            item.Show(BOOL::from(true)).unwrap();
            assert!(
                observations.borrow().is_empty(),
                "Show does not change the model or recurse into OnUpdate"
            );
            source.UnadviseSink(cookie).unwrap();
        }
    }

    #[test]
    fn sink_reconnection_replays_current_mode_without_notifying_inside_advise() {
        let (model, button) = button();
        let source: ITfSource = button.cast().unwrap();
        model.borrow_mut().ascii_mode = Some(true);
        notify_model(&model);
        assert!(model.borrow().notification_pending);
        let first = Rc::new(RefCell::new(Vec::new()));
        let cookie = advise_recording_sink(&source, &model, &first, false);
        assert!(first.borrow().is_empty());
        notify_model(&model);
        let all_flags = TF_LBI_STATUS | TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP;
        assert_eq!(&*first.borrow(), &[(all_flags, true)]);
        assert!(!model.borrow().notification_pending);
        unsafe { source.UnadviseSink(cookie).unwrap() };

        model.borrow_mut().ascii_mode = Some(false);
        notify_model(&model);
        let second = Rc::new(RefCell::new(Vec::new()));
        let cookie = advise_recording_sink(&source, &model, &second, false);
        assert!(second.borrow().is_empty());
        assert!(model.borrow().notification_pending);
        notify_model(&model);
        assert_eq!(&*second.borrow(), &[(all_flags, false)]);
        assert_eq!(first.borrow().len(), 1);
        unsafe { source.UnadviseSink(cookie).unwrap() };
    }

    #[test]
    fn synchronous_notification_reentry_is_deferred_without_recursing() {
        let (model, button) = button();
        let source: ITfSource = button.cast().unwrap();
        let observations = Rc::new(RefCell::new(Vec::new()));
        let cookie = advise_recording_sink(&source, &model, &observations, true);
        notify_model(&model);
        assert_eq!(observations.borrow().len(), 1);
        assert!(model.borrow().notification_pending);
        assert!(!model.borrow().notifying);
        notify_model(&model);
        assert_eq!(observations.borrow().len(), 2);
        assert!(!model.borrow().notification_pending);
        unsafe { source.UnadviseSink(cookie).unwrap() };
    }

    #[test]
    fn mode_icons_are_individually_owned_by_the_caller() {
        let instance = unsafe { GetModuleHandleW(None).unwrap() }.into();
        for ascii_mode in [false, true] {
            let first = load_mode_icon(instance, ascii_mode).unwrap();
            let second = load_mode_icon(instance, ascii_mode).unwrap();
            assert_ne!(first, second, "GetIcon must not return a shared icon");
            unsafe {
                DestroyIcon(first).unwrap();
                DestroyIcon(second).unwrap();
            }
        }
    }
}
