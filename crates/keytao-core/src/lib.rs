//! Pure librime engine wrapper — no Tauri, no D-Bus, no platform I/O.
//! Every platform frontend (Tauri app, ibus engine, macOS IMKit, Windows TSF)
//! links against this crate as its rime back-end.

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImeState {
    pub preedit: String,
    pub cursor: usize,
    pub candidates: Vec<Candidate>,
    pub all_candidates: Vec<Candidate>,
    pub highlighted_candidate_index: usize,
    pub page_size: usize,
    pub page: usize,
    pub is_last_page: bool,
    pub committed: Option<String>,
    pub select_keys: Option<String>,
    pub ascii_mode: bool,
    pub schema_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyProcessResult {
    pub state: ImeState,
    pub accepted: bool,
}

impl ImeState {
    pub fn empty() -> Self {
        Self {
            preedit: String::new(),
            cursor: 0,
            candidates: vec![],
            all_candidates: vec![],
            highlighted_candidate_index: 0,
            page_size: 0,
            page: 0,
            is_last_page: true,
            committed: None,
            select_keys: None,
            ascii_mode: false,
            schema_name: String::new(),
        }
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    test
))]
fn rime_build_dirs(user_data_dir: &Path, shared_data_dir: &Path) -> (PathBuf, PathBuf) {
    let staging_dir = user_data_dir.join("build");
    let prebuilt_dir = if user_data_dir == shared_data_dir {
        shared_data_dir.join("prebuilt")
    } else {
        shared_data_dir.join("build")
    };
    (staging_dir, prebuilt_dir)
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios",
    test
))]
fn rime_log_dir(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join("log")
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub fn librime_runtime_version() -> Option<String> {
    unsafe {
        let api = librime_sys::rime_get_api();
        let get_version = (*api).get_version?;
        let version = get_version();
        if version.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(version)
            .to_str()
            .ok()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
)))]
pub fn librime_runtime_version() -> Option<String> {
    None
}

// ── Native desktop engine (guarded at the module level) ──────────────────────

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
mod desktop {
    use super::*;
    use librime_sys::{rime_get_api, RimeCandidateListIterator, RimeTraits};
    use rime_api::{create_session, KeyEvent, KeyStatus};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::sync::{Mutex, OnceLock};

    // librime setup+initialize must run exactly once per process.
    static RIME_INITED: OnceLock<()> = OnceLock::new();
    static DEPLOY_RESULT: Mutex<Option<bool>> = Mutex::new(None);

    #[cfg(any(target_os = "android", target_os = "ios"))]
    extern "C" {
        // Static/mobile librime builds keep plugin modules dormant until required.
        #[link_name = "_Z23rime_require_module_luav"]
        fn rime_require_module_lua();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    extern "C" {
        fn dlopen(filename: *const c_char, flags: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }

    #[cfg(target_os = "windows")]
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const c_char) -> *mut c_void;
        fn GetModuleHandleExW(
            dw_flags: u32,
            lp_module_name: *const u16,
            ph_module: *mut *mut c_void,
        ) -> i32;
        fn GetModuleFileNameW(h_module: *mut c_void, lp_filename: *mut u16, n_size: u32) -> u32;
    }

    pub fn setup_only(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
        RIME_INITED.get_or_init(|| {
            let user_dir = Path::new(&user_data_dir);
            let shared_dir = Path::new(&shared_data_dir);
            let (staging_dir, prebuilt_dir) = rime_build_dirs(user_dir, shared_dir);
            let log_dir = rime_log_dir(user_dir);

            let _ = std::fs::create_dir_all(&staging_dir);
            let _ = std::fs::create_dir_all(&prebuilt_dir);
            let _ = std::fs::create_dir_all(&log_dir);

            setup_rime(
                &user_data_dir,
                &shared_data_dir,
                &staging_dir.to_string_lossy(),
                &prebuilt_dir.to_string_lossy(),
                &log_dir.to_string_lossy(),
            );
        });
        Ok(())
    }

    /// Initialize and fully deploy librime.
    /// `setup` + `initialize` run only on the first call; subsequent calls only
    /// re-run `full_deploy_and_wait` so that newly installed schemas are picked up.
    /// Blocking — run inside `tokio::task::spawn_blocking` when called from async code.
    pub fn deploy(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
        let log_dir = rime_log_dir(Path::new(&user_data_dir));

        setup_only(user_data_dir, shared_data_dir)?;
        if full_deploy_and_wait() {
            Ok(())
        } else {
            Err(format!(
                "Rime deployment failed. See librime logs in {}",
                log_dir.display()
            ))
        }
    }

