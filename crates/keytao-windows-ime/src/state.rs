//! Shared state between all TSF COM objects.

use keytao_core::{
    default_shared_data_dir, default_user_data_dir, ImeRuntime, ImeRuntimeSession, ImeState,
    InputContextPolicy, ReloadStamp, WINDOWS_IME_ENGINE_INIT_MUTEX_NAME,
};
use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, FreeLibrary, HANDLE, HMODULE, HWND, WAIT_ABANDONED, WAIT_OBJECT_0,
        },
        System::LibraryLoader::{
            LoadLibraryExW, LOAD_LIBRARY_FLAGS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        },
        System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        UI::Input::KeyboardAndMouse::GetFocus,
        UI::TextServices::{
            ITfComposition, ITfContext, ITfContextOwnerCompositionServices, ITfDocumentMgr,
            ITfKeyEventSink, ITfSource, ITfTextLayoutSink, ITfThreadFocusSink, ITfThreadMgr,
            ITfThreadMgrEventSink, GUID_PROP_ATTRIBUTE, TF_TMAE_UIELEMENTENABLEDONLY,
            TF_TMF_UIELEMENTENABLEDONLY,
        },
    },
};

use crate::{
    candidate_ui::CandidateUiManager,
    edit_session::with_write_session,
    globals::DllActivityGuard,
    input_context::{inspect_context, ContextInputState},
    key_event_sink::{has_visible_state, should_arm_caret_reprobe, CaretSource},
    language_bar::LanguageBarItem,
};

const ENGINE_RETRY_DELAY: Duration = Duration::from_secs(5);
const ENGINE_INIT_MUTEX_TIMEOUT_MS: u32 = 30_000;
/// How often the key path may stat the reload stamp. Focus changes always
/// force a fresh read, so a schema update is still picked up before the first
/// key of the next focus; this only keeps typing off the file system.
const RELOAD_STAMP_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How soon a context whose probes came back `Unknown` may be inspected again
/// from the key path. An unanswered probe fails closed, so without a retry a
/// single refused read session would keep the field in pass-through until the
/// next focus change; inspecting costs a synchronous read session, so a host
/// that never grants one must not get one request per keystroke.
const INPUT_CONTEXT_RETRY_INTERVAL: Duration = Duration::from_millis(250);
static FILE_DIAGNOSTICS_ENABLED: OnceLock<bool> = OnceLock::new();
static THEME_RESOLVER: OnceLock<keytao_theme::ThemeResolver> = OnceLock::new();

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
    reload_stamp_checked_at: Option<Instant>,
    reload_stamp_changed: bool,
    pub shift_pressed_without_key: bool,
    session_reset_pending: bool,
    /// What TSF says about the focused context (password field, disabled
    /// keyboard, empty context). Refreshed on focus changes.
    pub input_context: ContextInputState,
    /// Cached `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE`. A closed keyboard passes
    /// every keystroke through unaltered.
    pub keyboard_open: bool,
    /// Whether the running session already runs with the sensitive policy;
    /// `None` after an engine (re)build, so the policy is pushed again.
    input_policy_applied: Option<bool>,
    /// Earliest instant at which an unanswered `input_context` probe may be
    /// retried. Cleared by every inspection, so a fresh context is probed at
    /// once and only a repeatedly failing one is throttled.
    input_context_retry_after: Option<Instant>,
    /// Context of the last key event, used by candidate-window clicks which
    /// arrive outside any TSF callback.
    pub key_context: Option<ITfContext>,
    /// Last caret position the host reported. Reused while the layout is not
    /// computed yet (`TF_E_NOLAYOUT`) so the panel never jumps to a corner.
    pub last_caret: Option<CaretPosition>,
    pub caret_retry_attempts: u8,
    pub caret_retry_mode_hint: bool,
    pub ime_write_session_active: bool,
    pub caret_probe_session_in_progress: bool,
    /// COM identity of the composition temporarily owned by the active write
    /// session while `st.composition` is taken out of shared state.
    pub composition_in_flight: Option<usize>,
    pub composition_terminated_in_session: bool,
    layout_sink: Option<LayoutSinkRegistration>,
    compartment_sinks: Vec<CompartmentSinkRegistration>,
    /// Sinks on the focused context's `KEYBOARD_DISABLED` / `EMPTYCONTEXT`
    /// compartments. Unlike the thread-manager ones these live on the context,
    /// so they are re-advised whenever the focus moves.
    context_compartment_sinks: Vec<CompartmentSinkRegistration>,
    /// The context `context_compartment_sinks` are advised on.
    context_sink_context: Option<ITfContext>,
    engine_build_mailbox: Arc<EngineBuildMailbox>,
    reload_mailbox: Arc<EngineBuildMailbox>,
}

