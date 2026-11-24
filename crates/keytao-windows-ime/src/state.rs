//! Shared state between all TSF COM objects.

use keytao_core::{
    default_user_data_dir,
    runtime_log::{self, Level},
    DeploymentState, ImeState, InputContextPolicy,
};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use windows::{
    core::Interface,
    Win32::{
        Foundation::HWND,
        System::Threading::GetCurrentProcess,
        UI::Input::KeyboardAndMouse::GetFocus,
        UI::TextServices::{
            ITfComposition, ITfContext, ITfDocumentMgr, ITfKeyEventSink, ITfSource,
            ITfTextLayoutSink, ITfThreadFocusSink, ITfThreadMgr, ITfThreadMgrEventSink,
            TF_TMAE_UIELEMENTENABLEDONLY, TF_TMF_UIELEMENTENABLEDONLY,
        },
    },
};

use crate::{
    candidate_ui::CandidateUiManager,
    candidate_win::value_signature,
    engine_backend::{stamp_changed, EngineHandle, ImeEngineBackend},
    engine_status,
    globals::DllActivityGuard,
    input_context::{
        inspect_context, merge_probe_failed_state, ContextInputState, ContextProbe,
        InputBlockReason,
    },
    key_event_sink::{
        should_arm_caret_reprobe, CaretSource, PendingCommitRetry, PendingWindowUpdate,
    },
    language_bar::LanguageBarItem,
    panel_anchor::PanelAnchor,
};

/// How often the key path may stat the reload stamp. Focus changes always
/// force a fresh read, so a schema update is still picked up before the first
/// key of the next focus; this only keeps typing off the file system.
const RELOAD_STAMP_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How soon a context with an unanswered probe may be inspected again from the
/// key path. `Unknown` fails closed; unavailable optional scopes and refused
/// password edit sessions keep a known answer or stay open on the first attempt.
/// All need bounded retries,
/// but a host that never grants the session must not get one request per key.
const INPUT_CONTEXT_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const INPUT_CONTEXT_SYNC_BACKOFF: Duration = Duration::from_secs(1);
const THEME_POLL_INTERVAL: Duration = Duration::from_millis(250);
static THEME_CACHE: OnceLock<Mutex<ThemeCache>> = OnceLock::new();
static HOST_EXECUTABLE_BASENAME: OnceLock<Option<String>> = OnceLock::new();

struct ThemeCache {
    resolver: keytao_theme::ThemeResolver,
    theme: Arc<keytao_theme::ResolvedImeTheme>,
    signature: u64,
    checked_at: Instant,
}

#[derive(Default)]
struct InputContextRetry {
    last_attempt: Option<Instant>,
    last_context: Option<usize>,
    consecutive_sync_refusals: u8,
    retry_pending: bool,
}

impl InputContextRetry {
    fn due(&self, context: Option<usize>, client_id: u32, now: Instant) -> bool {
        let interval = if self.last_context == context && self.consecutive_sync_refusals >= 3 {
            INPUT_CONTEXT_SYNC_BACKOFF
        } else {
            INPUT_CONTEXT_RETRY_INTERVAL
        };
        client_id != 0
            && (!self.retry_pending
                || self
                    .last_attempt
                    .is_none_or(|attempt| now.saturating_duration_since(attempt) >= interval))
    }

    fn begin_attempt(&mut self, context: Option<usize>, now: Instant) {
        if self.last_context != context {
            self.consecutive_sync_refusals = 0;
        }
        self.last_context = context;
        self.last_attempt = Some(now);
        self.retry_pending = true;
    }

    fn finish_attempt(&mut self, needs_retry: bool, sync_refused: bool) {
        self.retry_pending = needs_retry;
        self.consecutive_sync_refusals = if sync_refused {
            self.consecutive_sync_refusals.saturating_add(1)
        } else {
            0
        };
    }
}

// ── Shared TsfState ───────────────────────────────────────────────────────────

pub struct TsfState {
    pub runtime: Option<Arc<dyn ImeEngineBackend>>,
    pub session: Option<EngineHandle>,
    private_engine: bool,
    pub engine_building: bool,
    pub engine_error: Option<String>,
    engine_retry_after: Option<Instant>,
    engine_failures: u32,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub thread_mgr_sink: Option<ITfThreadMgrEventSink>,
    pub thread_mgr_sink_cookie: Option<u32>,
    pub thread_focus_sink: Option<ITfThreadFocusSink>,
    pub thread_focus_sink_cookie: Option<u32>,
    pub client_id: u32,
    pub activation_flags: u32,
    pub thread_mgr_flags: u32,
    pub display_attribute_atom: Option<u32>,
    pub converted_display_attribute_atom: Option<u32>,
    pub key_sink: Option<ITfKeyEventSink>,
    pub(crate) language_bar: Option<LanguageBarItem>,
    pub(crate) language_bar_enabled: bool,
    pub(crate) language_bar_refreshing: bool,
    language_bar_add_error: Option<windows::core::HRESULT>,
    pub composition: Option<ITfComposition>,
    pub composition_context: Option<ITfContext>,
    pub panel_anchor: Option<PanelAnchor>,
    pub input_generation: u64,
    pub(crate) ime_update_generation: u64,
    pub panel_context: Option<ITfContext>,
    pub embedded_mode_cached: bool,
    pub layout_reposition_pending: bool,
    pub ime_state: Option<ImeState>,
    deployment_state: DeploymentState,
    pub(crate) deployment_hint: Option<String>,
    pub candidate_win: Option<crate::candidate_win::CandidateWindow>,
    pub mode_hint_win: Option<crate::candidate_win::CandidateWindow>,
    pub windows_dirty: bool,
    pub windows_hide_pending: bool,
    pub(crate) pending_window_update: Option<PendingWindowUpdate>,
    pub(crate) pending_commit_retry: Option<PendingCommitRetry>,
    pub(crate) pending_ime_apply: Option<crate::key_event_sink::PendingImeApply>,
    pub(crate) written_preedits: Vec<(usize, ITfContext, String)>,
    pub(crate) deferred_keys: crate::deferred_keys::DeferredKeys,
    pub(crate) deferred_context: Option<ITfContext>,
    pub(crate) deferred_replaying: bool,
    pub(crate) claimed_key: Option<(u16, isize)>,
    pub(crate) pending_composition_ends: Vec<crate::key_event_sink::PendingCompositionEnd>,
    pub(crate) composition_end_flush_in_progress: bool,
    pub candidate_ui: Option<CandidateUiManager>,
    pub ascii_mode: bool,
    /// UI language: English can use a candidate schema while raw ASCII is off.
    pub english_mode: bool,
    // Retained across failed reloads and for callers that seed a stamp before warmup.
    pub reload_stamp_path: Option<PathBuf>,
    pub reload_stamp_signature: Option<String>,
    pub reload_in_progress: bool,
    pub reload_clear_pending: bool,
    reload_retry_after: Option<Instant>,
    reload_stamp_checked_at: Option<Instant>,
    reload_stamp_changed: bool,
    deployment_loaded_checked_at: Option<Instant>,
    input_engine_enabled_checked_at: Option<Instant>,
    input_engine_enabled: bool,
    pub(crate) modifier_tap_press: Option<(u32, Instant)>,
    session_reset_pending: bool,
    focus_state_refresh_pending: bool,
    /// What TSF says about the focused context (password field, disabled
    /// keyboard, empty context). Refreshed on focus changes.
    pub input_context: ContextInputState,
    /// Strong reference used to scope cached probe answers to one live context.
    input_context_source: Option<ITfContext>,
    /// Retained while the context is attached, so Pop/Uninit notifications can
    /// still be attributed after TSF detaches it from its document manager.
    input_context_document_mgr: Option<ITfDocumentMgr>,
    /// Cached `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE`. A closed keyboard passes
    /// every keystroke through unaltered.
    pub keyboard_open: bool,
    /// The policy acknowledged by the running session;
    /// `None` after an engine (re)build, so the policy is pushed again.
    input_policy_applied: Option<InputContextPolicy>,
    /// Last actual input-scope attempt; focus and compartment notifications
    /// cannot reset its deadline. Sync refusal counts belong to one context.
    input_context_retry: InputContextRetry,
    input_context_retry_source: Option<ITfContext>,
    /// Context of the last key event, used by candidate-window clicks which
    /// arrive outside any TSF callback.
    pub key_context: Option<ITfContext>,
    /// Last caret position the host reported. Reused while the layout is not
    /// computed yet (`TF_E_NOLAYOUT`) so the panel never jumps to a corner.
    pub last_caret: Option<CaretPosition>,
    pub last_shown_caret: Option<CaretPosition>,
    pub caret_retry_attempts: u8,
    pub caret_retry_armed: bool,
    pub caret_retry_active: bool,
    pub caret_retry_mode_hint: bool,
    caret_rearm_pending: bool,
    pub ime_write_session_active: bool,
    pub caret_probe_session_in_progress: bool,
    /// COM identity of the composition temporarily owned by the active write
    /// session while `st.composition` is taken out of shared state.
    pub composition_in_flight: Option<usize>,
    pub composition_clear_pending: bool,
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
#[derive(Clone, Copy, PartialEq, Eq)]
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
    runtime: Arc<dyn ImeEngineBackend>,
    session: EngineHandle,
    private: bool,
    reload_stamp_path: Option<PathBuf>,
    reload_stamp_signature: Option<String>,
}

struct EngineBuildMailbox {
    result: Mutex<Option<(bool, Result<EngineBundle, String>)>>,
}

