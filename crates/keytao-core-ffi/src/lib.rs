use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(not(target_os = "android"))]
use keytao_core::{ImeRuntime, ImeRuntimeSession, ImeState, KeyProcessResult};
#[cfg(not(target_os = "android"))]
use keytao_theme::{
    resolve_theme_from_paths, resolve_theme_from_paths_with_system_scheme, resolved_theme_json,
    EffectiveColorScheme,
};

// ── C-compatible state struct ─────────────────────────────────────────────────

/// Flat view of IME state returned to C callers.
/// All strings are null-terminated UTF-8. Free with keytao_free_state().
#[repr(C)]
pub struct KeytaoState {
    pub preedit: *mut c_char,
    pub cursor: u32,
    pub candidate_texts: *mut *mut c_char,
    pub candidate_comments: *mut *mut c_char,
    pub candidate_count: u32,
    pub highlighted_candidate_index: u32,
    pub page: u32,
    pub is_last_page: bool,
    pub committed: *mut c_char,
    pub select_keys: *mut c_char,
    pub ascii_mode: bool,
    pub accepted: bool,
}

// ── Module-level singleton runtime session ────────────────────────────────────

#[cfg(not(target_os = "android"))]
struct Global {
    initialized: bool,
    runtime: Option<ImeRuntime>,
    singleton_session: Option<ImeRuntimeSession>,
}

#[cfg(not(target_os = "android"))]
static GLOBAL: Mutex<Global> = Mutex::new(Global {
    initialized: false,
    runtime: None,
    singleton_session: None,
});

#[cfg(not(target_os = "android"))]
struct SessionHandle {
    session: ImeRuntimeSession,
}

#[cfg(not(target_os = "android"))]
static THEME_PATHS: Mutex<(Option<PathBuf>, Option<PathBuf>)> = Mutex::new((None, None));

// ── Public C API ──────────────────────────────────────────────────────────────

/// Initialize the Rime runtime. Must be called once before any other function.
/// Both `user_dir` and `shared_dir` must be non-null UTF-8 strings.
/// Returns true on success.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_init(user_dir: *const c_char, shared_dir: *const c_char) -> bool {
    let user = match c_string_arg(user_dir, "user_dir") {
        Ok(value) => value,
        Err(e) => {
            eprintln!("keytao_init: {e}");
            return false;
        }
    };
    let shared = match c_string_arg(shared_dir, "shared_dir") {
        Ok(value) => value,
        Err(e) => {
            eprintln!("keytao_init: {e}");
            return false;
        }
    };

    let runtime = ImeRuntime::with_dirs(user, shared);
    if let Err(e) = runtime.init_without_deploy() {
        eprintln!("keytao_init: runtime init failed: {e}");
        return false;
    }
    match runtime.create_session() {
        Ok(singleton_session) => {
            let Ok(mut g) = GLOBAL.lock() else {
                return false;
            };
            g.initialized = true;
            g.runtime = Some(runtime);
            g.singleton_session = Some(singleton_session);
            true
        }
        Err(e) => {
            eprintln!("keytao_init: runtime.create_session failed: {e}");
            false
        }
    }
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_is_initialized() -> bool {
    GLOBAL.lock().map(|g| g.initialized).unwrap_or(false)
}

/// Redeploy Rime data through the shared runtime. Existing sessions refresh
/// lazily on their next operation.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload() -> bool {
    let runtime = {
        let Ok(g) = GLOBAL.lock() else {
            return false;
        };
        if !g.initialized {
            return false;
        }
        let Some(runtime) = g.runtime.clone() else {
            return false;
        };
        runtime
    };

    match runtime.reload_without_deploy() {
        Ok(()) => true,
        Err(e) => {
            eprintln!("keytao_reload: runtime reload failed: {e}");
            false
        }
    }
}

/// Create a per-client input session. Returns null if keytao_init() has not
/// completed successfully. Destroy with keytao_destroy_session().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_create_session() -> *mut c_void {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    if !g.initialized {
        return std::ptr::null_mut();
    }
    let Some(runtime) = g.runtime.clone() else {
        return std::ptr::null_mut();
    };
    drop(g);

    match runtime.create_session() {
        Ok(session) => Box::into_raw(Box::new(SessionHandle { session })) as *mut c_void,
        Err(e) => {
            eprintln!("keytao_create_session: runtime.create_session failed: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Destroy a session created by keytao_create_session().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_destroy_session(session: *mut c_void) {
    if session.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(session as *mut SessionHandle));
    }
}

