use crate::{
    english_mode::EnglishBackend, key_policy::*, lock_ignore_poison, Candidate, EngineCapabilities,
    ImeBackend, ImeState, KeyProcessResult, SchemaInfo, SchemaSwitch, SchemaSwitchRequest,
    DEFAULT_SELECT_KEYS, RIME_MOD_LOCK, RIME_MOD_SHIFT, RIME_RELEASE_MASK,
};
use libloading::Library;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{c_char, c_uint, c_void, CStr},
    mem::size_of,
    path::Path,
    ptr::{self, NonNull},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const SCHEMA_ID: &str = "keytao";
pub const SCHEMA_NAME: &str = "键道6 (ime-jd)";
pub const DEFAULT_PAGE_SIZE: usize = 5;
const HINT_CAPACITY: usize = 8;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;
const XK_CAPS_LOCK: u32 = 0xffe5;
const XK_INSERT: u32 = 0xff63;

// Local declarations of the subset of the documented C ABI used by KeyTao.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawState {
    commit_a: *const c_char,
    commit_b: *const c_char,
    commit_lit: c_char,
    options_count: c_uint,
    anchor_index: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawCandidate {
    value: *const c_char,
    hint: [u8; HINT_CAPACITY],
}

type Context = *mut c_void;

struct JdApi {
    layout: unsafe extern "C" fn(c_uint) -> c_uint,
    init: unsafe extern "C" fn(*const c_void, usize) -> Context,
    deinit: unsafe extern "C" fn(Context),
    state: unsafe extern "C" fn(Context) -> *const RawState,
    press_key: unsafe extern "C" fn(Context, c_char),
    backspace: unsafe extern "C" fn(Context),
    reset: unsafe extern "C" fn(Context),
    set_anchor: unsafe extern "C" fn(Context, c_uint),
    read_range: unsafe extern "C" fn(Context, c_uint, c_uint, *mut RawCandidate, c_uint) -> c_uint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JdAbiLayout {
    pub state_size: c_uint,
    pub option_size: c_uint,
    pub hint_capacity: c_uint,
}

impl JdAbiLayout {
    pub const EXPECTED: Self = Self {
        state_size: size_of::<RawState>() as c_uint,
        option_size: size_of::<RawCandidate>() as c_uint,
        hint_capacity: HINT_CAPACITY as c_uint,
    };

    fn check(query: unsafe extern "C" fn(c_uint) -> c_uint) -> Result<Self, String> {
        // The bootstrap function has no ctx and returns only scalar layout data.
        let actual = unsafe {
            Self {
                state_size: query(0),
                option_size: query(1),
                hint_capacity: query(2),
            }
        };
        if actual != Self::EXPECTED {
            return Err(format!(
                "libjd ABI mismatch: got {actual:?}, expected {:?}",
                Self::EXPECTED
            ));
        }
        Ok(actual)
    }
}

struct JdLibrary {
    api: JdApi,
    layout: JdAbiLayout,
    _library: Option<Library>,
}

#[cfg(target_os = "android")]
type AndroidLoader = fn(&Path) -> Result<(), String>;

#[cfg(target_os = "android")]
static ANDROID_LIBRARY_LOADER: std::sync::OnceLock<AndroidLoader> = std::sync::OnceLock::new();

#[cfg(target_os = "android")]
pub fn set_android_library_loader(loader: AndroidLoader) {
    let _ = ANDROID_LIBRARY_LOADER.set(loader);
}

pub struct JdBackend {
    session: Mutex<Session>,
}

impl JdBackend {
    /// # Safety
    /// `path` must identify a verified libjd library trusted to run native code.
    pub unsafe fn load(path: &Path) -> Result<Self, String> {
        unsafe { Self::load_with_page_size(path, DEFAULT_PAGE_SIZE) }
    }

