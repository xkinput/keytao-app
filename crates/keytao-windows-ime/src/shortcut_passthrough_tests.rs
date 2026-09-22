//! Exercise the real COM key callbacks without a TSF manager, native editor,
//! Rime session, message pump, or installed TIP. SetKeyboardState changes only
//! this dedicated test thread's table and is restored even when an assertion fails.

use super::KeyEventSink;
use crate::{
    globals::DllActivityGuard,
    input_context::ContextInputState,
    key_map::{current_mod_mask, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_SHIFT, RIME_MOD_SUPER},
    state::{new_shared_state, SharedState},
};
use keytao_core::{Candidate, ImeState};
use std::{cell::RefCell, rc::Rc};
use windows::{
    core::{implement, w, Result, GUID, HRESULT},
    Win32::{
        Foundation::{BOOL, E_NOTIMPL, HMODULE, LPARAM, WPARAM},
        System::{Com::IEnumGUID, LibraryLoader::GetModuleHandleW},
        UI::{Input::KeyboardAndMouse::*, TextServices::*},
    },
};

type Calls = Rc<RefCell<Vec<&'static str>>>;

#[implement(ITfContext, ITfCompartmentMgr)]
struct SpyContext {
    calls: Calls,
}

macro_rules! spy_methods {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        $(#[allow(unused_variables)]
        fn $name(&self $(, $arg: $ty)*) -> Result<$ret> {
            self.calls.borrow_mut().push(stringify!($name));
            Err(E_NOTIMPL.into())
        })*
    };
}

impl ITfContext_Impl for SpyContext_Impl {
    spy_methods! {
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
        fn GetAppProperty(&self, guid: *const GUID) -> ITfReadOnlyProperty;
        fn TrackProperties(&self, props: *const *const GUID, count: u32, app_props: *const *const GUID, app_count: u32) -> ITfReadOnlyProperty;
        fn EnumProperties(&self) -> IEnumTfProperties;
        fn GetDocumentMgr(&self) -> ITfDocumentMgr;
        fn CreateRangeBackup(&self, ec: u32, range: Option<&ITfRange>) -> ITfRangeBackup;
    }
}

impl ITfCompartmentMgr_Impl for SpyContext_Impl {
    spy_methods! {
        fn GetCompartment(&self, guid: *const GUID) -> ITfCompartment;
        fn ClearCompartment(&self, tid: u32, guid: *const GUID) -> ();
        fn EnumCompartments(&self) -> IEnumGUID;
    }
}

struct KeyboardStateGuard([u8; 256]);

impl KeyboardStateGuard {
    fn save() -> Self {
        let mut original = [0; 256];
        unsafe { GetKeyboardState(&mut original).unwrap() };
        Self(original)
    }

    fn set(&self, key: VIRTUAL_KEY, mods: u32) {
        self.set_table(Some(key), mods);
    }

    fn set_table(&self, key: Option<VIRTUAL_KEY>, mods: u32) {
        let mut keyboard = [0; 256];
        if let Some(key) = key {
            keyboard[key.0 as usize] = 0x80;
        }
        for (modifier, generic, side) in [
            (RIME_MOD_CONTROL, VK_CONTROL, VK_LCONTROL),
            (RIME_MOD_SHIFT, VK_SHIFT, VK_LSHIFT),
            (RIME_MOD_ALT, VK_MENU, VK_LMENU),
            (RIME_MOD_SUPER, VK_LWIN, VK_LWIN),
        ] {
            if mods & modifier != 0 {
                keyboard[generic.0 as usize] = 0x80;
                keyboard[side.0 as usize] = 0x80;
            }
        }
        unsafe { SetKeyboardState(&keyboard).unwrap() };
        assert_eq!(current_mod_mask(), mods, "test thread modifier setup");
    }
}

impl Drop for KeyboardStateGuard {
    fn drop(&mut self) {
        // The guard lives inside the dedicated thread, including while it
        // unwinds. No SendInput or shared/global keyboard state is involved.
        let restored = unsafe { SetKeyboardState(&self.0) };
        if !std::thread::panicking() {
            restored.unwrap();
        }
    }
}

fn state_and_sink(ime: ImeState) -> (SharedState, ITfKeyEventSink) {
    let state = new_shared_state();
    {
        let mut st = state.borrow_mut();
        st.client_id = 1;
        st.input_context = ContextInputState::unrestricted();
        st.ascii_mode = ime.ascii_mode;
        st.english_mode = ime.is_english_mode();
        st.ime_state = Some(ime);
        // A passed-through chord must still disarm solo-Shift tracking.
        st.shift_pressed_without_key = true;
    }
    let sink = KeyEventSink {
        state: Rc::downgrade(&state),
        _dll_guard: DllActivityGuard::new(),
    }
    .into();
    (state, sink)
}