    fn setup_rime(
        user_data_dir: &str,
        shared_data_dir: &str,
        staging_dir: &str,
        prebuilt_data_dir: &str,
        log_dir: &str,
    ) {
        let user_data_dir = CString::new(user_data_dir).expect("valid user data dir");
        let shared_data_dir = CString::new(shared_data_dir).expect("valid shared data dir");
        let staging_dir = CString::new(staging_dir).expect("valid staging dir");
        let prebuilt_data_dir = CString::new(prebuilt_data_dir).expect("valid prebuilt data dir");
        let log_dir = CString::new(log_dir).expect("valid log dir");
        let distribution_name = CString::new("KeyTao").unwrap();
        let distribution_code_name = CString::new("keytao").unwrap();
        let distribution_version = CString::new("1.0.0").unwrap();
        let app_name = CString::new("rime.keytao").unwrap();
        let module_default = CString::new("default").unwrap();
        let module_lua = CString::new("lua").unwrap();
        let mut modules = [
            module_default.as_ptr(),
            module_lua.as_ptr(),
            std::ptr::null::<c_char>(),
        ];

        librime_sys::rime_struct!(traits: RimeTraits);
        traits.user_data_dir = user_data_dir.as_ptr();
        traits.shared_data_dir = shared_data_dir.as_ptr();
        traits.staging_dir = staging_dir.as_ptr();
        traits.prebuilt_data_dir = prebuilt_data_dir.as_ptr();
        traits.log_dir = log_dir.as_ptr();
        traits.distribution_name = distribution_name.as_ptr();
        traits.distribution_code_name = distribution_code_name.as_ptr();
        traits.distribution_version = distribution_version.as_ptr();
        traits.app_name = app_name.as_ptr();
        traits.modules = modules.as_mut_ptr();

        unsafe {
            require_lua_module();

            let api = rime_get_api();
            if let Some(setup) = (*api).setup {
                setup(&mut traits);
            }
            if let Some(initialize) = (*api).initialize {
                initialize(&mut traits);
            }
            if let Some(set_notification_handler) = (*api).set_notification_handler {
                set_notification_handler(Some(notification_handler), std::ptr::null_mut());
            }
        }
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    unsafe fn require_lua_module() {
        rime_require_module_lua();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    unsafe fn require_lua_module() {
        if let Err(error) = load_unix_lua_plugin() {
            eprintln!(
                "KeyTao: failed to load {} librime-lua plugin: {error}",
                std::env::consts::OS
            );
        }
    }

    #[cfg(target_os = "windows")]
    unsafe fn require_lua_module() {
        if let Err(error) = load_windows_lua_plugin() {
            eprintln!("KeyTao: failed to load Windows librime-lua plugin: {error}");
        }
    }

    #[cfg(not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )))]
    unsafe fn require_lua_module() {}

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    unsafe fn load_unix_lua_plugin() -> Result<(), String> {
        const RTLD_NOW: i32 = 0x2;
        #[cfg(target_os = "macos")]
        const RTLD_GLOBAL: i32 = 0x8;
        #[cfg(target_os = "linux")]
        const RTLD_GLOBAL: i32 = 0x100;

        let candidates = unix_lua_plugin_candidates();
        let mut attempted = Vec::new();
        for path in &candidates {
            if !path.is_file() {
                continue;
            }
            let display = path.display().to_string();
            let path = CString::new(path.to_string_lossy().as_bytes())
                .map_err(|_| "plugin path contains NUL byte".to_string())?;
            let handle = dlopen(path.as_ptr(), RTLD_NOW | RTLD_GLOBAL);
            if handle.is_null() {
                attempted.push(format!("{display}: {}", dlerror_string()));
                continue;
            }
            if let Some(require) = find_unix_lua_require_symbol(handle) {
                let require: unsafe extern "C" fn() = std::mem::transmute(require);
                require();
                return Ok(());
            }
            attempted.push(format!("{display}: missing rime_require_module_lua symbol"));
        }
        lua_plugin_load_error(&candidates, &attempted)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn find_unix_lua_require_symbol(handle: *mut c_void) -> Option<*mut c_void> {
        for symbol in lua_require_symbol_names() {
            let symbol = CString::new(*symbol).ok()?;
            let require = unsafe { dlsym(handle, symbol.as_ptr()) };
            if !require.is_null() {
                return Some(require);
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    unsafe fn load_windows_lua_plugin() -> Result<(), String> {
        let candidates = windows_lua_plugin_candidates();
        let mut attempted = Vec::new();
        for path in &candidates {
            if !path.is_file() {
                continue;
            }
            let display = path.display().to_string();
            let path = path_to_wide(path);
            let handle = LoadLibraryW(path.as_ptr());
            if handle.is_null() {
                attempted.push(format!("{display}: {}", std::io::Error::last_os_error()));
                continue;
            }
            if let Some(require) = find_windows_lua_require_symbol(handle) {
                let require: unsafe extern "C" fn() = std::mem::transmute(require);
                require();
                return Ok(());
            }
            attempted.push(format!("{display}: missing rime_require_module_lua symbol"));
        }
        lua_plugin_load_error(&candidates, &attempted)
    }

    #[cfg(target_os = "windows")]
    fn find_windows_lua_require_symbol(handle: *mut c_void) -> Option<*mut c_void> {
        for symbol in lua_require_symbol_names() {
            let symbol = CString::new(*symbol).ok()?;
            let require = unsafe { GetProcAddress(handle, symbol.as_ptr()) };
            if !require.is_null() {
                return Some(require);
            }
        }
        None
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn unix_lua_plugin_candidates() -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(plugin_dir) = std::env::var("KEYTAO_RIME_PLUGIN_DIR") {
            push_lua_plugin_files(&mut candidates, Path::new(&plugin_dir));
        }
        if let Ok(lib_dir) = std::env::var("RIME_LIB_DIR") {
            let lib_dir = PathBuf::from(lib_dir);
            push_lua_plugin_files(&mut candidates, &lib_dir.join("rime-plugins"));
            push_lua_plugin_files(&mut candidates, &lib_dir);
        }
        append_platform_lua_plugin_candidates(&mut candidates);
        dedupe_paths(candidates)
    }

    #[cfg(target_os = "macos")]
    fn append_platform_lua_plugin_candidates(candidates: &mut Vec<PathBuf>) {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(contents_dir) = exe.parent().and_then(Path::parent) {
                let frameworks_dir = contents_dir.join("Frameworks");
                push_lua_plugin_files(candidates, &frameworks_dir.join("rime-plugins"));
                push_lua_plugin_files(candidates, &frameworks_dir);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn append_platform_lua_plugin_candidates(candidates: &mut Vec<PathBuf>) {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(bin_dir) = exe.parent() {
                for lib_dir in [
                    bin_dir.join("runtime/lib"),
                    bin_dir.join("runtime/lib64"),
                    bin_dir.join("resources/runtime/lib"),
                    bin_dir.join("resources/runtime/lib64"),
                    bin_dir.join("../runtime/lib"),
                    bin_dir.join("../runtime/lib64"),
                    bin_dir.join("../lib"),
                    bin_dir.join("../lib/keytao-app/runtime/lib"),
                    bin_dir.join("../lib/keytao-app/runtime/lib64"),
                    bin_dir.join("../lib/keytao-app/resources/runtime/lib"),
                    bin_dir.join("../lib/keytao-app/resources/runtime/lib64"),
                ] {
                    push_lua_plugin_files(candidates, &lib_dir.join("rime-plugins"));
                    push_lua_plugin_files(candidates, &lib_dir);
                }
            }
        }

        for lib_dir in linux_system_library_dirs() {
            push_lua_plugin_files(candidates, &lib_dir.join("rime-plugins"));
            push_lua_plugin_files(candidates, &lib_dir);
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_system_library_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![PathBuf::from("/usr/lib"), PathBuf::from("/usr/local/lib")];
        if let Ok(entries) = std::fs::read_dir("/usr/lib") {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if path.is_dir() && name.ends_with("-linux-gnu") {
                    dirs.push(path);
                }
            }
        }
        dirs.push(PathBuf::from("/usr/lib64"));
        dedupe_paths(dirs)
    }

    #[cfg(target_os = "windows")]
    fn windows_lua_plugin_candidates() -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(plugin_dir) = std::env::var("KEYTAO_RIME_PLUGIN_DIR") {
            push_lua_plugin_files(&mut candidates, Path::new(&plugin_dir));
        }
        if let Ok(lib_dir) = std::env::var("RIME_LIB_DIR") {
            let lib_dir = PathBuf::from(lib_dir);
            if let Some(prefix) = lib_dir.parent() {
                push_lua_plugin_files(&mut candidates, &prefix.join("bin"));
                push_lua_plugin_files(&mut candidates, &prefix.join("bin/rime-plugins"));
            }
            push_lua_plugin_files(&mut candidates, &lib_dir);
            push_lua_plugin_files(&mut candidates, &lib_dir.join("rime-plugins"));
        }
        if let Some(module_dir) = current_windows_module_dir() {
            append_windows_runtime_lua_plugin_candidates(&mut candidates, &module_dir);
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                append_windows_runtime_lua_plugin_candidates(&mut candidates, dir);
            }
        }
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                push_lua_plugin_files(&mut candidates, &dir);
                push_lua_plugin_files(&mut candidates, &dir.join("rime-plugins"));
            }
        }
        dedupe_paths(candidates)
    }

    #[cfg(target_os = "windows")]
    fn append_windows_runtime_lua_plugin_candidates(candidates: &mut Vec<PathBuf>, dir: &Path) {
        for plugin_dir in [
            dir.to_path_buf(),
            dir.join("rime-plugins"),
            dir.join("bin"),
            dir.join("bin/rime-plugins"),
            dir.join("lib"),
            dir.join("lib/rime-plugins"),
            dir.join("keytao-windows-ime-runtime/current"),
            dir.join("keytao-windows-ime-runtime/current/rime-plugins"),
            dir.join("resources/keytao-windows-ime-runtime/current"),
            dir.join("resources/keytao-windows-ime-runtime/current/rime-plugins"),
        ] {
            push_lua_plugin_files(candidates, &plugin_dir);
        }
    }

    #[cfg(target_os = "windows")]
    fn current_windows_module_dir() -> Option<PathBuf> {
        const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
        const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;

        let mut module = std::ptr::null_mut();
        let address = current_windows_module_dir as usize as *const u16;
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address,
                &mut module,
            )
        };
        if ok == 0 || module.is_null() {
            return None;
        }

        let mut buffer = vec![0u16; 32768];
        let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) }
            as usize;
        if len == 0 || len >= buffer.len() {
            return None;
        }
        buffer.truncate(len);
        PathBuf::from(String::from_utf16_lossy(&buffer))
            .parent()
            .map(Path::to_path_buf)
    }

    #[cfg(target_os = "macos")]
    fn lua_plugin_filenames() -> &'static [&'static str] {
        &["librime-lua.dylib"]
    }

    #[cfg(target_os = "linux")]
    fn lua_plugin_filenames() -> &'static [&'static str] {
        &["librime-lua.so"]
    }

    #[cfg(target_os = "windows")]
    fn lua_plugin_filenames() -> &'static [&'static str] {
        &["librime-lua.dll", "rime-lua.dll"]
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn lua_require_symbol_names() -> &'static [&'static str] {
        &[
            "_Z23rime_require_module_luav",
            "?rime_require_module_lua@@YAXXZ",
            "rime_require_module_lua",
        ]
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn push_lua_plugin_files(candidates: &mut Vec<PathBuf>, dir: &Path) {
        for filename in lua_plugin_filenames() {
            candidates.push(dir.join(filename));
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if path.is_file() && is_lua_plugin_filename(name) {
                    candidates.push(path);
                }
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn is_lua_plugin_filename(name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        name.contains("rime") && name.contains("lua") && lua_plugin_extension_matches(&name)
    }

    #[cfg(target_os = "macos")]
    fn lua_plugin_extension_matches(name: &str) -> bool {
        name.ends_with(".dylib")
    }

    #[cfg(target_os = "linux")]
    fn lua_plugin_extension_matches(name: &str) -> bool {
        name.contains(".so")
    }

    #[cfg(target_os = "windows")]
    fn lua_plugin_extension_matches(name: &str) -> bool {
        name.ends_with(".dll")
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut deduped = Vec::new();
        for path in paths {
            if !deduped.iter().any(|existing| existing == &path) {
                deduped.push(path);
            }
        }
        deduped
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn lua_plugin_load_error(candidates: &[PathBuf], attempted: &[String]) -> Result<(), String> {
        if attempted.is_empty() {
            Err(format!(
                "{} not found; checked: {}",
                lua_plugin_filenames().join(" or "),
                format_paths(candidates)
            ))
        } else {
            Err(format!(
                "could not load Lua plugin: {}",
                attempted.join("; ")
            ))
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn format_paths(paths: &[PathBuf]) -> String {
        if paths.is_empty() {
            return "(no candidate paths)".to_string();
        }
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[cfg(target_os = "windows")]
    fn path_to_wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    unsafe fn dlerror_string() -> String {
        let error = dlerror();
        if error.is_null() {
            return "unknown error".to_string();
        }
        CStr::from_ptr(error).to_string_lossy().into_owned()
    }

    extern "C" fn notification_handler(
        _obj: *mut c_void,
        _session_id: librime_sys::RimeSessionId,
        message_type: *const c_char,
        message_value: *const c_char,
    ) {
        let Some(message_type) = cstr_to_str(message_type) else {
            return;
        };
        let Some(message_value) = cstr_to_str(message_value) else {
            return;
        };
        if message_type == "deploy" {
            if let Ok(mut result) = DEPLOY_RESULT.lock() {
                match message_value.as_str() {
                    "success" => *result = Some(true),
                    "failure" => *result = Some(false),
                    _ => {}
                }
            }
        }
    }

    fn cstr_to_str(ptr: *const c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(ptr).to_str().ok().map(str::to_owned) }
    }

    fn full_deploy_and_wait() -> bool {
        if let Ok(mut result) = DEPLOY_RESULT.lock() {
            *result = None;
        }
        unsafe {
            let api = rime_get_api();
            let Some(start_maintenance) = (*api).start_maintenance else {
                return false;
            };
            if start_maintenance(1) == 0 {
                return false;
            }
            if let Some(join_maintenance_thread) = (*api).join_maintenance_thread {
                join_maintenance_thread();
            }
        }
        DEPLOY_RESULT
            .lock()
            .map(|result| *result == Some(true))
            .unwrap_or(false)
    }

    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    mod unix_lua_plugin_tests {
        use super::*;

        #[test]
        fn loads_lua_plugin_from_configured_rime_lib_dir() {
            if std::env::var("RIME_LIB_DIR").is_err() {
                return;
            }
            unsafe {
                load_unix_lua_plugin().expect("load librime-lua plugin");
            }
        }
    }

    /// An active rime input session.
    pub struct Engine {
        session: rime_api::Session,
    }

    // SAFETY: Session holds only a usize (session_id).
    // librime's C API is documented as thread-safe across different sessions.
    unsafe impl Send for Engine {}
    unsafe impl Sync for Engine {}

    fn key_event(keycode: u32, mask: u32) -> KeyEvent {
        KeyEvent::new(keycode as _, mask as _)
    }

    impl Engine {
        /// Create a new session. `deploy()` must have succeeded first.
        pub fn new() -> Result<Self, String> {
            Self::new_with_user_data_dir(None)
        }

        pub(crate) fn new_with_user_data_dir(user_data_dir: Option<&Path>) -> Result<Self, String> {
            let session = create_session().map_err(|e| format!("{e:?}"))?;
            select_preferred_schema(&session, user_data_dir);
            Ok(Self { session })
        }

        pub fn process_key(&self, keycode: u32, mask: u32) -> ImeState {
            self.process_key_result(keycode, mask).state
        }

        pub fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult {
            let status = self.session.process_key(key_event(keycode, mask));
            KeyProcessResult {
                state: extract_state(&self.session),
                accepted: matches!(status, KeyStatus::Accept),
            }
        }

        pub fn state(&self) -> ImeState {
            extract_state(&self.session)
        }

        pub fn select_candidate(&self, index: usize) -> ImeState {
            let state = extract_state(&self.session);
            let select_keys = state.select_keys.as_deref().unwrap_or("1234567890");
            if let Some(key) = select_keys.chars().nth(index) {
                self.session.process_key(key_event(key as u32, 0));
            }
            extract_state(&self.session)
        }

        pub fn select_candidate_global(&self, index: usize) -> ImeState {
            unsafe {
                let api = rime_get_api();
                if let Some(select_candidate) = (*api).select_candidate {
                    select_candidate(self.session.session_id, index);
                }
            }
            extract_state(&self.session)
        }

        pub fn all_candidates(&self) -> Vec<Candidate> {
            self.all_candidates_limited(usize::MAX)
        }

        pub fn all_candidates_limited(&self, max_count: usize) -> Vec<Candidate> {
            extract_all_candidates(&self.session, max_count).unwrap_or_default()
        }

        pub fn change_page(&self, backward: bool) -> ImeState {
            let kc = if backward { b'-' as u32 } else { b'=' as u32 };
            self.session.process_key(key_event(kc, 0));
            extract_state(&self.session)
        }

        pub fn reset(&self) -> ImeState {
            self.session.process_key(key_event(0xff1b, 0)); // XK_Escape
            extract_state(&self.session)
        }

        pub fn current_schema_name(&self) -> String {
            self.session
                .status()
                .map(|s| s.schema_name().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        }

        pub fn is_ascii_mode(&self) -> bool {
            self.session
                .status()
                .map(|s| s.is_ascii_mode)
                .unwrap_or(false)
        }

        pub fn set_ascii_mode(&self, enabled: bool) -> ImeState {
            set_session_option(&self.session, "ascii_mode", enabled);
            extract_state(&self.session)
        }
    }

    fn select_preferred_schema(session: &rime_api::Session, user_data_dir: Option<&Path>) {
        let Some(schema) = preferred_schema_id(user_data_dir) else {
            return;
        };
        if let Err(error) = session.select_schema(&schema) {
            eprintln!("KeyTao: failed to select preferred schema {schema}: {error:?}");
        } else {
            set_session_option(session, "ascii_mode", false);
        }
    }

    fn set_session_option(session: &rime_api::Session, option_name: &str, enabled: bool) {
        let Ok(option) = CString::new(option_name) else {
            return;
        };
        unsafe {
            let api = rime_get_api();
            if let Some(set_option) = (*api).set_option {
                set_option(session.session_id, option.as_ptr(), i32::from(enabled));
            }
        }
    }

    fn extract_state(session: &rime_api::Session) -> ImeState {
        let committed = session.commit().map(|c| c.text().to_string());

        let Some(ctx) = session.context() else {
            return ImeState {
                committed,
                ..ImeState::empty()
            };
        };

        let comp = ctx.composition();
        let preedit = comp.preedit.unwrap_or("").to_string();
        let cursor = comp.cursor_pos;

        let menu = ctx.menu();
        let candidates: Vec<Candidate> = menu
            .candidates
            .iter()
            .map(|c| Candidate {
                text: c.text.to_string(),
                comment: c.comment.map(|s: &str| s.to_string()),
            })
            .collect();

        let status = session.status().ok();
        let ascii_mode = status.as_ref().map(|s| s.is_ascii_mode).unwrap_or(false);
        let schema_name = status
            .as_ref()
            .map(|s| s.schema_name().to_string())
            .unwrap_or_default();

        ImeState {
            preedit,
            cursor,
            candidates,
            all_candidates: Vec::new(),
            highlighted_candidate_index: menu.highlighted_candidate_index,
            page_size: menu.page_size,
            page: menu.page_no,
            is_last_page: menu.is_last_page,
            committed,
            select_keys: menu.select_keys.map(|s: &str| s.to_string()),
            ascii_mode,
            schema_name,
        }
    }

    fn extract_all_candidates(
        session: &rime_api::Session,
        max_count: usize,
    ) -> Option<Vec<Candidate>> {
        if max_count == 0 {
            return Some(Vec::new());
        }
        unsafe {
            let api = rime_get_api();
            let candidate_list_begin = (*api).candidate_list_begin?;
            let candidate_list_next = (*api).candidate_list_next?;
            let candidate_list_end = (*api).candidate_list_end?;
            let mut iterator =
                std::mem::MaybeUninit::<RimeCandidateListIterator>::zeroed().assume_init();
            if candidate_list_begin(session.session_id, &mut iterator) == 0 {
                return None;
            }

            let mut candidates = Vec::new();
            loop {
                let text = candidate_string(iterator.candidate.text);
                let comment = candidate_optional_string(iterator.candidate.comment);
                if !text.is_empty() {
                    candidates.push(Candidate { text, comment });
                }
                if candidates.len() >= max_count {
                    break;
                }
                if candidate_list_next(&mut iterator) == 0 {
                    break;
                }
            }
            candidate_list_end(&mut iterator);
            Some(candidates)
        }
    }

    unsafe fn candidate_string(value: *mut std::os::raw::c_char) -> String {
        if value.is_null() {
            String::new()
        } else {
            CStr::from_ptr(value).to_string_lossy().into_owned()
        }
    }

    unsafe fn candidate_optional_string(value: *mut std::os::raw::c_char) -> Option<String> {
        if value.is_null() {
            None
        } else {
            let value = CStr::from_ptr(value).to_string_lossy().into_owned();
            (!value.is_empty()).then_some(value)
        }
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub use desktop::{deploy, setup_only, Engine};

pub const RIME_MOD_SHIFT: u32 = 0x0001;
pub const RIME_MOD_CONTROL: u32 = 0x0004;
pub const RIME_MOD_ALT: u32 = 0x0008;
pub const RIME_RELEASE_MASK: u32 = 1 << 30;

pub mod key_policy {
    use super::{ImeState, RIME_MOD_ALT, RIME_MOD_CONTROL};

    pub const XK_SPACE: u32 = 0x0020;
    pub const XK_BACK_SPACE: u32 = 0xff08;
    pub const XK_TAB: u32 = 0xff09;
    pub const XK_RETURN: u32 = 0xff0d;
    pub const XK_ESCAPE: u32 = 0xff1b;
    pub const XK_HOME: u32 = 0xff50;
    pub const XK_LEFT: u32 = 0xff51;
    pub const XK_UP: u32 = 0xff52;
    pub const XK_RIGHT: u32 = 0xff53;
    pub const XK_DOWN: u32 = 0xff54;
    pub const XK_PAGE_UP: u32 = 0xff55;
    pub const XK_PAGE_DOWN: u32 = 0xff56;
    pub const XK_END: u32 = 0xff57;
    pub const XK_DELETE: u32 = 0xffff;
    pub const XK_KP_ENTER: u32 = 0xff8d;
    pub const XK_F4: u32 = 0xffc1;

    pub fn is_enter_key(sym: u32) -> bool {
        matches!(sym, XK_RETURN | XK_KP_ENTER)
    }

    pub fn is_space_key(sym: u32) -> bool {
        sym == XK_SPACE
    }

    pub fn is_nonstarter_key(sym: u32) -> bool {
        matches!(
            sym,
            XK_SPACE | XK_BACK_SPACE | XK_DELETE | XK_TAB | XK_RETURN | XK_ESCAPE | XK_HOME
                ..=XK_END | XK_KP_ENTER
        )
    }

    pub fn should_bypass_empty_composition(sym: u32, mods: u32, state: &ImeState) -> bool {
        should_bypass_empty_composition_key(is_nonstarter_key(sym), mods, state)
    }

    pub fn should_bypass_empty_composition_key(
        is_nonstarter: bool,
        mods: u32,
        state: &ImeState,
    ) -> bool {
        if !state.preedit.is_empty() || !state.candidates.is_empty() {
            return false;
        }
        if mods & (RIME_MOD_CONTROL | RIME_MOD_ALT) != 0 {
            return true;
        }
        is_nonstarter
    }

    pub fn highlighted_candidate_index(state: &ImeState) -> Option<usize> {
        if state.candidates.is_empty() {
            None
        } else {
            Some(
                state
                    .highlighted_candidate_index
                    .min(state.candidates.len().saturating_sub(1)),
            )
        }
    }

    pub fn candidate_index_for_char(ch: char, state: &ImeState) -> Option<usize> {
        if state.candidates.is_empty() {
            return None;
        }
        let keys = state.select_keys.as_deref().unwrap_or("1234567890");
        keys.chars().position(|candidate_key| candidate_key == ch)
    }

    pub fn candidate_index_for_select_key(sym: u32, state: &ImeState) -> Option<usize> {
        let ch = char::from_u32(sym)?;
        candidate_index_for_char(ch, state)
    }

    pub fn candidate_index_for_space_or_select_key(sym: u32, state: &ImeState) -> Option<usize> {
        if is_space_key(sym) {
            highlighted_candidate_index(state)
        } else {
            candidate_index_for_select_key(sym, state)
        }
    }

    pub fn should_forward_consumed_shortcut(sym: u32, mods: u32) -> bool {
        let ctrl_held = mods & RIME_MOD_CONTROL != 0;
        ctrl_held && matches!(sym, 0x0060 | 0x007e)
    }
}

pub fn rime_modifier_mask(mask: u32) -> u32 {
    mask & (RIME_MOD_SHIFT | RIME_MOD_CONTROL | RIME_MOD_ALT | RIME_RELEASE_MASK)
}

#[cfg(test)]
mod ime_runtime_tests {
    use super::{
        key_policy, rime_modifier_mask, Candidate, ImeState, RIME_MOD_CONTROL, RIME_MOD_SHIFT,
        RIME_RELEASE_MASK,
    };

    #[test]
    fn rime_modifier_mask_strips_lock_and_pointer_modifiers() {
        assert_eq!(rime_modifier_mask(0x10), 0);
        assert_eq!(
            rime_modifier_mask(0x10 | RIME_MOD_SHIFT | RIME_MOD_CONTROL),
            RIME_MOD_SHIFT | RIME_MOD_CONTROL
        );
        assert_eq!(
            rime_modifier_mask(RIME_RELEASE_MASK | 0x10),
            RIME_RELEASE_MASK
        );
    }

    #[test]
    fn key_policy_bypasses_only_empty_composition_nonstarters() {
        let empty = ImeState::empty();
        assert!(key_policy::should_bypass_empty_composition(
            key_policy::XK_BACK_SPACE,
            0,
            &empty
        ));
        assert!(key_policy::should_bypass_empty_composition(
            b'a' as u32,
            RIME_MOD_CONTROL,
            &empty
        ));

        let mut composing = ImeState::empty();
        composing.preedit = "abc".to_owned();
        assert!(!key_policy::should_bypass_empty_composition(
            key_policy::XK_SPACE,
            0,
            &composing
        ));
    }

    #[test]
    fn key_policy_candidate_selection_requires_candidates() {
        let mut state = ImeState::empty();
        state.preedit = "ab".to_owned();
        assert_eq!(
            key_policy::candidate_index_for_space_or_select_key(key_policy::XK_SPACE, &state),
            None
        );

        state.candidates = vec![
            Candidate {
                text: "first".to_owned(),
                comment: None,
            },
            Candidate {
                text: "second".to_owned(),
                comment: None,
            },
        ];
        state.highlighted_candidate_index = 9;
        assert_eq!(
            key_policy::candidate_index_for_space_or_select_key(key_policy::XK_SPACE, &state),
            Some(1)
        );
        assert_eq!(
            key_policy::candidate_index_for_select_key(b'2' as u32, &state),
            Some(1)
        );
    }

    #[test]
    fn key_policy_forward_consumed_ctrl_grave() {
        assert!(key_policy::should_forward_consumed_shortcut(
            b'`' as u32,
            RIME_MOD_CONTROL
        ));
        assert!(key_policy::should_forward_consumed_shortcut(
            b'~' as u32,
            RIME_MOD_CONTROL
        ));
        assert!(!key_policy::should_forward_consumed_shortcut(
            b'a' as u32,
            RIME_MOD_CONTROL
        ));
    }

    #[test]
    fn key_policy_does_not_bypass_rime_menu_key() {
        assert!(!key_policy::should_bypass_empty_composition(
            key_policy::XK_F4,
            0,
            &ImeState::empty()
        ));
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
#[derive(Clone)]
pub struct ImeRuntime(Arc<ImeRuntimeState>);

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
#[derive(Clone)]
pub struct ImeRuntimeSession {
    shared: Arc<ImeRuntimeState>,
    inner: Arc<Mutex<ImeRuntimeSessionInner>>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
struct ImeRuntimeState {
    initialized: Mutex<bool>,
    generation: AtomicU64,
    user_data_dir: Option<PathBuf>,
    shared_data_dir: Option<String>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
struct ImeRuntimeSessionInner {
    engine: Engine,
    generation: u64,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
impl ImeRuntime {
    pub fn new() -> Self {
        Self::with_optional_dirs(None, None)
    }

    pub fn with_dirs(
        user_data_dir: impl Into<PathBuf>,
        shared_data_dir: impl Into<String>,
    ) -> Self {
        Self::with_optional_dirs(Some(user_data_dir.into()), Some(shared_data_dir.into()))
    }

    fn with_optional_dirs(user_data_dir: Option<PathBuf>, shared_data_dir: Option<String>) -> Self {
        Self(Arc::new(ImeRuntimeState {
            initialized: Mutex::new(false),
            generation: AtomicU64::new(0),
            user_data_dir,
            shared_data_dir,
        }))
    }

    pub fn init(&self) -> Result<(), String> {
        let mut initialized = self.0.initialized.lock().unwrap();
        if *initialized {
            return Ok(());
        }

        self.deploy_locked()?;
        *initialized = true;
        Ok(())
    }

    pub fn init_without_deploy(&self) -> Result<(), String> {
        let mut initialized = self.0.initialized.lock().unwrap();
        if *initialized {
            return Ok(());
        }

        self.setup_locked()?;
        *initialized = true;
        Ok(())
    }

    pub fn reload(&self) -> Result<(), String> {
        let mut initialized = self.0.initialized.lock().unwrap();
        self.deploy_locked()?;
        *initialized = true;
        self.0.generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn configured_dirs(&self) -> Result<(PathBuf, String), String> {
        let user_dir = self
            .0
            .user_data_dir
            .clone()
            .or_else(default_user_data_dir)
            .ok_or("cannot determine keytao data directory")?;
        let shared = self
            .0
            .shared_data_dir
            .clone()
            .unwrap_or_else(default_shared_data_dir);
        Ok((user_dir, shared))
    }

    fn setup_locked(&self) -> Result<(), String> {
        let (user_dir, shared) = self.configured_dirs()?;
        setup_only(user_dir.to_string_lossy().into_owned(), shared)
    }

    fn deploy_locked(&self) -> Result<(), String> {
        let (user_dir, shared) = self.configured_dirs()?;
        deploy(user_dir.to_string_lossy().into_owned(), shared)
    }

    pub fn create_session(&self) -> Result<ImeRuntimeSession, String> {
        let initialized = *self.0.initialized.lock().unwrap();
        if !initialized {
            self.init()?;
        }
        let generation = self.0.generation.load(Ordering::SeqCst);
        Ok(ImeRuntimeSession {
            shared: self.0.clone(),
            inner: Arc::new(Mutex::new(ImeRuntimeSessionInner {
                engine: Engine::new_with_user_data_dir(self.0.user_data_dir.as_deref())?,
                generation,
            })),
        })
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
impl Default for ImeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
impl ImeRuntimeSession {
    pub fn state(&self) -> ImeState {
        let mut inner = self.inner.lock().unwrap();
        if self.refresh_if_needed(&mut inner).is_err() {
            return ImeState::empty();
        }
        inner.engine.state()
    }

    pub fn process_key_result(&self, keycode: u32, mask: u32) -> Option<KeyProcessResult> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(
            inner
                .engine
                .process_key_result(keycode, rime_modifier_mask(mask)),
        )
    }

    pub fn select_candidate(&self, index: usize) -> Option<ImeState> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.select_candidate(index))
    }

    pub fn select_candidate_global(&self, index: usize) -> Option<ImeState> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.select_candidate_global(index))
    }

    pub fn all_candidates(&self) -> Option<Vec<Candidate>> {
        let mut inner = self.inner.lock().ok()?;
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.all_candidates())
    }

    pub fn all_candidates_limited(&self, max_count: usize) -> Option<Vec<Candidate>> {
        let mut inner = self.inner.lock().ok()?;
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.all_candidates_limited(max_count))
    }

    pub fn change_page(&self, backward: bool) -> Option<ImeState> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.change_page(backward))
    }

    pub fn reset(&self) -> Option<ImeState> {
        let mut inner = self.inner.lock().unwrap();
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.reset())
    }

    pub fn is_ascii_mode(&self) -> bool {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return false,
        };
        if self.refresh_if_needed(&mut inner).is_err() {
            return false;
        }
        inner.engine.is_ascii_mode()
    }

    pub fn set_ascii_mode(&self, enabled: bool) -> Option<ImeState> {
        let mut inner = self.inner.lock().ok()?;
        self.refresh_if_needed(&mut inner).ok()?;
        Some(inner.engine.set_ascii_mode(enabled))
    }

    fn refresh_if_needed(&self, inner: &mut ImeRuntimeSessionInner) -> Result<(), String> {
        let current = self.shared.generation.load(Ordering::SeqCst);
        if inner.generation == current {
            return Ok(());
        }
        inner.engine = Engine::new_with_user_data_dir(self.shared.user_data_dir.as_deref())?;
        inner.generation = current;
        Ok(())
    }
}

fn is_default_custom(filename: &str) -> bool {
    filename == "default.custom.yaml" || filename == "default-custom.yaml"
}

fn read_optional_default_custom(base: &Path) -> Option<String> {
    std::fs::read_to_string(base.join("default.custom.yaml"))
        .ok()
        .or_else(|| std::fs::read_to_string(base.join("default-custom.yaml")).ok())
}

fn preferred_schema_id(user_data_dir: Option<&Path>) -> Option<String> {
    if let Some(dir) = user_data_dir {
        if let Some(schema) = preferred_schema_id_from_dir(dir) {
            return Some(schema);
        }
    }
    default_user_data_dir().and_then(|dir| preferred_schema_id_from_dir(&dir))
}

fn preferred_schema_id_from_dir(dir: &Path) -> Option<String> {
    [
        dir.join("default.custom.yaml"),
        dir.join("default-custom.yaml"),
        dir.join("build/default.yaml"),
        dir.join("default.yaml"),
    ]
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .find_map(|content| preferred_schema_from_list(parse_schema_list(&content)))
}

fn preferred_schema_from_list(schemas: Vec<String>) -> Option<String> {
    let mut first_schema = None;
    for schema in schemas {
        if schema.trim().is_empty() {
            continue;
        }
        if first_schema.is_none() {
            first_schema = Some(schema.clone());
        }
        if is_keytao_managed_schema(&schema) {
            return Some(schema);
        }
    }
    first_schema
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
fn has_base_default_yaml(dir: &Path) -> bool {
    dir.join("default.yaml").is_file()
}

#[cfg(target_os = "linux")]
fn nix_store_rime_data_dirs() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir("/nix/store")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .into_string()
                .ok()
                .map(|name| (name, entry.path()))
        })
        .filter(|(name, _)| !name.ends_with(".drv") && name.contains("-rime-data-"))
        .map(|(_, path)| path.join("share/rime-data"))
        .filter(|path| has_base_default_yaml(path))
        .collect();
    paths.sort();
    paths.reverse();
    paths
}

pub fn parse_schema_list(content: &str) -> Vec<String> {
    let mut schemas = Vec::new();
    let mut in_list = false;
    for line in content.lines() {
        let t = line.trim();
        if t.contains("schema_list:") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(rest) = t.strip_prefix("- schema:") {
                let schema = clean_yaml_scalar(rest);
                if !schema.is_empty() {
                    schemas.push(schema);
                }
            } else if !t.is_empty() && !t.starts_with('#') && !t.starts_with('-') {
                in_list = false;
            }
        }
    }
    schemas
}

fn clean_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        let quote = trimmed.chars().next().unwrap();
        return trimmed[1..]
            .find(quote)
            .map(|end| trimmed[1..1 + end].to_string())
            .unwrap_or_else(|| trimmed[1..].to_string());
    }
    trimmed.split_once('#').map_or(trimmed, |(head, _)| head).trim().to_string()
}