    /// # Safety
    /// `path` must identify a verified libjd library trusted to run native code.
    pub unsafe fn load_with_page_size(path: &Path, page_size: usize) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("libjd library path must be absolute".into());
        }
        #[cfg(target_os = "android")]
        ANDROID_LIBRARY_LOADER
            .get()
            .ok_or("libjd Android library loader is unavailable")?(path)?;
        // The caller supplies the exact path of the verified release library.
        #[cfg(target_os = "windows")]
        let library: Result<Library, _> = unsafe {
            use libloading::os::windows::{
                Library as WindowsLibrary, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            };
            WindowsLibrary::load_with_flags(
                path,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .map(Into::into)
        };
        #[cfg(not(target_os = "windows"))]
        let library = unsafe { Library::new(path) };
        let library = library.map_err(|error| format!("load libjd: {error}"))?;
        // These signatures are from the public C ABI. The library is kept alive
        // by every ctx until its deinit has completed.
        unsafe {
            Self::from_api(
                JdApi {
                    layout: *library.get(b"jd_abi_layout\0").map_err(symbol_error)?,
                    init: *library.get(b"jd_init\0").map_err(symbol_error)?,
                    deinit: *library.get(b"jd_deinit\0").map_err(symbol_error)?,
                    state: *library.get(b"jd_state_ptr\0").map_err(symbol_error)?,
                    press_key: *library.get(b"jd_press_key\0").map_err(symbol_error)?,
                    backspace: *library.get(b"jd_backspace\0").map_err(symbol_error)?,
                    reset: *library.get(b"jd_reset\0").map_err(symbol_error)?,
                    set_anchor: *library.get(b"jd_set_anchor\0").map_err(symbol_error)?,
                    read_range: *library.get(b"jd_read_range\0").map_err(symbol_error)?,
                },
                Some(library),
                page_size,
            )
        }
    }

    fn from_api(api: JdApi, library: Option<Library>, page_size: usize) -> Result<Self, String> {
        let layout = JdAbiLayout::check(api.layout)?;
        Self::from_library(
            Arc::new(JdLibrary {
                api,
                layout,
                _library: library,
            }),
            page_size.clamp(1, DEFAULT_SELECT_KEYS.len()),
        )
    }

    fn from_library(library: Arc<JdLibrary>, page_size: usize) -> Result<Self, String> {
        Ok(Self {
            session: Mutex::new(Session::new(library, page_size)?),
        })
    }

    pub fn new_session(&self) -> Result<Self, String> {
        let source = lock_ignore_poison(&self.session);
        Self::from_library(Arc::clone(&source.library), source.page_size)
    }

    pub fn abi_layout(&self) -> JdAbiLayout {
        lock_ignore_poison(&self.session).library.layout
    }
}

fn symbol_error(error: libloading::Error) -> String {
    format!("resolve libjd symbol: {error}")
}

struct Session {
    library: Arc<JdLibrary>,
    ctx: NonNull<c_void>,
    state: NonNull<RawState>,
    preedit: String,
    page_size: usize,
    ascii_mode: bool,
    solo_shift: Option<(u32, Instant)>,
}

// A jd context can move between threads. Its only access is through the session mutex.
unsafe impl Send for Session {}

impl Session {
    fn new(library: Arc<JdLibrary>, page_size: usize) -> Result<Self, String> {
        // ABI validation precedes allocation. A null dictionary uses only built-in words.
        let ctx = NonNull::new(unsafe { (library.api.init)(ptr::null(), 0) })
            .ok_or("create libjd context")?;
        let Some(state) = NonNull::new(unsafe { (library.api.state)(ctx.as_ptr()) }.cast_mut())
        else {
            unsafe { (library.api.deinit)(ctx.as_ptr()) };
            return Err("libjd context returned no state".into());
        };
        Ok(Self {
            library,
            ctx,
            state,
            preedit: String::new(),
            page_size,
            ascii_mode: false,
            solo_shift: None,
        })
    }