/// Return current state for a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_state(session: *mut c_void) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(handle.session.state(), false)))
}

/// Process a key event on a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_process_key(
    session: *mut c_void,
    keyval: u32,
    modifiers: u32,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(result) = handle.session.process_key_result(keyval, modifiers) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(result_to_c(result)))
}

/// Select a candidate in a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_select_candidate(
    session: *mut c_void,
    index: u32,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.select_candidate(index as usize) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Flip to the next/previous candidate page in a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_change_page(
    session: *mut c_void,
    backward: bool,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.change_page(backward) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Clear current composition in a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_reset(session: *mut c_void) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.reset() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Return whether a per-client session is in ASCII mode.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_get_ascii_mode(session: *mut c_void) -> bool {
    let Some(handle) = session_handle(session) else {
        return false;
    };
    handle.session.is_ascii_mode()
}

/// Set ASCII mode on a per-client session and return the updated state.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_set_ascii_mode(
    session: *mut c_void,
    enabled: bool,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.set_ascii_mode(enabled) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Configure optional default/user theme paths used by JSON state helpers.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_set_theme_paths(
    default_theme_path: *const c_char,
    user_theme_path: *const c_char,
) {
    if let Ok(mut paths) = THEME_PATHS.lock() {
        *paths = (
            optional_path_arg(default_theme_path),
            optional_path_arg(user_theme_path),
        );
    }
}

/// Resolve theme YAML from the optional default and user paths and return a
/// normalized JSON theme. The caller must free the string with
/// keytao_free_string().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_resolve_theme_json(
    default_theme_path: *const c_char,
    user_theme_path: *const c_char,
) -> *mut c_char {
    let default_path = optional_path_arg(default_theme_path);
    let user_path = optional_path_arg(user_theme_path);
    let theme = resolve_theme_from_paths(default_path.as_deref(), user_path.as_deref());
    theme_json_cstring(&theme)
}