fn schema_list_from_yaml(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Sequence(entries)) = value else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| match entry {
            Value::Mapping(mapping) => mapping
                .get(Value::String("schema".to_string()))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            Value::String(schema) => Some(schema.clone()),
            _ => None,
        })
        .collect()
}

fn make_schema_list_value(schemas: &[String]) -> Value {
    Value::Sequence(
        schemas
            .iter()
            .map(|schema| {
                let mut mapping = Mapping::new();
                mapping.insert(
                    Value::String("schema".to_string()),
                    Value::String(schema.clone()),
                );
                Value::Mapping(mapping)
            })
            .collect(),
    )
}

fn is_keytao_managed_schema(schema: &str) -> bool {
    ["keytao", "txjx", "xmjd6", "keydo"]
        .iter()
        .any(|prefix| schema.starts_with(prefix))
}

fn dedupe_schemas(schemas: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    schemas
        .into_iter()
        .filter(|schema| !schema.trim().is_empty())
        .filter(|schema| seen.insert(schema.clone()))
        .collect()
}

fn is_managed_default_patch_key(key: &str) -> bool {
    matches!(
        key,
        "schema_list"
            | "switcher"
            | "menu"
            | "ascii_composer"
            | "recognizer"
            | "key_binder"
            | "punctuator"
            | "selector"
    ) || key.starts_with("menu/")
        || key.starts_with("ascii_composer/")
        || key.starts_with("recognizer/")
        || key.starts_with("key_binder/")
        || key.starts_with("punctuator/")
        || key.starts_with("selector/")
}