impl EngineBuildMailbox {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
        }
    }

    fn store(&self, private: bool, result: Result<EngineBundle, String>) {
        *self
            .result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((private, result));
    }

    fn take(&self) -> Option<(bool, Result<EngineBundle, String>)> {
        self.result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

impl TsfState {
    pub fn new() -> Self {
        Self {
            runtime: None,
            session: None,
            private_engine: false,
            engine_building: false,
            engine_error: None,
            engine_retry_after: None,
            engine_failures: 0,
            thread_mgr: None,
            thread_mgr_sink: None,
            thread_mgr_sink_cookie: None,
            thread_focus_sink: None,
            thread_focus_sink_cookie: None,
            client_id: 0,
            activation_flags: 0,
            thread_mgr_flags: 0,
            display_attribute_atom: None,
            converted_display_attribute_atom: None,
            key_sink: None,
            language_bar: None,
            language_bar_enabled: false,
            language_bar_refreshing: false,
            language_bar_add_error: None,
            composition: None,
            composition_context: None,
            panel_anchor: None,
            input_generation: 0,
            ime_update_generation: 0,
            panel_context: None,
            embedded_mode_cached: false,
            layout_reposition_pending: false,
            ime_state: None,
            deployment_state: DeploymentState::default(),
            deployment_hint: None,
            candidate_win: Some(crate::candidate_win::CandidateWindow::new()),
            mode_hint_win: Some(crate::candidate_win::CandidateWindow::new_mode_hint()),
            windows_dirty: false,
            windows_hide_pending: false,
            pending_window_update: None,
            pending_commit_retry: None,
            deferred_keys: crate::deferred_keys::DeferredKeys::default(),
            deferred_context: None,
            deferred_replaying: false,
            claimed_key: None,
            pending_composition_ends: Vec::new(),
            pending_ime_apply: None,
            written_preedits: Vec::new(),
            composition_end_flush_in_progress: false,
            candidate_ui: Some(CandidateUiManager::new()),
            ascii_mode: false,
            english_mode: false,
            reload_stamp_path: None,
            reload_stamp_signature: None,
            reload_in_progress: false,
            reload_clear_pending: false,
            reload_retry_after: None,
            reload_stamp_checked_at: None,
            reload_stamp_changed: false,
            deployment_loaded_checked_at: None,
            input_engine_enabled_checked_at: None,
            input_engine_enabled: true,
            modifier_tap_press: None,
            session_reset_pending: false,
            focus_state_refresh_pending: false,
            input_context: ContextInputState::default(),
            input_context_source: None,
            input_context_document_mgr: None,
            keyboard_open: true,
            input_policy_applied: None,
            input_context_retry: InputContextRetry::default(),
            input_context_retry_source: None,
            key_context: None,
            last_caret: None,
            last_shown_caret: None,
            caret_retry_attempts: 0,
            caret_retry_armed: false,
            caret_retry_active: false,
            caret_retry_mode_hint: false,
            caret_rearm_pending: false,
            ime_write_session_active: false,
            caret_probe_session_in_progress: false,
            composition_in_flight: None,
            composition_clear_pending: false,
            layout_sink: None,
            compartment_sinks: Vec::new(),
            context_compartment_sinks: Vec::new(),
            context_sink_context: None,
            engine_build_mailbox: Arc::new(EngineBuildMailbox::new()),
            reload_mailbox: Arc::new(EngineBuildMailbox::new()),
        }
    }

    pub(crate) fn build_engine(private: bool) -> Result<EngineBundle, String> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::try_build_engine(private).map(|bundle| {
                let schema = bundle.session.schema_id.clone();
                (bundle, schema)
            })
        }))
        .unwrap_or_else(|_| Err("engine initialization panicked".into()));
        let schema = result.as_ref().ok().map(|(_, schema)| schema.as_str());
        let error = match &result {
            Ok((bundle, _)) => bundle.runtime.status_error(),
            Err(error) => Some(error.as_str()),
        };
        if !private {
            record_engine_status(error, schema);
        }
        result.map(|(bundle, _)| bundle)
    }

    fn try_build_engine(private: bool) -> Result<EngineBundle, String> {
        let (runtime, session, reload_stamp_path, reload_stamp_signature) =
            crate::engine_backend::build_backend(private)?;
        Ok(EngineBundle {
            runtime,
            session,
            private,
            reload_stamp_path,
            reload_stamp_signature,
        })
    }

    pub(crate) fn install_engine(&mut self, bundle: EngineBundle) {
        self.private_engine = bundle.private;
        self.runtime = Some(bundle.runtime);
        self.session = Some(bundle.session);
        self.reload_stamp_signature = bundle.reload_stamp_signature;
        self.reload_stamp_path = bundle.reload_stamp_path;
        self.invalidate_reload_stamp_cache();
        // A fresh session starts with the default policy, so the sensitive
        // policy of the focused context has to be pushed again.
        self.input_policy_applied = None;
        if let Some(session) = self.session() {
            self.ime_state = Some(session.state());
            if let Some(ime_state) = &self.ime_state {
                self.ascii_mode = ime_state.ascii_mode;
                self.english_mode = ime_state.is_english_mode();
            }
        }
        self.engine_building = false;
        self.engine_error = None;
        self.engine_retry_after = None;
        self.engine_failures = 0;
        self.reload_retry_after = None;
        self.deployment_loaded();
    }

    fn deployment_loaded(&mut self) {
        self.deployment_loaded_checked_at = Some(Instant::now());
        if let Some(root) = user_data_dir() {
            self.deployment_state.loaded(root);
        }
    }

    pub(crate) fn deployment_loaded_cached(&mut self) {
        if self
            .deployment_loaded_checked_at
            .is_none_or(|checked_at| checked_at.elapsed() >= RELOAD_STAMP_POLL_INTERVAL)
        {
            self.deployment_loaded();
        }
    }

    pub(crate) fn engine_ready(&self) -> bool {
        self.session.is_some()
            && self.private_engine == self.input_context.uses_private_runtime()
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
        // The old route cannot service the new context. Release its runtime
        // before the build, because librime is process-global in InProc mode.
        if let Some(runtime) = self.runtime.take() {
            runtime.drop_session();
        }
        self.session = None;
        self.engine_building = true;
        self.engine_error = None;
        self.engine_retry_after = None;
        true
    }

    pub(crate) fn finish_engine_build_error(&mut self, error: String) {
        self.engine_building = false;
        self.engine_error = Some(error);
        self.engine_failures = self.engine_failures.saturating_add(1);
        self.engine_retry_after =
            Some(Instant::now() + engine_status::retry_delay(self.engine_failures));
    }

    pub(crate) fn begin_reload_if_changed(&mut self) -> bool {
        if self.engine_building || self.reload_in_progress || !self.reload_needed() {
            return false;
        }
        if self.runtime.is_none() && self.reload_stamp_path.is_none() {
            return false;
        }
        let now = Instant::now();
        if self
            .engine_retry_after
            .is_some_and(|retry_after| now < retry_after)
            || self
                .reload_retry_after
                .is_some_and(|retry_after| now < retry_after)
        {
            return false;
        }
        self.reload_in_progress = true;
        self.input_generation = self.input_generation.wrapping_add(1);
        self.pending_commit_retry = None;
        self.session = None;
        if let Some(runtime) = &self.runtime {
            runtime.drop_session();
        }
        self.runtime = None;
        self.ime_state = None;
        self.reload_clear_pending = true;
        self.reload_retry_after = None;
        true
    }

    pub(crate) fn reload_needed(&self) -> bool {
        if self.reload_in_progress {
            return true;
        }
        match &self.runtime {
            Some(runtime) => runtime.reload_needed(),
            None => stamp_changed(
                self.reload_stamp_path.as_deref(),
                self.reload_stamp_signature.as_deref(),
            ),
        }
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
        self.input_engine_enabled_checked_at = None;
    }

    pub(crate) fn input_engine_enabled_cached(&mut self) -> bool {
        let now = Instant::now();
        if self.input_engine_enabled_checked_at
            .is_none_or(|checked| now.duration_since(checked) >= RELOAD_STAMP_POLL_INTERVAL)
        {
            self.input_engine_enabled = crate::engine_backend::input_engine_enabled();
            self.input_engine_enabled_checked_at = Some(now);
        }
        self.input_engine_enabled
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
                self.finish_engine_build_error(error);
                self.reload_retry_after = self.engine_retry_after;
            }
        }
    }

    pub(crate) fn take_reload_clear_pending(&mut self) -> bool {
        std::mem::take(&mut self.reload_clear_pending)
    }

    fn poll_engine_builds(&mut self) {
        let current_private = self.input_context.uses_private_runtime();
        if let Some((private, result)) = self.engine_build_mailbox.take() {
            self.engine_building = false;
            match result {
                Ok(bundle) if private == current_private && !self.engine_ready() => {
                    self.install_engine(bundle);
                }
                Ok(bundle) => bundle.runtime.drop_session(),
                Err(error) if private == current_private => self.finish_engine_build_error(error),
                Err(_) => {}
            }
        }
        if let Some((private, result)) = self.reload_mailbox.take() {
            self.reload_in_progress = false;
            if private == current_private {
                self.finish_reload(result);
            } else if let Ok(bundle) = result {
                bundle.runtime.drop_session();
            }
        }
    }

    pub(crate) fn session(&self) -> Option<Arc<dyn ImeEngineBackend>> {
        // This guard also covers deferred keys and candidate-window clicks
        // while a background build for the previous context finishes.
        if !self.engine_ready() {
            return None;
        }
        self.runtime.clone()
    }
}

pub type SharedState = Rc<RefCell<TsfState>>;
pub type WeakState = Weak<RefCell<TsfState>>;

