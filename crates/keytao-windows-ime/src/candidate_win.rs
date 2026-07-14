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
        Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
            MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::Accessibility::NotifyWinEvent,
        UI::HiDpi::GetDpiForWindow,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, KillTimer,
            RegisterClassExW, SetTimer, ShowWindow, UnregisterClassW, UpdateLayeredWindow,
            CW_USEDEFAULT, EVENT_OBJECT_IME_CHANGE, EVENT_OBJECT_IME_HIDE, EVENT_OBJECT_IME_SHOW,
            OBJID_CLIENT, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_TIMER, WNDCLASSEXW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

use crate::{globals::DLL_INSTANCE, panel::PanelRenderer};

const CLASS_NAME: &str = "KeyTaoCandidate\0";
const MODE_HINT_TIMER_ID: usize = 1;
const WINDOWS_CANDIDATE_DENSITY: f32 = 0.82;

fn class_name_wide() -> Vec<u16> {
    CLASS_NAME.encode_utf16().collect()
}

fn module_handle() -> HMODULE {
    DLL_INSTANCE
        .get()
        .map(|raw| HMODULE(*raw as _))
        .unwrap_or_else(|| unsafe { GetModuleHandleW(None).unwrap_or_default() })
}

fn dpi_scale(owner_hwnd: HWND) -> f32 {
    if owner_hwnd.0.is_null() {
        return WINDOWS_CANDIDATE_DENSITY;
    }
    render_scale_for_dpi(unsafe { GetDpiForWindow(owner_hwnd) })
}

fn render_scale_for_dpi(dpi: u32) -> f32 {
    if dpi == 0 {
        WINDOWS_CANDIDATE_DENSITY
    } else {
        (dpi as f32 / 96.0).clamp(1.0, 3.0) * WINDOWS_CANDIDATE_DENSITY
    }
}

