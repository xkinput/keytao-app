//! Manual RichEdit/TSF regression for an isolated Windows test account or VM.
//! All HWNDs are hidden and no keys or profile changes are sent. Nevertheless,
//! NOACTIVATETIP only defers activation: RichEdit may later activate installed
//! TIPs even without a message pump. Module assertions detect that afterwards;
//! they do not prevent it. Do not run this fixture in the user's normal account.
//!
//! Run with --ignored --exact and the test name below. Optionally select a
//! copied RichEdit DLL/class with KEYTAO_TEST_RICHEDIT_DLL/_CLASS.
use super::{inspect_context, ContextProbe};
use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use windows::{
    core::{w, Interface, HRESULT, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            LibraryLoader::{
                GetModuleHandleW, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{GetFocus, GetKeyboardLayout},
            TextServices::{
                CLSID_TF_ThreadMgr, ITfContext, ITfThreadMgrEx, TF_TMAE_NOACTIVATEKEYBOARDLAYOUT,
                TF_TMAE_NOACTIVATETIP,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, GetClassInfoExW, SendMessageW, ES_MULTILINE,
                ES_PASSWORD, WINDOW_EX_STYLE, WINDOW_STYLE, WM_KILLFOCUS, WM_SETFOCUS, WNDCLASSEXW,
                WS_CHILD, WS_EX_NOACTIVATE, WS_OVERLAPPED,
            },
        },
    },
};

#[link(name = "ole32")]
extern "system" {
    fn OleInitialize(reserved: *mut std::ffi::c_void) -> HRESULT;
    fn OleUninitialize();
}

const EM_SETEDITSTYLE: u32 = 0x0400 + 204;
const SES_USECTF: usize = 0x0001_0000;
const EM_SETPASSWORDCHAR: u32 = 0x00cc;
const TEST_NAME: &str = "input_context::native_tests::native_richedit_input_scope_contract";
const CHILD_ENV: &str = "KEYTAO_TEST_NATIVE_RICHEDIT_CHILD";

struct IsolatedChild(Child);
impl Drop for IsolatedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
#[ignore = "requires an isolated Windows test account/VM; RichEdit may activate installed TIPs"]
fn native_richedit_input_scope_contract() {
    if std::env::var_os(CHILD_ENV).is_none() {
        let mut child = IsolatedChild(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", TEST_NAME, "--nocapture"])
                .env(CHILD_ENV, "1")
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                use std::io::Read;
                let mut stdout = String::new();
                let mut stderr = String::new();
                child
                    .0
                    .stdout
                    .take()
                    .unwrap()
                    .read_to_string(&mut stdout)
                    .unwrap();
                child
                    .0
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut stderr)
                    .unwrap();
                assert!(
                    status.success(),
                    "isolated RichEdit test failed:\n{stdout}\n{stderr}"
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "isolated RichEdit test timed out"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    unsafe {
        run_native_fixture();
    }
}

struct Fixture {
    manager: Option<ITfThreadMgrEx>,
    parent: HWND,
    initialized: bool,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            if !self.parent.0.is_null() {
                let _ = DestroyWindow(self.parent);
            }
            if let Some(manager) = self.manager.take() {
                let _ = manager.Deactivate();
            }
            if self.initialized {
                OleUninitialize();
            }
        }
    }
}

unsafe fn context_for_window(manager: &ITfThreadMgrEx, window: HWND) -> ITfContext {
    let documents = manager.EnumDocumentMgrs().unwrap();
    loop {
        let mut document = [None];
        let mut count = 0;
        documents.Next(&mut document, &mut count).unwrap();
        if count == 0 {
            break;
        }
        let Some(context) = document[0].as_ref().and_then(|doc| doc.GetTop().ok()) else {
            continue;
        };
        if context.GetActiveView().and_then(|view| view.GetWnd()).ok() == Some(window) {
            return context;
        }
    }
    panic!("hidden RichEdit did not publish its TSF context");
}