fn merge_yaml_mapping(existing: &Mapping, package: &Mapping, inside_patch: bool) -> Mapping {
    let mut merged = package.clone();

    for (key, existing_value) in existing {
        let key_name = key.as_str();
        match (key_name, package.get(key)) {
            (Some("schema_list"), Some(package_value)) => {
                let package_schemas = schema_list_from_yaml(Some(package_value));
                let user_schemas: Vec<String> = schema_list_from_yaml(Some(existing_value))
                    .into_iter()
                    .filter(|schema| !is_keytao_managed_schema(schema))
                    .collect();
                let merged_schemas =
                    dedupe_schemas(user_schemas.iter().chain(package_schemas.iter()).cloned());
                merged.insert(key.clone(), make_schema_list_value(&merged_schemas));
            }
            (Some(key_name), Some(_)) if inside_patch && is_managed_default_patch_key(key_name) => {
            }
            (Some(key_name), None) if inside_patch && is_managed_default_patch_key(key_name) => {}
            (_, Some(Value::Mapping(package_map))) => {
                if let Value::Mapping(existing_map) = existing_value {
                    merged.insert(
                        key.clone(),
                        Value::Mapping(merge_yaml_mapping(
                            existing_map,
                            package_map,
                            key_name == Some("patch"),
                        )),
                    );
                }
            }
            (_, Some(_)) => {}
            (_, None) => {
                merged.insert(key.clone(), existing_value.clone());
            }
        }
    }

    merged
}