    fn raw_state(&self) -> RawState {
        // libjd retains this block for the entire ctx lifetime. No other thread has the ctx.
        unsafe { *self.state.as_ptr() }
    }

    fn candidate_count(&self) -> usize {
        self.raw_state().options_count as usize
    }

    fn anchor(&self) -> usize {
        let raw = self.raw_state();
        raw.anchor_index.min(raw.options_count.saturating_sub(1)) as usize
    }

    fn candidates(&self, start: usize, count: usize) -> Vec<Candidate> {
        let count = count.min(self.candidate_count().saturating_sub(start));
        if count == 0 {
            return Vec::new();
        }
        let mut output = vec![
            RawCandidate {
                value: ptr::null(),
                hint: [0; HINT_CAPACITY]
            };
            count
        ];
        let written = unsafe {
            (self.library.api.read_range)(
                self.ctx.as_ptr(),
                start as c_uint,
                count as c_uint,
                output.as_mut_ptr(),
                count as c_uint,
            )
        };
        output.truncate(written as usize);
        output
            .into_iter()
            .map(|option| {
                let hint_len = option
                    .hint
                    .iter()
                    .position(|&byte| byte == 0)
                    .unwrap_or(HINT_CAPACITY);
                Candidate {
                    text: string_from_jd(option.value),
                    comment: (hint_len != 0)
                        .then(|| String::from_utf8_lossy(&option.hint[..hint_len]).into_owned()),
                }
            })
            .collect()
    }

    fn snapshot(&self, committed: Option<String>) -> ImeState {
        let anchor = self.anchor();
        let page = anchor / self.page_size;
        let start = page * self.page_size;
        let candidates = self.candidates(start, self.page_size);
        let index = anchor - start;
        let caret = self.preedit.len();
        ImeState {
            preedit: self.preedit.clone(),
            commit_preview: candidates
                .get(index)
                .map(|item| item.text.clone())
                .unwrap_or_default(),
            cursor: caret,
            sel_start: caret,
            sel_end: caret,
            candidates,
            highlighted_candidate_index: index,
            page_size: self.page_size,
            page,
            is_last_page: start + self.page_size >= self.candidate_count(),
            committed,
            select_keys: self.select_keys().map(str::to_owned),
            ascii_mode: self.ascii_mode,
            chord_composer: false,
            schema_shortcuts: Vec::new(),
            schema_id: SCHEMA_ID.to_owned(),
            schema_name: SCHEMA_NAME.to_owned(),
        }
    }