fn composition(english: bool) -> ImeState {
    ImeState {
        preedit: "fixture".into(),
        cursor: 3,
        sel_start: 1,
        sel_end: 3,
        candidates: vec![Candidate {
            text: "fixture candidate".into(),
            comment: Some("fixture comment".into()),
        }],
        page: 2,
        schema_id: if english { "easy_en" } else { "keytao" }.into(),
        schema_name: if english { "Easy English" } else { "KeyTao" }.into(),
        ..ImeState::empty()
    }
}

fn mapped_input_modules() -> [Option<HMODULE>; 3] {
    unsafe {
        [
            GetModuleHandleW(w!("rime.dll")).ok(),
            GetModuleHandleW(w!("rime-arm64.dll")).ok(),
            GetModuleHandleW(w!("keytao_windows_ime.dll")).ok(),
        ]
    }
}

#[test]
fn application_shortcuts_pass_through_com_before_context_or_engine_work() {
    std::thread::spawn(|| {
        let keyboard = KeyboardStateGuard::save();
        let initial_modules = mapped_input_modules();
        let calls = Calls::default();
        let context: ITfContext = SpyContext {
            calls: calls.clone(),
        }
        .into();
        let mut shortcuts = Vec::new();
        for key in [VK_C, VK_V, VK_X, VK_A, VK_Z, VK_Y, VK_S, VK_F] {
            shortcuts.push((key, RIME_MOD_CONTROL));
            shortcuts.push((key, RIME_MOD_CONTROL | RIME_MOD_SHIFT));
        }
        shortcuts.extend([
            (VK_F4, RIME_MOD_ALT),
            (VK_F4, RIME_MOD_CONTROL),
            (VK_SPACE, RIME_MOD_CONTROL),
            (VK_TAB, RIME_MOD_CONTROL),
            (VK_C, RIME_MOD_SUPER),
            (VK_V, RIME_MOD_SUPER | RIME_MOD_SHIFT),
            (VK_E, RIME_MOD_CONTROL | RIME_MOD_ALT),
            (VK_INSERT, RIME_MOD_SHIFT),
            (VK_DELETE, RIME_MOD_SHIFT),
            (VK_F4, RIME_MOD_SHIFT),
        ]);

        for (label, ime) in [
            ("idle", ImeState::empty()),
            ("Chinese composition", composition(false)),
            ("English dictionary composition", composition(true)),
        ] {
            for (key, mods) in &shortcuts {
                for test_callback in [true, false] {
                    keyboard.set(*key, *mods);
                    let (state, sink) = state_and_sink(ime.clone());
                    let before = format!("{:?}", state.borrow().ime_state);
                    let callback = if test_callback {
                        "OnTestKeyDown"
                    } else {
                        "OnKeyDown"
                    };
                    let eaten = unsafe {
                        if test_callback {
                            sink.OnTestKeyDown(&context, WPARAM(key.0 as usize), LPARAM(0))
                        } else {
                            // Hosts may call this without a preceding test.
                            sink.OnKeyDown(&context, WPARAM(key.0 as usize), LPARAM(0))
                        }
                    }
                    .unwrap()
                    .as_bool();
                    let case = format!("{callback}: {label}, vk={}, modifiers={mods}", key.0);
                    assert!(!eaten, "{case}");
                    assert!(
                        calls.borrow().is_empty(),
                        "{case}: probed host context: {:?}",
                        calls.borrow()
                    );
                    let st = state.borrow();
                    assert_eq!(
                        format!("{:?}", st.ime_state),
                        before,
                        "{case}: changed composition"
                    );
                    assert_eq!(st.ascii_mode, ime.ascii_mode, "{case}");
                    assert_eq!(st.english_mode, ime.is_english_mode(), "{case}");
                    assert_eq!(
                        st.input_context,
                        ContextInputState::unrestricted(),
                        "{case}"
                    );
                    assert!(!st.shift_pressed_without_key, "{case}: stale solo Shift");
                    assert!(
                        !st.engine_building && st.engine_error.is_none(),
                        "{case}: started warmup"
                    );
                    assert!(
                        st.runtime.is_none() && st.session.is_none(),
                        "{case}: initialized Rime"
                    );
                    assert!(
                        !st.reload_in_progress && !st.reload_clear_pending,
                        "{case}: started reload"
                    );
                    assert!(st.key_context.is_none(), "{case}: retained key context");
                    assert!(
                        !st.windows_dirty && !st.windows_hide_pending,
                        "{case}: changed UI"
                    );
                }
            }
        }

        // Positive control: without shortcut modifiers the same real COM entry
        // reaches the spy. Thus the matrix above cannot pass solely because the
        // runtime is absent. The failing spy blocks before any engine warmup.
        keyboard.set(VK_A, 0);
        let (state, sink) = state_and_sink(ImeState::empty());
        assert!(
            !unsafe { sink.OnTestKeyDown(&context, WPARAM(VK_A.0 as usize), LPARAM(0)) }
                .unwrap()
                .as_bool()
        );
        assert!(
            !calls.borrow().is_empty(),
            "ordinary key never reached context spy"
        );
        assert!(
            !state.borrow().engine_building,
            "spy must block before warmup"
        );
        assert_eq!(
            mapped_input_modules(),
            initial_modules,
            "COM-only test loaded Rime or installed TIP"
        );
    })
    .join()
    .unwrap();
}

