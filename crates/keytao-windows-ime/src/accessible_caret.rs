//! Read a custom caret exposed through MSAA, without accessing document text.
//!
//! Call only after releasing TSF edit locks and all `TsfState` borrows:
//! `AccessibleObjectFromWindow` and `accLocation` can call back into the host.
//! https://devblogs.microsoft.com/oldnewthing/20260108-00/?p=111973

use std::{cell::Cell, mem::MaybeUninit};

use windows::{
    core::{Interface, HRESULT, VARIANT},
    Win32::{
        Foundation::{HWND, RECT, S_OK},
        System::Threading::{GetCurrentProcessId, GetCurrentThreadId},
        UI::{
            Accessibility::{AccessibleObjectFromWindow, IAccessible},
            HiDpi::{
                SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            WindowsAndMessaging::{
                GetAncestor, GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics,
                GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, CHILDID_SELF,
                GA_ROOT, GUITHREADINFO, OBJID_CARET, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
                SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
            },
        },
    },
};

thread_local! {
    static QUERY_IN_PROGRESS: Cell<bool> = const { Cell::new(false) };
}

struct QueryGuard;

impl QueryGuard {
    fn enter() -> Option<Self> {
        QUERY_IN_PROGRESS.with(|active| {
            if active.replace(true) {
                None
            } else {
                Some(Self)
            }
        })
    }
}

impl Drop for QueryGuard {
    fn drop(&mut self) {
        QUERY_IN_PROGRESS.with(|active| active.set(false));
    }
}

/// Return the focused control's caret rectangle in screen coordinates.
///
/// The caller must not hold a document edit lock or a `TsfState` borrow. This
/// does not activate a window, change focus, or read the control's text.
pub(crate) fn accessible_caret_rect(owner_hwnd: HWND) -> Option<RECT> {
    let _guard = QueryGuard::enter()?;
    let focus = validated_focus(owner_hwnd)?;
    let screen = virtual_screen_bounds()?;
    let rect = query_accessible_location(focus, &screen, || {
        validated_focus(owner_hwnd) == Some(focus)
    })?;
    // Releasing the accessibility object can also reenter. Its lifetime ends
    // inside query_accessible_location, before this final focus check.
    (validated_focus(owner_hwnd) == Some(focus)).then_some(rect)
}

fn validated_focus(owner: HWND) -> Option<HWND> {
    unsafe {
        let thread_id = GetCurrentThreadId();
        let process_id = GetCurrentProcessId();
        if !usable_window(owner) {
            return None;
        }
        let mut owner_process = 0;
        if GetWindowThreadProcessId(owner, Some(&mut owner_process)) == 0
            || owner_process != process_id
        {
            return None;
        }

        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(thread_id, &mut info).ok()?;
        let focus = info.hwndFocus;
        let mut focus_process = 0;
        if !usable_window(focus)
            || GetWindowThreadProcessId(focus, Some(&mut focus_process)) != thread_id
            || focus_process != process_id
        {
            return None;
        }
        let root = GetAncestor(owner, GA_ROOT);
        if !usable_window(root)
            || GetAncestor(focus, GA_ROOT) != root
            || GetAncestor(GetForegroundWindow(), GA_ROOT) != root
        {
            return None;
        }
        Some(focus)
    }
}

unsafe fn usable_window(window: HWND) -> bool {
    !window.0.is_null()
        && IsWindow(window).as_bool()
        && IsWindowVisible(window).as_bool()
        && !IsIconic(window).as_bool()
}

struct PhysicalGeometryGuard(DPI_AWARENESS_CONTEXT);

impl PhysicalGeometryGuard {
    fn enter() -> Option<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            None
        } else {
            Some(Self(previous))
        }
    }
}

impl Drop for PhysicalGeometryGuard {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

fn virtual_screen_bounds() -> Option<RECT> {
    // MSAA returns physical screen coordinates. Query matching bounds even in
    // a DPI-unaware host, and restore its thread context on every return path.
    // This scope contains only geometry queries: it ends before any MSAA/COM
    // call can enter the host's code under a changed DPI awareness context.
    let _physical_geometry = PhysicalGeometryGuard::enter()?;
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(RECT {
            left,
            top,
            right: left.checked_add(width)?,
            bottom: top.checked_add(height)?,
        })
    }
}

