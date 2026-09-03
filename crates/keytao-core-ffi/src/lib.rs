#[cfg(not(target_os = "android"))]
use std::ffi::CStr;
use std::ffi::CString;
use std::os::raw::c_char;
#[cfg(not(target_os = "android"))]
use std::os::raw::c_void;
use std::panic::AssertUnwindSafe;
#[cfg(not(target_os = "android"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "android"))]
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(not(target_os = "android"))]
use keytao_core::{
    key_policy, EngineCapabilities, ImeRuntime, ImeRuntimeSession, ImeState, InputContextPolicy,
    KeyProcessResult, ReloadStamp,
};
#[cfg(not(target_os = "android"))]
use keytao_theme::{
    resolve_theme_from_paths, resolve_theme_from_paths_with_system_scheme, resolved_theme_json,
    EffectiveColorScheme, ThemeResolver,
};

// ── C-compatible state struct ─────────────────────────────────────────────────

/// Flat view of IME state returned to C callers.
/// All strings are null-terminated UTF-8. Free with keytao_free_state().
#[repr(C)]
pub struct KeytaoState {
    pub preedit: *mut c_char,
    /// Caret position inside `preedit`, counted in Unicode scalars. Use
    /// keytao_utf16_offset_from_chars() to map it to UTF-16 units.
    pub cursor: u32,
    /// Start of the selected (already converted) range inside `preedit`, in
    /// Unicode scalars. Equal to `sel_end` when nothing is selected.
    pub sel_start: u32,
    /// End of the selected range inside `preedit`, in Unicode scalars.
    pub sel_end: u32,
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
    /// The directories the runtime was initialized for, so that a repeated
    /// keytao_init() for the same ones is recognized as a no-op.
    dirs: Option<InitDirs>,
    runtime: Option<ImeRuntime>,
    singleton_session: Option<ImeRuntimeSession>,
}

#[cfg(not(target_os = "android"))]
#[derive(Clone, PartialEq, Eq)]
struct InitDirs {
    user: PathBuf,
    shared: String,
}

/// Everything the exported functions read out of before they call keytao-core.
///
/// It is only ever held for as long as a field copy takes: a call that kept it
/// across a keytao-core call would hold every other export up for the length of
/// a deployment, and a panic in there would poison the lock the whole library
/// depends on. Handles and runtimes are cloned out, and the clone is what the
/// core call runs on.
#[cfg(not(target_os = "android"))]
static GLOBAL: Mutex<Global> = Mutex::new(Global {
    initialized: false,
    dirs: None,
    runtime: None,
    singleton_session: None,
});

/// Serializes keytao_init(). Frontends do initialize concurrently — iOS runs a
/// synchronous and an asynchronous startup path against the same engine — and
/// two inits interleaving would install one init's runtime under the other's
/// session. Taken before `SESSION_EPOCH` and `GLOBAL`, never while holding one.
#[cfg(not(target_os = "android"))]
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Which generation of handles keytao_create_session() is currently handing
/// out. It is bumped whenever keytao_init() switched librime to different
/// directories: sessions created before that point belong to a librime instance
/// that no longer exists, so they stop answering instead of composing against
/// the wrong deployment.
///
/// The counter is behind an `RwLock` rather than an atomic because a check on
/// its own would only be advisory: a call that read the epoch just before a
/// switch would still walk into keytao-core and have its engine rebuilt on the
/// new deployment. Every session call holds it for reading for as long as it is
/// inside keytao-core and keytao_init() bumps it under the write guard, so a
/// retired handle can never be in flight while librime goes down.
///
/// Lock order: `INIT_LOCK` → `SESSION_EPOCH` → `GLOBAL` → keytao-core. Nothing
/// may take `SESSION_EPOCH` while holding `GLOBAL` or a keytao-core lock.
/// `RELOAD_SIGNAL`, `THEME_SOURCES` and `UI_CAPABILITIES` are leaves: they are
/// never held while any of the above is.
#[cfg(not(target_os = "android"))]
static SESSION_EPOCH: RwLock<u64> = RwLock::new(0);

/// The reload signal keytao_init() installed for the directories it brought up,
/// plus the signature of the deployment that is currently in memory.
///
/// It sits outside `GLOBAL` because checking it stats and reads a file: a stamp
/// poll on a timer thread must not be able to hold a key up behind a disk read.
#[cfg(not(target_os = "android"))]
static RELOAD_SIGNAL: Mutex<Option<ReloadSignal>> = Mutex::new(None);

#[cfg(not(target_os = "android"))]
struct ReloadSignal {
    path: PathBuf,
    /// Signature of the stamp the deployment in memory was loaded for. `None`
    /// means nothing has ever been loaded for this signal.
    loaded: Option<String>,
}

#[cfg(not(target_os = "android"))]
impl ReloadSignal {
    /// The deployment keytao_init() is about to load is the one on disk now, so
    /// a stamp that already exists is not a request to reload again right away.
    fn new(user_dir: &Path) -> Self {
        let path = ReloadStamp::path(user_dir);
        let loaded = ReloadStamp::signature_at(&path);
        Self { path, loaded }
    }
}

#[cfg(not(target_os = "android"))]
struct SessionHandle {
    session: ImeRuntimeSession,
    epoch: u64,
}

/// A handle that is still current, plus the read guard that keeps keytao_init()
/// from retiring it while the call below is inside keytao-core.
#[cfg(not(target_os = "android"))]
struct ActiveSession<'a> {
    handle: &'a SessionHandle,
    _epoch: RwLockReadGuard<'a, u64>,
}

#[cfg(not(target_os = "android"))]
impl std::ops::Deref for ActiveSession<'_> {
    type Target = SessionHandle;

    fn deref(&self) -> &SessionHandle {
        self.handle
    }
}

/// Poisoning must not outlive the call that caused it: [`guard`] already
/// contained the panic and returned a failure to the caller, and refusing every
/// later call would leave the user without an input method until the process
/// restarts.
#[cfg(not(target_os = "android"))]
fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(not(target_os = "android"))]
fn read_ignore_poison<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(not(target_os = "android"))]
fn write_ignore_poison<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// Run `action` on the session behind the legacy singleton API.
///
/// The session is cloned out so that librime is never called while `GLOBAL` is
/// held: the call can take as long as a deployment and a panic in there would
/// poison the lock every other exported function needs. The epoch guard is held
/// for the same reason a per-client handle holds it — keytao_init() must not be
/// able to take librime down mid-call.
#[cfg(not(target_os = "android"))]
fn with_singleton_session<T>(default: T, action: impl FnOnce(&ImeRuntimeSession) -> T) -> T {
    let _epoch = read_ignore_poison(&SESSION_EPOCH);
    let session = lock_ignore_poison(&GLOBAL).singleton_session.clone();
    match session {
        Some(session) => action(&session),
        None => default,
    }
}

/// Create a session, giving a lost directory race one more chance.
///
/// keytao-core brings librime up for the runtime's own directories before it
/// creates a session and only reports this failure when something switched
/// librime again in the window in between, so the answer is to ask a second
/// time rather than to hand the frontend a null handle. A second failure is
/// reported: retrying past that would be a loop against whoever keeps
/// switching.
#[cfg(not(target_os = "android"))]
fn create_session_retrying_a_switch(runtime: &ImeRuntime) -> Result<ImeRuntimeSession, String> {
    retry_once_on_directory_switch(|| runtime.create_session())
}

#[cfg(not(target_os = "android"))]
fn retry_once_on_directory_switch<T>(attempt: impl Fn() -> Result<T, String>) -> Result<T, String> {
    match attempt() {
        Err(e) if librime_switched_directories(&e) => attempt(),
        result => result,
    }
}

/// Whether keytao-core refused because librime is up for a different pair of
/// data directories. keytao-core reports its failures as strings, so the
/// message is the only thing there is to recognize it by.
#[cfg(not(target_os = "android"))]
fn librime_switched_directories(error: &str) -> bool {
    error.contains("other data directories")
}

/// Theme sources for the JSON state helpers plus the resolver built from them.
/// Holding a resolver (instead of the raw paths) keeps the key path free of
/// YAML parsing and disk reads: it only re-reads when the theme files change.
#[cfg(not(target_os = "android"))]
struct ThemeSources {
    default_theme_path: Option<PathBuf>,
    user_theme_path: Option<PathBuf>,
    /// Color scheme the platform observed, so the IME process never has to probe
    /// the system for it. `None` falls back to keytao-theme's own detection.
    system_scheme: Option<EffectiveColorScheme>,
    resolver: ThemeResolver,
}

