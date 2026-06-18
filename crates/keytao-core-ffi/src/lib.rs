use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::sync::Mutex;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use keytao_core::{deploy, Engine, ImeState, KeyProcessResult};

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

// ── Module-level singleton engine ─────────────────────────────────────────────

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct Global {
    initialized: bool,
    engine: Option<Engine>,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
static GLOBAL: Mutex<Global> = Mutex::new(Global {
    initialized: false,
    engine: None,
});

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct SessionHandle {
    engine: Mutex<Engine>,
}

// ── Public C API ──────────────────────────────────────────────────────────────

/// Initialize the Rime engine. Must be called once before any other function.
/// Both `user_dir` and `shared_dir` must be non-null UTF-8 strings.
/// Returns true on success.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
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

    if let Err(e) = deploy(user, shared) {
        eprintln!("keytao_init: deploy failed: {e}");
        return false;
    }
    match Engine::new() {
        Ok(engine) => {
            let Ok(mut g) = GLOBAL.lock() else {
                return false;
            };
            g.initialized = true;
            g.engine = Some(engine);
            true
        }
        Err(e) => {
            eprintln!("keytao_init: Engine::new failed: {e}");
            false
        }
    }
}

#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_is_initialized() -> bool {
    GLOBAL.lock().map(|g| g.initialized).unwrap_or(false)
}

/// Create a per-client input session. Returns null if keytao_init() has not
/// completed successfully. Destroy with keytao_destroy_session().
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_create_session() -> *mut c_void {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    if !g.initialized {
        return std::ptr::null_mut();
    }
    drop(g);

    match Engine::new() {
        Ok(engine) => Box::into_raw(Box::new(SessionHandle {
            engine: Mutex::new(engine),
        })) as *mut c_void,
        Err(e) => {
            eprintln!("keytao_create_session: Engine::new failed: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Destroy a session created by keytao_create_session().
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
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
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_session_state(session: *mut c_void) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Ok(engine) = handle.engine.lock() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(state_to_c(engine.state(), false)))
}

/// Process a key event on a per-client session.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_session_process_key(
    session: *mut c_void,
    keyval: u32,
    modifiers: u32,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Ok(engine) = handle.engine.lock() else {
        return std::ptr::null_mut();
    };
    let result = engine.process_key_result(keyval, modifiers);
    Box::into_raw(Box::new(result_to_c(result)))
}

/// Select a candidate in a per-client session.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_session_select_candidate(
    session: *mut c_void,
    index: u32,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Ok(engine) = handle.engine.lock() else {
        return std::ptr::null_mut();
    };
    let state = engine.select_candidate(index as usize);
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Flip to the next/previous candidate page in a per-client session.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_session_change_page(
    session: *mut c_void,
    backward: bool,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Ok(engine) = handle.engine.lock() else {
        return std::ptr::null_mut();
    };
    let state = engine.change_page(backward);
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Clear current composition in a per-client session.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_session_reset(session: *mut c_void) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Ok(engine) = handle.engine.lock() else {
        return std::ptr::null_mut();
    };
    let state = engine.reset();
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Return whether a per-client session is in ASCII mode.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_session_get_ascii_mode(session: *mut c_void) -> bool {
    let Some(handle) = session_handle(session) else {
        return false;
    };
    let Ok(engine) = handle.engine.lock() else {
        return false;
    };
    engine.is_ascii_mode()
}

/// Set ASCII mode on a per-client session and return the updated state.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_session_set_ascii_mode(
    session: *mut c_void,
    enabled: bool,
) -> *mut KeytaoState {
    let Some(handle) = session_handle(session) else {
        return std::ptr::null_mut();
    };
    let Ok(engine) = handle.engine.lock() else {
        return std::ptr::null_mut();
    };
    let state = engine.set_ascii_mode(enabled);
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Process a key event. Returns heap-allocated KeytaoState; caller must free
/// with keytao_free_state(). Returns null if the engine is not initialized.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_process_key(keyval: u32, modifiers: u32) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let result = engine.process_key_result(keyval, modifiers);
    Box::into_raw(Box::new(result_to_c(result)))
}

/// Select a candidate by 0-based index. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_select_candidate(index: u32) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let state = engine.select_candidate(index as usize);
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Flip to the next/previous candidate page. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_change_page(backward: bool) -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let state = engine.change_page(backward);
    Box::into_raw(Box::new(state_to_c(state, true)))
}

/// Clear current composition (Escape). Returns new state; caller must free.
#[no_mangle]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub extern "C" fn keytao_reset() -> *mut KeytaoState {
    let Ok(g) = GLOBAL.lock() else {
        return std::ptr::null_mut();
    };
    let Some(ref engine) = g.engine else {
        return std::ptr::null_mut();
    };
    let state = engine.reset();
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

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn session_handle<'a>(session: *mut c_void) -> Option<&'a SessionHandle> {
    if session.is_null() {
        return None;
    }
    Some(unsafe { &*(session as *mut SessionHandle) })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn result_to_c(result: KeyProcessResult) -> KeytaoState {
    state_to_c(result.state, result.accepted)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
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

fn c_string_arg(ptr: *const c_char, name: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("{name} is null"));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_string)
        .map_err(|e| format!("{name} is not UTF-8: {e}"))
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
