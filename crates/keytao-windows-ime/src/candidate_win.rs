//! Win32 candidate popup window.
//!
//! Mirrors the Linux panel: same BGRA buffer from panel.rs fed to
//! UpdateLayeredWindow instead of wl_shm / XCB image upload.
//!
//! Window style: WS_POPUP | WS_EX_LAYERED | WS_EX_TOPMOST |
//!               WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
//! This gives us a click-through, focus-stealing-safe, always-on-top popup.

use keytao_core::ImeState;
use windows::{
    core::Result,
    Win32::{
        Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, SetBitmapBits, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, MoveWindow,
            RegisterClassExW, SetLayeredWindowAttributes, ShowWindow, UpdateLayeredWindow,
            CW_USEDEFAULT, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE,
            ULW_ALPHA, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

use crate::{globals::DLL_INSTANCE, panel::PanelRenderer};

const CLASS_NAME: &str = "KeyTaoCandidate\0";

fn class_name_wide() -> Vec<u16> {
    CLASS_NAME.encode_utf16().collect()
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Manages the floating candidate panel window.
pub struct CandidateWindow {
    hwnd: HWND,
    renderer: Option<PanelRenderer>,
    visible: bool,
}

// SAFETY: TSF TIPs run in COM STA; all window operations happen on the same thread.
unsafe impl Send for CandidateWindow {}
unsafe impl Sync for CandidateWindow {}

impl CandidateWindow {
    pub fn new() -> Self {
        let renderer = crate::panel::load_font().map(PanelRenderer::new);
        if renderer.is_none() {
            tracing::warn!("candidate window: no CJK font found");
        }

        let hwnd = unsafe { Self::create_window() }.unwrap_or(HWND(std::ptr::null_mut()));
        Self {
            hwnd,
            renderer,
            visible: false,
        }
    }

    unsafe fn create_window() -> Result<HWND> {
        let hinstance: HMODULE = *DLL_INSTANCE
            .get()
            .unwrap_or(&GetModuleHandleW(None).unwrap_or_default());

        let class_name = class_name_wide();

        // Register window class (idempotent — fails silently if already registered)
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc); // ignore duplicate-class error

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::PCWSTR(std::ptr::null()),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1,
            1, // initial size — overwritten by redraw
            HWND(std::ptr::null_mut()),
            None,
            hinstance,
            None,
        )?;

        Ok(hwnd)
    }

    /// Show/update the panel near caret position (screen coordinates).
    pub fn show(&mut self, state: &ImeState, caret_x: i32, caret_y: i32) {
        if self.hwnd.0.is_null() {
            return;
        }
        let has_content = !state.candidates.is_empty() || !state.preedit.is_empty();
        if !has_content {
            self.hide();
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };

        let (pixels, w, h) = renderer.render(state);
        if w == 0 || h == 0 {
            return;
        }

        // Position: below caret, nudge up if off-screen
        let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        let y = if caret_y + h as i32 > screen_h {
            caret_y - h as i32 - 4
        } else {
            caret_y + 4
        };

        unsafe {
            self.upload_pixels(&pixels, w, h, caret_x, y);
        }

        if !self.visible {
            unsafe {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.visible = true;
        }
    }

    pub fn hide(&mut self) {
        if self.visible && !self.hwnd.0.is_null() {
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    /// Upload BGRA pixel buffer via UpdateLayeredWindow (per-pixel alpha).
    unsafe fn upload_pixels(&self, pixels: &[u8], w: u32, h: u32, x: i32, y: i32) {
        let screen_dc = GetDC(HWND(std::ptr::null_mut()));
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, w as i32, h as i32);
        let old_bmp = SelectObject(mem_dc, bitmap);

        // Copy pixels into the DIB
        SetBitmapBits(bitmap, pixels.len() as u32, pixels.as_ptr() as *const _);

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let pt_dst = POINT { x, y };
        let pt_src = POINT { x: 0, y: 0 };
        let sz = SIZE {
            cx: w as i32,
            cy: h as i32,
        };

        let _ = UpdateLayeredWindow(
            self.hwnd,
            screen_dc,
            Some(&pt_dst),
            Some(&sz),
            mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        SelectObject(mem_dc, old_bmp);
        DeleteObject(bitmap);
        DeleteDC(mem_dc);
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
    }
}

impl Drop for CandidateWindow {
    fn drop(&mut self) {
        if !self.hwnd.0.is_null() {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