#[cfg(not(target_os = "android"))]
impl ThemeSources {
    fn new(
        default_theme_path: Option<PathBuf>,
        user_theme_path: Option<PathBuf>,
        system_scheme: Option<EffectiveColorScheme>,
    ) -> Self {
        Self {
            resolver: ThemeResolver::with_system_scheme(
                default_theme_path.clone(),
                user_theme_path.clone(),
                system_scheme,
            ),
            default_theme_path,
            user_theme_path,
            system_scheme,
        }
    }
}

#[cfg(not(target_os = "android"))]
static THEME_SOURCES: Mutex<Option<ThemeSources>> = Mutex::new(None);

/// What the frontend can render, declared through keytao_set_ui_capabilities().
/// `None` keeps the soft-keyboard shape the JSON API has always produced.
#[cfg(not(target_os = "android"))]
static UI_CAPABILITIES: Mutex<Option<keytao_theme::UiCapabilities>> = Mutex::new(None);

#[cfg(not(target_os = "android"))]
fn current_ui_capabilities() -> keytao_theme::UiCapabilities {
    if let Some(capabilities) = lock_ignore_poison(&UI_CAPABILITIES).as_ref() {
        return capabilities.clone();
    }
    // A candidate bar above a soft keyboard cannot go vertical.
    keytao_theme::UiCapabilities {
        supports_vertical: false,
        ..keytao_theme::UiCapabilities::full_custom()
    }
}

// ── Panic boundary ────────────────────────────────────────────────────────────

/// Run `body` and turn a panic into `default`.
///
/// Unwinding out of an `extern "C"` function aborts the process, which for an
/// input method means the frontend disappears mid-typing, so every exported
/// function funnels through here.
fn guard<T>(name: &str, default: T, body: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            log_error(format_args!(
                "{name}: panicked: {}",
                panic_message(&payload)
            ));
            default
        }
    }
}

/// Report an error the way an ABI boundary has to: without a way to unwind.
///
/// `eprintln!` panics when the write fails, and reporting happens after
/// [`guard`] already caught one panic, outside its `catch_unwind`. A closed or
/// full stderr would turn the log line itself into the panic that crosses
/// `extern "C"` and aborts the process, so io errors are dropped and a panic
/// from formatting is caught by a boundary of its own.
fn log_error(args: std::fmt::Arguments<'_>) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        use std::io::Write;
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_fmt(args);
        let _ = stderr.write_all(b"\n");
    }));
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> &str {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else {
        "unknown panic"
    }
}

// ── Public C API ──────────────────────────────────────────────────────────────

/// Initialize the Rime runtime. Must be called once before any other function.
/// Both `user_dir` and `shared_dir` must be non-null UTF-8 strings.
/// Returns true on success.
///
/// Calls are serialized and a repeat for the directories already running is a
/// no-op, so a frontend that initializes from two paths at once keeps the
/// session it is composing in. Switching to different directories takes
/// librime down and back up: sessions created before the switch stop answering
/// and have to be recreated with keytao_create_session().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_init(user_dir: *const c_char, shared_dir: *const c_char) -> bool {
    guard("keytao_init", false, || {
        let user = match c_string_arg(user_dir, "user_dir") {
            Ok(value) => value,
            Err(e) => {
                log_error(format_args!("keytao_init: {e}"));
                return false;
            }
        };
        let shared = match c_string_arg(shared_dir, "shared_dir") {
            Ok(value) => value,
            Err(e) => {
                log_error(format_args!("keytao_init: {e}"));
                return false;
            }
        };

        let _serialized = lock_ignore_poison(&INIT_LOCK);
        let dirs = InitDirs {
            user: PathBuf::from(&user),
            shared: shared.clone(),
        };
        let previous = {
            let g = lock_ignore_poison(&GLOBAL);
            if g.initialized && g.dirs.as_ref() == Some(&dirs) && g.singleton_session.is_some() {
                // librime is already up for exactly these directories; tearing
                // the runtime down and rebuilding it would only throw away the
                // sessions and the composition they hold.
                return true;
            }
            g.dirs.clone()
        };

        let switching = previous.is_some_and(|previous| previous != dirs);
        // No session call may be inside keytao-core while librime is taken down
        // and brought back up, so the whole rebuild runs under the write guard.
        // `GLOBAL` was released above: it is only ever taken below this lock.
        let mut epoch = write_ignore_poison(&SESSION_EPOCH);
        if switching {
            // keytao-core finalizes librime before bringing it up for the new
            // directories, so every session handed out so far is done.
            log_error(format_args!(
                "keytao_init: switching to {}; sessions created earlier are now invalid",
                dirs.user.display()
            ));
            *epoch += 1;
            clear_global();
        }

        let runtime = ImeRuntime::with_dirs(user, shared);
        if let Err(e) = runtime.init_without_deploy() {
            log_error(format_args!("keytao_init: runtime init failed: {e}"));
            return false;
        }
        let singleton_session = match create_session_retrying_a_switch(&runtime) {
            Ok(session) => session,
            Err(e) => {
                log_error(format_args!(
                    "keytao_init: runtime.create_session failed: {e}"
                ));
                return false;
            }
        };
        drop(epoch);

        // Reads the stamp, so it is built before the lock is taken.
        let signal = ReloadSignal::new(&dirs.user);
        *lock_ignore_poison(&RELOAD_SIGNAL) = Some(signal);
        let stale = {
            let mut g = lock_ignore_poison(&GLOBAL);
            g.initialized = true;
            g.dirs = Some(dirs);
            g.runtime = Some(runtime);
            g.singleton_session.replace(singleton_session)
        };
        // Dropping a session destroys a librime session, so it happens outside
        // the lock like every other core call.
        drop(stale);
        true
    })
}

/// Forget the runtime, the session and the reload signal, for when librime went
/// down under them. The pieces are dropped after the lock is released.
#[cfg(not(target_os = "android"))]
fn clear_global() {
    lock_ignore_poison(&RELOAD_SIGNAL).take();
    let (runtime, session) = {
        let mut g = lock_ignore_poison(&GLOBAL);
        g.initialized = false;
        g.dirs = None;
        (g.runtime.take(), g.singleton_session.take())
    };
    drop(runtime);
    drop(session);
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_is_initialized() -> bool {
    guard("keytao_is_initialized", false, || {
        lock_ignore_poison(&GLOBAL).initialized
    })
}

/// Redeploy Rime data through the shared runtime. Every session created through
/// this library drops its engine first, so librime re-reads the deployment
/// instead of serving its cached config and dictionaries.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload() -> bool {
    guard("keytao_reload", false, reload_now)
}

#[cfg(not(target_os = "android"))]
fn reload_now() -> bool {
    // A reload finalizes librime and brings it back up for the runtime it was
    // started from. Held for reading so that keytao_init() cannot switch to
    // other directories while this one is in flight: the reload would otherwise
    // finish by pointing librime back at the directories the switch just left,
    // and mark that deployment as loaded on top of it.
    let _epoch = read_ignore_poison(&SESSION_EPOCH);
    let runtime = {
        let g = lock_ignore_poison(&GLOBAL);
        if !g.initialized {
            return false;
        }
        let Some(runtime) = g.runtime.clone() else {
            return false;
        };
        runtime
    };

    let loading = begin_reload();

    match runtime.reload_without_deploy() {
        Ok(()) => {
            if let Some((path, signature)) = loading {
                mark_deployment_loaded(&path, signature);
            }
            true
        }
        Err(e) => {
            log_error(format_args!("keytao_reload: runtime reload failed: {e}"));
            false
        }
    }
}

/// Path of the reload signal for the directory passed to keytao_init(), or null
/// before initialization. Free with keytao_free_string().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload_stamp_path() -> *mut c_char {
    guard("keytao_reload_stamp_path", std::ptr::null_mut(), || {
        let Some(path) = watched_stamp_path() else {
            return std::ptr::null_mut();
        };
        to_cstring(&path.to_string_lossy())
    })
}