/// Caret position in screen coordinates plus the window the candidate popup is
/// owned by.
#[derive(Clone, Copy)]
pub struct CaretPosition {
    pub x: i32,
    pub y: i32,
    pub owner_hwnd: HWND,
}

pub(crate) struct LayoutSinkRegistration {
    pub(crate) source: ITfSource,
    pub(crate) cookie: u32,
    pub(crate) sink: ITfTextLayoutSink,
    pub(crate) context: ITfContext,
}

pub(crate) struct CompartmentSinkRegistration {
    pub(crate) source: ITfSource,
    pub(crate) cookie: u32,
}

pub(crate) struct EngineBundle {
    runtime: ImeRuntime,
    session: ImeRuntimeSession,
    reload_stamp_path: PathBuf,
    reload_stamp_signature: Option<String>,
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
            mode_hint_win: crate::candidate_win::CandidateWindow::new_mode_hint(),
            candidate_ui: Some(CandidateUiManager::new()),
            ascii_mode: false,
            reload_stamp_path: None,
            reload_stamp_signature: None,
            reload_in_progress: false,
            reload_clear_pending: false,
            reload_retry_after: None,
            reload_stamp_checked_at: None,
            reload_stamp_changed: false,
            shift_pressed_without_key: false,
            session_reset_pending: false,
            input_context: ContextInputState::default(),
            keyboard_open: true,
            input_policy_applied: None,
            input_context_retry_after: None,
            key_context: None,
            last_caret: None,
            caret_retry_attempts: 0,
            caret_retry_mode_hint: false,
            ime_write_session_active: false,
            caret_probe_session_in_progress: false,
            composition_in_flight: None,
            composition_terminated_in_session: false,
            layout_sink: None,
            compartment_sinks: Vec::new(),
            context_compartment_sinks: Vec::new(),
            context_sink_context: None,
            engine_build_mailbox: Arc::new(EngineBuildMailbox::new()),
            reload_mailbox: Arc::new(EngineBuildMailbox::new()),
        }
    }

    pub(crate) fn build_engine() -> Result<EngineBundle, String> {
        let _init_guard = EngineInitGuard::acquire()?;
        let user_dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        let shared = bundled_shared_data_dir().unwrap_or_else(default_shared_data_dir);
        let rime_dll = preload_rime_dll(&shared)?;
        let reload_stamp_path = ReloadStamp::path(&user_dir);
        let runtime = ImeRuntime::with_dirs(user_dir, shared);
        runtime.init_without_deploy()?;
        let session = runtime.create_session()?;
        let reload_stamp_signature = ReloadStamp::signature_at(&reload_stamp_path);
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
        self.reload_stamp_signature = bundle.reload_stamp_signature;
        self.reload_stamp_path = Some(bundle.reload_stamp_path);
        self.rime_dll = bundle.rime_dll;
        self.invalidate_reload_stamp_cache();
        // A fresh session starts with the default policy, so the sensitive
        // policy of the focused context has to be pushed again.
        self.input_policy_applied = None;
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
        // A missing stamp is not a reload request, matching ReloadStampWatcher.
        let Some(signature) = ReloadStamp::signature_at(path) else {
            return false;
        };
        self.reload_stamp_signature.as_deref() != Some(signature.as_str())
    }

    /// `reload_needed` for the key path: stats the stamp at most every
    /// `RELOAD_STAMP_POLL_INTERVAL` and answers from the cache in between.
    pub(crate) fn reload_needed_cached(&mut self) -> bool {
        if self.reload_in_progress {
            return true;
        }
        let now = Instant::now();
        if let Some(checked_at) = self.reload_stamp_checked_at {
            if now.duration_since(checked_at) < RELOAD_STAMP_POLL_INTERVAL {
                return self.reload_stamp_changed;
            }
        }
        self.reload_stamp_checked_at = Some(now);
        self.reload_stamp_changed = self.reload_needed();
        self.reload_stamp_changed
    }

    pub(crate) fn invalidate_reload_stamp_cache(&mut self) {
        self.reload_stamp_checked_at = None;
        self.reload_stamp_changed = false;
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

pub(crate) fn diagnostics_enabled() -> bool {
    *FILE_DIAGNOSTICS_ENABLED.get_or_init(|| {
        cfg!(debug_assertions)
            || std::env::var("KEYTAO_WINDOWS_IME_DIAGNOSTICS")
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
    })
}

/// Missing theme file or key means panel-only preedit by default.
pub(crate) fn embedded_composition() -> bool {
    THEME_RESOLVER
        .get_or_init(|| {
            let theme_path = keytao_theme::default_user_theme_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "missing".to_string());
            let resolver = crate::panel::windows_theme_resolver();
            let embedded = resolver.current().ui.embedded_composition;
            append_diagnostic(format!(
                "embedded_composition setting={} theme={theme_path}",
                u8::from(embedded)
            ));
            resolver
        })
        .current()
        .ui
        .embedded_composition
}