    fn select_keys(&self) -> Option<&'static str> {
        (!cfg!(target_os = "android") && self.candidate_count() != 0)
            .then(|| &DEFAULT_SELECT_KEYS[..self.page_size])
    }

    fn press(&mut self, byte: u8) -> ImeState {
        unsafe { (self.library.api.press_key)(self.ctx.as_ptr(), byte as c_char) };
        let raw = self.raw_state();
        let committed = raw.committed();
        if committed.is_some() || raw.options_count == 0 {
            self.preedit.clear();
        }
        if raw.options_count != 0 {
            self.preedit.push(char::from(byte));
        }
        self.snapshot(committed)
    }

    fn set_anchor(&mut self, index: usize) {
        if index < self.candidate_count() {
            unsafe { (self.library.api.set_anchor)(self.ctx.as_ptr(), index as c_uint) };
        }
    }

    fn select(&mut self, index: usize) -> ImeState {
        if index >= self.candidate_count() {
            return self.snapshot(None);
        }
        self.set_anchor(index);
        self.press(b' ')
    }

    fn change_page(&mut self, backward: bool) -> ImeState {
        let start = self.anchor() / self.page_size * self.page_size;
        let next = if backward {
            start.saturating_sub(self.page_size)
        } else {
            start.saturating_add(self.page_size)
        };
        if next != start {
            self.set_anchor(next);
        }
        self.snapshot(None)
    }

    fn move_highlight(&mut self, backward: bool) -> ImeState {
        let anchor = self.anchor();
        let next = if backward {
            anchor.saturating_sub(1)
        } else {
            anchor.saturating_add(1).min(self.candidate_count().saturating_sub(1))
        };
        self.set_anchor(next);
        self.snapshot(None)
    }

    fn index_on_page(&self, index: usize) -> Option<usize> {
        (index < self.page_size).then(|| self.anchor() / self.page_size * self.page_size + index)
    }

    fn reset(&mut self) {
        unsafe { (self.library.api.reset)(self.ctx.as_ptr()) };
        self.preedit.clear();
        self.solo_shift = None;
    }

    fn commit_raw_input(&mut self) -> ImeState {
        let raw = std::mem::take(&mut self.preedit);
        self.reset();
        self.snapshot((!raw.is_empty()).then_some(raw))
    }

    fn set_ascii_mode(&mut self, enabled: bool) -> ImeState {
        self.ascii_mode = enabled;
        if enabled && self.candidate_count() != 0 {
            return self.press(b' ');
        }
        self.snapshot(None)
    }

    fn shift_key(&mut self, keycode: u32, mask: u32) -> Option<ImeState> {
        let armed = self.solo_shift.take();
        match mask & !RIME_MOD_LOCK {
            0 => self.solo_shift = Some((keycode, Instant::now())),
            RIME_RELEASE_MASK
                if armed.is_some_and(|(key, started)| {
                    key == keycode && started.elapsed() < Duration::from_millis(500)
                }) =>
            {
                // Match the KeyTao solo-Shift policy of committing the raw code.
                self.ascii_mode = !self.ascii_mode;
                return Some(self.commit_raw_input());
            }
            _ => {}
        }
        None
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { (self.library.api.deinit)(self.ctx.as_ptr()) };
    }
}

impl RawState {
    fn committed(self) -> Option<String> {
        let mut value = string_from_jd(self.commit_a);
        value.push_str(&string_from_jd(self.commit_b));
        if self.commit_lit != 0 {
            value.push(char::from(self.commit_lit as u8));
        }
        (!value.is_empty()).then_some(value)
    }
}

fn string_from_jd(value: *const c_char) -> String {
    if value.is_null() {
        return String::new();
    }
    // Candidate and commit string pointers refer to NUL-terminated data owned by libjd.
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
}

impl EnglishBackend for JdBackend {
    fn current_schema(&self) -> Option<SchemaInfo> {
        Some(SchemaInfo {
            id: SCHEMA_ID.into(),
            name: SCHEMA_NAME.into(),
        })
    }

    fn schema_switches(&self) -> Vec<SchemaSwitch> {
        Vec::new()
    }

    fn get_option(&self, name: &str) -> bool {
        name == "ascii_mode" && self.is_ascii_mode()
    }

    fn set_option(&self, name: &str, enabled: bool) -> ImeState {
        if name == "ascii_mode" {
            self.set_ascii_mode(enabled)
        } else {
            self.state()
        }
    }

    fn select_schema(&self, id: &str) -> Result<ImeState, String> {
        if id != SCHEMA_ID {
            return Err(format!("libjd does not provide schema {id}"));
        }
        Ok(self.state())
    }

    fn discard_composition(&self) {
        lock_ignore_poison(&self.session).reset();
    }

    fn state(&self) -> ImeState {
        lock_ignore_poison(&self.session).snapshot(None)
    }

    fn apply_ascii_mode(&self, enabled: bool) {
        lock_ignore_poison(&self.session).ascii_mode = enabled;
    }

    fn set_ascii_mode(&self, enabled: bool) -> ImeState {
        lock_ignore_poison(&self.session).set_ascii_mode(enabled)
    }
}