/// Where keytao_init() installed the reload signal, copied out so that the stat
/// and read that follow happen without the lock held.
#[cfg(not(target_os = "android"))]
fn watched_stamp_path() -> Option<PathBuf> {
    lock_ignore_poison(&RELOAD_SIGNAL)
        .as_ref()
        .map(|signal| signal.path.clone())
}

/// Signature of the stamp the loaded deployment corresponds to, next to the
/// path it was read from.
#[cfg(not(target_os = "android"))]
fn loaded_deployment() -> Option<(PathBuf, Option<String>)> {
    lock_ignore_poison(&RELOAD_SIGNAL)
        .as_ref()
        .map(|signal| (signal.path.clone(), signal.loaded.clone()))
}

/// The stamp a reload that starts now is going to load, to be handed to
/// [`mark_deployment_loaded`] once it succeeded.
///
/// It has to be read *before* the reload: committing whatever is on disk
/// afterwards would mark a deployment that finished while the reload was
/// running as loaded, and the frontend would keep serving the old dictionaries
/// until the next deploy.
#[cfg(not(target_os = "android"))]
fn begin_reload() -> Option<(PathBuf, Option<String>)> {
    let path = watched_stamp_path()?;
    let signature = ReloadStamp::signature_at(&path);
    Some((path, signature))
}

/// Record `signature` as the deployment now in memory, unless keytao_init()
/// swapped the signal out for a different directory in the meantime.
#[cfg(not(target_os = "android"))]
fn mark_deployment_loaded(path: &Path, signature: Option<String>) {
    let mut signal = lock_ignore_poison(&RELOAD_SIGNAL);
    if let Some(signal) = signal.as_mut() {
        if signal.path == path {
            signal.loaded = signature;
        }
    }
}

/// Current signature of the reload signal, or null when no stamp exists. The
/// format is keytao-core's and must not be reimplemented by frontends. Free
/// with keytao_free_string().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload_stamp_signature() -> *mut c_char {
    guard(
        "keytao_reload_stamp_signature",
        std::ptr::null_mut(),
        || {
            let Some(path) = watched_stamp_path() else {
                return std::ptr::null_mut();
            };
            ReloadStamp::signature_at(&path)
                .map(|signature| to_cstring(&signature))
                .unwrap_or(std::ptr::null_mut())
        },
    )
}

/// Path of the reload signal inside `user_dir`, for frontends that watch the
/// file before keytao_init() has succeeded. Free with keytao_free_string().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload_stamp_path_at(user_dir: *const c_char) -> *mut c_char {
    guard("keytao_reload_stamp_path_at", std::ptr::null_mut(), || {
        let Ok(user_dir) = c_string_arg(user_dir, "user_dir") else {
            return std::ptr::null_mut();
        };
        to_cstring(&ReloadStamp::path(&PathBuf::from(user_dir)).to_string_lossy())
    })
}

/// Signature of the reload signal inside `user_dir`, or null when no deployment
/// has requested a reload yet. Same format keytao_reload_stamp_signature()
/// returns. Free with keytao_free_string().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload_stamp_signature_at(user_dir: *const c_char) -> *mut c_char {
    guard(
        "keytao_reload_stamp_signature_at",
        std::ptr::null_mut(),
        || {
            let Ok(user_dir) = c_string_arg(user_dir, "user_dir") else {
                return std::ptr::null_mut();
            };
            ReloadStamp::current_signature(&PathBuf::from(user_dir))
                .map(|signature| to_cstring(&signature))
                .unwrap_or(std::ptr::null_mut())
        },
    )
}

/// Whether the reload signal changed since the last call, which also marks it as
/// seen. Frontends that want to schedule the reload themselves use this;
/// keytao_reload_if_stamp_changed() does both in one step.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload_stamp_changed() -> bool {
    guard("keytao_reload_stamp_changed", false, || {
        let Some((path, current)) = stamp_changed_peek() else {
            return false;
        };
        mark_deployment_loaded(&path, Some(current));
        true
    })
}

/// The pending reload request, without marking it as seen: the stamp path and
/// the signature that differs from what is loaded.
#[cfg(not(target_os = "android"))]
fn stamp_changed_peek() -> Option<(PathBuf, String)> {
    let (path, loaded) = loaded_deployment()?;
    // A missing stamp is not a reload request, so only an existing one that
    // differs from what was last loaded counts.
    let current = ReloadStamp::signature_at(&path)?;
    (loaded.as_deref() != Some(current.as_str())).then_some((path, current))
}

/// Reload when the app requested one, returning true only if a reload actually
/// ran. Cheap enough for a timer, but not for the key path: it stats a file.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reload_if_stamp_changed() -> bool {
    guard("keytao_reload_if_stamp_changed", false, || {
        // The request is only marked as seen once the reload really ran, so a
        // transient failure is retried on the next tick instead of swallowed.
        stamp_changed_peek().is_some() && reload_now()
    })
}

/// Create a per-client input session. Returns null if keytao_init() has not
/// completed successfully. Destroy with keytao_destroy_session().
///
/// The session belongs to the directories keytao_init() was last called with;
/// a later init for different ones retires it, and every call on it then
/// returns null or false.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_create_session() -> *mut c_void {
    guard("keytao_create_session", std::ptr::null_mut(), || {
        // Taken before `GLOBAL` and held across the core call, so the handle
        // cannot be stamped with an epoch that keytao_init() retires while the
        // session it belongs to is still being created.
        let epoch = read_ignore_poison(&SESSION_EPOCH);
        let runtime = {
            let g = lock_ignore_poison(&GLOBAL);
            if !g.initialized {
                return std::ptr::null_mut();
            }
            let Some(runtime) = g.runtime.clone() else {
                return std::ptr::null_mut();
            };
            runtime
        };

        match create_session_retrying_a_switch(&runtime) {
            Ok(session) => Box::into_raw(Box::new(SessionHandle {
                session,
                epoch: *epoch,
            })) as *mut c_void,
            Err(e) => {
                log_error(format_args!(
                    "keytao_create_session: runtime.create_session failed: {e}"
                ));
                std::ptr::null_mut()
            }
        }
    })
}

/// Destroy a session created by keytao_create_session(). Retired handles are
/// freed too; only their librime session is already gone.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_destroy_session(session: *mut c_void) {
    guard("keytao_destroy_session", (), || {
        if session.is_null() {
            return;
        }
        // Dropping the handle destroys a librime session, so it must not run
        // while keytao_init() is finalizing librime: a destroy that lands after
        // the finalize would be handing a stale id to a fresh librime, which
        // recycles session ids.
        let _epoch = read_ignore_poison(&SESSION_EPOCH);
        unsafe {
            drop(Box::from_raw(session as *mut SessionHandle));
        }
    })
}

/// Return current state for a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_state(session: *mut c_void) -> *mut KeytaoState {
    guard("keytao_session_state", std::ptr::null_mut(), || {
        let Some(handle) = session_handle(session) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(state_to_c(handle.session.state(), false)))
    })
}

/// Process a key event on a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_process_key(
    session: *mut c_void,
    keyval: u32,
    modifiers: u32,
) -> *mut KeytaoState {
    guard("keytao_session_process_key", std::ptr::null_mut(), || {
        let Some(handle) = session_handle(session) else {
            return std::ptr::null_mut();
        };
        let Some(result) = handle.session.process_key_result(keyval, modifiers) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(result_to_c(result)))
    })
}

/// Handle `Return` the way every frontend must: librime decides, and only if it
/// passes the key back is the raw input committed.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_process_enter(session: *mut c_void) -> *mut KeytaoState {
    guard("keytao_session_process_enter", std::ptr::null_mut(), || {
        let Some(handle) = session_handle(session) else {
            return std::ptr::null_mut();
        };
        let Some(result) = handle.session.process_enter() else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(result_to_c(result)))
    })
}

/// Select a candidate in a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_select_candidate(
    session: *mut c_void,
    index: u32,
) -> *mut KeytaoState {
    guard(
        "keytao_session_select_candidate",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.select_candidate_on_page(index as usize) else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        },
    )
}

/// Move the highlight without committing, for hover and arrow-key navigation.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_highlight_candidate(
    session: *mut c_void,
    index: u32,
) -> *mut KeytaoState {
    guard(
        "keytao_session_highlight_candidate",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.highlight_candidate_on_page(index as usize) else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        },
    )
}