/// Resolve the theme once before a TSF document write can synchronously call
/// `OnCompositionTerminated`. Later calls still observe theme-file changes.
pub(crate) fn prime_theme_resolver() {
    if THEME_RESOLVER.get().is_none() {
        let _ = embedded_composition();
    }
}

pub(crate) fn append_diagnostic(message: impl AsRef<str>) {
    if !diagnostics_enabled() {
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
    // A background build installs a session long after the focus change that
    // decided the policy, and `install_engine` clears the applied marker so this
    // pushes it onto the new session. It cannot wait for the key path: a
    // sensitive context never gets that far, so the session would keep composing
    // permissions it must not have.
    sync_input_policy(shared_state);
}

pub(crate) fn refresh_engine_for_focus(shared_state: &SharedState) {
    poll_engine_builds(shared_state);
    shared_state.borrow_mut().invalidate_reload_stamp_cache();
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

/// Push the input policy of the focused context onto the running session.
///
/// Runs on the key path too: the engine is built in the background and may
/// only become ready after the focus change that decided the policy.
pub(crate) fn sync_input_policy(shared_state: &SharedState) {
    let (session, sensitive, applied) = {
        let st = shared_state.borrow();
        (
            st.session(),
            st.input_context.is_sensitive(),
            st.input_policy_applied,
        )
    };
    if applied == Some(sensitive) {
        return;
    }
    let Some(session) = session else {
        return;
    };
    let policy = if sensitive {
        InputContextPolicy::sensitive()
    } else {
        InputContextPolicy::default()
    };
    // Turning composing off discards whatever librime was holding; the cached
    // state has to go with it so nothing is drawn for the sensitive context.
    let _ = session.set_input_policy(policy);
    let mut st = shared_state.borrow_mut();
    st.input_policy_applied = Some(sensitive);
    if sensitive {
        st.ime_state = None;
    }
}

/// Re-read what TSF declares about the focused context and stop composing when
/// it is a password field, a disabled keyboard or an empty context.
///
/// Also moves the `KEYBOARD_DISABLED` / `EMPTYCONTEXT` sinks onto the context
/// that was just inspected, so a host that revokes the keyboard later on is
/// heard without waiting for the next focus change.
pub(crate) fn refresh_input_context(shared_state: &SharedState, context: Option<&ITfContext>) {
    let (thread_mgr, client_id) = {
        let st = shared_state.borrow();
        (st.thread_mgr.clone(), st.client_id)
    };
    let context = crate::input_context::resolve_context(thread_mgr.as_ref(), context);
    crate::text_service::advise_context_compartment_sinks(shared_state, context.as_ref());
    let inspected = inspect_context(thread_mgr.as_ref(), context.as_ref(), client_id);
    if apply_input_context_state(shared_state, inspected) {
        // Ends a composition that may still be on screen as well as hiding the
        // UI. Every caller happens to reset first today, but a context that
        // turned out sensitive must not depend on that to stop showing preedit.
        reset_input_for_focus_change(shared_state);
    }
}

/// Store a freshly inspected context state and push the matching policy.
/// Returns whether the context turned out to be sensitive.
///
/// Takes the state rather than inspecting itself: the compartment sink refreshes
/// only the two compartments, while a focus change re-runs the input-scope probe
/// as well.
fn apply_input_context_state(shared_state: &SharedState, input_context: ContextInputState) -> bool {
    let sensitive = input_context.is_sensitive();
    {
        let mut st = shared_state.borrow_mut();
        st.input_context = input_context;
        // A fresh answer re-arms the throttle: the next failure gets an
        // immediate retry rather than inheriting an old deadline.
        st.input_context_retry_after = None;
        st.keyboard_open = crate::input_context::keyboard_is_open(st.thread_mgr.as_ref());
    }
    sync_input_policy(shared_state);
    sensitive
}

/// Inspect the context again when the last attempt could not answer.
///
/// Runs from the key callbacks. `ContextProbe::Unknown` fails closed, which is
/// right for the keystroke at hand but would strand an ordinary text field in
/// pass-through forever if the refusal was momentary — hosts routinely refuse a
/// synchronous read session while the document is locked.
pub(crate) fn retry_input_context_if_unknown(
    shared_state: &SharedState,
    context: Option<&ITfContext>,
) {
    let due = {
        let st = shared_state.borrow();
        input_context_retry_due(
            st.input_context.needs_retry(),
            st.client_id,
            st.input_context_retry_after,
            Instant::now(),
        )
    };
    if !due {
        return;
    }
    refresh_input_context(shared_state, context);
    // Set after the inspection: it clears the field, and a still-unanswered
    // probe has to wait before costing another edit session.
    shared_state.borrow_mut().input_context_retry_after =
        Some(Instant::now() + INPUT_CONTEXT_RETRY_INTERVAL);
}

/// Whether an unanswered context probe may be inspected again.
///
/// A context that answered is left alone, an inspection without a client id
/// cannot open the read session it needs, and a deadline in the future means
/// the previous attempt just failed.
fn input_context_retry_due(
    needs_retry: bool,
    client_id: u32,
    retry_after: Option<Instant>,
    now: Instant,
) -> bool {
    needs_retry && client_id != 0 && retry_after.is_none_or(|deadline| now >= deadline)
}

/// A `KEYBOARD_DISABLED` / `EMPTYCONTEXT` compartment on the focused context
/// changed.
///
/// The host may have done this while a composition was already on screen, so a
/// context that just became sensitive has its preedit, candidate window and
/// native composition torn down here rather than at the next focus change.
pub(crate) fn apply_context_compartment_change(shared_state: &SharedState) {
    let (thread_mgr, context, previous) = {
        let st = shared_state.borrow();
        (
            st.thread_mgr.clone(),
            st.context_sink_context.clone(),
            st.input_context,
        )
    };
    let refreshed = crate::input_context::refresh_context_compartments(
        thread_mgr.as_ref(),
        context.as_ref(),
        previous,
    );
    let sensitive = apply_input_context_state(shared_state, refreshed);
    if sensitive && !previous.is_sensitive() {
        // Ends the composition through a queued write session; the document is
        // very likely still locked underneath this callback.
        reset_input_for_focus_change(shared_state);
    }
}

/// Drop whatever is still on screen for a context that has just been blocked.
///
/// Safe to call from `OnTestKeyDown` / `OnTestKeyUp`: the composition is ended
/// through a queued write session, never inside the callback, and the early
/// return keeps the common "blocked and nothing to clean" case free.
pub(crate) fn clear_input_for_blocked_context(shared_state: &SharedState) {
    let has_input = {
        let st = shared_state.borrow();
        st.composition.is_some() || st.ime_state.is_some()
    };
    if !has_input {
        return;
    }
    reset_input_for_focus_change(shared_state);
}

pub(crate) fn set_keyboard_open_state(shared_state: &SharedState, open: bool) {
    shared_state.borrow_mut().keyboard_open = open;
    if !open {
        reset_input_for_focus_change(shared_state);
    }
}

/// Declare the thread compartments a keyboard TIP owns: the keyboard is open
/// and, unless the user already switched, in native (Chinese) conversion mode.
pub(crate) fn publish_initial_compartments(shared_state: &SharedState) {
    let (thread_mgr, client_id, ascii_mode) = {
        let st = shared_state.borrow();
        (st.thread_mgr.clone(), st.client_id, st.ascii_mode)
    };
    crate::input_context::set_keyboard_open(thread_mgr.as_ref(), client_id, true);
    shared_state.borrow_mut().keyboard_open = true;
    update_language_bar_mode(shared_state, ascii_mode);
}

/// `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE` changed (system Ctrl+Space, the input
/// indicator). A closed keyboard stops composing and passes keys through.
pub(crate) fn apply_open_close_change(shared_state: &SharedState) {
    let (thread_mgr, current) = {
        let st = shared_state.borrow();
        (st.thread_mgr.clone(), st.keyboard_open)
    };
    let open = crate::input_context::keyboard_is_open(thread_mgr.as_ref());
    if open == current {
        return;
    }
    set_keyboard_open_state(shared_state, open);
}

/// `GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION` changed. The language bar
/// writes the same value back, so only an actual difference reaches Rime.
pub(crate) fn apply_conversion_mode_change(shared_state: &SharedState) {
    let (thread_mgr, current) = {
        let st = shared_state.borrow();
        (st.thread_mgr.clone(), st.ascii_mode)
    };
    let Some(ascii_mode) = crate::input_context::conversion_mode_is_ascii(thread_mgr.as_ref())
    else {
        return;
    };
    if ascii_mode == current {
        return;
    }
    set_ascii_mode_from_language_bar(shared_state, ascii_mode);
}

/// True when neither the context nor the system state allows composing.
pub(crate) fn input_is_blocked(shared_state: &SharedState, context: Option<&ITfContext>) -> bool {
    let (thread_mgr, keyboard_open, sensitive) = {
        let st = shared_state.borrow();
        (
            st.thread_mgr.clone(),
            st.keyboard_open,
            st.input_context.is_sensitive(),
        )
    };
    if !keyboard_open || sensitive {
        return true;
    }
    crate::input_context::keyboard_is_disabled(thread_mgr.as_ref(), context)
}

pub(crate) fn store_layout_sink(shared_state: &SharedState, registration: LayoutSinkRegistration) {
    let previous = shared_state.borrow_mut().layout_sink.replace(registration);
    drop_layout_sink(previous);
}

pub(crate) fn clear_layout_sink(shared_state: &SharedState) {
    let previous = shared_state.borrow_mut().layout_sink.take();
    drop_layout_sink(previous);
}

pub(crate) fn layout_sink_context(shared_state: &SharedState) -> Option<ITfContext> {
    shared_state
        .borrow()
        .layout_sink
        .as_ref()
        .map(|registration| registration.context.clone())
}

fn drop_layout_sink(registration: Option<LayoutSinkRegistration>) {
    let Some(registration) = registration else {
        return;
    };
    unsafe {
        let _ = registration.source.UnadviseSink(registration.cookie);
    }
    drop(registration.sink);
}

pub(crate) fn store_compartment_sink(
    shared_state: &SharedState,
    registration: CompartmentSinkRegistration,
) {
    shared_state
        .borrow_mut()
        .compartment_sinks
        .push(registration);
}

pub(crate) fn clear_compartment_sinks(shared_state: &SharedState) {
    let registrations = std::mem::take(&mut shared_state.borrow_mut().compartment_sinks);
    unadvise_all(registrations);
}

/// True when the context sinks are already advised on exactly this context, so
/// a refresh does not have to churn the registrations.
pub(crate) fn context_compartment_sinks_cover(
    shared_state: &SharedState,
    context: Option<&ITfContext>,
) -> bool {
    let st = shared_state.borrow();
    match (st.context_sink_context.as_ref(), context) {
        // COM identity: the same context handed out twice is the same pointer.
        (Some(advised), Some(wanted)) => advised.as_raw() == wanted.as_raw(),
        (None, None) => true,
        _ => false,
    }
}

pub(crate) fn store_context_compartment_sinks(
    shared_state: &SharedState,
    context: Option<ITfContext>,
    registrations: Vec<CompartmentSinkRegistration>,
) {
    let previous = {
        let mut st = shared_state.borrow_mut();
        st.context_sink_context = context;
        std::mem::replace(&mut st.context_compartment_sinks, registrations)
    };
    unadvise_all(previous);
}

/// Unadvise the focused context's compartment sinks and forget the context.
///
/// The registration keeps a reference to a host object, so it must not outlive
/// the focus that created it.
pub(crate) fn clear_context_compartment_sinks(shared_state: &SharedState) {
    let registrations = {
        let mut st = shared_state.borrow_mut();
        st.context_sink_context = None;
        std::mem::take(&mut st.context_compartment_sinks)
    };
    unadvise_all(registrations);
}

fn unadvise_all(registrations: Vec<CompartmentSinkRegistration>) {
    for registration in registrations {
        unsafe {
            let _ = registration.source.UnadviseSink(registration.cookie);
        }
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
    caret: Option<(CaretPosition, CaretSource)>,
    owner_hwnd: HWND,
    show_mode_hint: bool,
    embedded: bool,
) {
    let (thread_mgr, allow_fallback_window) = {
        let st = shared_state.borrow();
        let uiless = host_is_uiless(&st);
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
    let weak_state = Rc::downgrade(shared_state);
    with_detached_windows(shared_state, |candidate_win, mode_hint_win| {
        let show = !ime_state.candidates.is_empty() || !ime_state.preedit.is_empty();
        if show && allow_candidate_window {
            if let Some((caret, _)) = caret {
                candidate_win.show(
                    ime_state,
                    caret.x,
                    caret.y,
                    caret.owner_hwnd,
                    &weak_state,
                    embedded,
                );
            }
        } else {
            candidate_win.hide();
        }
        let caret_source = caret.map(|(_, source)| source);
        if caret_source == Some(CaretSource::Probe) {
            candidate_win.disarm_caret_reprobe();
        } else if should_arm_caret_reprobe(caret_source)
            && ((show && allow_candidate_window) || show_mode_hint)
        {
            candidate_win.arm_caret_reprobe(owner_hwnd, &weak_state);
        }
        if let Some((caret, _)) = caret.filter(|_| show_mode_hint) {
            mode_hint_win.show_mode_hint(
                ime_state.ascii_mode,
                caret.x,
                caret.y,
                caret.owner_hwnd,
                &weak_state,
            );
        }
    });
}

pub(crate) fn host_is_uiless(state: &TsfState) -> bool {
    state.activation_flags & TF_TMAE_UIELEMENTENABLEDONLY != 0
        || state.thread_mgr_flags & TF_TMF_UIELEMENTENABLEDONLY != 0
}

pub(crate) fn hide_ime_windows(shared_state: &SharedState) {
    clear_layout_sink(shared_state);
    with_detached_candidate_ui(shared_state, CandidateUiManager::end);
    with_detached_windows(shared_state, |candidate_win, mode_hint_win| {
        candidate_win.hide();
        mode_hint_win.hide();
    });
}

pub(crate) fn reset_input_for_focus_change(shared_state: &SharedState) {
    let active_composition = take_active_composition(shared_state);
    if let Some((context, composition, client_id)) = active_composition {
        request_composition_end(context, composition, client_id);
    }
    hide_ime_windows(shared_state);
}

/// Same cleanup, but the composition is guaranteed to be gone by the time this
/// returns. Used where TSF releases the text service right after the call
/// (`Deactivate`) or where the thread stops pumping our sessions
/// (`OnKillThreadFocus`); a queued session would simply never run.
pub(crate) fn terminate_input_now(shared_state: &SharedState) {
    let active_composition = take_active_composition(shared_state);
    if let Some((context, composition, client_id)) = active_composition {
        end_composition_now(&context, composition, client_id);
    }
    hide_ime_windows(shared_state);
}

fn take_active_composition(
    shared_state: &SharedState,
) -> Option<(ITfContext, ITfComposition, u32)> {
    let mut st = shared_state.borrow_mut();
    st.shift_pressed_without_key = false;
    st.session_reset_pending = true;
    let active_composition = st
        .composition
        .take()
        .zip(st.composition_context.take())
        .map(|(composition, context)| (context, composition, st.client_id));
    st.ime_state = None;
    st.last_caret = None;
    st.caret_retry_attempts = 0;
    st.caret_retry_mode_hint = false;
    st.composition_in_flight = None;
    st.composition_terminated_in_session = false;
    // The panel is about to be hidden, so no click can still be pending; the
    // stale context must not keep the host object alive.
    st.key_context = None;
    active_composition
}

pub(crate) fn clear_input_after_composition_terminated(
    shared_state: &SharedState,
    terminated: Option<&ITfComposition>,
) {
    let Some(terminated) = terminated else {
        return;
    };
    let terminated_identity = terminated.as_raw() as usize;
    let (is_active_composition, terminated_in_session) = {
        let st = shared_state.borrow();
        (
            st.composition
                .as_ref()
                .is_some_and(|active| active.as_raw() == terminated.as_raw()),
            should_record_termination_in_session(
                st.ime_write_session_active,
                st.composition_in_flight,
                terminated_identity,
            ),
        )
    };
    if !is_active_composition {
        if terminated_in_session {
            shared_state.borrow_mut().composition_terminated_in_session = true;
        }
        return;
    }

    // A composition can only become active after `apply_ime_state` has already
    // primed the resolver outside the document write session. Keep this lookup
    // below the identity check so stale callbacks do no filesystem work.
    let configured_embedded = embedded_composition();
    let panel_mode_with_visible_state = {
        let st = shared_state.borrow();
        !configured_embedded
            && !host_is_uiless(&st)
            && st.ime_state.as_ref().is_some_and(has_visible_state)
    };
    if panel_mode_with_visible_state {
        let mut st = shared_state.borrow_mut();
        st.composition = None;
        st.composition_context = None;
        drop(st);
        append_diagnostic("composition terminated mode=panel kept_state=1");
        return;
    }

    {
        let mut st = shared_state.borrow_mut();
        st.shift_pressed_without_key = false;
        st.session_reset_pending = true;
        st.composition = None;
        st.composition_context = None;
        st.ime_state = None;
        st.last_caret = None;
        st.caret_retry_attempts = 0;
        st.caret_retry_mode_hint = false;
    }
    hide_ime_windows(shared_state);
}

fn should_record_termination_in_session(
    in_write_session: bool,
    composition_in_flight: Option<usize>,
    terminated_identity: usize,
) -> bool {
    in_write_session && composition_in_flight == Some(terminated_identity)
}

fn request_composition_end(context: ITfContext, composition: ITfComposition, client_id: u32) {
    if client_id == 0 {
        return;
    }
    let result =
        crate::edit_session::with_async_write_session(&context, client_id, move |ec, ctx| {
            clear_composition_range(ec, ctx, &composition)
        });
    if let Err(error) = result {
        append_diagnostic(format!(
            "failed to end composition after focus change: {error}"
        ));
    }
}

fn end_composition_now(context: &ITfContext, composition: ITfComposition, client_id: u32) {
    if client_id != 0 {
        let composition_for_session = composition.clone();
        let cleared = with_write_session(context, client_id, move |ec, ctx| {
            clear_composition_range(ec, ctx, &composition_for_session)
        });
        if cleared.is_ok() {
            return;
        }
    }
    // The host refused a synchronous lock. TerminateComposition works outside
    // one, and a null view terminates every composition this TIP owns in the
    // context; the text stays in the document but no dangling composition range
    // survives the text service.
    if let Ok(services) = context.cast::<ITfContextOwnerCompositionServices>() {
        let terminated = unsafe { services.TerminateComposition(None) };
        if let Err(error) = terminated {
            append_diagnostic(format!("failed to terminate composition: {error}"));
        }
    }
}

fn clear_composition_range(
    edit_cookie: u32,
    context: &ITfContext,
    composition: &ITfComposition,
) -> windows::core::Result<()> {
    unsafe {
        let range = composition.GetRange()?;
        if let Ok(property) = context.GetProperty(&GUID_PROP_ATTRIBUTE) {
            let _ = property.Clear(edit_cookie, &range);
        }
        range.SetText(edit_cookie, 0, &[])?;
        composition.EndComposition(edit_cookie)
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
                crate::candidate_win::CandidateWindow::new_mode_hint(),
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

#[cfg(test)]
mod tests {
    use super::{
        input_context_retry_due, should_record_termination_in_session, INPUT_CONTEXT_RETRY_INTERVAL,
    };
    use std::time::Instant;

    #[test]
    fn a_context_that_answered_is_not_probed_again() {
        let now = Instant::now();
        assert!(!input_context_retry_due(false, 1, None, now));
        // Even long after the throttle would have expired.
        assert!(!input_context_retry_due(
            false,
            1,
            Some(now - INPUT_CONTEXT_RETRY_INTERVAL),
            now
        ));
    }

    #[test]
    fn an_unanswered_probe_is_retried_at_once_then_throttled() {
        let now = Instant::now();
        // No deadline yet: the first key after the failed inspection retries.
        assert!(input_context_retry_due(true, 1, None, now));
        // A retry just ran; the next keystroke must not cost another session.
        assert!(!input_context_retry_due(
            true,
            1,
            Some(now + INPUT_CONTEXT_RETRY_INTERVAL),
            now
        ));
        // Once the interval has passed it is due again.
        assert!(input_context_retry_due(
            true,
            1,
            Some(now - INPUT_CONTEXT_RETRY_INTERVAL),
            now
        ));
    }

    #[test]
    fn without_a_client_id_there_is_nothing_to_probe_with() {
        // `inspect_context` needs an edit cookie, and a read session cannot be
        // requested before Activate handed one out. Retrying would only burn
        // the throttle and keep the context stuck in pass-through.
        let now = Instant::now();
        assert!(!input_context_retry_due(true, 0, None, now));
    }

    #[test]
    fn only_matching_in_flight_composition_arms_session_termination() {
        assert!(should_record_termination_in_session(
            true,
            Some(0x1234),
            0x1234
        ));
        assert!(!should_record_termination_in_session(
            true,
            Some(0x1234),
            0x5678
        ));
        assert!(!should_record_termination_in_session(
            false,
            Some(0x1234),
            0x1234
        ));
    }
}
