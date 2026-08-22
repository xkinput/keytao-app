//! Win32 candidate popup window.
//!
//! Mirrors the Linux panel: same BGRA buffer from panel.rs fed to
//! UpdateLayeredWindow instead of wl_shm / XCB image upload.
//!
//! Window style: WS_POPUP | WS_EX_LAYERED | WS_EX_TOPMOST |
//!               WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
//! The candidate panel takes mouse input (clicking a candidate selects it)
//! without ever taking activation away from the host; the transient mode hint
//! adds WS_EX_TRANSPARENT so it stays click-through.

use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};

use keytao_core::ImeState;
use windows::{
    core::Result,
    Win32::{
        Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
            MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        System::LibraryLoader::GetModuleHandleW,
        UI::Accessibility::NotifyWinEvent,
        UI::HiDpi::GetDpiForWindow,
        UI::Shell::{FrameworkInputPane, IFrameworkInputPane},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, GetWindowLongPtrW,
            KillTimer, RegisterClassExW, SetTimer, SetWindowLongPtrW, ShowWindow, UnregisterClassW,
            UpdateLayeredWindow, CW_USEDEFAULT, EVENT_OBJECT_IME_CHANGE, EVENT_OBJECT_IME_HIDE,
            EVENT_OBJECT_IME_SHOW, GWLP_USERDATA, MA_NOACTIVATE, OBJID_CLIENT, SM_CXVIRTUALSCREEN,
            SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE,
            ULW_ALPHA, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_TIMER, WNDCLASSEXW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            WS_POPUP,
        },
    },
};

use crate::{
    globals::DLL_INSTANCE,
    key_event_sink::{handle_panel_click, PanelClick},
    panel::{PanelHitAreas, PanelRenderer},
    state::{append_diagnostic, diagnostics_enabled, WeakState},
};

const CLASS_NAME: &str = "KeyTaoCandidate\0";
const MODE_HINT_TIMER_ID: usize = 1;
const CARET_RETRY_TIMER_ID: usize = 2;
const WINDOWS_CANDIDATE_DENSITY: f32 = 0.82;
const INPUT_PANE_POLL_INTERVAL: Duration = Duration::from_millis(250);

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

/// The touch keyboard sits in a higher z-order band than any application and
/// does not shrink the monitor work area, so a bottom-docked pane has to be
/// carved out by hand (`IME and touch keyboard` in the IME requirements).
fn subtract_touch_keyboard(work: RECT, pane: RECT) -> RECT {
    let overlaps_horizontally = pane.left < work.right && pane.right > work.left;
    let overlaps_vertically = pane.top < work.bottom && pane.bottom > work.top;
    if !overlaps_horizontally || !overlaps_vertically || pane.top <= work.top {
        return work;
    }
    RECT {
        bottom: pane.top,
        ..work
    }
}

fn popup_position_in(
    work: RECT,
    caret_x: i32,
    caret_y: i32,
    width: u32,
    height: u32,
    gap: i32,
) -> POINT {
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

/// Per-window click state, owned by the HWND through `GWLP_USERDATA`.
struct ClickTarget {
    state: WeakState,
    hit_areas: RefCell<PanelHitAreas>,
    caret_reprobe_pending: Cell<bool>,
}

fn click_target<'a>(hwnd: HWND) -> Option<&'a ClickTarget> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if raw == 0 {
        return None;
    }
    // SAFETY: the pointer is a Box<ClickTarget> installed right after window
    // creation and taken back on WM_NCDESTROY; the window and its user data
    // only ever live on the STA thread that created them.
    Some(unsafe { &*(raw as *const ClickTarget) })
}