/// Forget a learned phrase, the action behind "delete candidate" gestures.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_delete_candidate(
    session: *mut c_void,
    index: u32,
) -> *mut KeytaoState {
    guard(
        "keytao_session_delete_candidate",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some((state, deleted)) = handle
                .session
                .delete_candidate_on_page_result(index as usize)
            else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, deleted)))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_candidate_is_user_phrase(
    session: *mut c_void,
    index: u32,
) -> bool {
    guard("keytao_session_candidate_is_user_phrase", false, || {
        let Some(handle) = session_handle(session) else {
            return false;
        };
        handle
            .session
            .candidate_is_user_phrase_on_page(index as usize)
            .unwrap_or(false)
    })
}

/// Flip to the next/previous candidate page in a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_change_page(
    session: *mut c_void,
    backward: bool,
) -> *mut KeytaoState {
    guard("keytao_session_change_page", std::ptr::null_mut(), || {
        let Some(handle) = session_handle(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = handle.session.change_page(backward) else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(state_to_c(state, true)))
    })
}

/// Clear current composition in a per-client session.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_reset(session: *mut c_void) -> *mut KeytaoState {
    guard("keytao_session_reset", std::ptr::null_mut(), || {
        let Some(handle) = session_handle(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = handle.session.reset() else {
            return std::ptr::null_mut();
        };
        Box::into_raw(Box::new(state_to_c(state, true)))
    })
}

/// Commit what is being composed. The path to take when the input context ends
/// and the composition must not be lost.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_commit_composition(session: *mut c_void) -> *mut KeytaoState {
    guard(
        "keytao_session_commit_composition",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.commit_composition() else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        },
    )
}

/// Discard what is being composed. The path to take when the input context ends
/// and the composition must not reach the client.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_clear_composition(session: *mut c_void) -> *mut KeytaoState {
    guard(
        "keytao_session_clear_composition",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.clear_composition() else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        },
    )
}

/// Declare what the current input context allows. Password and PIN fields pass
/// `composing = false`, which stops keys from reaching librime at all, so no
/// preedit appears and nothing is learned. Returns the state to apply.
///
/// A null return means there was no state to hand back — the handle was
/// retired, or no engine was up — **not** that the policy was rejected. The
/// policy is recorded either way and applies to every later key, so a frontend
/// must never read null as "still composing" and fall back to sending the
/// password to librime.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_set_input_policy(
    session: *mut c_void,
    composing: bool,
    learning: bool,
) -> *mut KeytaoState {
    guard(
        "keytao_session_set_input_policy",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.set_input_policy(InputContextPolicy {
                composing,
                learning,
            }) else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        },
    )
}

/// Whether the current input context still lets keys reach librime.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_input_policy_composing(session: *mut c_void) -> bool {
    guard("keytao_session_input_policy_composing", false, || {
        session_handle(session)
            .map(|handle| handle.session.input_policy().composing)
            .unwrap_or(false)
    })
}

/// Whether the current input context may contribute to user learning.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_input_policy_learning(session: *mut c_void) -> bool {
    guard("keytao_session_input_policy_learning", false, || {
        session_handle(session)
            .map(|handle| handle.session.input_policy().learning)
            .unwrap_or(false)
    })
}

/// Return whether a per-client session is in ASCII mode.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_get_ascii_mode(session: *mut c_void) -> bool {
    guard("keytao_session_get_ascii_mode", false, || {
        let Some(handle) = session_handle(session) else {
            return false;
        };
        handle.session.is_ascii_mode()
    })
}

/// Set ASCII mode on a per-client session and return the updated state.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_set_ascii_mode(
    session: *mut c_void,
    enabled: bool,
) -> *mut KeytaoState {
    guard(
        "keytao_session_set_ascii_mode",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.set_ascii_mode(enabled) else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        },
    )
}

// ── Engine capabilities ───────────────────────────────────────────────────────

// The values are written as plain literals rather than shifts because these go
// through cbindgen into `#define`s, and Swift's importer only picks up macros
// whose body is a literal — `(1 << 4)` would arrive in Swift as nothing at all.

/// Bit 0. `RimeSelectCandidateOnCurrentPage` is there: tapping a candidate
/// selects it. Without this bit a tap can only send the schema's
/// `menu/select_keys` character, which schemas without select keys type into
/// the composition instead — so the frontend must not offer tap-to-select.
pub const KEYTAO_CAP_CANDIDATE_SELECTION: u32 = 1;
/// Bit 1. `RimeSelectCandidate`: selection by index into the whole candidate
/// list, what keytao_session_select_candidate_global_json() needs.
pub const KEYTAO_CAP_GLOBAL_CANDIDATE_SELECTION: u32 = 2;
/// Bit 2. `RimeHighlightCandidateOnCurrentPage`: moving the highlight without
/// committing. Without this bit hover and arrow-key highlighting do nothing.
pub const KEYTAO_CAP_CANDIDATE_HIGHLIGHT: u32 = 4;
/// Bit 3. `RimeDeleteCandidateOnCurrentPage`, the "forget this phrase" gesture.
pub const KEYTAO_CAP_CANDIDATE_DELETION: u32 = 8;
/// Bit 4. `RimeChangePage`: paging through librime. Without this bit paging
/// replays the `-`/`=` bindings of the default `key_binder` preset, which many
/// schemas never import — so the frontend must not offer page up/down controls.
pub const KEYTAO_CAP_NATIVE_PAGING: u32 = 16;
/// Bit 5. `RimeCommitComposition`. Without it, committing sends `Return`.
pub const KEYTAO_CAP_COMMIT_COMPOSITION: u32 = 32;
/// Bit 6. `RimeClearComposition`. Without it, discarding sends `Escape`.
pub const KEYTAO_CAP_CLEAR_COMPOSITION: u32 = 64;

#[cfg(not(target_os = "android"))]
fn capability_bits(capabilities: EngineCapabilities) -> u32 {
    let mut bits = 0;
    for (bit, present) in [
        (
            KEYTAO_CAP_CANDIDATE_SELECTION,
            capabilities.candidate_selection,
        ),
        (
            KEYTAO_CAP_GLOBAL_CANDIDATE_SELECTION,
            capabilities.global_candidate_selection,
        ),
        (
            KEYTAO_CAP_CANDIDATE_HIGHLIGHT,
            capabilities.candidate_highlight,
        ),
        (
            KEYTAO_CAP_CANDIDATE_DELETION,
            capabilities.candidate_deletion,
        ),
        (KEYTAO_CAP_NATIVE_PAGING, capabilities.native_paging),
        (
            KEYTAO_CAP_COMMIT_COMPOSITION,
            capabilities.commit_composition,
        ),
        (KEYTAO_CAP_CLEAR_COMPOSITION, capabilities.clear_composition),
    ] {
        if present {
            bits |= bit;
        }
    }
    bits
}

/// What the linked librime can do through its own entry points, as a mask of
/// the `KEYTAO_CAP_*` bits.
///
/// A missing capability degrades to a synthesized key stroke whose meaning
/// depends on the schema — a select key, `-`/`=`, `Escape` — so a control that
/// needs one must be hidden rather than shown and quietly typing characters
/// into the composition. The iOS build ships librime 1.8.5, where
/// `KEYTAO_CAP_NATIVE_PAGING` and `KEYTAO_CAP_CANDIDATE_HIGHLIGHT` are always
/// absent.
///
/// Answerable before keytao_init(): it only inspects the ABI.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_engine_capabilities() -> u32 {
    guard("keytao_engine_capabilities", 0, || {
        capability_bits(keytao_core::engine_capabilities())
    })
}

/// The `KEYTAO_CAP_*` mask for the librime a session is composing against.
///
/// Zero — every control disabled — for a null or retired handle, which is the
/// safe answer: a handle that cannot reach librime cannot select or page
/// either.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_capabilities(session: *mut c_void) -> u32 {
    guard("keytao_session_capabilities", 0, || {
        session_handle(session)
            .map(|handle| capability_bits(handle.session.capabilities()))
            .unwrap_or(0)
    })
}