#[test]
fn control_shift_release_order_never_toggles_mode_or_probes_context() {
    std::thread::spawn(|| {
        let keyboard = KeyboardStateGuard::save();
        let initial_modules = mapped_input_modules();
        let calls = Calls::default();
        let context: ITfContext = SpyContext {
            calls: calls.clone(),
        }
        .into();
        let control = RIME_MOD_CONTROL;
        let shift = RIME_MOD_SHIFT;
        // bool = key down. The modifier mask is the state at that callback,
        // so a released modifier is already absent from the thread table.
        let sequences = [
            [
                (true, VK_CONTROL, control),
                (true, VK_SHIFT, control | shift),
                (false, VK_SHIFT, control),
                (false, VK_CONTROL, 0),
            ],
            [
                (true, VK_CONTROL, control),
                (true, VK_SHIFT, control | shift),
                (false, VK_CONTROL, shift),
                (false, VK_SHIFT, 0),
            ],
            [
                (true, VK_SHIFT, shift),
                (true, VK_CONTROL, control | shift),
                (false, VK_CONTROL, shift),
                (false, VK_SHIFT, 0),
            ],
            [
                (true, VK_SHIFT, shift),
                (true, VK_CONTROL, control | shift),
                (false, VK_SHIFT, control),
                (false, VK_CONTROL, 0),
            ],
        ];
        for ime in [ImeState::empty(), composition(false), composition(true)] {
            for (index, sequence) in sequences.iter().enumerate() {
                for test_callback in [true, false] {
                    let (state, sink) = state_and_sink(ime.clone());
                    state.borrow_mut().shift_pressed_without_key = false;
                    let before = format!("{:?}", state.borrow().ime_state);
                    for (down, key, mods) in sequence {
                        keyboard.set_table(None, *mods);
                        let eaten = unsafe {
                            match (*down, test_callback) {
                                (true, true) => {
                                    sink.OnTestKeyDown(&context, WPARAM(key.0 as usize), LPARAM(0))
                                }
                                (true, false) => {
                                    sink.OnKeyDown(&context, WPARAM(key.0 as usize), LPARAM(0))
                                }
                                (false, true) => {
                                    sink.OnTestKeyUp(&context, WPARAM(key.0 as usize), LPARAM(0))
                                }
                                (false, false) => {
                                    sink.OnKeyUp(&context, WPARAM(key.0 as usize), LPARAM(0))
                                }
                            }
                        }
                        .unwrap()
                        .as_bool();
                        assert!(
                            !eaten,
                            "sequence {index}: down={down}, test={test_callback}, vk={}",
                            key.0
                        );
                        assert!(
                            calls.borrow().is_empty(),
                            "Ctrl+Shift probed context: {:?}",
                            calls.borrow()
                        );
                        let st = state.borrow();
                        assert_eq!(format!("{:?}", st.ime_state), before);
                        assert_eq!(st.english_mode, ime.is_english_mode());
                        assert_eq!(st.ascii_mode, ime.ascii_mode);
                        assert!(!st.engine_building && st.session.is_none());
                    }
                    assert!(
                        !state.borrow().shift_pressed_without_key,
                        "sequence {index}: left solo Shift armed"
                    );
                }
            }
        }
        assert_eq!(
            mapped_input_modules(),
            initial_modules,
            "modifier test loaded Rime or installed TIP"
        );
    })
    .join()
    .unwrap();
}