impl ImeBackend for JdBackend {
    fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult {
        let mut session = lock_ignore_poison(&self.session);
        if matches!(keycode, XK_SHIFT_L | XK_SHIFT_R) {
            let switched = session.shift_key(keycode, mask);
            return KeyProcessResult {
                accepted: switched.is_some(),
                state: switched.unwrap_or_else(|| session.snapshot(None)),
            };
        }
        if mask & RIME_RELEASE_MASK == 0 {
            session.solo_shift = None;
        }
        if mask & !(RIME_MOD_SHIFT | RIME_MOD_LOCK) != 0 {
            return KeyProcessResult {
                state: session.snapshot(None),
                accepted: false,
            };
        }
        if keycode == XK_CAPS_LOCK {
            let state = session.set_ascii_mode(mask & RIME_MOD_LOCK == 0);
            return KeyProcessResult {
                state,
                accepted: false,
            };
        }
        if session.ascii_mode {
            return KeyProcessResult {
                state: session.snapshot(None),
                accepted: false,
            };
        }
        if session.candidate_count() != 0 {
            let state = match keycode {
                XK_BACK_SPACE => {
                    unsafe { (session.library.api.backspace)(session.ctx.as_ptr()) };
                    session.preedit.pop();
                    session.snapshot(None)
                }
                XK_ESCAPE => {
                    session.reset();
                    session.snapshot(None)
                }
                XK_RETURN | XK_KP_ENTER => session.commit_raw_input(),
                XK_PAGE_UP | XK_PAGE_DOWN => session.change_page(keycode == XK_PAGE_UP),
                XK_UP | XK_LEFT => session.move_highlight(true),
                XK_DOWN | XK_RIGHT => session.move_highlight(false),
                XK_HOME | XK_END | XK_DELETE | XK_INSERT => session.snapshot(None),
                _ => {
                    if let Some(index) = session
                        .select_keys()
                        .and_then(|keys| keys.bytes().position(|byte| u32::from(byte) == keycode))
                        .and_then(|index| session.index_on_page(index))
                        .filter(|&index| index < session.candidate_count())
                    {
                        return KeyProcessResult {
                            state: session.select(index),
                            accepted: true,
                        };
                    }
                    return process_printable(&mut session, keycode);
                }
            };
            return KeyProcessResult {
                state,
                accepted: true,
            };
        }
        process_printable(&mut session, keycode)
    }

    fn process_enter(&self) -> KeyProcessResult {
        self.process_key_result(XK_RETURN, 0)
    }

    fn select_candidate_on_page(&self, index: usize) -> ImeState {
        let mut session = lock_ignore_poison(&self.session);
        match session.index_on_page(index) {
            Some(index) => session.select(index),
            None => session.snapshot(None),
        }
    }

    fn select_candidate_global(&self, index: usize) -> ImeState {
        lock_ignore_poison(&self.session).select(index)
    }

    fn highlight_candidate_on_page(&self, index: usize) -> ImeState {
        let mut session = lock_ignore_poison(&self.session);
        if let Some(index) = session.index_on_page(index) {
            session.set_anchor(index);
        }
        session.snapshot(None)
    }

    fn candidate_is_user_phrase_on_page(&self, _index: usize) -> bool {
        false
    }

    fn delete_candidate_on_page_result(&self, _index: usize) -> (ImeState, bool) {
        (self.state(), false)
    }

    fn all_candidates_limited(&self, max_count: usize) -> Vec<Candidate> {
        lock_ignore_poison(&self.session).candidates(0, max_count)
    }

    fn change_page(&self, backward: bool) -> ImeState {
        lock_ignore_poison(&self.session).change_page(backward)
    }

    fn reset(&self) -> ImeState {
        self.clear_composition()
    }

    fn clear_composition(&self) -> ImeState {
        let mut session = lock_ignore_poison(&self.session);
        session.reset();
        session.snapshot(None)
    }

    fn commit_composition(&self) -> ImeState {
        let mut session = lock_ignore_poison(&self.session);
        if session.candidate_count() == 0 {
            return session.snapshot(None);
        }
        session.press(b' ')
    }

