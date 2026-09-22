//! Menu contract checks without displaying UI or launching the external App.

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use windows::{
    core::Interface,
    Win32::{
        Foundation::{E_FAIL, E_INVALIDARG},
        UI::{
            TextServices::{
                ITfLangBarItem, ITfLangBarItemButton, TF_LANGBARITEMINFO, TF_LBI_CLK_LEFT,
                TF_LBI_CLK_RIGHT, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_BTN_MENU,
            },
            WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID, GetMenuStringW, MF_BYPOSITION},
        },
    },
};

use super::{
    create_context_menu, dispatch_menu_action, handle_button_click, AppAction, LanguageBarButton,
    LanguageBarModel, MenuOpenGuard, MENU_OPEN_APP, MENU_REDEPLOY,
};
use crate::globals::DllActivityGuard;

#[test]
fn native_menu_has_exactly_the_two_requested_labels_and_commands() {
    let menu = create_context_menu().unwrap();
    // Creating an HMENU does not display it or change desktop focus.
    unsafe {
        assert_eq!(GetMenuItemCount(menu.0), 2);
        for (position, id, expected) in [
            (0, MENU_REDEPLOY, "重新部署"),
            (1, MENU_OPEN_APP, "打开 App"),
        ] {
            assert_eq!(GetMenuItemID(menu.0, position), id);
            let mut label = [0u16; 32];
            let count = GetMenuStringW(menu.0, position as u32, Some(&mut label), MF_BYPOSITION);
            assert_eq!(
                String::from_utf16(&label[..count as usize]).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn mode_button_has_a_single_right_click_menu_route() {
    let button: ITfLangBarItemButton = LanguageBarButton {
        model: Rc::new(RefCell::new(LanguageBarModel::default())),
        state: Weak::new(),
        _dll_guard: DllActivityGuard::new(),
    }
    .into();
    let item: ITfLangBarItem = button.cast().unwrap();
    unsafe {
        let mut info = TF_LANGBARITEMINFO::default();
        item.GetInfo(&mut info).unwrap();
        assert_ne!(info.dwStyle & TF_LBI_STYLE_BTN_BUTTON, 0);
        assert_eq!(info.dwStyle & TF_LBI_STYLE_BTN_MENU, 0);
        button.InitMenu(None).unwrap();
    }
}

#[test]
fn left_click_only_toggles_the_input_mode() {
    let toggles = Cell::new(0);
    handle_button_click(
        TF_LBI_CLK_LEFT,
        || {
            toggles.set(toggles.get() + 1);
            Ok(())
        },
        || panic!("left-click must not open a menu"),
        |_| panic!("left-click must not launch the App"),
    )
    .unwrap();
    assert_eq!(toggles.get(), 1);
}

#[test]
fn right_click_dispatches_each_command_once_without_changing_mode() {
    for id in [MENU_REDEPLOY, MENU_OPEN_APP] {
        let menus = Cell::new(0);
        let launches = Cell::new(0);
        handle_button_click(
            TF_LBI_CLK_RIGHT,
            || panic!("right-click must not toggle the input mode"),
            || {
                menus.set(menus.get() + 1);
                Ok(id)
            },
            |action| {
                launches.set(launches.get() + 1);
                assert!(matches!(
                    (id, action),
                    (MENU_REDEPLOY, AppAction::Redeploy) | (MENU_OPEN_APP, AppAction::OpenApp)
                ));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(menus.get(), 1);
        assert_eq!(launches.get(), 1);
    }
}

#[test]
fn cancellation_unknown_commands_and_menu_errors_never_launch_actions() {
    dispatch_menu_action(0, |_| panic!("cancelled menu must not launch")).unwrap();
    assert_eq!(
        dispatch_menu_action(999, |_| panic!("unknown command must not launch"))
            .unwrap_err()
            .code(),
        E_INVALIDARG
    );
    assert_eq!(
        handle_button_click(
            TF_LBI_CLK_RIGHT,
            || panic!("right-click must not toggle"),
            || Err(E_FAIL.into()),
            |_| panic!("failed menu must not launch"),
        )
        .unwrap_err()
        .code(),
        E_FAIL
    );
    assert_eq!(
        dispatch_menu_action(MENU_OPEN_APP, |_| Err(E_FAIL.into()))
            .unwrap_err()
            .code(),
        E_FAIL
    );
}

#[test]
fn menu_reentry_is_ignored_and_closing_allows_the_next_click() {
    let model = Rc::new(RefCell::new(LanguageBarModel::default()));
    let first = MenuOpenGuard::begin(&model).unwrap();
    assert!(MenuOpenGuard::begin(&model).is_none());
    // The guard leaves no borrow alive across the native menu's message loop.
    model.borrow_mut().english_mode = Some(true);
    drop(first);
    assert!(!model.borrow().menu_open);
    let second = MenuOpenGuard::begin(&model).unwrap();
    drop(second);
    assert!(!model.borrow().menu_open);
}