/// Resolve theme YAML with a platform-provided system color scheme and return a
/// normalized JSON theme. The caller must free the string with
/// keytao_free_string().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_resolve_theme_json_with_system_scheme(
    default_theme_path: *const c_char,
    user_theme_path: *const c_char,
    system_color_scheme: *const c_char,
) -> *mut c_char {
    let default_path = optional_path_arg(default_theme_path);
    let user_path = optional_path_arg(user_theme_path);
    let Some(system_scheme) = optional_effective_color_scheme_arg(system_color_scheme) else {
        let theme = resolve_theme_from_paths(default_path.as_deref(), user_path.as_deref());
        return theme_json_cstring(&theme);
    };
    let theme = resolve_theme_from_paths_with_system_scheme(
        default_path.as_deref(),
        user_path.as_deref(),
        system_scheme,
    );
    theme_json_cstring(&theme)
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_default_keyboard_yaml() -> *mut c_char {
    to_cstring(keytao_theme::default_keyboard_yaml())
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_resolve_keyboard_json(
    default_keyboard_path: *const c_char,
    user_keyboard_path: *const c_char,
) -> *mut c_char {
    let default_path = optional_path_arg(default_keyboard_path);
    let user_path = optional_path_arg(user_keyboard_path);
    let keyboard =
        keytao_theme::resolve_keyboard_from_paths(default_path.as_deref(), user_path.as_deref());
    match keytao_theme::resolved_keyboard_json(&keyboard) {
        Ok(json) => to_cstring(&json),
        Err(e) => {
            eprintln!("keytao_resolve_keyboard_json: serialize failed: {e}");
            to_cstring("{}")
        }
    }
}

#[cfg(not(target_os = "android"))]
fn theme_json_cstring(theme: &keytao_theme::ResolvedImeTheme) -> *mut c_char {
    match resolved_theme_json(&theme) {
        Ok(json) => to_cstring(&json),
        Err(e) => {
            eprintln!("keytao_resolve_theme_json: serialize failed: {e}");
            to_cstring("{}")
        }
    }
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_state_json(session: *mut c_void) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    to_cstring(&state_json(handle.session.state(), false))
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_process_key_json(
    session: *mut c_void,
    keyval: u32,
    modifiers: u32,
) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(result) = handle.session.process_key_result(keyval, modifiers) else {
        return std::ptr::null_mut();
    };
    to_cstring(&result_json(result))
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_select_candidate_json(
    session: *mut c_void,
    index: u32,
) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.select_candidate(index as usize) else {
        return std::ptr::null_mut();
    };
    to_cstring(&state_json(state, true))
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_select_candidate_global_json(
    session: *mut c_void,
    index: u32,
) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.select_candidate_global(index as usize) else {
        return std::ptr::null_mut();
    };
    to_cstring(&state_json(state, true))
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_all_candidates_json(
    session: *mut c_void,
    limit: u32,
) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(candidates) = handle.session.all_candidates_limited(limit as usize) else {
        return std::ptr::null_mut();
    };
    let json = serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".into());
    to_cstring(&json)
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_change_page_json(
    session: *mut c_void,
    backward: bool,
) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.change_page(backward) else {
        return std::ptr::null_mut();
    };
    to_cstring(&state_json(state, true))
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_reset_json(session: *mut c_void) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.reset() else {
        return std::ptr::null_mut();
    };
    to_cstring(&state_json(state, true))
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_set_ascii_mode_json(
    session: *mut c_void,
    enabled: bool,
) -> *mut c_char {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Some(state) = handle.session.set_ascii_mode(enabled) else {
        return std::ptr::null_mut();
    };
    to_cstring(&state_json(state, true))
}

/// Free a UTF-8 string returned by keytao-core-ffi.
#[no_mangle]
pub extern "C" fn keytao_free_string(ptr: *mut c_char) {
    unsafe { free_cstring(ptr) };
}

/// Process a key event. Returns heap-allocated KeytaoState; caller must free
/// with keytao_free_state(). Returns null if the runtime is not initialized.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_process_key(keyval: u32, modifiers: u32) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref session) = g.singleton_session else {
        return std::ptr::null_mut();
    };
    let Some(result) = session.process_key_result(keyval, modifiers) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(result_to_c(result)))
}

/// Select a candidate by 0-based index. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_select_candidate(index: u32) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref session) = g.singleton_session else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.select_candidate(index as usize) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Flip to the next/previous candidate page. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_change_page(backward: bool) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref session) = g.singleton_session else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.change_page(backward) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Clear current composition (Escape). Returns new state; caller must free.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reset() -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref session) = g.singleton_session else {
        return std::ptr::null_mut();
    };
    let Some(state) = session.reset() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Free a KeytaoState returned by any keytao_* function.