fn monitor_work_area(point: POINT) -> windows::Win32::Foundation::RECT {
    unsafe {
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !monitor.0.is_null() && GetMonitorInfoW(monitor, &mut info).as_bool() {
            return info.rcWork;
        }

        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        windows::Win32::Foundation::RECT {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn popup_position(caret_x: i32, caret_y: i32, width: u32, height: u32, gap: i32) -> POINT {
    let work = monitor_work_area(POINT {
        x: caret_x,
        y: caret_y,
    });
    let max_x = (work.right - width as i32).max(work.left);
    let x = caret_x.clamp(work.left, max_x);
    let below = caret_y + gap;
    let above = caret_y - height as i32 - gap;
    let y = if below + height as i32 <= work.bottom {
        below
    } else {
        above
    }
    .clamp(work.top, (work.bottom - height as i32).max(work.top));
    POINT { x, y }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TIMER && wparam.0 == MODE_HINT_TIMER_ID {
        let _ = KillTimer(hwnd, MODE_HINT_TIMER_ID);
        let _ = ShowWindow(hwnd, SW_HIDE);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Manages the floating candidate panel window.
pub struct CandidateWindow {
    hwnd: HWND,
    owner_hwnd: HWND,
    renderer: Option<PanelRenderer>,
    visible: bool,
}

impl CandidateWindow {
    pub fn new() -> Self {
        Self {
            hwnd: HWND(std::ptr::null_mut()),
            owner_hwnd: HWND(std::ptr::null_mut()),
            renderer: None,
            visible: false,
        }
    }

    fn ensure_window(&mut self, owner_hwnd: HWND) -> bool {
        if !self.hwnd.0.is_null() && self.owner_hwnd != owner_hwnd {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
            self.hwnd = HWND(std::ptr::null_mut());
            self.visible = false;
        }
        if !self.hwnd.0.is_null() {
            return true;
        }
        match unsafe { Self::create_window(owner_hwnd) } {
            Ok(hwnd) => {
                self.hwnd = hwnd;
                self.owner_hwnd = owner_hwnd;
                true
            }
            Err(e) => {
                tracing::warn!("candidate window: failed to create popup window: {e}");
                false
            }
        }
    }

    fn ensure_renderer(&mut self) -> bool {
        if self.renderer.is_some() {
            return true;
        }
        self.renderer = crate::panel::load_font().map(PanelRenderer::new);
        if self.renderer.is_none() {
            tracing::warn!("candidate window: no CJK font found");
        }
        self.renderer.is_some()
    }

    unsafe fn create_window(owner_hwnd: HWND) -> Result<HWND> {
        let hinstance = module_handle();

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
            owner_hwnd,
            None,
            hinstance,
            None,
        )?;

        Ok(hwnd)
    }

    /// Show/update the panel near caret position (screen coordinates).
    pub fn show(&mut self, state: &ImeState, caret_x: i32, caret_y: i32, owner_hwnd: HWND) {
        let has_content = !state.candidates.is_empty() || !state.preedit.is_empty();
        if !has_content {
            self.hide();
            return;
        }
        if !self.ensure_window(owner_hwnd) || !self.ensure_renderer() {
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };

        let scale = dpi_scale(owner_hwnd);
        let (pixels, w, h) = renderer.render(state, scale);
        if w == 0 || h == 0 {
            return;
        }

        let position = popup_position(caret_x, caret_y, w, h, (4.0 * scale).round() as i32);

        unsafe {
            self.upload_pixels(&pixels, w, h, position.x, position.y);
        }

        if !self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.visible = true;
            self.notify_ime_event(EVENT_OBJECT_IME_SHOW);
        }
    }

    pub fn hide(&mut self) {
        if self.visible && !self.hwnd.0.is_null() {
            unsafe {
                let _ = KillTimer(self.hwnd, MODE_HINT_TIMER_ID);
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
            self.notify_ime_event(EVENT_OBJECT_IME_HIDE);
        }
    }

    pub fn show_mode_hint(
        &mut self,
        ascii_mode: bool,
        caret_x: i32,
        caret_y: i32,
        owner_hwnd: HWND,
    ) {
        if !self.ensure_window(owner_hwnd) || !self.ensure_renderer() {
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };

        let scale = dpi_scale(owner_hwnd);
        let (pixels, w, h) = renderer.render_mode_hint(ascii_mode, scale);
        if w == 0 || h == 0 {
            return;
        }

        let position = popup_position(
            caret_x - w as i32 / 2,
            caret_y,
            w,
            h,
            (8.0 * scale).round() as i32,
        );

        unsafe {
            self.upload_pixels(&pixels, w, h, position.x, position.y);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = SetTimer(
                self.hwnd,
                MODE_HINT_TIMER_ID,
                renderer.mode_hint_duration_ms(),
                None,
            );
        }
        self.visible = true;
        self.notify_ime_event(EVENT_OBJECT_IME_SHOW);
    }

    /// Upload BGRA pixel buffer via UpdateLayeredWindow (per-pixel alpha).
    unsafe fn upload_pixels(&self, pixels: &[u8], w: u32, h: u32, x: i32, y: i32) {
        let screen_dc = GetDC(HWND(std::ptr::null_mut()));
        let mem_dc = CreateCompatibleDC(screen_dc);
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down BGRA
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let bitmap = match CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) => bitmap,
            Err(_) => {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
                return;
            }
        };
        if bitmap.0.is_null() || bits.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return;
        }
        let old_bmp = SelectObject(mem_dc, bitmap);

        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());

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
        self.notify_ime_event(EVENT_OBJECT_IME_CHANGE);

        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
    }

    fn notify_ime_event(&self, event: u32) {
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            NotifyWinEvent(event, self.hwnd, OBJID_CLIENT.0, 0);
        }
    }
}

impl Drop for CandidateWindow {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.0.is_null() {
                let _ = DestroyWindow(self.hwnd);
            }
            let class_name = class_name_wide();
            let _ = UnregisterClassW(windows::core::PCWSTR(class_name.as_ptr()), module_handle());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render_scale_for_dpi;

    #[test]
    fn render_scale_combines_monitor_dpi_with_compact_windows_density() {
        assert!((render_scale_for_dpi(96) - 0.82).abs() < 0.001);
        assert!((render_scale_for_dpi(144) - 1.23).abs() < 0.001);
        assert!((render_scale_for_dpi(192) - 1.64).abs() < 0.001);
    }
}