fn string_merge_default_custom(
    existing: Option<&str>,
    package_content: &str,
) -> (String, Vec<String>) {
    let package_schemas = parse_schema_list(package_content);
    let user_schemas: Vec<String> = existing
        .map(|content| {
            parse_schema_list(content)
                .into_iter()
                .filter(|schema| !is_keytao_managed_schema(schema))
                .collect()
        })
        .unwrap_or_default();
    let merged_schemas = dedupe_schemas(user_schemas.iter().chain(package_schemas.iter()).cloned());

    let mut out = String::new();
    let mut in_list = false;
    for line in package_content.lines() {
        let t = line.trim();
        if !in_list {
            out.push_str(line);
            out.push('\n');
            if t.contains("schema_list:") {
                in_list = true;
                for schema in &merged_schemas {
                    out.push_str(&format!("    - schema: {schema}\n"));
                }
            }
        } else if t.starts_with("- schema:") {
        } else {
            in_list = false;
            out.push_str(line);
            out.push('\n');
        }
    }

    (out, user_schemas)
}

pub fn merge_default_custom_content(
    existing: Option<&str>,
    package_content: &str,
) -> Result<(String, Vec<String>), String> {
    let package_yaml = match serde_yaml::from_str::<Value>(package_content) {
        Ok(Value::Mapping(mapping)) => mapping,
        _ => return Ok(string_merge_default_custom(existing, package_content)),
    };

    let user_schemas: Vec<String> = existing
        .map(parse_schema_list)
        .unwrap_or_default()
        .into_iter()
        .filter(|schema| !is_keytao_managed_schema(schema))
        .collect();

    let merged_yaml = if let Some(existing) = existing {
        match serde_yaml::from_str::<Value>(existing) {
            Ok(Value::Mapping(existing_mapping)) => {
                Value::Mapping(merge_yaml_mapping(&existing_mapping, &package_yaml, false))
            }
            _ => Value::Mapping(package_yaml.clone()),
        }
    } else {
        Value::Mapping(package_yaml.clone())
    };

    let mut merged = serde_yaml::to_string(&merged_yaml).map_err(|e| e.to_string())?;
    if let Some(stripped) = merged.strip_prefix("---\n") {
        merged = stripped.to_string();
    }

    Ok((merged, user_schemas))
}