unsafe fn run_native_fixture() {
    let initial_rime = GetModuleHandleW(w!("rime.dll")).ok();
    eprintln!("native fixture startup Rime mapping: {initial_rime:?}");
    let original_focus = GetFocus();
    let original_layout = GetKeyboardLayout(0);
    OleInitialize(std::ptr::null_mut()).ok().unwrap();
    let mut fixture = Fixture {
        manager: None,
        parent: HWND::default(),
        initialized: true,
    };
    let manager: ITfThreadMgrEx =
        CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
    let mut client_id = 0;
    manager
        .ActivateEx(
            &mut client_id,
            TF_TMAE_NOACTIVATETIP | TF_TMAE_NOACTIVATEKEYBOARDLAYOUT,
        )
        .unwrap();
    fixture.manager = Some(manager.clone());

    let dll = std::env::var_os("KEYTAO_TEST_RICHEDIT_DLL")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/msftedit.dll")
        });
    let dll = dll.canonicalize().expect("RichEdit fixture DLL must exist");
    let dll_wide: Vec<u16> = dll
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let rich = LoadLibraryExW(
        PCWSTR(dll_wide.as_ptr()),
        None,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
    )
    .unwrap();
    // Keep the library mapped until process exit because TSF may retain its
    // control implementation beyond the final HWND's destruction.
    let classes = std::env::var("KEYTAO_TEST_RICHEDIT_CLASS")
        .map(|name| vec![name])
        .unwrap_or_else(|_| {
            vec![
                "RICHEDIT50W".into(),
                "RichEditD2DPT".into(),
                "RichEditD2D".into(),
                "RICHEDIT60W".into(),
                "RichEdit20W".into(),
            ]
        });
    let class = classes
        .into_iter()
        .find_map(|name| {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let mut info = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                ..Default::default()
            };
            (GetClassInfoExW(HINSTANCE(rich.0), PCWSTR(wide.as_ptr()), &mut info).is_ok()
                || GetClassInfoExW(None, PCWSTR(wide.as_ptr()), &mut info).is_ok())
            .then_some((name, wide))
        })
        .expect("fixture DLL did not register a supported RichEdit class");
    eprintln!("RichEdit native fixture: {} / {}", dll.display(), class.0);
    let instance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
    fixture.parent = CreateWindowExW(
        WS_EX_NOACTIVATE,
        w!("STATIC"),
        w!("KeyTao isolated RichEdit regression"),
        WS_OVERLAPPED,
        0,
        0,
        600,
        400,
        None,
        None,
        instance,
        None,
    )
    .unwrap();

    for (label, style_password, mask, text, expected) in [
        (
            "ordinary_nonempty",
            false,
            false,
            "fixture",
            ContextProbe::Clear,
        ),
        ("ordinary_empty", false, false, "", ContextProbe::Clear),
        (
            "password_style",
            true,
            false,
            "fixture",
            ContextProbe::Restricted,
        ),
        (
            "password_character",
            false,
            true,
            "fixture",
            ContextProbe::Restricted,
        ),
        (
            "password_style_and_character",
            true,
            true,
            "fixture",
            ContextProbe::Restricted,
        ),
    ] {
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let style = WS_CHILD
            | WINDOW_STYLE(ES_MULTILINE as u32)
            | WINDOW_STYLE(if style_password {
                ES_PASSWORD as u32
            } else {
                0
            });
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.1.as_ptr()),
            PCWSTR(text.as_ptr()),
            style,
            0,
            0,
            500,
            300,
            fixture.parent,
            None,
            instance,
            None,
        )
        .unwrap();
        SendMessageW(
            edit,
            EM_SETEDITSTYLE,
            WPARAM(SES_USECTF),
            LPARAM(SES_USECTF as isize),
        );
        if mask {
            SendMessageW(edit, EM_SETPASSWORDCHAR, WPARAM('*' as usize), LPARAM(0));
        }
        // Deliver control-local notifications; never SetFocus, SendInput, or
        // show/activate a real window in the user's desktop.
        SendMessageW(edit, WM_SETFOCUS, WPARAM(0), LPARAM(0));
        let context = context_for_window(&manager, edit);
        let thread_manager = manager.cast().unwrap();
        let (result, refused) = inspect_context(Some(&thread_manager), Some(&context), client_id);
        eprintln!("{label}: {result:?}, synchronous_refusal={refused}");
        assert!(
            !refused,
            "{label}: native read session was unexpectedly refused"
        );
        if expected == ContextProbe::Clear {
            // Hosts may return either an empty optional value or E_FAIL.
            // Both ordinary cases allow input; E_FAIL remains retryable.
            assert!(
                matches!(
                    result.password,
                    ContextProbe::Clear | ContextProbe::Unavailable
                ),
                "{label}: {:?}",
                result.password
            );
        } else {
            assert_eq!(result.password, expected, "{label}");
        }
        assert_eq!(
            result.is_sensitive(),
            expected == ContextProbe::Restricted,
            "{label}"
        );
        drop(context);
        SendMessageW(edit, WM_KILLFOCUS, WPARAM(0), LPARAM(0));
        DestroyWindow(edit).unwrap();
    }
    assert_eq!(
        GetFocus(),
        original_focus,
        "fixture changed Win32 keyboard focus"
    );
    assert_eq!(
        GetKeyboardLayout(0),
        original_layout,
        "fixture switched keyboard layout"
    );
    assert_eq!(
        GetModuleHandleW(w!("rime.dll")).ok(),
        initial_rime,
        "fixture loaded Rime"
    );
    assert!(
        GetModuleHandleW(w!("keytao_windows_ime.dll")).is_err(),
        "fixture activated the installed TIP"
    );
    drop(manager);
    drop(fixture);
}