#[no_mangle]
pub extern "C" fn keytao_free_state(ptr: *mut KeytaoState) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let s = Box::from_raw(ptr);
        free_cstring(s.preedit);
        free_cstring(s.committed);
        free_cstring(s.select_keys);
        if !s.candidate_texts.is_null() {
            let texts = Vec::from_raw_parts(
                s.candidate_texts,
                s.candidate_count as usize,
                s.candidate_count as usize,
            );
            for t in texts {
                free_cstring(t);
            }
        }
        if !s.candidate_comments.is_null() {
            let comments = Vec::from_raw_parts(
                s.candidate_comments,
                s.candidate_count as usize,
                s.candidate_count as usize,
            );
            for c in comments {
                free_cstring(c);
            }
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

#[cfg(not(target_os = "android"))]
fn session_handle<'a>(session: *mut c_void) -> Option<&'a SessionHandle> {
    if session.is_null() {
        return None;
    }
    Some(unsafe { &*(session as *mut SessionHandle) })
}

#[cfg(not(target_os = "android"))]
fn result_to_c(result: KeyProcessResult) -> KeytaoState {
    state_to_c(result.state, result.accepted)
}

#[cfg(not(target_os = "android"))]
fn state_to_c(state: ImeState, accepted: bool) -> KeytaoState {
    let count = state.candidates.len();
    let (texts_ptr, comments_ptr) = if count == 0 {
        (std::ptr::null_mut(), std::ptr::null_mut())
    } else {
        let mut texts: Vec<*mut c_char> = state
            .candidates
            .iter()
            .map(|c| to_cstring(&c.text))
            .collect();
        let mut comments: Vec<*mut c_char> = state
            .candidates
            .iter()
            .map(|c| to_cstring(c.comment.as_deref().unwrap_or("")))
            .collect();
        let tp = texts.as_mut_ptr();
        let cp = comments.as_mut_ptr();
        std::mem::forget(texts);
        std::mem::forget(comments);
        (tp, cp)
    };

    KeytaoState {
        preedit: to_cstring(&state.preedit),
        cursor: state.cursor as u32,
        candidate_texts: texts_ptr,
        candidate_comments: comments_ptr,
        candidate_count: count as u32,
        highlighted_candidate_index: state.highlighted_candidate_index as u32,
        page: state.page as u32,
        is_last_page: state.is_last_page,
        committed: to_cstring(state.committed.as_deref().unwrap_or("")),
        select_keys: to_cstring(state.select_keys.as_deref().unwrap_or("")),
        ascii_mode: state.ascii_mode,
        accepted,
    }
}

#[cfg(not(target_os = "android"))]
fn result_json(result: KeyProcessResult) -> String {
    state_json(result.state, result.accepted)
}

#[cfg(not(target_os = "android"))]
fn state_json(state: ImeState, accepted: bool) -> String {
    let theme = current_theme();
    let mut ui_capabilities = keytao_theme::UiCapabilities::full_custom();
    ui_capabilities.supports_vertical = false;
    let candidate_panel = theme.candidate_panel_model(
        keytao_theme::CandidatePanelInput {
            preedit: state.preedit.clone(),
            candidates: state
                .candidates
                .iter()
                .map(|candidate| keytao_theme::ThemeCandidate {
                    text: candidate.text.clone(),
                    comment: candidate.comment.clone(),
                })
                .collect(),
            highlighted_candidate_index: state.highlighted_candidate_index,
            page: state.page,
            is_last_page: state.is_last_page,
            select_keys: state.select_keys.clone(),
        },
        &ui_capabilities,
    );
    let mode_hint = theme.mode_hint_model(state.ascii_mode);
    let value = serde_json::json!({
        "preedit": state.preedit,
        "cursor": state.cursor,
        "candidates": state.candidates,
        "allCandidates": state.all_candidates,
        "highlightedCandidateIndex": state.highlighted_candidate_index,
        "pageSize": state.page_size,
        "page": state.page,
        "isLastPage": state.is_last_page,
        "committed": state.committed.unwrap_or_default(),
        "selectKeys": state.select_keys.unwrap_or_default(),
        "asciiMode": state.ascii_mode,
        "schemaName": state.schema_name,
        "accepted": accepted,
        "candidatePanel": candidate_panel,
        "modeHint": mode_hint,
    });
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".into())
}

#[cfg(not(target_os = "android"))]
fn current_theme() -> keytao_theme::ResolvedImeTheme {
    let (default_path, user_path) = THEME_PATHS
        .lock()
        .map(|paths| paths.clone())
        .unwrap_or((None, None));
    resolve_theme_from_paths(default_path.as_deref(), user_path.as_deref())
}

fn c_string_arg(ptr: *const c_char, name: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("{name} is null"));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_string)
        .map_err(|e| format!("{name} is not UTF-8: {e}"))
}

fn optional_path_arg(ptr: *const c_char) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }
    let Ok(value) = (unsafe { CStr::from_ptr(ptr) }).to_str() else {
        return None;
    };
    let value = value.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

#[cfg(not(target_os = "android"))]
fn optional_effective_color_scheme_arg(ptr: *const c_char) -> Option<EffectiveColorScheme> {
    if ptr.is_null() {
        return None;
    }
    let Ok(value) = (unsafe { CStr::from_ptr(ptr) }).to_str() else {
        return None;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "dark" | "night" => Some(EffectiveColorScheme::Dark),
        "light" | "day" => Some(EffectiveColorScheme::Light),
        _ => None,
    }
}

fn to_cstring(s: &str) -> *mut c_char {
    CString::new(s)
        .map(|cs| cs.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

unsafe fn free_cstring(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}