    fn raw_input(&self) -> Option<String> {
        let session = lock_ignore_poison(&self.session);
        (!session.preedit.is_empty()).then(|| session.preedit.clone())
    }

    fn commit_raw_input(&self) -> ImeState {
        lock_ignore_poison(&self.session).commit_raw_input()
    }

    fn list_schemas(&self) -> Vec<SchemaInfo> {
        self.current_schema().into_iter().collect()
    }

    fn active_switches(&self) -> Vec<crate::SchemaSwitchSetting> {
        vec![crate::SchemaSwitchSetting {
            schema_id: "keytao".into(),
            name: "ascii_mode".into(),
            states: vec!["中文".into(), "西文".into()],
            value: usize::from(self.is_ascii_mode()),
            options: Vec::new(),
        }]
    }

    fn persist_switch_options(&self, _options: &[(String, bool)]) {}

    fn apply_switch_request(
        &self,
        _request: &SchemaSwitchRequest,
        _reset_only: bool,
    ) -> Result<(), String> {
        Ok(())
    }

    fn apply_reset_switch_request(&self, _root: &Path) -> Result<(), String> {
        Ok(())
    }

    fn is_ascii_mode(&self) -> bool {
        lock_ignore_poison(&self.session).ascii_mode
    }

    fn is_composing(&self) -> bool {
        lock_ignore_poison(&self.session).candidate_count() != 0
    }

    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            candidate_selection: true,
            global_candidate_selection: true,
            candidate_highlight: true,
            candidate_deletion: false,
            native_paging: true,
            commit_composition: true,
            clear_composition: true,
        }
    }
}