pub fn new_shared_state() -> SharedState {
    HOST_EXECUTABLE_BASENAME.get_or_init(|| {
        let image = crate::pipe_security::process_image(unsafe { GetCurrentProcess() }).ok()?;
        Some(image.file_name()?.to_string_lossy().into_owned())
    });
    Rc::new(RefCell::new(TsfState::new()))
}

pub(crate) fn host_executable_basename() -> Option<&'static str> {
    HOST_EXECUTABLE_BASENAME.get().and_then(Option::as_deref)
}

pub(crate) fn diagnostics_enabled() -> bool {
    runtime_log::enabled(Level::Verbose)
}

/// Missing theme file or key means panel-only preedit by default.
pub(crate) fn embedded_composition() -> bool {
    with_current_theme(|theme| theme.ui.effective_embedded_composition())
}

pub(crate) fn current_theme() -> (Arc<keytao_theme::ResolvedImeTheme>, u64) {
    let cache = current_theme_cache();
    (Arc::clone(&cache.theme), cache.signature)
}

pub(crate) fn with_current_theme<R>(read: impl FnOnce(&keytao_theme::ResolvedImeTheme) -> R) -> R {
    read(&current_theme_cache().theme)
}

fn current_theme_cache() -> MutexGuard<'static, ThemeCache> {
    let cache = THEME_CACHE.get_or_init(|| {
        let theme_path = keytao_theme::default_user_theme_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing".to_string());
        let resolver = crate::panel::windows_theme_resolver();
        let theme = resolver.current();
        append_diagnostic(format!(
            "embedded_composition setting={} theme={theme_path}",
            u8::from(theme.ui.embedded_composition)
        ));
        Mutex::new(ThemeCache {
            resolver,
            signature: value_signature(&theme),
            theme: Arc::new(theme),
            checked_at: Instant::now(),
        })
    });
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.checked_at.elapsed() >= THEME_POLL_INTERVAL {
        cache.checked_at = Instant::now();
        let theme = cache.resolver.current();
        cache.signature = value_signature(&theme);
        cache.theme = Arc::new(theme);
    }
    cache
}

/// Resolve the theme once before a TSF document write can synchronously call
/// `OnCompositionTerminated`. Later calls still observe theme-file changes.
pub(crate) fn prime_theme_resolver() {
    if THEME_CACHE.get().is_none() {
        let _ = embedded_composition();
    }
}

pub(crate) fn append_diagnostic(message: impl AsRef<str>) {
    keytao_core::rt_log!(Level::Verbose, "ui", "tsf", msg = message.as_ref());
}

fn record_engine_status(error: Option<&str>, schema: Option<&str>) {
    let result = default_user_data_dir()
        .ok_or_else(|| "cannot determine engine status directory".to_string())
        .and_then(|dir| {
            engine_status::write_status(&dir, error, schema).map_err(|error| error.to_string())
        });
    if let Err(error) = result {
        keytao_core::rt_log!(Level::Info, "error", "engine_status_write", msg = error);
    }
}