fn query_accessible_location(
    focus: HWND,
    screen: &RECT,
    still_focused: impl FnOnce() -> bool,
) -> Option<RECT> {
    unsafe {
        let mut raw = std::ptr::null_mut();
        let result =
            AccessibleObjectFromWindow(focus, OBJID_CARET.0 as u32, &IAccessible::IID, &mut raw);
        // Own any returned interface even if a broken provider also fails.
        let accessible = (!raw.is_null()).then(|| IAccessible::from_raw(raw));
        result.ok()?;
        let accessible = accessible?;
        if !still_focused() {
            return None;
        }
        location_from_query(screen, |child, left, top, width, height| {
            // The projection maps S_FALSE to Ok(()). Only S_OK confirms a
            // location, as in the Microsoft sample. CHILDID_SELF is VT_I4 and
            // owns no allocations when passed by value to the COM method.
            (Interface::vtable(&accessible).accLocation)(
                accessible.as_raw(),
                left,
                top,
                width,
                height,
                MaybeUninit::new(child.clone()),
            )
        })
    }
}

fn location_from_query(
    screen: &RECT,
    query: impl FnOnce(&VARIANT, &mut i32, &mut i32, &mut i32, &mut i32) -> HRESULT,
) -> Option<RECT> {
    let (mut left, mut top, mut width, mut height) = (0, 0, 0, 0);
    let child = VARIANT::from(CHILDID_SELF as i32);
    if query(&child, &mut left, &mut top, &mut width, &mut height) != S_OK
        || width < 0
        || height <= 0
        || screen.right <= screen.left
        || screen.bottom <= screen.top
    {
        return None;
    }
    let rect = RECT {
        left,
        top,
        right: left.checked_add(width)?,
        bottom: top.checked_add(height)?,
    };
    // accLocation already returns screen coordinates. Preserve zero-width
    // carets and negative coordinates on secondary monitors, but reject
    // off-screen, oversized, or overflowing provider output.
    (rect.left >= screen.left
        && rect.top >= screen.top
        && rect.right <= screen.right
        && rect.bottom <= screen.bottom)
        .then_some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{E_FAIL, S_FALSE};

    const SCREEN: RECT = RECT {
        left: -1920,
        top: -1080,
        right: 1920,
        bottom: 1080,
    };

    fn location(hr: HRESULT, values: (i32, i32, i32, i32)) -> Option<RECT> {
        location_from_query(&SCREEN, |child, left, top, width, height| {
            assert_eq!(i32::try_from(child).unwrap(), CHILDID_SELF as i32);
            (*left, *top, *width, *height) = values;
            hr
        })
    }

    #[test]
    fn successful_msaa_location_preserves_screen_coordinates_and_zero_width() {
        let rect = location(S_OK, (-1200, -500, 0, 24)).unwrap();
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (-1200, -500, -1200, -476)
        );
        assert!(location(S_OK, (0, 0, 2, 24)).is_some());
    }

    #[test]
    fn failed_or_false_com_result_never_supplies_a_location() {
        for hr in [E_FAIL, S_FALSE] {
            assert!(location(hr, (100, 200, 2, 24)).is_none());
        }
    }

    #[test]
    fn malformed_and_overflowing_msaa_extents_are_rejected() {
        for values in [
            (100, 200, -1, 24),
            (100, 200, 2, 0),
            (100, 200, 2, -1),
            (i32::MAX, 200, 1, 24),
            (100, i32::MAX, 2, 1),
        ] {
            assert!(location(S_OK, values).is_none(), "{values:?}");
        }
    }

    #[test]
    fn off_screen_and_oversized_msaa_extents_are_rejected() {
        for values in [
            (-1921, 200, 2, 24),
            (100, -1081, 2, 24),
            (1920, 200, 2, 24),
            (100, 1070, 2, 24),
            (0, 0, 4000, 24),
            (0, 0, 2, 4000),
        ] {
            assert!(location(S_OK, values).is_none(), "{values:?}");
        }
    }

    #[test]
    fn query_guard_blocks_reentry_and_recovers_after_return() {
        let first = QueryGuard::enter().unwrap();
        assert!(QueryGuard::enter().is_none());
        assert!(QueryGuard::enter().is_none());
        drop(first);
        assert!(QueryGuard::enter().is_some());
    }
}