fn process_printable(session: &mut Session, keycode: u32) -> KeyProcessResult {
    let byte = if is_space_key(keycode) {
        u32::from(b' ')
    } else {
        keycode
    };
    if !(0x20..=0x7e).contains(&byte) {
        return KeyProcessResult {
            state: session.snapshot(None),
            accepted: false,
        };
    }
    let state = session.press(byte as u8);
    let accepted = state.committed.is_some() || !state.candidates.is_empty();
    KeyProcessResult { state, accepted }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const VALUES: [&CStr; 5] = [c"甲", c"乙", c"丙", c"丁", c"戊"];
    const EMPTY: RawState = RawState {
        commit_a: ptr::null(),
        commit_b: ptr::null(),
        commit_lit: 0,
        options_count: 0,
        anchor_index: 0,
    };

    unsafe extern "C" fn fake_layout(query: c_uint) -> c_uint {
        match query {
            0 => JdAbiLayout::EXPECTED.state_size,
            1 => JdAbiLayout::EXPECTED.option_size,
            2 => JdAbiLayout::EXPECTED.hint_capacity,
            _ => 0,
        }
    }

    unsafe extern "C" fn fake_init(_: *const c_void, _: usize) -> Context {
        Box::into_raw(Box::new(EMPTY)).cast()
    }

    unsafe extern "C" fn fake_deinit(ctx: Context) {
        drop(unsafe { Box::from_raw(ctx.cast::<RawState>()) });
    }

    unsafe extern "C" fn fake_state(ctx: Context) -> *const RawState {
        ctx.cast()
    }

    unsafe extern "C" fn fake_press(ctx: Context, key: c_char) {
        let state = unsafe { &mut *ctx.cast::<RawState>() };
        let previous = *state;
        *state = EMPTY;
        match key as u8 {
            b'a' => state.options_count = VALUES.len() as c_uint,
            b'b' => {
                state.options_count = 3;
                if previous.options_count != 0 {
                    state.commit_a = VALUES[previous.anchor_index as usize].as_ptr();
                }
            }
            b' ' if previous.options_count != 0 => {
                state.commit_a = VALUES[previous.anchor_index as usize].as_ptr();
            }
            b'!' => {
                state.commit_a = c"一".as_ptr();
                state.commit_b = c"二".as_ptr();
                state.commit_lit = b'!' as c_char;
            }
            _ => state.commit_lit = key,
        }
    }

    unsafe extern "C" fn fake_clear(ctx: Context) {
        unsafe { *ctx.cast::<RawState>() = EMPTY };
    }

    unsafe extern "C" fn fake_anchor(ctx: Context, index: c_uint) {
        let state = unsafe { &mut *ctx.cast::<RawState>() };
        state.anchor_index = index;
    }

    unsafe extern "C" fn fake_candidates(
        ctx: Context,
        start: c_uint,
        count: c_uint,
        out: *mut RawCandidate,
        capacity: c_uint,
    ) -> c_uint {
        let state = unsafe { &*ctx.cast::<RawState>() };
        let written = count
            .min(capacity)
            .min(state.options_count.saturating_sub(start));
        for index in 0..written {
            let mut hint = [0; HINT_CAPACITY];
            if start + index == 0 {
                hint[0] = b'x';
            }
            unsafe {
                out.add(index as usize).write(RawCandidate {
                    value: VALUES[(start + index) as usize].as_ptr(),
                    hint,
                });
            }
        }
        written
    }

    fn fake_api() -> JdApi {
        JdApi {
            layout: fake_layout,
            init: fake_init,
            deinit: fake_deinit,
            state: fake_state,
            press_key: fake_press,
            backspace: fake_clear,
            reset: fake_clear,
            set_anchor: fake_anchor,
            read_range: fake_candidates,
        }
    }

    #[test]
    fn refuses_mismatched_layout_before_context_init() {
        static INITIALIZED: AtomicBool = AtomicBool::new(false);
        unsafe extern "C" fn bad_layout(query: c_uint) -> c_uint {
            if query == 1 {
                1
            } else {
                unsafe { fake_layout(query) }
            }
        }
        unsafe extern "C" fn track_init(_: *const c_void, _: usize) -> Context {
            INITIALIZED.store(true, Ordering::Relaxed);
            ptr::null_mut()
        }
        let api = JdApi {
            layout: bad_layout,
            init: track_init,
            ..fake_api()
        };
        let error = JdBackend::from_api(api, None, 2).err().unwrap();
        assert!(error.contains("ABI mismatch"));
        assert!(!INITIALIZED.load(Ordering::Relaxed));
    }

    #[test]
    fn maps_preedit_paging_selection_commit_and_ascii_passthrough() {
        let backend = JdBackend::from_api(fake_api(), None, 2).unwrap();
        let peer = backend.new_session().unwrap();
        let typed = backend.process_key_result(b'a'.into(), 0);
        assert!(typed.accepted);
        assert_eq!(typed.state.preedit, "a");
        assert_eq!(
            (
                typed.state.cursor,
                typed.state.sel_start,
                typed.state.sel_end
            ),
            (1, 1, 1)
        );
        assert_eq!(typed.state.commit_preview, "甲");
        assert_eq!(typed.state.candidates[0].comment.as_deref(), Some("x"));
        assert_eq!(peer.state().preedit, "");
        assert!(peer.commit_composition().committed.is_none());

        let page = backend.change_page(false);
        assert_eq!(page.page, 1);
        assert_eq!(page.candidates[0].text, "丙");
        assert!(!page.is_last_page);
        let last = backend.change_page(false);
        assert_eq!(last.page, 2);
        assert!(last.is_last_page);
        assert_eq!(backend.change_page(false).page, 2);
        assert_eq!(backend.change_page(true).page, 1);
        assert_eq!(backend.highlight_candidate_on_page(1).commit_preview, "丁");
        assert_eq!(
            backend.select_candidate_on_page(0).committed.as_deref(),
            Some("丙")
        );
        assert!(backend.state().committed.is_none());

        backend.process_key_result(b'a'.into(), 0);
        let jump = backend.process_key_result(b'b'.into(), 0).state;
        assert_eq!(jump.committed.as_deref(), Some("甲"));
        assert_eq!(jump.preedit, "b");
        let parts = backend.process_key_result(b'!'.into(), 0).state;
        assert_eq!(parts.committed.as_deref(), Some("一二!"));
        assert!(parts.preedit.is_empty());
        assert!(backend.set_ascii_mode(true).ascii_mode);
        assert!(!backend.process_key_result(b'a'.into(), 0).accepted);
        assert!(backend.state().candidates.is_empty());

        backend.process_key_result(XK_SHIFT_L, 0);
        assert!(
            !backend
                .process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK)
                .state
                .ascii_mode
        );
        backend.process_key_result(XK_SHIFT_L, 0);
        backend.process_key_result(b'a'.into(), 0);
        assert!(
            !backend
                .process_key_result(XK_SHIFT_L, RIME_RELEASE_MASK)
                .state
                .ascii_mode
        );
        backend.process_key_result(XK_SHIFT_R, 0);
        let shifted = backend.process_key_result(XK_SHIFT_R, RIME_RELEASE_MASK);
        assert!(shifted.state.ascii_mode);
        assert_eq!(shifted.state.committed.as_deref(), Some("a"));
    }

    #[test]
    fn arrow_keys_move_candidate_highlight_and_commit_selected_option() {
        let backend = JdBackend::from_api(fake_api(), None, 2).unwrap();
        let initial = backend.process_key_result(b'a'.into(), 0).state;
        assert_eq!(initial.highlighted_candidate_index, 0);
        assert_eq!(initial.commit_preview, "甲");

        let right = backend.process_key_result(XK_RIGHT, 0).state;
        assert_eq!(right.page, 0);
        assert_eq!(right.highlighted_candidate_index, 1);
        assert_eq!(right.commit_preview, "乙");

        let down_next_page = backend.process_key_result(XK_DOWN, 0).state;
        assert_eq!(down_next_page.page, 1);
        assert_eq!(down_next_page.highlighted_candidate_index, 0);
        assert_eq!(down_next_page.commit_preview, "丙");

        let up_prev = backend.process_key_result(XK_UP, 0).state;
        assert_eq!(up_prev.page, 0);
        assert_eq!(up_prev.highlighted_candidate_index, 1);
        assert_eq!(up_prev.commit_preview, "乙");

        let committed = backend.process_key_result(b' '.into(), 0).state;
        assert_eq!(committed.committed.as_deref(), Some("乙"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_macos_release_smoke() {
        if std::env::var("LIBJD_TEST").as_deref() != Ok("1") {
            return;
        }
        let path =
            std::env::var_os("LIBJD_TEST_LIBRARY").expect("provide a verified libjd library");
        // The opt-in test runner supplies the digested GitHub release library.
        let backend = unsafe { JdBackend::load(Path::new(&path)) }.unwrap();
        let press = |byte: u8| backend.process_key_result(byte.into(), 0).state;

        let first = press(b'a');
        let first_choice = first.candidates[0].text.clone();
        let second_choice = first.candidates[1].text.clone();
        assert_eq!(press(b' ').committed, Some(first_choice));
        press(b'a');
        assert_eq!(press(b';').committed, Some(second_choice));

        press(b'a');
        press(b'a');
        let unique = press(b'a');
        assert_eq!(unique.candidates.len(), 1);
        assert!(unique.committed.is_none());
        let jump = press(b'h');
        assert_eq!(jump.committed, Some(unique.candidates[0].text.clone()));
        assert_eq!(jump.preedit, "h");
        assert!(backend
            .process_key_result(XK_BACK_SPACE, 0)
            .state
            .preedit
            .is_empty());

        press(b'a');
        let next = backend.change_page(false);
        assert_eq!(next.page, 1);
        assert_eq!(
            backend.select_candidate_on_page(1).committed.as_ref(),
            Some(&next.candidates[1].text)
        );
        assert!(backend.state().candidates.is_empty());
    }
}