/// Same capabilities as keytao_engine_capabilities(), as a JSON object with the
/// camelCase keys the other JSON helpers use. Free with keytao_free_string().
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_engine_capabilities_json() -> *mut c_char {
    guard(
        "keytao_engine_capabilities_json",
        std::ptr::null_mut(),
        || {
            let capabilities = keytao_core::engine_capabilities();
            let value = serde_json::json!({
                "candidateSelection": capabilities.candidate_selection,
                "globalCandidateSelection": capabilities.global_candidate_selection,
                "candidateHighlight": capabilities.candidate_highlight,
                "candidateDeletion": capabilities.candidate_deletion,
                "nativePaging": capabilities.native_paging,
                "commitComposition": capabilities.commit_composition,
                "clearComposition": capabilities.clear_composition,
            });
            to_cstring(&serde_json::to_string(&value).unwrap_or_else(|_| "{}".into()))
        },
    )
}

// ── key_policy, the one implementation every frontend shares ──────────────────

/// The X11 keysym a soft keyboard key should send, or 0 when the text is not a
/// single typable character and has to be committed directly instead.
///
/// Latin-1 maps onto itself and everything else uses X11's `0x01000000 | cp`
/// encoding, so `（` (U+FF08) never arrives as `XK_BackSpace`.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_text_to_keysym(text: *const c_char) -> u32 {
    guard("keytao_text_to_keysym", 0, || {
        let Ok(text) = c_string_arg(text, "text") else {
            return 0;
        };
        key_policy::keysym_for_text(&text).unwrap_or(0)
    })
}

/// Whether a keysym is `Return` or keypad `Return`.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_key_policy_is_enter(keyval: u32) -> bool {
    guard("keytao_key_policy_is_enter", false, || {
        key_policy::is_enter_key(keyval)
    })
}

/// Whether a key must be handed straight to the application instead of librime.
///
/// `ascii_mode` deliberately plays no part: English mode still needs librime's
/// `ascii_composer` to see the key, and Control/Alt chords carry Rime's own
/// switcher hotkeys, so only window-system modifiers pass through early.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_key_policy_should_bypass(
    session: *mut c_void,
    keyval: u32,
    modifiers: u32,
) -> bool {
    guard("keytao_key_policy_should_bypass", false, || {
        let Some(handle) = session_handle(session) else {
            return false;
        };
        let state = handle.session.state();
        key_policy::should_bypass_empty_composition(keyval, modifiers, &state)
    })
}

/// Map a Unicode scalar offset into `text` to a UTF-16 code unit offset, the
/// unit IMKit, TSF and Android's InputConnection count in.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_utf16_offset_from_chars(text: *const c_char, char_offset: u32) -> u32 {
    guard("keytao_utf16_offset_from_chars", 0, || {
        let Ok(text) = c_string_arg(text, "text") else {
            return 0;
        };
        keytao_core::utf16_offset_from_chars(&text, char_offset as usize) as u32
    })
}

// ── Theme ─────────────────────────────────────────────────────────────────────

/// What the frontend's candidate panel can actually render. A soft keyboard bar
/// and an IMKit panel disagree on this, and the theme layer needs to know which
/// one it is building a model for.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_set_ui_capabilities(
    supports_custom_colors: bool,
    supports_vertical: bool,
    supports_hover: bool,
    supports_shadow: bool,
    supports_separator: bool,
    system_lookup_table_only: bool,
) {
    guard("keytao_set_ui_capabilities", (), || {
        let mut capabilities = lock_ignore_poison(&UI_CAPABILITIES);
        *capabilities = Some(keytao_theme::UiCapabilities {
            supports_custom_colors,
            supports_vertical,
            supports_hover,
            supports_shadow,
            supports_separator,
            system_lookup_table_only,
        });
    })
}

/// Configure optional default/user theme paths used by JSON state helpers.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_set_theme_paths(
    default_theme_path: *const c_char,
    user_theme_path: *const c_char,
) {
    guard("keytao_set_theme_paths", (), || {
        let default_path = optional_path_arg(default_theme_path);
        let user_path = optional_path_arg(user_theme_path);
        let mut sources = lock_ignore_poison(&THEME_SOURCES);
        let system_scheme = sources.as_ref().and_then(|sources| sources.system_scheme);
        install_theme_sources(&mut sources, default_path, user_path, system_scheme);
    })
}

/// Tell the JSON state helpers which color scheme the platform is showing, so
/// the IME process never probes the system itself. Pass null to go back to
/// keytao-theme's own detection.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_set_system_color_scheme(system_color_scheme: *const c_char) {
    guard("keytao_set_system_color_scheme", (), || {
        let system_scheme = optional_effective_color_scheme_arg(system_color_scheme);
        let mut sources = lock_ignore_poison(&THEME_SOURCES);
        let (default_path, user_path) = sources
            .as_ref()
            .map(|sources| {
                (
                    sources.default_theme_path.clone(),
                    sources.user_theme_path.clone(),
                )
            })
            .unwrap_or((None, None));
        install_theme_sources(&mut sources, default_path, user_path, system_scheme);
    })
}