fn click_at(hit_areas: &PanelHitAreas, x: i32, y: i32) -> Option<PanelClick> {
    if let Some(rect) = hit_areas.previous_page {
        if rect.contains(x, y) {
            return Some(PanelClick::PreviousPage);
        }
    }
    if let Some(rect) = hit_areas.next_page {
        if rect.contains(x, y) {
            return Some(PanelClick::NextPage);
        }
    }
    hit_areas
        .candidates
        .iter()
        .position(|rect| rect.contains(x, y))
        .map(PanelClick::Candidate)
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == CARET_RETRY_TIMER_ID => {
            let _ = KillTimer(hwnd, CARET_RETRY_TIMER_ID);
            if let Some(target) = click_target(hwnd) {
                target.caret_reprobe_pending.set(false);
                if let Some(state) = target.state.upgrade() {
                    crate::key_event_sink::retry_caret_probe(&state);
                }
            }
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == MODE_HINT_TIMER_ID => {
            let _ = KillTimer(hwnd, MODE_HINT_TIMER_ID);
            let _ = ShowWindow(hwnd, SW_HIDE);
            return LRESULT(0);
        }
        // Selecting a candidate must never move the caret out of the host.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let clicked = click_target(hwnd).and_then(|target| {
                let click = click_at(&target.hit_areas.borrow(), x, y)?;
                Some((target.state.clone(), click))
            });
            if let Some((state, click)) = clicked {
                if let Some(state) = state.upgrade() {
                    handle_panel_click(&state, click);
                }
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            let _ = KillTimer(hwnd, CARET_RETRY_TIMER_ID);
            let raw = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if raw != 0 {
                drop(Box::from_raw(raw as *mut ClickTarget));
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Manages the floating candidate panel window.
pub struct CandidateWindow {
    hwnd: HWND,
    owner_hwnd: HWND,
    renderer: Option<PanelRenderer>,
    visible: bool,
    click_through: bool,
    input_pane: Option<IFrameworkInputPane>,
    input_pane_unavailable: bool,
    input_pane_rect: Option<RECT>,
    input_pane_queried_at: Option<Instant>,
}

impl CandidateWindow {
    pub fn new() -> Self {
        Self {
            hwnd: HWND(std::ptr::null_mut()),
            owner_hwnd: HWND(std::ptr::null_mut()),
            renderer: None,
            visible: false,
            click_through: false,
            input_pane: None,
            input_pane_unavailable: false,
            input_pane_rect: None,
            input_pane_queried_at: None,
        }
    }

    /// The transient Chinese/English hint carries no interaction.
    pub fn new_mode_hint() -> Self {
        let mut window = Self::new();
        window.click_through = true;
        window
    }

    fn ensure_window(&mut self, owner_hwnd: HWND, state: &WeakState) -> bool {
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
        match unsafe { Self::create_window(owner_hwnd, self.click_through) } {
            Ok(hwnd) => {
                if !self.click_through {
                    let target = Box::new(ClickTarget {
                        state: state.clone(),
                        hit_areas: RefCell::new(PanelHitAreas::default()),
                        caret_reprobe_pending: Cell::new(false),
                    });
                    unsafe {
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(target) as _);
                    }
                }
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

    /// Work area minus the touch keyboard, if one is showing.
    fn available_area(&mut self, point: POINT) -> RECT {
        let work = monitor_work_area(point);
        match self.touch_keyboard_rect() {
            Some(pane) => subtract_touch_keyboard(work, pane),
            None => work,
        }
    }

    /// `Location()` reaches into the shell, so the answer is cached for a few
    /// frames — the panel is redrawn on every keystroke.
    fn touch_keyboard_rect(&mut self) -> Option<RECT> {
        if self.input_pane_unavailable {
            return None;
        }
        if let Some(queried_at) = self.input_pane_queried_at {
            if queried_at.elapsed() < INPUT_PANE_POLL_INTERVAL {
                return self.input_pane_rect;
            }
        }
        if self.input_pane.is_none() {
            match unsafe { CoCreateInstance(&FrameworkInputPane, None, CLSCTX_INPROC_SERVER) } {
                Ok(pane) => self.input_pane = Some(pane),
                Err(_) => {
                    self.input_pane_unavailable = true;
                    return None;
                }
            }
        }
        self.input_pane_queried_at = Some(Instant::now());
        self.input_pane_rect = unsafe { self.input_pane.as_ref()?.Location() }
            .ok()
            .filter(|rect| rect.right > rect.left && rect.bottom > rect.top);
        self.input_pane_rect
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

    unsafe fn create_window(owner_hwnd: HWND, click_through: bool) -> Result<HWND> {
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

        let mut ex_style = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
        if click_through {
            ex_style |= WS_EX_TRANSPARENT;
        }
        let hwnd = CreateWindowExW(
            ex_style,
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

    pub fn arm_caret_reprobe(&mut self, owner_hwnd: HWND, state: &WeakState) {
        if self.hwnd.0.is_null() && !self.ensure_window(owner_hwnd, state) {
            return;
        }
        let Some(target) = click_target(self.hwnd) else {
            return;
        };
        if target.caret_reprobe_pending.get() {
            return;
        }
        let Some(shared_state) = state.upgrade() else {
            return;
        };
        let completed_attempts = shared_state.borrow().caret_retry_attempts;
        if completed_attempts >= 6 {
            if diagnostics_enabled() {
                append_diagnostic(format!("caret retry attempt={completed_attempts} gave_up"));
            }
            return;
        }
        let attempt = completed_attempts + 1;
        let timer_id = unsafe { SetTimer(self.hwnd, CARET_RETRY_TIMER_ID, 16, None) };
        if timer_id != 0 {
            target.caret_reprobe_pending.set(true);
            if diagnostics_enabled() {
                append_diagnostic(format!("caret retry attempt={attempt} armed"));
            }
        }
    }

    /// Show/update the panel near caret position (screen coordinates).
    pub fn show(
        &mut self,
        ime_state: &ImeState,
        caret_x: i32,
        caret_y: i32,
        owner_hwnd: HWND,
        state: &WeakState,
    ) {
        let has_content = !ime_state.candidates.is_empty() || !ime_state.preedit.is_empty();
        if !has_content {
            self.hide();
            return;
        }
        if !self.ensure_window(owner_hwnd, state) || !self.ensure_renderer() {
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };

        let scale = dpi_scale(owner_hwnd);
        let rendered = renderer.render(ime_state, scale);
        let (w, h) = (rendered.width, rendered.height);
        if w == 0 || h == 0 {
            return;
        }

        let work = self.available_area(POINT {
            x: caret_x,
            y: caret_y,
        });
        let position =
            popup_position_in(work, caret_x, caret_y, w, h, (4.0 * scale).round() as i32);
        if diagnostics_enabled() {
            append_diagnostic(format!(
                "panel show caret=({caret_x},{caret_y}) pos=({},{}) size={w}x{h}",
                position.x, position.y
            ));
        }

        if let Some(target) = click_target(self.hwnd) {
            *target.hit_areas.borrow_mut() = rendered.hit_areas;
        }

        unsafe {
            self.upload_pixels(&rendered.pixels, w, h, position.x, position.y);
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
        if !self.hwnd.0.is_null() {
            if let Some(target) = click_target(self.hwnd) {
                target.caret_reprobe_pending.set(false);
            }
            unsafe {
                let _ = KillTimer(self.hwnd, MODE_HINT_TIMER_ID);
                let _ = KillTimer(self.hwnd, CARET_RETRY_TIMER_ID);
                if self.visible {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            }
            if self.visible {
                self.visible = false;
                self.notify_ime_event(EVENT_OBJECT_IME_HIDE);
            }
        }
    }

    pub fn show_mode_hint(
        &mut self,
        ascii_mode: bool,
        caret_x: i32,
        caret_y: i32,
        owner_hwnd: HWND,
        state: &WeakState,
    ) {
        if !self.ensure_window(owner_hwnd, state) || !self.ensure_renderer() {
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };

        let scale = dpi_scale(owner_hwnd);
        let (pixels, w, h) = renderer.render_mode_hint(ascii_mode, scale);
        let hint_duration_ms = renderer.mode_hint_duration_ms();
        if w == 0 || h == 0 {
            return;
        }

        let anchor_x = caret_x - w as i32 / 2;
        let work = self.available_area(POINT {
            x: anchor_x,
            y: caret_y,
        });
        let position =
            popup_position_in(work, anchor_x, caret_y, w, h, (8.0 * scale).round() as i32);

        unsafe {
            self.upload_pixels(&pixels, w, h, position.x, position.y);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = SetTimer(self.hwnd, MODE_HINT_TIMER_ID, hint_duration_ms, None);
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
    use windows::Win32::Foundation::RECT;

    use super::{click_at, popup_position_in, render_scale_for_dpi, subtract_touch_keyboard};
    use crate::{
        key_event_sink::PanelClick,
        panel::{PanelHitAreas, PanelRect},
    };

    fn work_area() -> RECT {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    }

    #[test]
    fn render_scale_combines_monitor_dpi_with_compact_windows_density() {
        assert!((render_scale_for_dpi(96) - 0.82).abs() < 0.001);
        assert!((render_scale_for_dpi(144) - 1.23).abs() < 0.001);
        assert!((render_scale_for_dpi(192) - 1.64).abs() < 0.001);
    }

    #[test]
    fn touch_keyboard_is_carved_out_of_the_work_area() {
        let pane = RECT {
            left: 0,
            top: 700,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(subtract_touch_keyboard(work_area(), pane).bottom, 700);

        // A pane on another monitor, or one that covers everything, is ignored.
        let elsewhere = RECT {
            left: 2000,
            top: 700,
            right: 3800,
            bottom: 1080,
        };
        assert_eq!(subtract_touch_keyboard(work_area(), elsewhere).bottom, 1080);
        let covering = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(subtract_touch_keyboard(work_area(), covering).bottom, 1080);
    }

    #[test]
    fn panel_flips_above_the_caret_when_the_touch_keyboard_takes_the_bottom() {
        let work = subtract_touch_keyboard(
            work_area(),
            RECT {
                left: 0,
                top: 700,
                right: 1920,
                bottom: 1080,
            },
        );
        let position = popup_position_in(work, 300, 660, 400, 120, 4);
        assert!(position.y + 120 <= 700, "panel must stay above the pane");
    }

    #[test]
    fn clicks_map_to_candidates_and_paging() {
        let hit_areas = PanelHitAreas {
            candidates: vec![
                PanelRect {
                    x: 0,
                    y: 0,
                    width: 50,
                    height: 30,
                },
                PanelRect {
                    x: 50,
                    y: 0,
                    width: 50,
                    height: 30,
                },
            ],
            previous_page: Some(PanelRect {
                x: 100,
                y: 0,
                width: 20,
                height: 30,
            }),
            next_page: Some(PanelRect {
                x: 120,
                y: 0,
                width: 20,
                height: 30,
            }),
        };
        assert_eq!(click_at(&hit_areas, 10, 10), Some(PanelClick::Candidate(0)));
        assert_eq!(click_at(&hit_areas, 60, 10), Some(PanelClick::Candidate(1)));
        assert_eq!(
            click_at(&hit_areas, 105, 10),
            Some(PanelClick::PreviousPage)
        );
        assert_eq!(click_at(&hit_areas, 125, 10), Some(PanelClick::NextPage));
        assert_eq!(click_at(&hit_areas, 300, 10), None);
    }
}