fn extract_lua_require(line: &str) -> Option<String> {
    let pos = line.find("require")?;
    let after = line[pos + 7..].trim_start();
    if !after.starts_with('(') {
        return None;
    }
    let after = after[1..].trim_start();
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let content = &after[1..];
    let end = content.find(quote)?;
    Some(content[..end].to_string())
}

pub fn parse_rime_lua_requires(content: &str) -> Vec<String> {
    let mut requires = Vec::new();
    let mut in_block_comment = false;
    for line in content.lines() {
        let t = line.trim();
        if in_block_comment {
            if t.contains("--]]") {
                in_block_comment = false;
            }
            continue;
        }
        if t.starts_with("--[[") {
            in_block_comment = true;
            continue;
        }
        if t.is_empty() || t.starts_with("--") {
            continue;
        }
        if let Some(module) = extract_lua_require(t) {
            if !requires.contains(&module) {
                requires.push(module);
            }
        }
    }
    requires
}

pub fn merge_rime_lua_content(
    local_content: Option<&str>,
    package_content: &str,
    package_lua_filenames: &HashSet<String>,
) -> (String, Vec<(String, String)>) {
    let Some(local_content) = local_content else {
        return (package_content.to_string(), Vec::new());
    };

    let package_requires: HashSet<String> = parse_rime_lua_requires(package_content)
        .into_iter()
        .collect();
    let mut renames = Vec::new();
    let mut extra_lines = Vec::new();
    let mut in_block_comment = false;

    for line in local_content.lines() {
        let t = line.trim();
        if in_block_comment {
            if t.contains("--]]") {
                in_block_comment = false;
            }
            continue;
        }
        if t.starts_with("--[[") {
            in_block_comment = true;
            continue;
        }
        if t.is_empty() || t.starts_with("--") {
            continue;
        }
        if let Some(module) = extract_lua_require(t) {
            if package_requires.contains(&module) {
                continue;
            }
            let filename = format!("{module}.lua");
            if package_lua_filenames.contains(&filename) {
                let new_name = format!("{module}_user");
                let new_line = line
                    .replace(&format!("\"{}\"", module), &format!("\"{}\"", new_name))
                    .replace(&format!("'{}'", module), &format!("'{}'", new_name));
                renames.push((module, new_name));
                extra_lines.push(new_line);
            } else {
                extra_lines.push(line.to_string());
            }
        } else {
            extra_lines.push(line.to_string());
        }
    }

    let mut merged = package_content.to_string();
    if !extra_lines.is_empty() {
        if !merged.ends_with('\n') {
            merged.push('\n');
        }
        for line in &extra_lines {
            merged.push_str(line);
            merged.push('\n');
        }
    }

    (merged, renames)
}