/// Rebuild the resolver only when the sources really changed: an unconditional
/// rebuild would throw away the parsed theme on every call and put YAML parsing
/// back on the key path.
#[cfg(not(target_os = "android"))]
fn install_theme_sources(
    slot: &mut Option<ThemeSources>,
    default_theme_path: Option<PathBuf>,
    user_theme_path: Option<PathBuf>,
    system_scheme: Option<EffectiveColorScheme>,
) {
    let unchanged = slot.as_ref().is_some_and(|sources| {
        sources.default_theme_path == default_theme_path
            && sources.user_theme_path == user_theme_path
            && sources.system_scheme == system_scheme
    });
    if unchanged {
        return;
    }
    *slot = Some(ThemeSources::new(
        default_theme_path,
        user_theme_path,
        system_scheme,
    ));
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
    guard("keytao_resolve_theme_json", std::ptr::null_mut(), || {
        let default_path = optional_path_arg(default_theme_path);
        let user_path = optional_path_arg(user_theme_path);
        let theme = resolve_theme_from_paths(default_path.as_deref(), user_path.as_deref());
        theme_json_cstring(&theme)
    })
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
    guard(
        "keytao_resolve_theme_json_with_system_scheme",
        std::ptr::null_mut(),
        || {
            let default_path = optional_path_arg(default_theme_path);
            let user_path = optional_path_arg(user_theme_path);
            let Some(system_scheme) = optional_effective_color_scheme_arg(system_color_scheme)
            else {
                let theme = resolve_theme_from_paths(default_path.as_deref(), user_path.as_deref());
                return theme_json_cstring(&theme);
            };
            let theme = resolve_theme_from_paths_with_system_scheme(
                default_path.as_deref(),
                user_path.as_deref(),
                system_scheme,
            );
            theme_json_cstring(&theme)
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_default_keyboard_yaml() -> *mut c_char {
    guard("keytao_default_keyboard_yaml", std::ptr::null_mut(), || {
        to_cstring(keytao_theme::default_keyboard_yaml())
    })
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_resolve_keyboard_json(
    default_keyboard_path: *const c_char,
    user_keyboard_path: *const c_char,
) -> *mut c_char {
    guard("keytao_resolve_keyboard_json", std::ptr::null_mut(), || {
        let default_path = optional_path_arg(default_keyboard_path);
        let user_path = optional_path_arg(user_keyboard_path);
        let keyboard = keytao_theme::resolve_keyboard_from_paths(
            default_path.as_deref(),
            user_path.as_deref(),
        );
        match keytao_theme::resolved_keyboard_json(&keyboard) {
            Ok(json) => to_cstring(&json),
            Err(e) => {
                log_error(format_args!(
                    "keytao_resolve_keyboard_json: serialize failed: {e}"
                ));
                to_cstring("{}")
            }
        }
    })
}

#[cfg(not(target_os = "android"))]
fn theme_json_cstring(theme: &keytao_theme::ResolvedImeTheme) -> *mut c_char {
    match resolved_theme_json(theme) {
        Ok(json) => to_cstring(&json),
        Err(e) => {
            log_error(format_args!(
                "keytao_resolve_theme_json: serialize failed: {e}"
            ));
            to_cstring("{}")
        }
    }
}

// ── JSON session API ──────────────────────────────────────────────────────────

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_state_json(session: *mut c_void) -> *mut c_char {
    guard("keytao_session_state_json", std::ptr::null_mut(), || {
        let Some(handle) = session_handle(session) else {
            return std::ptr::null_mut();
        };
        to_cstring(&state_json(handle.session.state(), false))
    })
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_process_key_json(
    session: *mut c_void,
    keyval: u32,
    modifiers: u32,
) -> *mut c_char {
    guard(
        "keytao_session_process_key_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(result) = handle.session.process_key_result(keyval, modifiers) else {
                return std::ptr::null_mut();
            };
            to_cstring(&result_json(result))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_process_enter_json(session: *mut c_void) -> *mut c_char {
    guard(
        "keytao_session_process_enter_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(result) = handle.session.process_enter() else {
                return std::ptr::null_mut();
            };
            to_cstring(&result_json(result))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_select_candidate_json(
    session: *mut c_void,
    index: u32,
) -> *mut c_char {
    guard(
        "keytao_session_select_candidate_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.select_candidate_on_page(index as usize) else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_highlight_candidate_json(
    session: *mut c_void,
    index: u32,
) -> *mut c_char {
    guard(
        "keytao_session_highlight_candidate_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.highlight_candidate_on_page(index as usize) else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_delete_candidate_json(
    session: *mut c_void,
    index: u32,
) -> *mut c_char {
    guard(
        "keytao_session_delete_candidate_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some((state, deleted)) = handle
                .session
                .delete_candidate_on_page_result(index as usize)
            else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, deleted))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_select_candidate_global_json(
    session: *mut c_void,
    index: u32,
) -> *mut c_char {
    guard(
        "keytao_session_select_candidate_global_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.select_candidate_global(index as usize) else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_all_candidates_json(
    session: *mut c_void,
    limit: u32,
) -> *mut c_char {
    guard(
        "keytao_session_all_candidates_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(candidates) = handle.session.all_candidates_limited(limit as usize) else {
                return std::ptr::null_mut();
            };
            let json = serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".into());
            to_cstring(&json)
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_list_schemas_json(session: *mut c_void) -> *mut c_char {
    guard(
        "keytao_session_list_schemas_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(schemas) = handle.session.list_schemas() else {
                return std::ptr::null_mut();
            };
            match serde_json::to_string(&schemas) {
                Ok(json) => to_cstring(&json),
                Err(error) => {
                    log_error(format_args!(
                        "keytao_session_list_schemas_json: serialize failed: {error}"
                    ));
                    std::ptr::null_mut()
                }
            }
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_schema_switches_json(session: *mut c_void) -> *mut c_char {
    guard(
        "keytao_session_schema_switches_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(switches) = handle.session.schema_switches() else {
                return std::ptr::null_mut();
            };
            match serde_json::to_string(&switches) {
                Ok(json) => to_cstring(&json),
                Err(error) => {
                    log_error(format_args!(
                        "keytao_session_schema_switches_json: serialize failed: {error}"
                    ));
                    std::ptr::null_mut()
                }
            }
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_current_schema_json(session: *mut c_void) -> *mut c_char {
    guard(
        "keytao_session_current_schema_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(schema) = handle.session.current_schema() else {
                return std::ptr::null_mut();
            };
            match serde_json::to_string(&schema) {
                Ok(json) => to_cstring(&json),
                Err(error) => {
                    log_error(format_args!(
                        "keytao_session_current_schema_json: serialize failed: {error}"
                    ));
                    std::ptr::null_mut()
                }
            }
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_select_schema_json(
    session: *mut c_void,
    schema_id: *const c_char,
) -> *mut c_char {
    guard(
        "keytao_session_select_schema_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Ok(schema_id) = c_string_arg(schema_id, "schema_id") else {
                return std::ptr::null_mut();
            };
            match handle.session.select_schema(&schema_id) {
                Ok(state) => to_cstring(&state_json(state, false)),
                Err(error) => {
                    log_error(format_args!("keytao_session_select_schema_json: {error}"));
                    std::ptr::null_mut()
                }
            }
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_get_option(
    session: *mut c_void,
    option_name: *const c_char,
) -> bool {
    guard("keytao_session_get_option", false, || {
        let Some(handle) = session_handle(session) else {
            return false;
        };
        let Ok(option_name) = c_string_arg(option_name, "option_name") else {
            return false;
        };
        handle.session.get_option(&option_name)
    })
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_set_option_json(
    session: *mut c_void,
    option_name: *const c_char,
    enabled: bool,
) -> *mut c_char {
    guard(
        "keytao_session_set_option_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Ok(option_name) = c_string_arg(option_name, "option_name") else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.set_option(&option_name, enabled) else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, false))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_change_page_json(
    session: *mut c_void,
    backward: bool,
) -> *mut c_char {
    guard(
        "keytao_session_change_page_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.change_page(backward) else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_reset_json(session: *mut c_void) -> *mut c_char {
    guard("keytao_session_reset_json", std::ptr::null_mut(), || {
        let Some(handle) = session_handle(session) else {
            return std::ptr::null_mut();
        };
        let Some(state) = handle.session.reset() else {
            return std::ptr::null_mut();
        };
        to_cstring(&state_json(state, true))
    })
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_commit_composition_json(session: *mut c_void) -> *mut c_char {
    guard(
        "keytao_session_commit_composition_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.commit_composition() else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_clear_composition_json(session: *mut c_void) -> *mut c_char {
    guard(
        "keytao_session_clear_composition_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.clear_composition() else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_set_input_policy_json(
    session: *mut c_void,
    composing: bool,
    learning: bool,
) -> *mut c_char {
    guard(
        "keytao_session_set_input_policy_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.set_input_policy(InputContextPolicy {
                composing,
                learning,
            }) else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_session_set_ascii_mode_json(
    session: *mut c_void,
    enabled: bool,
) -> *mut c_char {
    guard(
        "keytao_session_set_ascii_mode_json",
        std::ptr::null_mut(),
        || {
            let Some(handle) = session_handle(session) else {
                return std::ptr::null_mut();
            };
            let Some(state) = handle.session.set_ascii_mode(enabled) else {
                return std::ptr::null_mut();
            };
            to_cstring(&state_json(state, true))
        },
    )
}

/// Free a UTF-8 string returned by keytao-core-ffi.
#[no_mangle]
pub extern "C" fn keytao_free_string(ptr: *mut c_char) {
    guard("keytao_free_string", (), || unsafe { free_cstring(ptr) })
}

// ── Singleton session API ─────────────────────────────────────────────────────

/// Process a key event. Returns heap-allocated KeytaoState; caller must free
/// with keytao_free_state(). Returns null if the runtime is not initialized.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_process_key(keyval: u32, modifiers: u32) -> *mut KeytaoState {
    guard("keytao_process_key", std::ptr::null_mut(), || {
        with_singleton_session(std::ptr::null_mut(), |session| {
            let Some(result) = session.process_key_result(keyval, modifiers) else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(result_to_c(result)))
        })
    })
}

/// Select a candidate by 0-based index. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_select_candidate(index: u32) -> *mut KeytaoState {
    guard("keytao_select_candidate", std::ptr::null_mut(), || {
        with_singleton_session(std::ptr::null_mut(), |session| {
            let Some(state) = session.select_candidate_on_page(index as usize) else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        })
    })
}

/// Flip to the next/previous candidate page. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_change_page(backward: bool) -> *mut KeytaoState {
    guard("keytao_change_page", std::ptr::null_mut(), || {
        with_singleton_session(std::ptr::null_mut(), |session| {
            let Some(state) = session.change_page(backward) else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        })
    })
}

/// Clear current composition. Returns new state; caller must free.
#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn keytao_reset() -> *mut KeytaoState {
    guard("keytao_reset", std::ptr::null_mut(), || {
        with_singleton_session(std::ptr::null_mut(), |session| {
            let Some(state) = session.reset() else {
                return std::ptr::null_mut();
            };
            Box::into_raw(Box::new(state_to_c(state, true)))
        })
    })
}

/// Free a KeytaoState returned by any keytao_* function.
#[no_mangle]
pub extern "C" fn keytao_free_state(ptr: *mut KeytaoState) {
    guard("keytao_free_state", (), || {
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
    })
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// The handle a session export was called with, or `None` when it was retired.
///
/// The returned guard keeps the epoch pinned for as long as the caller holds
/// it, which is what makes the check more than advisory: keytao_init() cannot
/// take librime down between this check and the keytao-core call that follows.
#[cfg(not(target_os = "android"))]
fn session_handle<'a>(session: *mut c_void) -> Option<ActiveSession<'a>> {
    if session.is_null() {
        return None;
    }
    let epoch = read_ignore_poison(&SESSION_EPOCH);
    let handle = unsafe { &*(session as *mut SessionHandle) };
    // A handle from before a directory switch was created for a librime that
    // has since been finalized; answering with it would compose against the
    // wrong deployment. keytao_destroy_session() still frees it.
    (handle.epoch == *epoch).then_some(ActiveSession {
        handle,
        _epoch: epoch,
    })
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
        sel_start: state.sel_start as u32,
        sel_end: state.sel_end as u32,
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
    let ui_capabilities = current_ui_capabilities();
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
        "selStart": state.sel_start,
        "selEnd": state.sel_end,
        "candidates": state.candidates,
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
    let mut sources = lock_ignore_poison(&THEME_SOURCES);
    // Without keytao_set_theme_paths() the bundled theme is the whole story, but
    // it still goes through a resolver so the key path never re-parses the YAML.
    sources
        .get_or_insert_with(|| ThemeSources::new(None, None, None))
        .resolver
        .current()
}

#[cfg(not(target_os = "android"))]
fn c_string_arg(ptr: *const c_char, name: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("{name} is null"));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_string)
        .map_err(|e| format!("{name} is not UTF-8: {e}"))
}

#[cfg(not(target_os = "android"))]
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

#[cfg(not(target_os = "android"))]
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

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    #[test]
    fn guard_turns_a_panic_into_the_default_value() {
        let value = guard("test", -1, || panic!("boom"));
        assert_eq!(value, -1);
        assert_eq!(guard("test", -1, || 7), 7);
        // A payload that is neither &str nor String must not send the reporting
        // path looking for one it cannot produce.
        assert_eq!(guard("test", 5, || std::panic::panic_any(42u32)), 5);
    }

    struct PanicsWhenFormatted;

    impl std::fmt::Display for PanicsWhenFormatted {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("formatting exploded");
        }
    }

    #[test]
    fn reporting_an_error_cannot_panic_past_the_abi_boundary() {
        // Whatever makes the log line fail — a closed stderr, a Display that
        // panics — must stop here. Unwinding out of this call would cross an
        // `extern "C"` frame and abort the process while the user is typing.
        log_error(format_args!("test: {}", PanicsWhenFormatted));
        // Same failure on the path guard itself uses after catching a panic.
        let reported = std::panic::catch_unwind(|| {
            guard("test", 0, || panic!("boom"));
            log_error(format_args!("test: {}", PanicsWhenFormatted));
            1
        });
        assert_eq!(reported.ok(), Some(1));
    }

    #[test]
    fn text_to_keysym_never_lands_in_the_function_key_block() {
        let full_width_paren = CString::new("（").unwrap();
        assert_eq!(
            keytao_text_to_keysym(full_width_paren.as_ptr()),
            key_policy::XK_UNICODE_BASE | 0xff08
        );
        let letter = CString::new("a").unwrap();
        assert_eq!(keytao_text_to_keysym(letter.as_ptr()), 0x61);
        let pair = CString::new("ab").unwrap();
        assert_eq!(keytao_text_to_keysym(pair.as_ptr()), 0);
        assert_eq!(keytao_text_to_keysym(std::ptr::null()), 0);
    }

    #[test]
    fn utf16_offsets_count_surrogate_pairs() {
        let text = CString::new("a𝄞b").unwrap();
        assert_eq!(keytao_utf16_offset_from_chars(text.as_ptr(), 0), 0);
        assert_eq!(keytao_utf16_offset_from_chars(text.as_ptr(), 1), 1);
        assert_eq!(keytao_utf16_offset_from_chars(text.as_ptr(), 2), 3);
        assert_eq!(keytao_utf16_offset_from_chars(text.as_ptr(), 3), 4);
    }

    #[test]
    fn key_policy_helpers_are_reachable_through_the_c_abi() {
        assert!(keytao_key_policy_is_enter(key_policy::XK_RETURN));
        assert!(keytao_key_policy_is_enter(key_policy::XK_KP_ENTER));
        assert!(!keytao_key_policy_is_enter(0x61));
        // A null session has no state to judge, so nothing is bypassed.
        assert!(!keytao_key_policy_should_bypass(
            std::ptr::null_mut(),
            key_policy::XK_LEFT,
            0
        ));
    }

    #[test]
    fn ui_capabilities_default_to_a_soft_keyboard_bar_until_declared() {
        // The JSON API was written for the iOS candidate bar; a frontend with a
        // real panel has to say so before it gets vertical layouts.
        assert!(!current_ui_capabilities().supports_vertical);
        keytao_set_ui_capabilities(true, true, true, true, true, false);
        assert!(current_ui_capabilities().supports_vertical);
        *lock_ignore_poison(&UI_CAPABILITIES) = None;
    }

    #[test]
    fn stamp_accessors_report_what_keytao_core_writes() {
        let dir = std::env::temp_dir().join(format!("keytao-ffi-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_arg = CString::new(dir.to_string_lossy().as_ref()).unwrap();

        // No deployment has asked for a reload yet.
        assert!(keytao_reload_stamp_signature_at(dir_arg.as_ptr()).is_null());

        let stamp = keytao_core::ReloadStamp::write(&dir).unwrap();
        let path = keytao_reload_stamp_path_at(dir_arg.as_ptr());
        assert!(!path.is_null());
        assert_eq!(
            unsafe { CStr::from_ptr(path) }.to_str().unwrap(),
            stamp.to_string_lossy()
        );
        keytao_free_string(path);

        let signature = keytao_reload_stamp_signature_at(dir_arg.as_ptr());
        assert!(!signature.is_null());
        assert_eq!(
            unsafe { CStr::from_ptr(signature) }.to_str().unwrap(),
            keytao_core::ReloadStamp::current_signature(&dir).unwrap()
        );
        keytao_free_string(signature);

        assert!(keytao_reload_stamp_signature_at(std::ptr::null()).is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Poison a mutex the way a panic inside a guarded export would.
    fn poison<T: Send + 'static>(mutex: &'static Mutex<T>) {
        let panicked = std::thread::spawn(move || {
            let _held = lock_ignore_poison(mutex);
            panic!("boom");
        })
        .join();
        assert!(panicked.is_err());
        assert!(mutex.is_poisoned());
    }

    #[test]
    fn a_poisoned_global_does_not_disable_the_library() {
        // guard() already contained the panic and told the caller it failed.
        // Refusing every later call on top of that would leave the user without
        // an input method until the process restarts.
        lock_ignore_poison(&GLOBAL).initialized = true;
        poison(&GLOBAL);

        assert!(keytao_is_initialized());
        assert!(keytao_create_session().is_null());
        assert!(keytao_reset().is_null());
        assert!(!keytao_reload());

        lock_ignore_poison(&GLOBAL).initialized = false;
        GLOBAL.clear_poison();
    }

    #[test]
    fn a_poisoned_theme_lock_does_not_freeze_the_theme() {
        poison(&THEME_SOURCES);

        // With the poison honored, the resolver would never be installed again
        // and every later key would render against a default theme.
        keytao_set_theme_paths(std::ptr::null(), std::ptr::null());
        assert!(lock_ignore_poison(&THEME_SOURCES).is_some());
        let _ = current_theme();

        THEME_SOURCES.clear_poison();
    }

    #[test]
    fn a_rejected_init_does_not_retire_live_sessions() {
        // Sessions are retired by bumping the epoch, which is irreversible: a
        // call that never reached librime must not cost the frontend the
        // composition it is holding.
        let before = *read_ignore_poison(&SESSION_EPOCH);
        assert!(!keytao_init(std::ptr::null(), std::ptr::null()));

        let not_utf8 = [0xffu8, 0x00];
        let not_utf8 = not_utf8.as_ptr() as *const c_char;
        let ok = CString::new("/tmp/keytao-ffi-init").unwrap();
        assert!(!keytao_init(not_utf8, ok.as_ptr()));
        assert!(!keytao_init(ok.as_ptr(), not_utf8));

        assert_eq!(*read_ignore_poison(&SESSION_EPOCH), before);
    }

    #[test]
    fn init_never_deadlocks_against_the_session_exports() {
        // keytao_init() retires handles under a write guard that every session
        // export holds for reading, and both go on to take `GLOBAL` and then
        // keytao-core's locks. Anything acquiring them in the other order hangs
        // the input method for good, so the chain is driven from several
        // threads at once. The directory has no scheme in it, so init fails
        // before librime is touched but after the whole lock chain was walked.
        let dir = std::env::temp_dir().join(format!("keytao-ffi-init-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let user_dir = dir.to_string_lossy().into_owned();

        std::thread::scope(|scope| {
            for _ in 0..4 {
                let user_dir = user_dir.clone();
                scope.spawn(move || {
                    let user = CString::new(user_dir).unwrap();
                    let shared = CString::new("").unwrap();
                    for _ in 0..50 {
                        assert!(!keytao_init(user.as_ptr(), shared.as_ptr()));
                    }
                });
            }
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..50 {
                        assert!(keytao_create_session().is_null());
                        assert!(keytao_reset().is_null());
                        assert!(keytao_session_state(std::ptr::null_mut()).is_null());
                        keytao_destroy_session(std::ptr::null_mut());
                    }
                });
            }
            // The reload path pins the epoch too, so it has to take it in the
            // same order as everything else. No test ever installs a runtime,
            // so both calls bail out before librime — after walking the chain.
            for _ in 0..2 {
                scope.spawn(|| {
                    for _ in 0..50 {
                        assert!(!keytao_reload());
                        assert!(!keytao_reload_if_stamp_changed());
                    }
                });
            }
        });

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_lost_to_a_directory_switch_is_asked_for_once_more() {
        // keytao-core reports this verbatim when librime came up for other
        // directories between its own initialization and the session it was
        // about to create. Failing the call on it would cost the frontend a
        // session over a race that a second attempt settles.
        let raced = || Err::<u32, String>("librime is running for other data directories".into());
        assert!(librime_switched_directories(&raced().unwrap_err()));

        let attempts = std::cell::Cell::new(0);
        let result = retry_once_on_directory_switch(|| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                raced()
            } else {
                Ok(7)
            }
        });
        assert_eq!(result, Ok(7));
        assert_eq!(attempts.get(), 2);

        // A race that is still lost gives up: retrying further would be a loop
        // against whoever keeps switching librime.
        let attempts = std::cell::Cell::new(0);
        let result = retry_once_on_directory_switch(|| {
            attempts.set(attempts.get() + 1);
            raced()
        });
        assert!(result.is_err());
        assert_eq!(attempts.get(), 2);

        // Every other failure is real, and a retry would only repeat the
        // deployment checks keytao-core just failed.
        let attempts = std::cell::Cell::new(0);
        let result = retry_once_on_directory_switch(|| {
            attempts.set(attempts.get() + 1);
            Err::<u32, String>("no KeyTao scheme is installed".into())
        });
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn every_capability_maps_to_its_own_bit() {
        // A frontend disables a control on a single bit, so a bit that stands
        // for the wrong entry point makes it hide paging and offer selection
        // librime cannot do.
        let mut seen = 0;
        for (bit, capabilities) in [
            (
                KEYTAO_CAP_CANDIDATE_SELECTION,
                EngineCapabilities {
                    candidate_selection: true,
                    ..EngineCapabilities::none()
                },
            ),
            (
                KEYTAO_CAP_GLOBAL_CANDIDATE_SELECTION,
                EngineCapabilities {
                    global_candidate_selection: true,
                    ..EngineCapabilities::none()
                },
            ),
            (
                KEYTAO_CAP_CANDIDATE_HIGHLIGHT,
                EngineCapabilities {
                    candidate_highlight: true,
                    ..EngineCapabilities::none()
                },
            ),
            (
                KEYTAO_CAP_CANDIDATE_DELETION,
                EngineCapabilities {
                    candidate_deletion: true,
                    ..EngineCapabilities::none()
                },
            ),
            (
                KEYTAO_CAP_NATIVE_PAGING,
                EngineCapabilities {
                    native_paging: true,
                    ..EngineCapabilities::none()
                },
            ),
            (
                KEYTAO_CAP_COMMIT_COMPOSITION,
                EngineCapabilities {
                    commit_composition: true,
                    ..EngineCapabilities::none()
                },
            ),
            (
                KEYTAO_CAP_CLEAR_COMPOSITION,
                EngineCapabilities {
                    clear_composition: true,
                    ..EngineCapabilities::none()
                },
            ),
        ] {
            assert_eq!(capability_bits(capabilities), bit);
            assert_eq!(seen & bit, 0, "two capabilities share a bit");
            seen |= bit;
        }
        assert_eq!(capability_bits(EngineCapabilities::none()), 0);
    }

    #[test]
    fn capabilities_reach_the_c_abi_as_bits_and_json() {
        let expected = keytao_core::engine_capabilities();
        let bits = keytao_engine_capabilities();
        assert_eq!(bits, capability_bits(expected));
        // The librime this crate is linked against has the official entry
        // points; reporting none of them would send every frontend back to
        // synthesized select keys and `-`/`=`.
        assert_ne!(bits, 0, "the host librime reports no capability at all");

        // A handle that cannot reach librime cannot select or page either.
        assert_eq!(keytao_session_capabilities(std::ptr::null_mut()), 0);

        let json = keytao_engine_capabilities_json();
        assert!(!json.is_null());
        let parsed: serde_json::Value =
            serde_json::from_str(unsafe { CStr::from_ptr(json) }.to_str().unwrap()).unwrap();
        keytao_free_string(json);
        assert_eq!(parsed["nativePaging"], expected.native_paging);
        assert_eq!(parsed["candidateSelection"], expected.candidate_selection);
        assert_eq!(parsed["candidateHighlight"], expected.candidate_highlight);
    }

    #[test]
    fn a_deployment_that_lands_during_a_reload_stays_pending() {
        // The signal is installed the way keytao_init() installs it and then
        // driven through the peek/commit pair keytao_reload() uses. Committing
        // whatever is on disk *after* the reload instead would mark the second
        // deployment as loaded and leave the frontend on the old dictionaries.
        let dir = std::env::temp_dir().join(format!("keytao-ffi-signal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        *lock_ignore_poison(&RELOAD_SIGNAL) = Some(ReloadSignal::new(&dir));

        assert!(stamp_changed_peek().is_none(), "nothing was deployed yet");

        ReloadStamp::write(&dir).unwrap();
        let (path, _) = stamp_changed_peek().expect("a written stamp is a reload request");
        assert_eq!(path, ReloadStamp::path(&dir));

        // reload_now()'s order: snapshot, reload, commit the snapshot. A second
        // deployment finishes while that reload is still running.
        let (path, loading) = begin_reload().unwrap();
        std::fs::write(&path, "deployed again\n").unwrap();
        mark_deployment_loaded(&path, loading);

        let (_, pending) =
            stamp_changed_peek().expect("the deployment that landed mid-reload is still pending");
        let (path, loading) = begin_reload().unwrap();
        assert_eq!(loading.as_deref(), Some(pending.as_str()));
        mark_deployment_loaded(&path, loading);
        assert!(
            stamp_changed_peek().is_none(),
            "a reload that saw the latest stamp settles"
        );

        // The consuming accessor answers once per request.
        std::fs::write(&path, "deployed a third time\n").unwrap();
        assert!(keytao_reload_stamp_changed());
        assert!(!keytao_reload_stamp_changed());

        *lock_ignore_poison(&RELOAD_SIGNAL) = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_exports_refuse_a_null_handle() {
        assert!(keytao_session_state(std::ptr::null_mut()).is_null());
        assert!(keytao_session_process_key(std::ptr::null_mut(), 0x61, 0).is_null());
        assert!(keytao_session_process_enter(std::ptr::null_mut()).is_null());
        assert!(keytao_session_select_candidate(std::ptr::null_mut(), 0).is_null());
        assert!(keytao_session_change_page(std::ptr::null_mut(), false).is_null());
        assert!(keytao_session_commit_composition(std::ptr::null_mut()).is_null());
        assert!(keytao_session_clear_composition(std::ptr::null_mut()).is_null());
        assert!(keytao_session_set_input_policy(std::ptr::null_mut(), false, false).is_null());
        assert!(!keytao_session_input_policy_composing(std::ptr::null_mut()));
        assert!(!keytao_session_get_ascii_mode(std::ptr::null_mut()));
        // Destroying nothing is not an error either.
        keytao_destroy_session(std::ptr::null_mut());
    }
}