pub(crate) fn start_engine_warmup(shared_state: &SharedState) {
    let (mailbox, private) = {
        let mut st = shared_state.borrow_mut();
        if !st.begin_engine_build() {
            return;
        }
        (
            Arc::clone(&st.engine_build_mailbox),
            st.input_context.uses_private_runtime(),
        )
    };

    let started = Instant::now();
    keytao_core::rt_log!(Level::Info, "lifecycle", "engine_warmup_start");
    let dll_guard = DllActivityGuard::new();
    let spawn_result = std::thread::Builder::new()
        .name("keytao-ime-warmup".into())
        .spawn(move || {
            let result = TsfState::build_engine(private);
            with_current_theme(|theme| {
                let _ = crate::panel::load_font(theme.font.effective_family.as_deref());
            });
            match &result {
                Ok(_) => {
                    tracing::info!("KeyTao Windows IME engine warmed up");
                    keytao_core::rt_log!(
                        Level::Info,
                        "lifecycle",
                        "engine_warmup_end",
                        dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                        ok = true,
                    );
                }
                Err(error) => {
                    tracing::error!("librime init failed: {error}");
                    keytao_core::rt_log!(
                        Level::Info,
                        "error",
                        "engine_warmup_end",
                        dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                        ok = false,
                        msg = error,
                    );
                }
            }
            mailbox.store(private, result);
            drop(mailbox);
            drop(dll_guard);
        });
    if let Err(error) = spawn_result {
        let message = format!("start engine warmup thread: {error}");
        if !private {
            record_engine_status(Some(&message), None);
        }
        keytao_core::rt_log!(
            Level::Info,
            "error",
            "engine_warmup_end",
            dur_ms = started.elapsed().as_secs_f64() * 1000.0,
            ok = false,
            msg = message,
        );
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

    hide_ime_windows(shared_state);

    let (mailbox, private) = {
        let st = shared_state.borrow();
        (
            Arc::clone(&st.reload_mailbox),
            st.input_context.uses_private_runtime(),
        )
    };
    let dll_guard = DllActivityGuard::new();
    let started = Instant::now();
    keytao_core::rt_log!(Level::Info, "lifecycle", "engine_reload_start");
    let spawn_result = std::thread::Builder::new()
        .name("keytao-ime-reload".into())
        .spawn(move || {
            let bundle = TsfState::build_engine(private);
            with_current_theme(|theme| {
                let _ = crate::panel::load_font(theme.font.effective_family.as_deref());
            });
            match &bundle {
                Ok(_) => {
                    tracing::info!("librime session refreshed after reload stamp change");
                    keytao_core::rt_log!(
                        Level::Info,
                        "lifecycle",
                        "engine_reload_end",
                        dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                        ok = true,
                    );
                }
                Err(error) => {
                    tracing::error!("librime reload failed: {error}");
                    keytao_core::rt_log!(
                        Level::Info,
                        "error",
                        "engine_reload_end",
                        dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                        ok = false,
                        msg = error,
                    );
                }
            }
            mailbox.store(private, bundle);
            drop(mailbox);
            drop(dll_guard);
        });
    if let Err(error) = spawn_result {
        let message = format!("start engine reload thread: {error}");
        if !private {
            record_engine_status(Some(&message), None);
        }
        keytao_core::rt_log!(
            Level::Info,
            "error",
            "engine_reload_end",
            dur_ms = started.elapsed().as_secs_f64() * 1000.0,
            ok = false,
            msg = message,
        );
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
    refresh_pending_focus_state(shared_state);
}

pub(crate) fn refresh_engine_for_focus(shared_state: &SharedState) {
    shared_state.borrow_mut().focus_state_refresh_pending = true;
    poll_engine_builds(shared_state);
    shared_state.borrow_mut().invalidate_reload_stamp_cache();
    if shared_state.borrow().engine_ready() {
        start_reload_if_needed(shared_state);
    } else {
        start_engine_warmup(shared_state);
    }
}

fn refresh_pending_focus_state(shared_state: &SharedState) {
    if !shared_state.borrow().focus_state_refresh_pending {
        return;
    }
    let session = shared_state.borrow().session();
    let Some(session) = session else {
        return;
    };
    if session.set_context_ascii_mode(None).is_none() {
        return;
    }
    let ime_state = session.state();
    if ime_state.schema_id.is_empty() {
        return;
    }
    let english_mode = ime_state.is_english_mode();
    {
        let mut st = shared_state.borrow_mut();
        st.ascii_mode = ime_state.ascii_mode;
        st.english_mode = english_mode;
        st.ime_state = Some(ime_state);
        st.focus_state_refresh_pending = false;
    }
    update_language_bar_mode(shared_state, english_mode);
}

pub(crate) fn apply_pending_session_reset(shared_state: &SharedState) -> bool {
    let session = {
        let mut state = shared_state.borrow_mut();
        if !std::mem::take(&mut state.session_reset_pending) {
            return true;
        }
        state.session()
    };
    let ime_state = session.and_then(|session| session.reset());
    let acknowledged = ime_state.is_some();
    let mut state = shared_state.borrow_mut();
    state.ime_state = ime_state;
    // A busy pipe must not carry the previous context's preedit into the next key.
    state.session_reset_pending |= !acknowledged;
    drop(state);
    if acknowledged {
        append_diagnostic("session reset applied");
    }
    acknowledged
}

/// Push the input policy of the focused context onto the running session.
///
/// Runs on the key path too: the engine is built in the background and may
/// only become ready after the focus change that decided the policy.
pub(crate) fn sync_input_policy(shared_state: &SharedState) {
    let (session, policy, applied) = {
        let st = shared_state.borrow();
        (
            st.session(),
            st.input_context.input_policy(),
            st.input_policy_applied,
        )
    };
    if applied == Some(policy) {
        return;
    }
    let Some(session) = session else {
        return;
    };
    // Turning composing off discards whatever librime was holding; the cached
    // state has to go with it so nothing is drawn for the sensitive context.
    let ime_state = session.set_input_policy(policy);
    let acknowledged = ime_state.is_some();
    let mut st = shared_state.borrow_mut();
    // Pipe contention may skip the RPC entirely; retry until the server acknowledges it.
    st.input_policy_applied = acknowledged.then_some(policy);
    if !policy.composing {
        st.ime_state = None;
    } else if let Some(ime_state) = ime_state {
        st.ascii_mode = ime_state.ascii_mode;
        st.english_mode = ime_state.is_english_mode();
        st.ime_state = Some(ime_state);
        let english_mode = st.english_mode;
        drop(st);
        update_language_bar_mode(shared_state, english_mode);
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
    let context_identity = context.as_ref().map(|context| context.as_raw() as usize);
    let same_context = input_context_matches(shared_state, context.as_ref());
    let (probe_due, previous) = {
        let st = shared_state.borrow();
        (
            context.is_some()
                && st
                    .input_context_retry
                    .due(context_identity, client_id, Instant::now()),
            if same_context {
                st.input_context
            } else {
                ContextInputState::default()
            },
        )
    };
    let inspected = if probe_due {
        let previous_source = {
            let mut st = shared_state.borrow_mut();
            st.input_context_retry
                .begin_attempt(context_identity, Instant::now());
            std::mem::replace(&mut st.input_context_retry_source, context.clone())
        };
        drop(previous_source);
        let (inspected, sync_refused) =
            inspect_context(thread_mgr.as_ref(), context.as_ref(), client_id);
        shared_state
            .borrow_mut()
            .input_context_retry
            .finish_attempt(inspected.needs_retry(), sync_refused);
        inspected
    } else {
        // Churn still gets fresh safety compartments, but cannot spend another
        // synchronous input-scope request before the last attempt's deadline.
        crate::input_context::refresh_context_compartments(
            thread_mgr.as_ref(),
            context.as_ref(),
            previous,
        )
    };
    if apply_input_context_state(shared_state, context.as_ref(), inspected) {
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
fn apply_input_context_state(
    shared_state: &SharedState,
    context: Option<&ITfContext>,
    inspected: ContextInputState,
) -> bool {
    let same_context = input_context_matches(shared_state, context);
    let document_mgr = context
        .and_then(|context| unsafe { context.GetDocumentMgr() }.ok())
        .or_else(|| {
            same_context
                .then(|| shared_state.borrow().input_context_document_mgr.clone())
                .flatten()
        });
    let (input_context, thread_mgr, changed, route_changed) = {
        let st = shared_state.borrow();
        let previous_same_context = context.filter(|_| same_context).map(|_| st.input_context);
        let merged = merge_probe_failed_state(previous_same_context, inspected);
        (
            merged,
            st.thread_mgr.clone(),
            st.input_context != merged,
            st.input_context.uses_private_runtime() != merged.uses_private_runtime(),
        )
    };
    let sensitive = input_context.is_sensitive();
    let retry_due = input_context.needs_retry();
    let keyboard_open = crate::input_context::keyboard_is_open(thread_mgr.as_ref());
    let (previous_context_handles, keyboard_open_changed) = {
        let mut st = shared_state.borrow_mut();
        st.input_context = input_context;
        let keyboard_open_changed =
            std::mem::replace(&mut st.keyboard_open, keyboard_open) != keyboard_open;
        if route_changed {
            st.engine_retry_after = None;
            st.reload_retry_after = None;
            st.engine_failures = 0;
        }
        (
            (
                std::mem::replace(&mut st.input_context_source, context.cloned()),
                std::mem::replace(&mut st.input_context_document_mgr, document_mgr),
            ),
            keyboard_open_changed,
        )
    };
    if route_changed {
        reset_input_for_focus_change(shared_state);
    }
    // COM Release, like QueryInterface, may reenter the text service.
    drop(previous_context_handles);
    let password_diagnostic = if input_context.password == ContextProbe::Unavailable
        || input_context.uses_private_runtime()
    {
        input_context.password.diagnostic_name()
    } else if input_context.password_probe_failed {
        "probe_failed"
    } else {
        input_context.password.diagnostic_name()
    };
    if changed {
        keytao_core::rt_log!(
            Level::Info,
            "input",
            "context_policy",
            host_exe = host_executable_basename(),
            keyboard_disabled = input_context.keyboard_disabled.diagnostic_name(),
            empty_context = input_context.empty_context.diagnostic_name(),
            input_scope = password_diagnostic,
            keyboard_open = keyboard_open,
            blocked = sensitive
        );
    }
    append_diagnostic(format!(
        "input context inspected kd={} ec={} pw={} sensitive={} retry_due={}",
        input_context.keyboard_disabled.diagnostic_name(),
        input_context.empty_context.diagnostic_name(),
        password_diagnostic,
        sensitive,
        retry_due,
    ));
    sync_input_policy(shared_state);
    if keyboard_open_changed {
        let english_mode = shared_state.borrow().english_mode;
        update_language_bar_mode(shared_state, english_mode);
    }
    sensitive
}

/// Inspect the context again when the last attempt could not answer or a
/// host cleared either restriction without notifying the compartment sinks.
///
/// Runs from the key callbacks. `ContextProbe::Unknown` fails closed, while a
/// password `ProbeFailed` keeps a known answer or allows input until the retry;
/// hosts routinely refuse a synchronous read session while the document is
/// locked, so neither result may become permanent.
fn refresh_input_context_for_key(shared_state: &SharedState, context: Option<&ITfContext>) {
    let thread_mgr = shared_state.borrow().thread_mgr.clone();
    let context = crate::input_context::resolve_context(thread_mgr.as_ref(), context);
    let changed_context = !input_context_matches(shared_state, context.as_ref());
    let (cached_disabled, due) = {
        let st = shared_state.borrow();
        let context_identity = context.as_ref().map(|context| context.as_raw() as usize);
        let cached_disabled =
            st.input_context.block_reason() == Some(InputBlockReason::ContextDisabled);
        (
            cached_disabled,
            (cached_disabled || st.input_context.needs_retry())
                && st
                    .input_context_retry
                    .due(context_identity, st.client_id, Instant::now()),
        )
    };
    // These reads need no edit session. A restricted or unreadable answer
    // stays blocked without probing the input scope on each key.
    let refresh = changed_context
        || (due
            && (!cached_disabled
                || context.as_ref().is_some_and(|context| {
                    crate::input_context::context_block_reason(None, Some(context)).is_none()
                })));
    if !refresh {
        return;
    }
    // A different context must never inherit either a clear or a restricted
    // input scope. refresh_input_context still bounds synchronous probes; if
    // throttled, this context starts Unknown until its own probe can run.
    refresh_input_context(shared_state, context.as_ref());
}

pub(crate) fn same_com_object<T: Interface>(left: &T, right: &T) -> bool {
    left.as_raw() == right.as_raw()
        || match (
            left.cast::<windows::core::IUnknown>(),
            right.cast::<windows::core::IUnknown>(),
        ) {
            (Ok(left), Ok(right)) => left.as_raw() == right.as_raw(),
            _ => false,
        }
}

pub(crate) fn input_context_matches(
    shared_state: &SharedState,
    context: Option<&ITfContext>,
) -> bool {
    let previous = shared_state.borrow().input_context_source.clone();
    // QueryInterface can call back into the TIP; release RefCell borrows first.
    match (previous.as_ref(), context) {
        (Some(previous), Some(context)) => same_com_object(previous, context),
        (None, None) => true,
        _ => false,
    }
}

pub(crate) fn input_document_matches(
    shared_state: &SharedState,
    document: &ITfDocumentMgr,
) -> bool {
    let previous = shared_state.borrow().input_context_document_mgr.clone();
    previous.is_some_and(|previous| same_com_object(&previous, document))
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
    let sensitive = apply_input_context_state(shared_state, context.as_ref(), refreshed);
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
        st.composition.is_some() || st.panel_anchor.is_some() || st.ime_state.is_some()
    };
    if !has_input {
        return;
    }
    reset_input_for_focus_change(shared_state);
}

pub(crate) fn set_keyboard_open_state(shared_state: &SharedState, open: bool) {
    {
        let mut st = shared_state.borrow_mut();
        st.keyboard_open = open;
        st.modifier_tap_press = None;
    }
    if !open {
        reset_input_for_focus_change(shared_state);
    }
    let english_mode = shared_state.borrow().english_mode;
    update_language_bar_mode(shared_state, english_mode);
}

/// Declare the thread compartments a keyboard TIP owns: the keyboard is open
/// and, unless the user already switched, in native (Chinese) conversion mode.
pub(crate) fn publish_initial_compartments(shared_state: &SharedState) {
    let (thread_mgr, client_id) = {
        let st = shared_state.borrow();
        (st.thread_mgr.clone(), st.client_id)
    };
    crate::input_context::set_keyboard_open(thread_mgr.as_ref(), client_id, true);
    shared_state.borrow_mut().keyboard_open = true;
    refresh_language_bar(shared_state);
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
        (st.thread_mgr.clone(), st.english_mode)
    };
    let Some(ascii_mode) = crate::input_context::conversion_mode_is_ascii(thread_mgr.as_ref())
    else {
        return;
    };
    if ascii_mode == current {
        return;
    }
    reset_input_for_focus_change(shared_state);
    let session = shared_state.borrow().session();
    let Some(ime_state) =
        session.and_then(|session| session.set_context_ascii_mode(Some(ascii_mode)))
    else {
        return;
    };
    let english_mode = ime_state.is_english_mode();
    {
        let mut st = shared_state.borrow_mut();
        st.ascii_mode = ime_state.ascii_mode;
        st.english_mode = english_mode;
        st.ime_state = Some(ime_state);
        st.focus_state_refresh_pending = false;
    }
    update_language_bar_mode(shared_state, english_mode);
}

/// True when neither the context nor the system state allows composing.
pub(crate) fn input_is_blocked(shared_state: &SharedState, context: Option<&ITfContext>) -> bool {
    // OnKeyUp and candidate clicks do not all pass through OnTestKeyDown.
    // Validate their supplied context too before consulting any cached policy.
    refresh_input_context_for_key(shared_state, context);
    let reason = input_block_reason(shared_state, context);
    if let Some(reason) = reason {
        append_input_blocked_diagnostic(reason, context);
        return true;
    }
    false
}

fn input_block_reason(
    shared_state: &SharedState,
    context: Option<&ITfContext>,
) -> Option<InputBlockReason> {
    if context.is_some() && !input_context_matches(shared_state, context) {
        return Some(InputBlockReason::ProbeFailed);
    }
    let (thread_mgr, keyboard_open, cached_reason) = {
        let st = shared_state.borrow();
        (
            st.thread_mgr.clone(),
            st.keyboard_open,
            st.input_context.block_reason(),
        )
    };
    if !keyboard_open {
        return Some(InputBlockReason::KeyboardClosed);
    }
    cached_reason
        .or_else(|| crate::input_context::context_block_reason(thread_mgr.as_ref(), context))
}

fn append_input_blocked_diagnostic(reason: InputBlockReason, context: Option<&ITfContext>) {
    append_diagnostic(format!(
        "input blocked reason={} ctx=0x{:x}",
        reason.diagnostic_name(),
        context.map_or(0, |context| context.as_raw() as usize),
    ));
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

/// Recover a failed initial registration and republish the current mode on
/// focus. Never remove/re-add a healthy item: only the first INPUTMODE item is
/// shown by Windows, and another service may own other entries in the manager.
pub(crate) fn refresh_language_bar(shared_state: &SharedState) {
    let (thread_mgr, client_id, english_mode, keyboard_open, existing) = {
        let mut st = shared_state.borrow_mut();
        if !st.language_bar_enabled || st.language_bar_refreshing {
            return;
        }
        let Some(thread_mgr) = st.thread_mgr.clone() else {
            return;
        };
        st.language_bar_refreshing = true;
        (
            thread_mgr,
            st.client_id,
            st.english_mode,
            st.keyboard_open,
            st.language_bar.clone(),
        )
    };
    struct RefreshGuard(SharedState);
    impl Drop for RefreshGuard {
        fn drop(&mut self) {
            self.0.borrow_mut().language_bar_refreshing = false;
        }
    }
    let _refresh_guard = RefreshGuard(Rc::clone(shared_state));
    let show_tray_mode_indicator = with_current_theme(|theme| theme.ui.show_tray_mode_indicator);
    let language_bar = match existing {
        Some(item) => item,
        None => {
            // AddItem can synchronously call TSF sinks. The guard above prevents
            // a nested focus event from registering a second button, without
            // keeping a RefCell borrow alive across the COM call.
            let item = match LanguageBarItem::add(
                &thread_mgr,
                client_id,
                Rc::downgrade(shared_state),
                english_mode,
                keyboard_open,
                show_tray_mode_indicator,
            ) {
                Ok(item) => item,
                Err(error) => {
                    let code = error.code();
                    let changed = shared_state
                        .borrow_mut()
                        .language_bar_add_error
                        .replace(code)
                        != Some(code);
                    if changed {
                        keytao_core::rt_log!(
                            Level::Info,
                            "error",
                            "language_bar_add",
                            hresult = format!("0x{:08X}", code.0 as u32),
                            msg = error.to_string()
                        );
                    }
                    return;
                }
            };
            let still_active = {
                let mut st = shared_state.borrow_mut();
                st.language_bar_add_error = None;
                let active = st.language_bar_enabled
                    && st.thread_mgr.as_ref() == Some(&thread_mgr)
                    && st.client_id == client_id;
                if active {
                    st.language_bar = Some(item.clone());
                }
                active
            };
            if !still_active {
                // Deactivate may also arrive from a synchronous TSF callback.
                item.remove();
                return;
            }
            item
        }
    };
    let (english_mode, keyboard_open) = {
        let st = shared_state.borrow();
        (st.english_mode, st.keyboard_open)
    };
    language_bar.refresh_mode(english_mode, keyboard_open, show_tray_mode_indicator);
}

pub(crate) fn update_language_bar_mode(shared_state: &SharedState, english_mode: bool) {
    let (language_bar, keyboard_open) = {
        let st = shared_state.borrow();
        (st.language_bar.clone(), st.keyboard_open)
    };
    if let Some(language_bar) = language_bar {
        language_bar.update_mode(
            english_mode,
            keyboard_open,
            with_current_theme(|theme| theme.ui.show_tray_mode_indicator),
        );
    }
}

#[cfg(test)]
#[path = "language_bar_lifecycle_tests.rs"]
mod language_bar_lifecycle_tests;

pub(crate) fn set_english_mode_from_language_bar(shared_state: &SharedState, english_mode: bool) {
    let reopen_keyboard = !english_mode && !shared_state.borrow().keyboard_open;
    reset_input_for_focus_change(shared_state);
    poll_engine_builds(shared_state);
    let session = shared_state.borrow().session();
    let ime_state = session.as_ref().map(|session| {
        session
            .set_english_mode(english_mode)
            .unwrap_or_else(|error| {
                keytao_core::rt_log!(Level::Info, "error", "set_english_mode", msg = error);
                session.state()
            })
    });
    let english_mode = ime_state
        .as_ref()
        .map_or(english_mode, ImeState::is_english_mode);
    {
        let mut state = shared_state.borrow_mut();
        state.ascii_mode = ime_state.as_ref().is_some_and(|state| state.ascii_mode);
        state.english_mode = english_mode;
        state.ime_state = ime_state;
    }
    if session.is_none() {
        start_engine_warmup(shared_state);
    }
    if reopen_keyboard {
        let (thread_mgr, client_id) = {
            let st = shared_state.borrow();
            (st.thread_mgr.clone(), st.client_id)
        };
        crate::input_context::set_keyboard_open(thread_mgr.as_ref(), client_id, true);
        apply_open_close_change(shared_state);
    }
    update_language_bar_mode(shared_state, english_mode);
}

pub(crate) fn update_ime_windows(
    shared_state: &SharedState,
    ime_state: &ImeState,
    document_mgr: Option<&ITfDocumentMgr>,
    caret: Option<(CaretPosition, CaretSource)>,
    show_mode_hint: bool,
    embedded: bool,
) -> bool {
    let (deployment_hint, input_generation) = {
        let st = shared_state.borrow();
        (
            keytao_core::DeploymentState::visible_hint(
                st.deployment_hint.as_deref(),
                !ime_state.candidates.is_empty(),
            )
            .map(str::to_owned),
            st.input_generation,
        )
    };
    let hint_state = deployment_hint.as_deref().map(|hint| {
        let mut state = ImeState::empty();
        state.candidates.push(keytao_core::Candidate {
            text: hint.to_string(),
            comment: None,
        });
        state
    });
    let ime_state = hint_state.as_ref().unwrap_or(ime_state);
    let (thread_mgr, allow_fallback_window) = {
        let st = shared_state.borrow();
        let uiless = host_is_uiless(&st);
        (st.thread_mgr.clone(), !uiless)
    };
    let allow_candidate_window = with_detached_candidate_ui(shared_state, |candidate_ui| {
        let allow = candidate_ui.update(
            thread_mgr.as_ref(),
            document_mgr,
            ime_state,
            allow_fallback_window,
        );
        let st = shared_state.borrow();
        if st.input_generation != input_generation
            || (st.ime_state.is_none() && st.deployment_hint.is_none())
        {
            drop(st);
            candidate_ui.end();
            false
        } else {
            allow
        }
    }) && (!with_current_theme(|theme| theme.ui.hide_candidate_panel)
        || ime_state.is_package_switcher()
        || deployment_hint.is_some());
    {
        let st = shared_state.borrow();
        if st.input_generation != input_generation
            || (st.ime_state.is_none() && st.deployment_hint.is_none())
        {
            return true;
        }
    }
    keytao_core::rt_log!(
        Level::Verbose,
        "ui",
        "candidate_window_policy",
        allowed = allow_candidate_window,
        caret_source = format!("{:?}", caret.map(|(_, source)| source)),
    );
    let weak_state = Rc::downgrade(shared_state);
    with_detached_windows(shared_state, false, |candidate_win, mode_hint_win| {
        let show = deployment_hint.is_some()
            || !ime_state.candidates.is_empty()
            || !ime_state.preedit.is_empty();
        if show && allow_candidate_window {
            if let Some((caret, _)) = caret {
                if candidate_win.show(
                    ime_state,
                    caret.x,
                    caret.y,
                    caret.owner_hwnd,
                    &weak_state,
                    embedded,
                    deployment_hint.as_deref(),
                ) {
                    shared_state.borrow_mut().last_shown_caret = Some(caret);
                }
            }
        } else {
            candidate_win.hide();
            shared_state.borrow_mut().last_shown_caret = None;
        }
        let caret_source = caret.map(|(_, source)| source);
        let retry_armed = shared_state.borrow().caret_retry_armed;
        if caret_source == Some(CaretSource::Probe) {
            {
                let mut st = shared_state.borrow_mut();
                st.caret_retry_armed = true;
                st.caret_retry_active = false;
            }
            candidate_win.disarm_caret_reprobe();
        } else if should_start_caret_reprobe(
            retry_armed,
            caret_source,
            (show && allow_candidate_window) || show_mode_hint,
        ) {
            {
                let mut st = shared_state.borrow_mut();
                st.caret_retry_armed = true;
                st.caret_retry_active = true;
            }
            if !candidate_win.arm_caret_reprobe(&weak_state) {
                let mut st = shared_state.borrow_mut();
                st.caret_retry_armed = false;
                st.caret_retry_active = false;
            }
        }
        if let Some((caret, _)) = caret.filter(|(_, source)| {
            deployment_hint.is_none()
                && show_mode_hint
                && !matches!(source, CaretSource::View | CaretSource::Window)
        }) {
            mode_hint_win.show_mode_hint(
                ime_state,
                caret.x,
                caret.y,
                caret.owner_hwnd,
                &weak_state,
            );
        }
    })
    .is_some()
}

fn should_start_caret_reprobe(armed: bool, source: Option<CaretSource>, visible: bool) -> bool {
    !armed && visible && should_arm_caret_reprobe(source)
}

pub(crate) fn user_data_dir() -> Option<&'static Path> {
    static USER_DATA_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    USER_DATA_DIR.get_or_init(default_user_data_dir).as_deref()
}

pub(crate) fn deployment_key_pressed(
    shared_state: &SharedState,
    context: Option<&ITfContext>,
) -> bool {
    let input_generation = {
        let mut st = shared_state.borrow_mut();
        if let Some(root) = user_data_dir() {
            st.deployment_state.key_pressed(root);
        }
        if let Some(context) = context {
            st.key_context = Some(context.clone());
        }
        st.input_generation
    };
    refresh_deployment_hint(shared_state);
    shared_state.borrow().input_generation == input_generation
}

pub(crate) fn poll_deployment_hint(shared_state: &SharedState) {
    let ready = shared_state.try_borrow().is_ok_and(|st| {
        !st.ime_write_session_active
            && !st.caret_probe_session_in_progress
            && st.candidate_win.is_some()
            && st.deployment_hint.is_some()
    });
    if !ready {
        return;
    }
    poll_engine_builds(shared_state);
    start_reload_if_needed(shared_state);
    refresh_deployment_hint(shared_state);
}

pub(crate) fn refresh_deployment_hint(shared_state: &SharedState) {
    let (has_hint, changed, context, has_candidates) = {
        let mut st = shared_state.borrow_mut();
        let hint = user_data_dir().and_then(|root| {
            keytao_core::DeploymentState::display_hint(root, st.deployment_state.hint(root))
        });
        let changed = st.deployment_hint != hint;
        st.deployment_hint = hint;
        if changed {
            st.last_shown_caret = None;
        }
        (
            st.deployment_hint.is_some(),
            changed,
            st.key_context.clone().or_else(|| st.panel_context.clone()),
            st.ime_state
                .as_ref()
                .is_some_and(|state| !state.preedit.is_empty() || !state.candidates.is_empty()),
        )
    };
    if !has_hint && !changed {
        return;
    }
    if !has_hint && !has_candidates {
        hide_candidate_window(shared_state);
    } else if let Some(context) = context {
        crate::key_event_sink::reposition_ime_windows(shared_state, &context, false);
    }
    let weak_state = Rc::downgrade(shared_state);
    let _ = with_detached_windows(shared_state, false, |candidate_win, _| {
        let active = shared_state.borrow().deployment_hint.is_some();
        candidate_win.set_deployment_hint_polling(&weak_state, active);
    });
}

pub(crate) fn host_is_uiless(state: &TsfState) -> bool {
    state.activation_flags & TF_TMAE_UIELEMENTENABLEDONLY != 0
        || state.thread_mgr_flags & TF_TMF_UIELEMENTENABLEDONLY != 0
}

pub(crate) fn hide_ime_windows(shared_state: &SharedState) {
    {
        let mut st = shared_state.borrow_mut();
        st.panel_context = None;
        st.pending_window_update = None;
        st.caret_rearm_pending = false;
        st.last_shown_caret = None;
        st.layout_reposition_pending = false;
        st.caret_retry_armed = false;
        st.caret_retry_active = false;
    }
    clear_layout_sink(shared_state);
    with_detached_candidate_ui(shared_state, CandidateUiManager::end);
    let _ = with_detached_windows(shared_state, true, |candidate_win, mode_hint_win| {
        candidate_win.hide();
        mode_hint_win.hide();
        if shared_state.borrow().deployment_hint.is_none() {
            candidate_win.set_deployment_hint_polling(&Rc::downgrade(shared_state), false);
        }
    });
}

pub(crate) fn defer_ime_ui_hide(shared_state: &SharedState) {
    let mut st = shared_state.borrow_mut();
    st.session_reset_pending = true;
    st.deployment_state.reset_input();
    st.deployment_hint = None;
    st.panel_context = None;
    st.pending_window_update = None;
    st.windows_dirty = true;
    st.windows_hide_pending = true;
}

pub(crate) fn post_pending_ime_ui(shared_state: &SharedState) {
    let posted = shared_state
        .borrow()
        .candidate_win
        .as_ref()
        .is_some_and(crate::candidate_win::CandidateWindow::post_pending_ime_ui);
    if !posted {
        flush_pending_ime_ui(shared_state);
    }
}

pub(crate) fn flush_pending_ime_ui(shared_state: &SharedState) {
    let (hide_pending, rearm_pending) = {
        let mut st = shared_state.borrow_mut();
        // A failed PostMessage may reach this fallback from DoEditSession.
        // Keep the queued work until the document lock has been released;
        // accessibility and window calls can synchronously reenter the host.
        if st.ime_write_session_active || st.caret_probe_session_in_progress {
            st.windows_dirty = true;
            return;
        }
        st.windows_dirty = false;
        (
            std::mem::take(&mut st.windows_hide_pending),
            std::mem::take(&mut st.caret_rearm_pending),
        )
    };
    if hide_pending {
        hide_ime_windows(shared_state);
    }
    crate::key_event_sink::flush_pending_composition_ends(shared_state);
    crate::key_event_sink::flush_pending_commit_retry(shared_state);
    crate::key_event_sink::flush_pending_ime_apply(shared_state);
    crate::key_event_sink::flush_pending_window_update(shared_state);
    if rearm_pending && !hide_pending {
        rearm_caret_reprobe(shared_state);
    }
}

pub(crate) fn request_caret_reprobe_rearm(shared_state: &SharedState) {
    shared_state.borrow_mut().caret_rearm_pending = true;
    post_pending_ime_ui(shared_state);
}

pub(crate) fn rearm_caret_reprobe(shared_state: &SharedState) {
    let weak_state = Rc::downgrade(shared_state);
    let _ = with_detached_windows(shared_state, false, |candidate_win, _mode_hint_win| {
        let active = {
            let mut st = shared_state.borrow_mut();
            if crate::candidate_win::caret_retry_delay_ms(st.caret_retry_attempts).is_none() {
                st.caret_retry_active = false;
            }
            st.caret_retry_active
        };
        if active && !candidate_win.arm_caret_reprobe(&weak_state) {
            let mut st = shared_state.borrow_mut();
            st.caret_retry_armed = false;
            st.caret_retry_active = false;
        }
    });
}

pub(crate) fn stop_caret_reprobe(shared_state: &SharedState) {
    shared_state.borrow_mut().caret_retry_active = false;
    let _ = with_detached_windows(shared_state, false, |candidate_win, _mode_hint_win| {
        candidate_win.disarm_caret_reprobe();
    });
}

pub(crate) fn schedule_layout_reposition(shared_state: &SharedState) {
    let weak_state = Rc::downgrade(shared_state);
    let armed = with_detached_windows(shared_state, false, |candidate_win, _mode_hint_win| {
        candidate_win.arm_layout_reposition(&weak_state)
    });
    if armed != Some(true) {
        shared_state.borrow_mut().layout_reposition_pending = false;
    }
}

pub(crate) fn reset_input_for_focus_change(shared_state: &SharedState) {
    let (anchor, client_id) = {
        let mut st = shared_state.borrow_mut();
        (st.panel_anchor.take(), st.client_id)
    };
    let active_composition = take_active_composition(shared_state);
    if let Some(anchor) = anchor {
        request_panel_anchor_end(shared_state, anchor, client_id);
    }
    if let Some((context, composition, client_id)) = active_composition {
        end_composition_now(shared_state, &context, composition, client_id);
    }
    hide_ime_windows(shared_state);
}

/// Try synchronous cleanup before the host stops pumping edit sessions.
pub(crate) fn terminate_input_now(shared_state: &SharedState) {
    let (anchor, client_id) = {
        let mut st = shared_state.borrow_mut();
        (st.panel_anchor.take(), st.client_id)
    };
    let active_composition = take_active_composition(shared_state);
    if let Some(anchor) = anchor {
        request_panel_anchor_end(shared_state, anchor, client_id);
    }
    if let Some((context, composition, client_id)) = active_composition {
        end_composition_now(shared_state, &context, composition, client_id);
    }
    // The thread may stop pumping the committed compositions' queued ends too.
    for pending in &mut shared_state.borrow_mut().pending_composition_ends {
        if !pending.exhausted {
            pending.queued = false;
            pending.retry_after = None;
        }
    }
    crate::key_event_sink::flush_pending_composition_ends(shared_state);
    hide_ime_windows(shared_state);
}

/// End only the native positioning anchor. Its range never owned any text.
pub(crate) fn request_panel_anchor_end(
    shared_state: &SharedState,
    anchor: PanelAnchor,
    _client_id: u32,
) {
    crate::key_event_sink::queue_panel_anchor_end(shared_state, anchor);
    crate::key_event_sink::flush_pending_composition_ends(shared_state);
}

fn take_active_composition(
    shared_state: &SharedState,
) -> Option<(ITfContext, ITfComposition, u32)> {
    let mut st = shared_state.borrow_mut();
    st.deferred_keys.reset();
    st.deferred_context = None;
    st.claimed_key = None;
    st.input_generation = st.input_generation.wrapping_add(1);
    st.pending_commit_retry = None;
    st.pending_ime_apply = None;
    let modifier_pending = st.modifier_tap_press.take().is_some();
    st.session_reset_pending = true;
    st.deployment_state.reset_input();
    st.deployment_hint = None;
    let active_composition = st
        .composition
        .take()
        .zip(st.composition_context.take())
        .map(|(composition, context)| (context, composition, st.client_id));
    st.ime_state = None;
    st.panel_context = None;
    st.last_caret = None;
    st.last_shown_caret = None;
    st.layout_reposition_pending = false;
    st.caret_retry_attempts = 0;
    st.caret_retry_armed = false;
    st.caret_retry_active = false;
    st.caret_retry_mode_hint = false;
    st.caret_rearm_pending = false;
    st.composition_in_flight = None;
    st.composition_clear_pending = false;
    st.pending_window_update = None;
    // The panel is about to be hidden, so no click can still be pending; the
    // stale context must not keep the host object alive.
    st.key_context = None;
    drop(st);
    append_diagnostic(format!(
        "focus reset cleared modifier_pending={}",
        u8::from(modifier_pending)
    ));
    active_composition
}

pub(crate) fn clear_input_after_composition_terminated(
    shared_state: &SharedState,
    terminated: Option<&ITfComposition>,
) {
    let Some(terminated) = terminated else {
        append_diagnostic("composition terminated identity_match=0 cleared=0");
        return;
    };
    let terminated_identity = terminated.as_raw() as usize;
    if let Some(pending) =
        crate::key_event_sink::take_pending_composition_end(shared_state, terminated_identity)
    {
        // A failed old EndComposition can finish after a different input frame
        // became active. Release only that old handle, not the new Rime state.
        let needs_terminated_cleanup = !pending.has_terminated_range();
        drop(pending);
        if needs_terminated_cleanup {
            crate::key_event_sink::queue_terminated_preedit_cleanup(shared_state, terminated);
            post_pending_ime_ui(shared_state);
        }
        append_diagnostic("composition terminated pending_cleanup=1 cleared=0");
        return;
    }
    let policy = {
        let st = shared_state.borrow();
        if !crate::composition_rules::should_clear_terminated_composition(
            st.composition
                .as_ref()
                .map(|composition| composition.as_raw() as usize),
            terminated_identity,
        ) {
            append_diagnostic("composition terminated stale_identity=1 cleared=0");
            return;
        }
        termination_policy(st.composition_in_flight, terminated_identity)
    };
    if policy == TerminationPolicy::Ignore {
        append_diagnostic("composition terminated identity_match=1 cleared=0");
        return;
    }
    crate::key_event_sink::queue_terminated_preedit_cleanup(shared_state, terminated);

    let (in_write_session, modifier_pending, panel_anchor, client_id) = {
        let mut st = shared_state.borrow_mut();
        st.input_generation = st.input_generation.wrapping_add(1);
        st.pending_commit_retry = None;
        st.pending_ime_apply = None;
        let in_write_session = st.ime_write_session_active;
        st.deferred_keys.reset();
        st.deferred_context = None;
        st.claimed_key = None;
        let modifier_pending = st.modifier_tap_press.take().is_some();
        st.session_reset_pending = true;
        st.composition = None;
        st.composition_context = None;
        st.panel_context = None;
        st.ime_state = None;
        st.last_caret = None;
        st.last_shown_caret = None;
        st.layout_reposition_pending = false;
        st.caret_retry_attempts = 0;
        st.caret_retry_armed = false;
        st.caret_retry_active = false;
        st.caret_retry_mode_hint = false;
        st.pending_window_update = None;
        st.composition_clear_pending = in_write_session;
        if in_write_session {
            st.windows_dirty = true;
            st.windows_hide_pending = true;
        }
        (
            in_write_session,
            modifier_pending,
            st.panel_anchor.take(),
            st.client_id,
        )
    };
    if let Some(anchor) = panel_anchor {
        request_panel_anchor_end(shared_state, anchor, client_id);
    }
    append_diagnostic(format!(
        "composition terminated identity_match=0 cleared=1 modifier_pending={}",
        u8::from(modifier_pending)
    ));
    if !in_write_session {
        hide_ime_windows(shared_state);
    }
    post_pending_ime_ui(shared_state);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminationPolicy {
    Ignore,
    Clear,
}

fn termination_policy(
    composition_in_flight: Option<usize>,
    terminated_identity: usize,
) -> TerminationPolicy {
    if composition_in_flight == Some(terminated_identity) {
        TerminationPolicy::Ignore
    } else {
        TerminationPolicy::Clear
    }
}

fn end_composition_now(
    shared_state: &SharedState,
    context: &ITfContext,
    composition: ITfComposition,
    _client_id: u32,
) {
    crate::key_event_sink::defer_failed_composition_end(shared_state, context, &composition, true);
    crate::key_event_sink::flush_pending_composition_ends(shared_state);
}

pub(crate) fn hide_candidate_window(shared_state: &SharedState) {
    {
        let mut st = shared_state.borrow_mut();
        st.panel_context = None;
        st.pending_window_update = None;
        st.caret_rearm_pending = false;
        st.last_shown_caret = None;
        st.layout_reposition_pending = false;
        st.caret_retry_armed = false;
        st.caret_retry_active = false;
    }
    with_detached_candidate_ui(shared_state, CandidateUiManager::end);
    let _ = with_detached_windows(shared_state, true, |candidate_win, _mode_hint_win| {
        candidate_win.hide();
    });
}

pub(crate) fn with_detached_windows<R>(
    shared_state: &SharedState,
    hide_request: bool,
    mut f: impl FnMut(
        &mut crate::candidate_win::CandidateWindow,
        &mut crate::candidate_win::CandidateWindow,
    ) -> R,
) -> Option<R> {
    fn run<R, F>(
        shared_state: &SharedState,
        hide_request: bool,
        f: &mut F,
        allow_followup: bool,
    ) -> Option<R>
    where
        F: FnMut(
            &mut crate::candidate_win::CandidateWindow,
            &mut crate::candidate_win::CandidateWindow,
        ) -> R,
    {
        let (mut candidate_win, mut mode_hint_win) = {
            let mut st = shared_state.borrow_mut();
            let Some(candidate_win) = st.candidate_win.take() else {
                st.windows_dirty = true;
                st.windows_hide_pending |= hide_request;
                return None;
            };
            let Some(mode_hint_win) = st.mode_hint_win.take() else {
                st.candidate_win = Some(candidate_win);
                st.windows_dirty = true;
                st.windows_hide_pending |= hide_request;
                return None;
            };
            (candidate_win, mode_hint_win)
        };

        let result = f(&mut candidate_win, &mut mode_hint_win);

        let (rerun, hide_pending, has_pending_update) = {
            let mut st = shared_state.borrow_mut();
            debug_assert!(st.candidate_win.is_none());
            debug_assert!(st.mode_hint_win.is_none());
            st.candidate_win = Some(candidate_win);
            st.mode_hint_win = Some(mode_hint_win);
            (
                st.windows_dirty,
                st.windows_hide_pending,
                st.pending_window_update.is_some(),
            )
        };

        if hide_pending || has_pending_update {
            flush_pending_ime_ui(shared_state);
        } else if rerun && allow_followup {
            shared_state.borrow_mut().windows_dirty = false;
            let _ = run(shared_state, false, f, false);
        }
        Some(result)
    }

    run(shared_state, hide_request, &mut f, true)
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

pub(crate) fn fallback_focus_window() -> windows::Win32::Foundation::HWND {
    unsafe { GetFocus() }
}

#[cfg(test)]
mod tests {
    use super::{
        should_start_caret_reprobe, termination_policy, with_detached_windows, CaretSource,
        InputContextRetry, TerminationPolicy, INPUT_CONTEXT_RETRY_INTERVAL,
        INPUT_CONTEXT_SYNC_BACKOFF,
    };
    use std::cell::Cell;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn deployment_loaded_check_is_throttled() {
        let mut state = super::TsfState::new();
        let checked_at = Instant::now() + Duration::from_secs(60);
        state.deployment_loaded_checked_at = Some(checked_at);

        state.deployment_loaded_cached();

        assert_eq!(state.deployment_loaded_checked_at, Some(checked_at));
    }

    struct FocusStateEngine {
        clear_calls: AtomicUsize,
        state_calls: AtomicUsize,
    }

    impl super::ImeEngineBackend for FocusStateEngine {
        fn build(&self) -> Result<super::EngineHandle, String> {
            unreachable!()
        }
        fn reload_needed(&self) -> bool {
            false
        }
        fn drop_session(&self) {}
        fn process_key(&self, _: u32, _: u32) -> Option<keytao_core::KeyProcessResult> {
            unreachable!()
        }
        fn process_keys(&self, _: &[(u32, u32)]) -> Option<keytao_core::KeyProcessResult> {
            unreachable!()
        }
        fn process_enter(&self) -> Option<keytao_core::KeyProcessResult> {
            unreachable!()
        }
        fn select_candidate_on_page(&self, _: usize) -> Option<keytao_core::ImeState> {
            unreachable!()
        }
        fn change_page(&self, _: bool) -> Option<keytao_core::ImeState> {
            unreachable!()
        }
        fn reset(&self) -> Option<keytao_core::ImeState> {
            unreachable!()
        }
        fn state(&self) -> keytao_core::ImeState {
            let mut state = keytao_core::ImeState::empty();
            if self.state_calls.fetch_add(1, Ordering::SeqCst) > 0 {
                state.schema_id = "xmjd6".into();
            }
            state
        }
        fn set_input_policy(
            &self,
            _: keytao_core::InputContextPolicy,
        ) -> Option<keytao_core::ImeState> {
            unreachable!()
        }
        fn set_context_ascii_mode(&self, enabled: Option<bool>) -> Option<keytao_core::ImeState> {
            assert_eq!(enabled, None);
            (self.clear_calls.fetch_add(1, Ordering::SeqCst) > 0).then(keytao_core::ImeState::empty)
        }
        fn set_english_mode(&self, _: bool) -> Result<keytao_core::ImeState, String> {
            unreachable!()
        }
    }

    #[test]
    fn focus_refresh_retries_busy_calls_before_ascii_key_routing() {
        let state = super::new_shared_state();
        let engine = Arc::new(FocusStateEngine {
            clear_calls: 0.into(),
            state_calls: 0.into(),
        });
        {
            let mut st = state.borrow_mut();
            st.runtime = Some(engine.clone());
            st.session = Some(super::EngineHandle {
                schema_id: "xmjd6".into(),
            });
            st.input_context = super::ContextInputState::unrestricted();
            st.input_policy_applied = Some(super::InputContextPolicy::default());
            st.focus_state_refresh_pending = true;
            st.ascii_mode = true;
            st.english_mode = true;
        }
        for _ in 0..2 {
            super::poll_engine_builds(&state);
            assert!(state.borrow().focus_state_refresh_pending);
            assert!(state.borrow().ascii_mode);
        }
        super::poll_engine_builds(&state);
        assert!(!state.borrow().focus_state_refresh_pending);
        assert!(!state.borrow().ascii_mode);
        assert!(!state.borrow().english_mode);
        super::poll_engine_builds(&state);
        assert_eq!(engine.clear_calls.load(Ordering::SeqCst), 3);
        assert_eq!(engine.state_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_stale_normal_build_cannot_supply_a_private_context_session() {
        let mut state = super::TsfState::new();
        let new_bundle = |private| super::EngineBundle {
            runtime: Arc::new(FocusStateEngine {
                clear_calls: 0.into(),
                state_calls: 0.into(),
            }),
            session: super::EngineHandle {
                schema_id: "xmjd6".into(),
            },
            private,
            reload_stamp_path: None,
            reload_stamp_signature: None,
        };
        state.input_context = super::ContextInputState::unrestricted();
        state.install_engine(new_bundle(false));
        assert!(state.session().is_some());
        state.input_context.password = super::ContextProbe::Private;
        assert!(state.session().is_none());
        assert!(state.begin_engine_build());
        state.engine_build_mailbox.store(false, Ok(new_bundle(false)));
        state.poll_engine_builds();
        assert!(state.session().is_none());
        assert!(state.begin_engine_build());
        state.engine_build_mailbox.store(true, Ok(new_bundle(true)));
        state.poll_engine_builds();
        assert!(state.session().is_some());
        state.input_context.password = super::ContextProbe::Clear;
        assert!(state.session().is_none());
    }

    #[test]
    fn missing_public_seed_failure_does_not_permanently_disable_warmup() {
        let mut state = super::TsfState::new();
        assert!(state.begin_engine_build());
        state.finish_engine_build_error("public schemas are unavailable".into());
        assert!(!state.begin_engine_build());
        state.engine_retry_after = Some(Instant::now());
        assert!(state.begin_engine_build());
        assert!(state.engine_error.is_none());
        assert!(!state.begin_engine_build());
    }

    #[test]
    fn reload_failure_cannot_bypass_engine_backoff() {
        let mut state = super::TsfState::new();
        state.reload_in_progress = true;
        state.finish_reload(Err("engine initialization timed out".into()));
        assert!(!state.reload_in_progress);
        assert_eq!(state.reload_retry_after, state.engine_retry_after);
        assert!(!state.begin_engine_build());
        assert_eq!(state.engine_failures, 1);
        state.engine_retry_after = Some(Instant::now());
        assert!(state.begin_engine_build());
        state.finish_engine_build_error("still unavailable".into());
        assert_eq!(state.engine_failures, 2);
        assert!(!state.begin_engine_build());
    }

    #[test]
    fn warmup_prevents_a_concurrent_reload() {
        let mut state = super::TsfState::new();
        let dir = std::env::temp_dir().join(format!(
            "keytao-warmup-reload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        state.reload_stamp_path = Some(keytao_core::ReloadStamp::write(&dir).unwrap());
        assert!(state.reload_needed());
        assert!(state.begin_engine_build());
        assert!(!state.begin_reload_if_changed());
        assert!(!state.reload_in_progress);
        state.finish_engine_build_error("warmup failed after reload failure".into());
        state.reload_retry_after = Some(Instant::now());
        assert!(!state.begin_reload_if_changed());
        assert!(!state.reload_in_progress);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn probe_retry_timestamp_survives_focus_and_compartment_churn() {
        let now = Instant::now();
        let mut retry = InputContextRetry::default();
        retry.begin_attempt(Some(1), now);
        retry.finish_attempt(true, true);
        for context in [Some(1), None, Some(2), Some(1)] {
            // Every callback consults the last attempt; querying or refreshing
            // compartments cannot create an earlier deadline.
            assert!(!retry.due(context, 1, now + Duration::from_millis(249)));
            assert_eq!(retry.last_attempt, Some(now));
        }
        assert!(retry.due(Some(1), 1, now + INPUT_CONTEXT_RETRY_INTERVAL));
    }

    #[test]
    fn context_state_and_compartment_callbacks_do_not_reset_the_last_attempt() {
        let state = super::new_shared_state();
        let now = Instant::now();
        {
            let mut st = state.borrow_mut();
            st.input_context_retry.begin_attempt(Some(1), now);
            st.input_context_retry.finish_attempt(true, true);
        }
        // No COM context is needed to exercise the actual state-write paths.
        // The initial context is already sensitive, so no UI teardown occurs.
        super::apply_input_context_state(&state, None, super::ContextInputState::default());
        super::apply_context_compartment_change(&state);
        let st = state.borrow();
        assert_eq!(st.input_context_retry.last_attempt, Some(now));
        assert_eq!(st.input_context_retry.consecutive_sync_refusals, 1);
        assert!(!st.input_context_retry.due(Some(1), 1, now));
    }

    #[test]
    fn three_sync_refusals_back_off_only_the_same_context() {
        let now = Instant::now();
        let mut retry = InputContextRetry::default();
        for attempt in 0..3 {
            let at = now + INPUT_CONTEXT_RETRY_INTERVAL * attempt;
            assert!(retry.due(Some(1), 1, at));
            retry.begin_attempt(Some(1), at);
            retry.finish_attempt(true, true);
        }
        let last_attempt = retry.last_attempt.unwrap();
        assert!(!retry.due(Some(1), 1, last_attempt + INPUT_CONTEXT_RETRY_INTERVAL));
        assert!(retry.due(Some(1), 1, last_attempt + INPUT_CONTEXT_SYNC_BACKOFF));
        assert!(retry.due(Some(2), 1, last_attempt + INPUT_CONTEXT_RETRY_INTERVAL));
        retry.begin_attempt(Some(2), last_attempt + INPUT_CONTEXT_RETRY_INTERVAL);
        retry.finish_attempt(true, true);
        assert_eq!(retry.consecutive_sync_refusals, 1);
        retry.begin_attempt(Some(2), last_attempt + INPUT_CONTEXT_RETRY_INTERVAL * 2);
        retry.finish_attempt(false, false);
        assert_eq!(retry.consecutive_sync_refusals, 0);
    }

    #[test]
    fn without_a_client_id_there_is_nothing_to_probe_with() {
        // `inspect_context` needs an edit cookie, and a read session cannot be
        // requested before Activate handed one out. Retrying would only burn
        // the throttle and keep the context stuck in pass-through.
        let now = Instant::now();
        assert!(!InputContextRetry::default().due(Some(1), 0, now));
    }

    #[test]
    fn caret_ladder_starts_at_most_once_per_apply() {
        for source in [
            None,
            Some(CaretSource::Cache),
            Some(CaretSource::System),
            Some(CaretSource::Accessibility),
            Some(CaretSource::View),
            Some(CaretSource::Window),
        ] {
            assert!(should_start_caret_reprobe(false, source, true));
            assert!(!should_start_caret_reprobe(true, source, true));
            assert!(!should_start_caret_reprobe(false, source, false));
        }
        assert!(!should_start_caret_reprobe(
            false,
            Some(CaretSource::Probe),
            true
        ));
    }

    #[test]
    fn ui_flush_preserves_pending_work_inside_document_sessions() {
        for write_session in [true, false] {
            let state = super::new_shared_state();
            {
                let mut st = state.borrow_mut();
                st.ime_write_session_active = write_session;
                st.caret_probe_session_in_progress = !write_session;
                st.windows_hide_pending = true;
                st.caret_rearm_pending = true;
            }
            super::flush_pending_ime_ui(&state);
            let st = state.borrow();
            assert!(st.windows_dirty);
            assert!(st.windows_hide_pending);
            assert!(st.caret_rearm_pending);
        }
    }

    #[test]
    fn only_matching_in_flight_composition_is_ignored() {
        assert_eq!(
            termination_policy(Some(0x1234), 0x1234),
            TerminationPolicy::Ignore
        );
        assert_eq!(
            termination_policy(Some(0x1234), 0x5678),
            TerminationPolicy::Clear
        );
        assert_eq!(termination_policy(None, 0x1234), TerminationPolicy::Clear);
    }

    #[test]
    fn detached_window_reentrancy_only_sets_pending_flags() {
        let state = super::new_shared_state();
        let (candidate_win, mode_hint_win) = {
            let mut st = state.borrow_mut();
            (st.candidate_win.take(), st.mode_hint_win.take())
        };
        let called = Cell::new(false);

        assert!(with_detached_windows(&state, false, |_, _| called.set(true)).is_none());
        {
            let st = state.borrow();
            assert!(st.windows_dirty);
            assert!(!st.windows_hide_pending);
            assert!(st.candidate_win.is_none());
            assert!(st.mode_hint_win.is_none());
        }
        assert!(!called.get());

        assert!(with_detached_windows(&state, true, |_, _| called.set(true)).is_none());
        {
            let mut st = state.borrow_mut();
            assert!(st.windows_dirty);
            assert!(st.windows_hide_pending);
            assert!(st.candidate_win.is_none());
            assert!(st.mode_hint_win.is_none());
            st.candidate_win = candidate_win;
            st.mode_hint_win = mode_hint_win;
        }
        assert!(!called.get());
    }
}