pub fn sync_user_rime_assets(user_data_dir: &Path, shared_data_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(user_data_dir).map_err(|e| format!("create user dir: {e}"))?;

    let package_default_custom = std::fs::read_dir(shared_data_dir).ok().and_then(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .find(|entry| is_default_custom(&entry.file_name().to_string_lossy()))
            .and_then(|entry| std::fs::read_to_string(entry.path()).ok())
    });

    if let Some(package_content) = package_default_custom {
        let existing = read_optional_default_custom(user_data_dir);
        let (merged, _) = merge_default_custom_content(existing.as_deref(), &package_content)?;
        std::fs::write(user_data_dir.join("default.custom.yaml"), merged)
            .map_err(|e| format!("write default.custom.yaml: {e}"))?;
    }

    let package_rime_lua = std::fs::read_to_string(shared_data_dir.join("rime.lua")).ok();
    if let Some(package_content) = package_rime_lua {
        let package_lua_filenames: HashSet<String> = std::fs::read_dir(shared_data_dir.join("lua"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_file() {
                    Some(entry.file_name().to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .collect();

        let local_content = std::fs::read_to_string(user_data_dir.join("rime.lua")).ok();
        let (merged, renames) = merge_rime_lua_content(
            local_content.as_deref(),
            &package_content,
            &package_lua_filenames,
        );

        if !renames.is_empty() {
            let user_lua_dir = user_data_dir.join("lua");
            std::fs::create_dir_all(&user_lua_dir).map_err(|e| format!("create lua dir: {e}"))?;
            for (old_name, new_name) in renames {
                let old_path = user_lua_dir.join(format!("{old_name}.lua"));
                let new_path = user_lua_dir.join(format!("{new_name}.lua"));
                if !new_path.exists() && old_path.exists() {
                    let bytes = std::fs::read(&old_path)
                        .map_err(|e| format!("read lua/{old_name}.lua: {e}"))?;
                    std::fs::write(&new_path, bytes)
                        .map_err(|e| format!("write lua/{new_name}.lua: {e}"))?;
                }
            }
        }

        std::fs::write(user_data_dir.join("rime.lua"), merged)
            .map_err(|e| format!("write rime.lua: {e}"))?;
    }

    Ok(())
}

// ── Platform path helpers (all platforms) ────────────────────────────────────

/// Dedicated keytao user data directory for this platform.
pub fn default_user_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|h| h.join("Library/keytao"));
    }
    #[cfg(target_os = "windows")]
    {
        return dirs::config_dir().map(|c| c.join("keytao"));
    }
    #[cfg(target_os = "linux")]
    {
        return dirs::data_local_dir().map(|d| d.join("keytao"));
    }
    #[cfg(target_os = "android")]
    {
        return dirs::data_local_dir().map(|d| d.join("keytao"));
    }
    #[cfg(target_os = "ios")]
    {
        return dirs::data_local_dir().map(|d| d.join("keytao"));
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        None
    }
}

/// Best-guess shared rime data directory (system-installed schemas/essay.txt).
pub fn default_shared_data_dir() -> String {
    #[cfg(target_os = "macos")]
    {
        for key in [
            "KEYTAO_RIME_SHARED_DATA_DIR",
            "RIME_SHARED_DATA_DIR",
            "RIME_DATA_DIR",
        ] {
            if let Ok(value) = std::env::var(key) {
                let value = value.trim();
                if !value.is_empty() && has_base_default_yaml(Path::new(value)) {
                    return value.to_string();
                }
            }
        }

        let squirrel = "/Library/Input Methods/Squirrel.app/Contents/SharedSupport";
        if has_base_default_yaml(Path::new(squirrel)) {
            return squirrel.to_string();
        }
        for p in [
            "/opt/homebrew/share/rime-data",
            "/usr/local/share/rime-data",
        ] {
            if has_base_default_yaml(Path::new(p)) {
                return p.to_string();
            }
        }
        return String::new();
    }
    #[cfg(target_os = "linux")]
    {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();

        for key in [
            "KEYTAO_RIME_SHARED_DATA_DIR",
            "RIME_SHARED_DATA_DIR",
            "RIME_DATA_DIR",
        ] {
            if let Ok(value) = std::env::var(key) {
                let value = value.trim();
                if !value.is_empty() {
                    candidates.push(PathBuf::from(value));
                }
            }
        }

        if let Ok(lib_dir) = std::env::var("RIME_LIB_DIR") {
            let lib_dir = PathBuf::from(lib_dir);
            if let Some(prefix) = lib_dir.parent() {
                candidates.push(prefix.join("share/rime-data"));
            }
        }

        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(bin_dir) = current_exe.parent() {
                candidates.extend([
                    bin_dir.join("runtime/rime-data"),
                    bin_dir.join("resources/runtime/rime-data"),
                    bin_dir.join("../runtime/rime-data"),
                    bin_dir.join("../lib/keytao-app/runtime/rime-data"),
                    bin_dir.join("../lib/keytao-app/resources/runtime/rime-data"),
                ]);
            }
        }

        if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
            for base in xdg_data_dirs.split(':').filter(|part| !part.is_empty()) {
                candidates.push(PathBuf::from(base).join("rime-data"));
            }
        }

        candidates.extend(nix_store_rime_data_dirs());

        candidates.extend([
            PathBuf::from("/run/current-system/sw/share/rime-data"),
            PathBuf::from("/usr/local/share/rime-data"),
            PathBuf::from("/usr/share/rime-data"),
        ]);

        for path in candidates {
            if !seen.insert(path.clone()) {
                continue;
            }
            if has_base_default_yaml(&path) {
                return path.to_string_lossy().into_owned();
            }
        }
        return "/usr/share/rime-data".to_string();
    }
    #[cfg(target_os = "windows")]
    {
        let mut candidates = Vec::new();

        for key in [
            "KEYTAO_RIME_SHARED_DATA_DIR",
            "RIME_SHARED_DATA_DIR",
            "RIME_DATA_DIR",
        ] {
            if let Ok(value) = std::env::var(key) {
                let value = value.trim();
                if !value.is_empty() {
                    candidates.push(PathBuf::from(value));
                }
            }
        }

        if let Ok(root) = std::env::var("WEASEL_ROOT") {
            candidates.push(PathBuf::from(root).join("data"));
        }

        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates.push(
                PathBuf::from(&program_files)
                    .join("KeyTao")
                    .join("rime-data"),
            );
            candidates.push(
                PathBuf::from(&program_files)
                    .join("KeyTao")
                    .join("share")
                    .join("rime-data"),
            );
        }

        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(
                PathBuf::from(&program_files_x86)
                    .join("KeyTao")
                    .join("rime-data"),
            );
            candidates.push(
                PathBuf::from(&program_files_x86)
                    .join("KeyTao")
                    .join("share")
                    .join("rime-data"),
            );
        }

        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates.push(
                PathBuf::from(program_files)
                    .join("Rime")
                    .join("weasel-data"),
            );
        }

        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(
                PathBuf::from(program_files_x86)
                    .join("Rime")
                    .join("weasel-data"),
            );
        }

        candidates.extend([
            PathBuf::from(r"C:\Program Files\KeyTao\rime-data"),
            PathBuf::from(r"C:\Program Files\KeyTao\share\rime-data"),
            PathBuf::from(r"C:\Program Files\Rime\weasel-data"),
            PathBuf::from(r"C:\Program Files (x86)\Rime\weasel-data"),
        ]);

        for path in candidates {
            if path.join("default.yaml").is_file() {
                return path.to_string_lossy().into_owned();
            }
        }

        return r"C:\Program Files\Rime\weasel-data".to_string();
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        for key in [
            "KEYTAO_RIME_SHARED_DATA_DIR",
            "RIME_SHARED_DATA_DIR",
            "RIME_DATA_DIR",
        ] {
            if let Ok(value) = std::env::var(key) {
                let value = value.trim();
                if !value.is_empty() && has_base_default_yaml(Path::new(value)) {
                    return value.to_string();
                }
            }
        }

        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(bin_dir) = current_exe.parent() {
                for path in [
                    bin_dir.join("rime-data"),
                    bin_dir.join("runtime/rime-data"),
                    bin_dir.join("resources/rime-data"),
                    bin_dir.join("resources/runtime/rime-data"),
                ] {
                    if has_base_default_yaml(&path) {
                        return path.to_string_lossy().into_owned();
                    }
                }
            }
        }

        return String::new();
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        String::new()
    }
}

