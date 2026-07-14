//! Shared state between all TSF COM objects.

use keytao_core::{
    default_shared_data_dir, default_user_data_dir, ImeRuntime, ImeRuntimeSession, ImeState,
    WINDOWS_IME_ENGINE_INIT_MUTEX_NAME,
};
use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::{
    core::{implement, Interface, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, FreeLibrary, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0},
        System::LibraryLoader::{
            LoadLibraryExW, LOAD_LIBRARY_FLAGS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        },
        System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        UI::Input::KeyboardAndMouse::GetFocus,
        UI::TextServices::{
            ITfComposition, ITfContext, ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl,
            ITfKeyEventSink, ITfThreadFocusSink, ITfThreadMgr, ITfThreadMgrEventSink,
            GUID_PROP_ATTRIBUTE, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_ASYNC, TF_ES_READWRITE,
            TF_TMAE_UIELEMENTENABLEDONLY, TF_TMF_UIELEMENTENABLEDONLY,
        },
    },
};

use crate::{
    candidate_ui::CandidateUiManager, globals::DllActivityGuard, language_bar::LanguageBarItem,
};

const RELOAD_STAMP_FILE: &str = "keytao-ime.reload";
const ENGINE_RETRY_DELAY: Duration = Duration::from_secs(5);
const ENGINE_INIT_MUTEX_TIMEOUT_MS: u32 = 30_000;
static FILE_DIAGNOSTICS_ENABLED: OnceLock<bool> = OnceLock::new();

// ── Shared TsfState ───────────────────────────────────────────────────────────

pub struct TsfState {
    pub runtime: Option<ImeRuntime>,
    pub session: Option<ImeRuntimeSession>,
    rime_dll: Option<LoadedRimeDll>,
    pub engine_building: bool,
    pub engine_error: Option<String>,
    engine_retry_after: Option<Instant>,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub thread_mgr_sink: Option<ITfThreadMgrEventSink>,
    pub thread_mgr_sink_cookie: Option<u32>,
    pub thread_focus_sink: Option<ITfThreadFocusSink>,
    pub thread_focus_sink_cookie: Option<u32>,
    pub client_id: u32,
    pub activation_flags: u32,
    pub thread_mgr_flags: u32,
    pub display_attribute_atom: Option<u32>,
    pub key_sink: Option<ITfKeyEventSink>,
    pub(crate) language_bar: Option<LanguageBarItem>,
    pub composition: Option<ITfComposition>,
    pub composition_context: Option<ITfContext>,
    pub ime_state: Option<ImeState>,
    pub candidate_win: crate::candidate_win::CandidateWindow,
    pub mode_hint_win: crate::candidate_win::CandidateWindow,
    pub candidate_ui: Option<CandidateUiManager>,
    pub ascii_mode: bool,
    pub reload_stamp_path: Option<PathBuf>,
    pub reload_stamp_signature: Option<String>,
    pub reload_in_progress: bool,
    pub reload_clear_pending: bool,
    reload_retry_after: Option<Instant>,
    pub shift_pressed_without_key: bool,
    session_reset_pending: bool,
    engine_build_mailbox: Arc<EngineBuildMailbox>,
    reload_mailbox: Arc<EngineBuildMailbox>,
}

pub(crate) struct EngineBundle {
    runtime: ImeRuntime,
    session: ImeRuntimeSession,
    reload_stamp_path: PathBuf,
    reload_stamp_signature: String,
    rime_dll: Option<LoadedRimeDll>,
}

struct EngineBuildMailbox {
    result: Mutex<Option<Result<EngineBundle, String>>>,
}