/// Returns true if `dir` exists and contains at least one `.schema.yaml` file.
pub fn has_schemas(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with(".schema.yaml"))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        merge_default_custom_content, merge_rime_lua_content, parse_rime_lua_requires,
        parse_schema_list, preferred_schema_id_from_dir, rime_build_dirs, rime_log_dir,
    };
    use std::collections::HashSet;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_schema_list_reads_schema_entries() {
        let content = "patch:\n  schema_list:\n    - schema: keytao\n    - schema: foo\n";
        assert_eq!(parse_schema_list(content), vec!["keytao", "foo"]);
    }

    #[test]
    fn parse_schema_list_strips_inline_comments() {
        let content = "patch:\n  schema_list:\n    - schema: keydo # 键道·我流\n";
        assert_eq!(parse_schema_list(content), vec!["keydo"]);
    }

    #[test]
    fn preferred_schema_id_reads_current_user_schema() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-core-schema-test-{suffix}"));
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(
            dir.join("build/default.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: user_schema\n    - schema: xmjd6\n",
        )
        .unwrap();

        assert_eq!(
            preferred_schema_id_from_dir(&dir),
            Some("xmjd6".to_string())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn merge_default_custom_keeps_user_schemas() {
        let existing =
            "patch:\n  schema_list:\n    - schema: user_schema\n    - schema: keytao_old\n    - schema: txjx\n";
        let package = "patch:\n  schema_list:\n    - schema: keytao\n    - schema: keytao-dz\n";
        let (merged, user) = merge_default_custom_content(Some(existing), package).unwrap();
        assert_eq!(user, vec!["user_schema"]);
        assert!(merged.contains("- schema: user_schema"));
        assert!(merged.contains("- schema: keytao"));
        assert!(merged.contains("- schema: keytao-dz"));
        assert!(!merged.contains("keytao_old"));
    }

    #[test]
    fn merge_default_custom_accepts_non_keytao_package_schemas() {
        let existing =
            "patch:\n  schema_list:\n    - schema: user_schema\n    - schema: keytao\n    - schema: xmjd6\n";
        let package = "patch:\n  schema_list:\n    - schema: txjx\n";
        let (merged, user) = merge_default_custom_content(Some(existing), package).unwrap();
        assert_eq!(user, vec!["user_schema"]);
        assert!(merged.contains("- schema: user_schema"));
        assert!(merged.contains("- schema: txjx"));
        assert!(!merged.contains("- schema: keytao"));
        assert!(!merged.contains("- schema: xmjd6"));
    }

    #[test]
    fn merge_default_custom_keeps_user_keys_and_replaces_managed_patch_keys() {
        let existing = "patch:\n  custom_user_patch: true\n  menu:\n    page_size: 9\n  ascii_composer:\n    switch_key:\n      Caps_Lock: noop\n  ascii_composer/good_old_caps_lock: true\n  schema_list:\n    - schema: user_schema\n    - schema: keydo\n";
        let package = "patch:\n  switcher:\n    caption: current\n  menu:\n    page_size: 6\n  schema_list:\n    - schema: txjx\n";
        let (merged, _) = merge_default_custom_content(Some(existing), package).unwrap();
        assert!(merged.contains("custom_user_patch: true"));
        assert!(merged.contains("page_size: 6"));
        assert!(merged.contains("- schema: user_schema"));
        assert!(merged.contains("- schema: txjx"));
        assert!(!merged.contains("Caps_Lock"));
        assert!(!merged.contains("good_old_caps_lock"));
        assert!(!merged.contains("- schema: keydo"));
    }

    #[test]
    fn parse_rime_lua_requires_skips_block_comments() {
        let content = "--[[\nfoo = require(\"bar\")\n--]]\nreal = require(\"real\")\n";
        assert_eq!(parse_rime_lua_requires(content), vec!["real"]);
    }

    #[test]
    fn merge_rime_lua_appends_user_module() {
        let local = "my_mod = require(\"my_mod\")\n";
        let package = "keytao_filter = require(\"keytao_filter\")\n";
        let (merged, renames) = merge_rime_lua_content(Some(local), package, &HashSet::new());
        assert!(merged.contains("require(\"keytao_filter\")"));
        assert!(merged.contains("require(\"my_mod\")"));
        assert!(renames.is_empty());
    }

    #[test]
    fn merge_rime_lua_renames_conflicting_user_module() {
        let local = "my_mod = require(\"my_mod\")\n";
        let package = "keytao = require(\"keytao\")\n";
        let package_files: HashSet<String> = ["my_mod.lua".to_string()].into();
        let (merged, renames) = merge_rime_lua_content(Some(local), package, &package_files);
        assert_eq!(
            renames,
            vec![("my_mod".to_string(), "my_mod_user".to_string())]
        );
        assert!(merged.contains("require(\"my_mod_user\")"));
    }

    #[test]
    fn same_root_user_and_shared_use_separate_build_dirs() {
        let root = Path::new("/tmp/keytao");
        let (staging, prebuilt) = rime_build_dirs(root, root);
        assert_eq!(staging, Path::new("/tmp/keytao/build"));
        assert_eq!(prebuilt, Path::new("/tmp/keytao/prebuilt"));
    }

    #[test]
    fn rime_logs_are_written_under_dedicated_keytao_dir() {
        let root = Path::new("/tmp/keytao");
        assert_eq!(rime_log_dir(root), Path::new("/tmp/keytao/log"));
    }
}