impl EngineBuildMailbox {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
        }
    }

    fn store(&self, result: Result<EngineBundle, String>) {
        *self
            .result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
    }

    fn take(&self) -> Option<Result<EngineBundle, String>> {
        self.result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

struct LoadedRimeDll(HMODULE);

struct EngineInitGuard(HANDLE);

#[implement(ITfEditSession)]
struct CompositionEndEditSession {
    context: ITfContext,
    composition: ITfComposition,
    _dll_guard: DllActivityGuard,
}

impl ITfEditSession_Impl for CompositionEndEditSession_Impl {
    fn DoEditSession(&self, edit_cookie: u32) -> windows::core::Result<()> {
        unsafe {
            let range = self.composition.GetRange()?;
            if let Ok(property) = self.context.GetProperty(&GUID_PROP_ATTRIBUTE) {
                let _ = property.Clear(edit_cookie, &range);
            }
            range.SetText(edit_cookie, 0, &[])?;
            self.composition.EndComposition(edit_cookie)
        }
    }
}

// SAFETY: The handle is retained only to keep the lazily loaded module alive
// while the TSF text service owns the librime runtime. It is not dereferenced.
unsafe impl Send for LoadedRimeDll {}

unsafe impl Send for EngineInitGuard {}

impl EngineInitGuard {
    fn acquire() -> Result<Self, String> {
        let mut name: Vec<u16> = WINDOWS_IME_ENGINE_INIT_MUTEX_NAME.encode_utf16().collect();
        name.push(0);
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
            .map_err(|error| format!("create engine initialization mutex: {error}"))?;
        let wait = unsafe { WaitForSingleObject(handle, ENGINE_INIT_MUTEX_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(format!(
                "wait for engine initialization mutex: 0x{:08x}",
                wait.0
            ));
        }
        Ok(Self(handle))
    }
}

impl Drop for EngineInitGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

impl Drop for LoadedRimeDll {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

impl TsfState {
    pub fn new() -> Self {
        Self {
            runtime: None,
            session: None,
            rime_dll: None,
            engine_building: false,
            engine_error: None,
            engine_retry_after: None,
            thread_mgr: None,
            thread_mgr_sink: None,
            thread_mgr_sink_cookie: None,
            thread_focus_sink: None,
            thread_focus_sink_cookie: None,
            client_id: 0,
            activation_flags: 0,
            thread_mgr_flags: 0,
            display_attribute_atom: None,
            key_sink: None,
            language_bar: None,
            composition: None,
            composition_context: None,
            ime_state: None,
            candidate_win: crate::candidate_win::CandidateWindow::new(),
            mode_hint_win: crate::candidate_win::CandidateWindow::new(),
            candidate_ui: Some(CandidateUiManager::new()),
            ascii_mode: false,
            reload_stamp_path: None,
            reload_stamp_signature: None,
            reload_in_progress: false,
            reload_clear_pending: false,
            reload_retry_after: None,
            shift_pressed_without_key: false,
            session_reset_pending: false,
            engine_build_mailbox: Arc::new(EngineBuildMailbox::new()),
            reload_mailbox: Arc::new(EngineBuildMailbox::new()),
        }
    }

    pub(crate) fn build_engine() -> Result<EngineBundle, String> {
        let _init_guard = EngineInitGuard::acquire()?;
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let shared = bundled_shared_data_dir().unwrap_or_else(default_shared_data_dir);
        let rime_dll = preload_rime_dll(&shared)?;
        let reload_stamp_path = user_dir.join(RELOAD_STAMP_FILE);
        let runtime = ImeRuntime::with_dirs(user_dir, shared);
        runtime.init_without_deploy()?;
        let session = runtime.create_session()?;
        let reload_stamp_signature = reload_stamp_signature(&reload_stamp_path);
        Ok(EngineBundle {
            runtime,
            session,
            reload_stamp_path,
            reload_stamp_signature,
            rime_dll,
        })
    }

    pub(crate) fn install_engine(&mut self, bundle: EngineBundle) {
        self.runtime = Some(bundle.runtime);
        self.session = Some(bundle.session);
        self.reload_stamp_signature = Some(bundle.reload_stamp_signature);
        self.reload_stamp_path = Some(bundle.reload_stamp_path);
        self.rime_dll = bundle.rime_dll;
        if self.ascii_mode {
            self.ime_state = self
                .session
                .as_ref()
                .and_then(|session| session.set_ascii_mode(true));
        }
        self.engine_building = false;
        self.engine_error = None;
        self.engine_retry_after = None;
    }

    pub(crate) fn engine_ready(&self) -> bool {
        self.session.is_some()
    }

    pub(crate) fn begin_engine_build(&mut self) -> bool {
        if self.engine_ready() || self.engine_building || self.reload_in_progress {
            return false;
        }
        if self
            .engine_retry_after
            .is_some_and(|retry_after| Instant::now() < retry_after)
        {
            return false;
        }
        self.engine_building = true;
        self.engine_error = None;
        self.engine_retry_after = None;
        true
    }

    pub(crate) fn finish_engine_build_error(&mut self, error: String) {
        self.engine_building = false;
        self.engine_error = Some(error);
        self.engine_retry_after = Some(Instant::now() + ENGINE_RETRY_DELAY);
    }

    pub(crate) fn begin_reload_if_changed(&mut self) -> bool {
        if self.reload_in_progress || !self.reload_needed() {
            return false;
        }
        if self.reload_stamp_path.is_none() {
            return false;
        }
        if self
            .reload_retry_after
            .is_some_and(|retry_after| Instant::now() < retry_after)
        {
            return false;
        }
        self.reload_in_progress = true;
        self.session = None;
        self.runtime = None;
        self.rime_dll = None;
        self.ime_state = None;
        self.candidate_win.hide();
        self.mode_hint_win.hide();
        self.reload_clear_pending = true;
        self.reload_retry_after = None;
        true
    }

    pub(crate) fn reload_needed(&self) -> bool {
        if self.reload_in_progress {
            return true;
        }
        let Some(path) = &self.reload_stamp_path else {
            return false;
        };
        let signature = reload_stamp_signature(path);
        self.reload_stamp_signature.as_deref() != Some(signature.as_str())
    }

    pub(crate) fn finish_reload(&mut self, bundle: Result<EngineBundle, String>) {
        self.reload_in_progress = false;
        match bundle {
            Ok(bundle) => {
                self.install_engine(bundle);
                self.reload_clear_pending = true;
                self.reload_retry_after = None;
            }
            Err(error) => {
                self.engine_error = Some(error);
                self.reload_retry_after = Some(Instant::now() + ENGINE_RETRY_DELAY);
            }
        }
    }

    pub(crate) fn take_reload_clear_pending(&mut self) -> bool {
        std::mem::take(&mut self.reload_clear_pending)
    }

    fn poll_engine_builds(&mut self) {
        if let Some(result) = self.engine_build_mailbox.take() {
            match result {
                Ok(bundle) if !self.engine_ready() => self.install_engine(bundle),
                Ok(_) => self.engine_building = false,
                Err(error) => self.finish_engine_build_error(error),
            }
        }
        if let Some(result) = self.reload_mailbox.take() {
            self.finish_reload(result);
        }
    }

    pub(crate) fn session(&self) -> Option<ImeRuntimeSession> {
        self.session.clone()
    }
}

pub type SharedState = Rc<RefCell<TsfState>>;
pub type WeakState = Weak<RefCell<TsfState>>;

pub fn new_shared_state() -> SharedState {
    Rc::new(RefCell::new(TsfState::new()))
}

pub(crate) fn append_diagnostic(message: impl AsRef<str>) {
    let enabled = FILE_DIAGNOSTICS_ENABLED.get_or_init(|| {
        cfg!(debug_assertions)
            || std::env::var("KEYTAO_WINDOWS_IME_DIAGNOSTICS")
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
    });
    if !enabled {
        return;
    }

    let Some(user_dir) = default_user_data_dir() else {
        return;
    };

    let log_dir = user_dir.join("log");
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let log_path = log_dir.join("windows-ime.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "[{timestamp}] {}", message.as_ref());
    }
}

pub(crate) fn start_engine_warmup(shared_state: &SharedState) {
    let mailbox = {
        let mut st = shared_state.borrow_mut();
        if !st.begin_engine_build() {
            return;
        }
        Arc::clone(&st.engine_build_mailbox)
    };

    append_diagnostic("engine warmup started");
    let dll_guard = DllActivityGuard::new();
    let spawn_result = std::thread::Builder::new()
        .name("keytao-ime-warmup".into())
        .spawn(move || {
            let result = TsfState::build_engine();
            match &result {
                Ok(_) => {
                    tracing::info!("KeyTao Windows IME engine warmed up");
                    append_diagnostic("engine warmup succeeded");
                }
                Err(error) => {
                    tracing::error!("librime init failed: {error}");
                    append_diagnostic(format!("engine warmup failed: {error}"));
                }
            }
            mailbox.store(result);
            drop(mailbox);
            drop(dll_guard);
        });
    if let Err(error) = spawn_result {
        let message = format!("start engine warmup thread: {error}");
        append_diagnostic(&message);
        shared_state.borrow_mut().finish_engine_build_error(message);
    }
}

pub(crate) fn start_reload_if_needed(shared_state: &SharedState) -> bool {
    let should_reload = {
        let mut st = shared_state.borrow_mut();
        st.begin_reload_if_changed()
    };

    if !should_reload {
        return false;
    }

    let mailbox = {
        let st = shared_state.borrow();
        Arc::clone(&st.reload_mailbox)
    };
    let dll_guard = DllActivityGuard::new();
    append_diagnostic("engine reload started");
    let spawn_result = std::thread::Builder::new()
        .name("keytao-ime-reload".into())
        .spawn(move || {
            let bundle = TsfState::build_engine();
            match &bundle {
                Ok(_) => {
                    tracing::info!("librime session refreshed after reload stamp change");
                    append_diagnostic("engine reload succeeded");
                }
                Err(error) => {
                    tracing::error!("librime reload failed: {error}");
                    append_diagnostic(format!("engine reload failed: {error}"));
                }
            }
            mailbox.store(bundle);
            drop(mailbox);
            drop(dll_guard);
        });
    if let Err(error) = spawn_result {
        let message = format!("start engine reload thread: {error}");
        append_diagnostic(&message);
        shared_state.borrow_mut().finish_reload(Err(message));
        return false;
    }
    true
}

pub(crate) fn poll_engine_builds(shared_state: &SharedState) {
    shared_state.borrow_mut().poll_engine_builds();
}

pub(crate) fn refresh_engine_for_focus(shared_state: &SharedState) {
    poll_engine_builds(shared_state);
    if shared_state.borrow().engine_ready() {
        start_reload_if_needed(shared_state);
    } else {
        start_engine_warmup(shared_state);
    }
}

pub(crate) fn apply_pending_session_reset(shared_state: &SharedState) {
    let session = {
        let mut state = shared_state.borrow_mut();
        if !std::mem::take(&mut state.session_reset_pending) {
            return;
        }
        state.session()
    };
    if let Some(session) = session {
        let ime_state = session.reset();
        shared_state.borrow_mut().ime_state = ime_state;
    }
}

pub(crate) fn update_language_bar_mode(shared_state: &SharedState, ascii_mode: bool) {
    let language_bar = shared_state.borrow().language_bar.clone();
    if let Some(language_bar) = language_bar {
        language_bar.update_mode(ascii_mode);
    }
}

pub(crate) fn set_ascii_mode_from_language_bar(shared_state: &SharedState, ascii_mode: bool) {
    reset_input_for_focus_change(shared_state);
    poll_engine_builds(shared_state);
    let session = shared_state.borrow().session();
    let ime_state = session
        .as_ref()
        .and_then(|session| session.set_ascii_mode(ascii_mode));
    {
        let mut state = shared_state.borrow_mut();
        state.ascii_mode = ascii_mode;
        state.ime_state = ime_state;
    }
    if session.is_none() {
        start_engine_warmup(shared_state);
    }
    update_language_bar_mode(shared_state, ascii_mode);
}

pub(crate) fn update_ime_windows(
    shared_state: &SharedState,
    ime_state: &ImeState,
    document_mgr: Option<&ITfDocumentMgr>,
    caret_x: i32,
    caret_y: i32,
    owner_hwnd: windows::Win32::Foundation::HWND,
    show_mode_hint: bool,
) {
    let (thread_mgr, allow_fallback_window) = {
        let st = shared_state.borrow();
        let uiless = st.activation_flags & TF_TMAE_UIELEMENTENABLEDONLY != 0
            || st.thread_mgr_flags & TF_TMF_UIELEMENTENABLEDONLY != 0;
        (st.thread_mgr.clone(), !uiless)
    };
    let allow_candidate_window = with_detached_candidate_ui(shared_state, |candidate_ui| {
        candidate_ui.update(
            thread_mgr.as_ref(),
            document_mgr,
            ime_state,
            allow_fallback_window,
        )
    });
    with_detached_windows(shared_state, |candidate_win, mode_hint_win| {
        let show = !ime_state.candidates.is_empty() || !ime_state.preedit.is_empty();
        if show && allow_candidate_window {
            candidate_win.show(ime_state, caret_x, caret_y, owner_hwnd);
        } else {
            candidate_win.hide();
        }
        if show_mode_hint {
            mode_hint_win.show_mode_hint(ime_state.ascii_mode, caret_x, caret_y, owner_hwnd);
        }
    });
}

pub(crate) fn hide_ime_windows(shared_state: &SharedState) {
    with_detached_candidate_ui(shared_state, CandidateUiManager::end);
    with_detached_windows(shared_state, |candidate_win, mode_hint_win| {
        candidate_win.hide();
        mode_hint_win.hide();
    });
}

pub(crate) fn reset_input_for_focus_change(shared_state: &SharedState) {
    let active_composition = {
        let mut st = shared_state.borrow_mut();
        st.shift_pressed_without_key = false;
        st.session_reset_pending = true;
        let active_composition = st
            .composition
            .take()
            .zip(st.composition_context.take())
            .map(|(composition, context)| (context, composition, st.client_id));
        st.ime_state = None;
        active_composition
    };
    if let Some((context, composition, client_id)) = active_composition {
        request_composition_end(context, composition, client_id);
    }
    hide_ime_windows(shared_state);
}

pub(crate) fn clear_input_after_composition_terminated(
    shared_state: &SharedState,
    terminated: Option<&ITfComposition>,
) {
    let Some(terminated) = terminated else {
        return;
    };
    let is_active_composition = shared_state
        .borrow()
        .composition
        .as_ref()
        .is_some_and(|active| active.as_raw() == terminated.as_raw());
    if !is_active_composition {
        return;
    }

    {
        let mut st = shared_state.borrow_mut();
        st.shift_pressed_without_key = false;
        st.session_reset_pending = true;
        st.composition = None;
        st.composition_context = None;
        st.ime_state = None;
    }
    hide_ime_windows(shared_state);
}

fn request_composition_end(context: ITfContext, composition: ITfComposition, client_id: u32) {
    if client_id == 0 {
        return;
    }
    let edit_session: ITfEditSession = CompositionEndEditSession {
        context: context.clone(),
        composition,
        _dll_guard: DllActivityGuard::new(),
    }
    .into();
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_ASYNC.0 | TF_ES_READWRITE.0);
    let result = unsafe { context.RequestEditSession(client_id, &edit_session, flags) }
        .and_then(|session_result| session_result.ok());
    if let Err(error) = result {
        append_diagnostic(format!(
            "failed to end composition after focus change: {error}"
        ));
    }
}

pub(crate) fn hide_candidate_window(shared_state: &SharedState) {
    with_detached_candidate_ui(shared_state, CandidateUiManager::end);
    with_detached_windows(shared_state, |candidate_win, _mode_hint_win| {
        candidate_win.hide();
    });
}

fn with_detached_windows<R>(
    shared_state: &SharedState,
    f: impl FnOnce(
        &mut crate::candidate_win::CandidateWindow,
        &mut crate::candidate_win::CandidateWindow,
    ) -> R,
) -> R {
    let (mut candidate_win, mut mode_hint_win) = {
        let mut st = shared_state.borrow_mut();
        (
            std::mem::replace(
                &mut st.candidate_win,
                crate::candidate_win::CandidateWindow::new(),
            ),
            std::mem::replace(
                &mut st.mode_hint_win,
                crate::candidate_win::CandidateWindow::new(),
            ),
        )
    };

    let result = f(&mut candidate_win, &mut mode_hint_win);

    let (replaced_candidate, replaced_mode_hint) = {
        let mut st = shared_state.borrow_mut();
        (
            std::mem::replace(&mut st.candidate_win, candidate_win),
            std::mem::replace(&mut st.mode_hint_win, mode_hint_win),
        )
    };
    drop((replaced_candidate, replaced_mode_hint));
    result
}

fn with_detached_candidate_ui<R>(
    shared_state: &SharedState,
    f: impl FnOnce(&mut CandidateUiManager) -> R,
) -> R {
    let mut candidate_ui = {
        let mut st = shared_state.borrow_mut();
        st.candidate_ui
            .take()
            .unwrap_or_else(CandidateUiManager::new)
    };

    let result = f(&mut candidate_ui);

    let replaced = {
        let mut st = shared_state.borrow_mut();
        st.candidate_ui.replace(candidate_ui)
    };
    drop(replaced);
    result
}

fn has_rime_base_data(dir: &Path) -> bool {
    dir.join("default.yaml").is_file()
}

fn reload_stamp_signature(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            format!("{}:{}", metadata.len(), modified)
        }
        Err(_) => "missing".to_string(),
    }
}

fn preload_rime_dll(shared_data_dir: &str) -> Result<Option<LoadedRimeDll>, String> {
    let Some(runtime_dir) = Path::new(shared_data_dir).parent() else {
        return Ok(None);
    };
    let rime_dll = if cfg!(target_arch = "aarch64") {
        runtime_dir.join("rime-arm64.dll")
    } else {
        runtime_dir.join("rime.dll")
    };
    if !rime_dll.is_file() {
        return Ok(None);
    }

    let mut wide: Vec<u16> = rime_dll.to_string_lossy().encode_utf16().collect();
    wide.push(0);
    let flags =
        LOAD_LIBRARY_FLAGS(LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR.0 | LOAD_LIBRARY_SEARCH_SYSTEM32.0);
    let module = unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), HANDLE::default(), flags) }
        .map_err(|e| format!("load bundled rime.dll from {}: {e}", rime_dll.display()))?;
    Ok(Some(LoadedRimeDll(module)))
}

fn bundled_shared_data_dir() -> Option<String> {
    for base in dll_related_dirs() {
        for candidate in [
            base.join("rime-data"),
            base.join("resources").join("rime-data"),
            base.join("share").join("rime-data"),
        ] {
            if has_rime_base_data(&candidate) {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn dll_related_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(hmodule) = crate::globals::DLL_INSTANCE.get().copied() {
        let mut buf = vec![0u16; 32768];
        let len = unsafe {
            windows::Win32::System::LibraryLoader::GetModuleFileNameW(
                windows::Win32::Foundation::HMODULE(hmodule as _),
                &mut buf,
            )
        } as usize;
        if len > 0 {
            if let Some(parent) = PathBuf::from(String::from_utf16_lossy(&buf[..len])).parent() {
                dirs.push(parent.to_path_buf());
            }
        }
    }

    if let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        dirs.push(parent);
    }

    dirs
}

pub(crate) fn fallback_focus_window() -> windows::Win32::Foundation::HWND {
    unsafe { GetFocus() }
}
