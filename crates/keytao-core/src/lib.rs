//! Pure librime engine wrapper — no D-Bus, no platform I/O.
//! Every consumer (the Flutter app via keytao-app-bridge, the C FFI and the
//! platform IMEs) links against this crate as its rime back-end.

mod build_prune;
pub use build_prune::{is_rime_compiled_output, prune_rime_build_artifacts};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub mod deploy_child;
mod deployment_log;
mod deployment_state;
pub use deployment_state::{
    current_deploy_progress, deployment_in_progress, deployment_progress_message,
    mark_deployment_failed, scheme_operation_progress_message, track_usable_input_session,
    DeployProgress, DeployStep, DeploymentState, SchemeConversionProgress,
};
#[cfg(test)]
mod deployment_state_tests;
pub mod english_mode;
mod file_lock;
pub use file_lock::lock_file;
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub mod isolated_deploy;
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub use isolated_deploy::{package_build_ready, switch_ready_package};
pub mod key_batch;
pub mod package_store;
mod preview;
pub use preview::{CandidatePreview, PreviewCandidate, PreviewRequest};
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "ios"
))]
mod desktop_inputs;
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
mod global_input;
#[cfg(test)]
mod grammar_tests;
mod reload_stamp_cache;
mod runtime_dirs;
pub use runtime_dirs::RuntimeDirs;
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub mod switcher_model;
mod schema_inputs;
pub use schema_inputs::{record_schema_input_fingerprints, schema_input_fingerprints};
mod schema_shortcuts;
pub use schema_shortcuts::SchemaShortcut;
mod rime_lua;
#[cfg(all(
    test,
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
mod runtime_access_tests;
pub mod runtime_log;
pub use reload_stamp_cache::ReloadStampCache;
mod schema_switch_request;
pub use schema_switch_request::{
    read_schema_switch_request, request_schema, schema_switch_applied_generation,
    write_file_atomic, SchemaSwitchRequest, SCHEMA_SWITCH_APPLIED_FILE, SCHEMA_SWITCH_REQUEST_FILE,
};
mod user_data_sync;
pub use user_data_sync::{
    cancel_user_data_sync, read_user_data_sync_result, request_user_data_sync, UserDataSyncResult,
    USER_DATA_SYNC_REQUEST_FILE, USER_DATA_SYNC_RESULT_FILE,
};

use runtime_log::Level;
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        TryLockError, Weak,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub enum DeployProcess {
    InProcess,
    DeployChild,
    AndroidDeployer,
    PublicDeploy,
}

impl DeployProcess {
    fn needs_user_dictionary_cache(self) -> bool {
        matches!(self, Self::InProcess)
    }
}

/// A poisoned lock only means some other caller panicked while holding it; the
/// input method must keep working, so the data is recovered instead of
/// propagating the panic (a panic across an FFI boundary aborts the process).
fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn try_read_ignore_poison<T>(lock: &RwLock<T>) -> Option<RwLockReadGuard<'_, T>> {
    match lock.try_read() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

fn try_lock_ignore_poison<T>(lock: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match lock.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

fn write_ignore_poison<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

struct EngineBuildLock {
    busy: Mutex<bool>,
    foreground_waiters: std::sync::atomic::AtomicUsize,
    ready: Condvar,
}

impl EngineBuildLock {
    const fn new() -> Self {
        Self {
            busy: Mutex::new(false),
            foreground_waiters: std::sync::atomic::AtomicUsize::new(0),
            ready: Condvar::new(),
        }
    }

    fn acquire(&self) -> Option<EngineBuildGuard<'_>> {
        self.acquire_with_priority(true)
    }

    fn acquire_background(&self) -> Option<EngineBuildGuard<'_>> {
        self.acquire_with_priority(false)
    }

    fn try_acquire_background(&self) -> Option<EngineBuildGuard<'_>> {
        let mut busy = try_lock_ignore_poison(&self.busy)?;
        if *busy || self.foreground_waiters.load(Ordering::Relaxed) > 0 {
            return None;
        }
        *busy = true;
        Some(EngineBuildGuard(self))
    }

    fn acquire_with_priority(&self, foreground: bool) -> Option<EngineBuildGuard<'_>> {
        let busy = lock_ignore_poison(&self.busy);
        if foreground {
            self.foreground_waiters.fetch_add(1, Ordering::Relaxed);
        }
        let unavailable = |busy: &mut bool| {
            *busy || (!foreground && self.foreground_waiters.load(Ordering::Relaxed) > 0)
        };
        let (mut busy, _) = self
            .ready
            .wait_timeout_while(busy, Duration::from_secs(20), unavailable)
            .unwrap_or_else(PoisonError::into_inner);
        if foreground {
            self.foreground_waiters.fetch_sub(1, Ordering::Relaxed);
        }
        if unavailable(&mut busy) {
            return None;
        }
        *busy = true;
        Some(EngineBuildGuard(self))
    }
}

struct EngineBuildGuard<'a>(&'a EngineBuildLock);

impl Drop for EngineBuildGuard<'_> {
    fn drop(&mut self) {
        *lock_ignore_poison(&self.0.busy) = false;
        self.0.ready.notify_all();
    }
}

// ── Public types ──────────────────────────────────────────────────────────────

pub const WINDOWS_IME_ENGINE_INIT_MUTEX_NAME: &str = "Local\\KeyTao.WindowsIme.EngineInit";

#[cfg(target_os = "linux")]
pub const LINUX_UPGRADE_DEPLOY_SUBCOMMAND: &str = "--deploy-librime-upgrade";

/// Serialize Linux App deployments and upgrade children across processes.
#[cfg(target_os = "linux")]
pub fn linux_deploy_lock(
    user_dir: &Path,
    timeout: Duration,
) -> Result<Option<std::fs::File>, String> {
    use std::fs::{OpenOptions, TryLockError};
    use std::os::unix::fs::OpenOptionsExt;

    // Linux UAPI O_NOFOLLOW, including the ARM, PowerPC and m68k overrides.
    #[cfg(any(
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "m68k",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    ))]
    const O_NOFOLLOW: i32 = 0x8000;
    #[cfg(not(any(
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "m68k",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    )))]
    const O_NOFOLLOW: i32 = 0x20000;

    std::fs::create_dir_all(user_dir)
        .map_err(|error| format!("cannot create {}: {error}", user_dir.display()))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(user_dir.join("keytao-librime-upgrade.lock"))
        .map_err(|error| format!("cannot open librime deployment lock: {error}"))?;
    let started = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(Some(lock)),
            Err(TryLockError::WouldBlock) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Ok(None);
                }
                std::thread::sleep(remaining.min(Duration::from_millis(100)));
            }
            Err(TryLockError::Error(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
                if started.elapsed() >= timeout {
                    return Ok(None);
                }
            }
            Err(TryLockError::Error(error)) => {
                return Err(format!("cannot lock librime deployment: {error}"));
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImeState {
    pub preedit: String,
    /// librime's full commit preview, including converted segments; empty
    /// when there is no candidate.
    #[serde(default)]
    pub commit_preview: String,
    /// Caret position inside `preedit`, counted in Unicode scalars.
    ///
    /// librime reports UTF-8 byte offsets; the conversion happens here so that
    /// every frontend starts from the same unit and only has to map it to its
    /// own (UTF-16 on macOS/Windows/Android, scalars on IBus).
    pub cursor: usize,
    /// Start of the selected (already converted) range inside `preedit`, in
    /// Unicode scalars. Equal to `sel_end` when nothing is selected.
    pub sel_start: usize,
    /// End of the selected range inside `preedit`, in Unicode scalars.
    pub sel_end: usize,
    pub candidates: Vec<Candidate>,
    pub highlighted_candidate_index: usize,
    pub page_size: usize,
    pub page: usize,
    pub is_last_page: bool,
    pub committed: Option<String>,
    pub select_keys: Option<String>,
    pub ascii_mode: bool,
    /// Label of a Rime switch changed by this key, absent in ordinary snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_hint: Option<String>,
    #[serde(default)]
    pub chord_composer: bool,
    /// Active Rime option/schema switch bindings and switcher hotkeys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_shortcuts: Vec<SchemaShortcut>,
    /// Configured Control and Alt keysyms that may switch mode on a solo tap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifier_taps: Vec<u32>,
    #[serde(default)]
    pub schema_id: String,
    pub schema_name: String,
}

/// What a frontend allows the engine to do for the current input context.
///
/// Password fields and PIN entries must not produce a composition or teach the
/// user dictionary. Private contexts may keep composition while frontends turn
/// off their own learning-related stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputContextPolicy {
    /// Whether keys may reach librime at all. With `false` every key is
    /// reported as not accepted and no preedit or candidate is produced.
    pub composing: bool,
    /// Whether the context may contribute to user learning. Core cannot enforce
    /// this while `composing` is true because librime has no per-session
    /// no-memorize switch. The shipped KeyTao schemas set
    /// `enable_user_dict: false`; frontends also use this flag for their own
    /// clipboard and commit-history stores. A third-party schema with a user
    /// dictionary can still learn while composition remains enabled.
    pub learning: bool,
}

impl Default for InputContextPolicy {
    fn default() -> Self {
        Self {
            composing: true,
            learning: true,
        }
    }
}

impl InputContextPolicy {
    /// Policy for password, PIN and other sensitive contexts.
    pub fn sensitive() -> Self {
        Self {
            composing: false,
            learning: false,
        }
    }

    /// Policy for contexts that forbid personalization but still expect normal
    /// input, such as incognito and no-suggestion fields.
    pub fn private() -> Self {
        Self {
            composing: true,
            learning: false,
        }
    }
}

/// Recording a sensitive context must never wait for deployment or be lost
/// when an engine is temporarily busy. The clear bit survives later policy
/// changes until a session can safely discard its old composition.
struct SessionInputPolicy(AtomicU8);

impl SessionInputPolicy {
    const CLEAR: u8 = 4;

    fn new(policy: InputContextPolicy) -> Self {
        Self(AtomicU8::new(Self::encode(policy)))
    }

    fn encode(policy: InputContextPolicy) -> u8 {
        u8::from(policy.composing) | (u8::from(policy.learning) << 1)
    }

    fn decode(bits: u8) -> InputContextPolicy {
        InputContextPolicy {
            composing: bits & 1 != 0,
            learning: bits & 2 != 0,
        }
    }

    fn current(&self) -> InputContextPolicy {
        Self::decode(self.0.load(Ordering::Acquire))
    }

    fn set(&self, policy: InputContextPolicy) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |previous| {
                let clear = previous & Self::CLEAR != 0 || (previous & 1 != 0 && !policy.composing);
                Some(Self::encode(policy) | if clear { Self::CLEAR } else { 0 })
            });
    }

    fn take_for_engine(&self) -> (InputContextPolicy, bool) {
        let bits = self.0.fetch_and(!Self::CLEAR, Ordering::AcqRel);
        (Self::decode(bits), bits & Self::CLEAR != 0)
    }
}

/// What the librime the process is linked against can do through its own
/// entry points, as opposed to through synthesized key strokes.
///
/// A user-supplied system librime can be anything (paging and highlighting
/// arrived in 1.9), so a frontend must be able to ask instead of assume: a capability that is missing degrades to a
/// fallback that depends on the schema (a select key, `-`/`=`, `Escape`) and
/// therefore silently misbehaves on schemas that bind those keys differently.
/// Frontends disable the affected UI instead of offering a control that types
/// characters into the composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineCapabilities {
    /// `RimeSelectCandidateOnCurrentPage`. Without it, clicking a candidate
    /// sends the matching `menu/select_keys` character.
    pub candidate_selection: bool,
    /// `RimeSelectCandidate` (selection by index in the whole list).
    pub global_candidate_selection: bool,
    /// `RimeHighlightCandidateOnCurrentPage`. Without it, moving the highlight
    /// without committing is a no-op.
    pub candidate_highlight: bool,
    /// `RimeDeleteCandidateOnCurrentPage`. Without it, forgetting a learned
    /// phrase from the candidate menu is a no-op.
    pub candidate_deletion: bool,
    /// `RimeChangePage`. Without it, paging replays the `-`/`=` bindings of the
    /// default `key_binder` preset, which many schemas do not import.
    pub native_paging: bool,
    /// `RimeCommitComposition`. Without it, committing sends `Return`.
    pub commit_composition: bool,
    /// `RimeClearComposition`. Without it, discarding sends `Escape`.
    pub clear_composition: bool,
}

impl EngineCapabilities {
    /// Everything degraded — what a frontend sees when librime is not linked
    /// in at all.
    pub const fn none() -> Self {
        Self {
            candidate_selection: false,
            global_candidate_selection: false,
            candidate_highlight: false,
            candidate_deletion: false,
            native_paging: false,
            commit_composition: false,
            clear_composition: false,
        }
    }

    /// Whether a frontend may offer page up/down controls.
    pub const fn supports_native_paging(&self) -> bool {
        self.native_paging
    }

    /// Whether a frontend may offer click-to-select on the candidate list.
    pub const fn supports_candidate_selection(&self) -> bool {
        self.candidate_selection
    }

    /// Whether a frontend may offer hover/arrow highlighting.
    pub const fn supports_candidate_highlight(&self) -> bool {
        self.candidate_highlight
    }

    /// Whether a frontend may offer "forget this phrase".
    pub const fn supports_candidate_deletion(&self) -> bool {
        self.candidate_deletion
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaSwitch {
    pub name: Option<String>,
    pub options: Vec<String>,
    pub states: Vec<String>,
    pub reset: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaSwitchSetting {
    pub schema_id: String,
    pub name: String,
    pub states: Vec<String>,
    pub value: usize,
    pub options: Vec<String>,
}

fn visible_switch_settings(
    schema_id: &str,
    switches: Vec<SchemaSwitch>,
    mut saved_option: impl FnMut(&str, bool) -> Option<bool>,
) -> Vec<SchemaSwitchSetting> {
    switches
        .into_iter()
        .filter_map(|switch| {
            let count = if switch.options.is_empty() {
                2
            } else {
                switch.options.len()
            };
            if switch.states.len() != count || count < 2 {
                return None;
            }
            let reset = switch
                .reset
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value < count)
                .unwrap_or(0);
            let (name, value) = if switch.options.is_empty() {
                let name = switch.name?;
                let value = saved_option(&name, switch.reset.is_some())
                    .map(usize::from)
                    .unwrap_or(reset);
                (name, value)
            } else {
                let value = switch
                    .options
                    .iter()
                    .position(|name| saved_option(name, switch.reset.is_some()) == Some(true))
                    .unwrap_or(reset);
                (switch.options[0].clone(), value)
            };
            Some(SchemaSwitchSetting {
                schema_id: schema_id.into(),
                name,
                states: switch.states,
                value,
                options: switch.options,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyProcessResult {
    pub state: ImeState,
    pub accepted: bool,
}

impl ImeState {
    /// Display mode only. Dictionary English still has `ascii_mode == false`
    /// because its letters must reach librime and produce candidates.
    pub fn is_english_mode(&self) -> bool {
        self.ascii_mode || english_mode::is_english_schema(&self.schema_id, &self.schema_name)
    }

    pub fn is_package_switcher(&self) -> bool {
        self.schema_id == switcher_model::SCHEMA_ID
    }

    pub fn empty() -> Self {
        Self {
            preedit: String::new(),
            commit_preview: String::new(),
            cursor: 0,
            sel_start: 0,
            sel_end: 0,
            candidates: vec![],
            highlighted_candidate_index: 0,
            page_size: 0,
            page: 0,
            is_last_page: true,
            committed: None,
            select_keys: None,
            ascii_mode: false,
            switch_hint: None,
            chord_composer: false,
            schema_shortcuts: Vec::new(),
            modifier_taps: Vec::new(),
            schema_id: String::new(),
            schema_name: String::new(),
        }
    }
}

/// Convert a UTF-8 byte offset into `text` (what librime reports) to a Unicode
/// scalar offset (what [`ImeState`] exposes). An offset that lands inside a
/// character rounds up to the next boundary; one past the end clamps to the
/// end.
pub fn char_offset_from_utf8(text: &str, byte_offset: usize) -> usize {
    if byte_offset >= text.len() {
        return text.chars().count();
    }
    text.char_indices()
        .position(|(index, _)| index >= byte_offset)
        .unwrap_or_else(|| text.chars().count())
}

/// Convert a Unicode scalar offset into `text` to a UTF-16 code unit offset,
/// the unit used by IMKit, TSF and Android's `InputConnection`.
pub fn utf16_offset_from_chars(text: &str, char_offset: usize) -> usize {
    text.chars()
        .take(char_offset)
        .map(char::len_utf16)
        .sum::<usize>()
}

#[cfg(test)]
fn rime_build_dirs(user_data_dir: &Path, shared_data_dir: &Path) -> (PathBuf, PathBuf) {
    let dirs = RuntimeDirs::flat(user_data_dir, shared_data_dir);
    (dirs.staging_dir, dirs.prebuilt_data_dir)
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

fn prune_rime_logs(log_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    let mut logs: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if ![".log.INFO.", ".log.WARNING.", ".log.ERROR.", ".log.FATAL."]
                .iter()
                .any(|marker| name.contains(marker))
                || !entry.file_type().ok()?.is_file()
            {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, entry.path()))
        })
        .collect();
    logs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, path) in logs.into_iter().skip(20) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub fn librime_runtime_version() -> Option<String> {
    let _rime = desktop::rime_api_lock();
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

/// Check whether the loaded librime has registered its Lua module without loading it.
pub fn librime_lua_available() -> bool {
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    {
        let _rime = desktop::rime_api_lock();
        unsafe { desktop::lua_module_registered() }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    )))]
    {
        false
    }
}

/// Probe the optional grammar plugin without initializing its grammar component.
/// macOS preflights loadability; other desktop platforms check module/file presence
/// because loading a separate plugin runs its registration constructor.
pub fn octagram_available() -> bool {
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    ))]
    {
        let _rime = desktop::rime_api_lock();
        unsafe { desktop::octagram_available() }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    )))]
    false
}

const WANXIANG_GRAMMAR_FILE: &str = "wanxiang-lts-zh-hans.gram";
// librime 1.17.0 grammar.h: Grammar::Evaluate's no-grammar log(1e-6).
const NEUTRAL_GRAMMAR_PENALTY: f64 = -13.815510557964274;

fn valid_grammar_language(language: &str) -> bool {
    !language.is_empty()
        && !matches!(language, "." | "..")
        && !language.contains(['/', '\\', '\0'])
}

fn deployed_grammar_model_files(user_dir: &Path) -> Vec<PathBuf> {
    if cfg!(target_os = "ios") {
        return Vec::new();
    }
    let Ok(dirs) = RuntimeDirs::resolve(user_dir, user_dir) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dirs.staging_dir) else {
        return Vec::new();
    };
    let search_dirs = [
        dirs.user_data_dir.as_path(),
        user_dir,
        dirs.shared_data_dir.as_path(),
    ];
    let mut models = std::collections::BTreeSet::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(".schema.yaml") {
            continue;
        }
        let Some(language) = std::fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_yaml::from_slice::<Value>(&bytes).ok())
            .and_then(|schema| schema["grammar"]["language"].as_str().map(str::to_owned))
            .filter(|language| valid_grammar_language(language))
        else {
            continue;
        };
        let file_name = format!("{language}.gram");
        for dir in search_dirs {
            let candidate = dir.join(&file_name);
            if candidate.is_file() {
                models.insert(candidate);
                break;
            }
        }
    }
    models.into_iter().collect()
}

fn wanxiang_grammar_enabled(user_dir: &Path) -> bool {
    !deployed_grammar_model_files(user_dir).is_empty()
}

/// Patch generated configs only; package sources and user overrides stay intact.
// shortcut: Octagram still selects BeamSearch, so equal-weight sentences may reorder;
// exact 1.2.7 tie parity needs a schema-aware C++ factory unavailable through RimeApi.
fn patch_neutral_grammar(user_dir: &Path) {
    update_neutral_grammar(user_dir, true);
}

fn update_neutral_grammar(user_dir: &Path, enabled: bool) {
    if !wanxiang_grammar_enabled(user_dir) {
        return;
    }
    let Ok(dirs) = RuntimeDirs::resolve(user_dir, user_dir) else {
        return;
    };
    let build = dirs.staging_dir;
    match std::fs::symlink_metadata(&build) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            eprintln!("KeyTao: warning: refuse symbolic link: {}", build.display());
            return;
        }
        Err(error) => {
            eprintln!("KeyTao: warning: inspect {}: {error}", build.display());
            return;
        }
        Ok(_) => {}
    }
    for schema_id in configured_schema_list_from_dir(user_dir) {
        let result = (|| -> Result<(), String> {
            if !valid_rime_build_id(&schema_id) {
                return Err(format!("invalid schema id: {schema_id}"));
            }
            let path = build.join(format!("{schema_id}.schema.yaml"));
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(format!("inspect {}: {error}", path.display())),
            };
            if metadata.file_type().is_symlink() {
                return Err(format!("refuse symbolic link: {}", path.display()));
            }
            let mut content = std::fs::read(&path)
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            let grammar = format!(
                "grammar:\n  language: \"\"\n  non_collocation_penalty: {NEUTRAL_GRAMMAR_PENALTY}\n"
            );
            let block = format!("# KeyTao neutral grammar\n{grammar}");
            let block_with_newline =
                format!("\n# KeyTao neutral grammar (added newline)\n{grammar}");
            if !enabled {
                // Legacy patches added a separator to librime's unterminated YAML.
                let legacy = format!("\n{grammar}");
                let original = content
                    .strip_suffix(block_with_newline.as_bytes())
                    .or_else(|| {
                        content
                            .strip_suffix(block.as_bytes())
                            .filter(|bytes| bytes.is_empty() || bytes.ends_with(b"\n"))
                    })
                    .or_else(|| content.strip_suffix(legacy.as_bytes()));
                if let Some(original) = original {
                    write_file_atomic(&path, original)
                        .map_err(|error| format!("write {}: {error}", path.display()))?;
                }
                return Ok(());
            }
            if content
                .split(|byte| *byte == b'\n')
                .any(|line| line.starts_with(b"grammar:"))
            {
                let recognized =
                    serde_yaml::from_slice::<Value>(&content)
                        .ok()
                        .is_some_and(|schema| {
                            let grammar = &schema["grammar"];
                            grammar["language"]
                                .as_str()
                                .is_some_and(|language| !language.is_empty())
                                || (grammar.as_mapping().is_some_and(|keys| keys.len() == 2)
                                    && grammar["language"].as_str() == Some("")
                                    && grammar["non_collocation_penalty"].as_f64()
                                        == Some(NEUTRAL_GRAMMAR_PENALTY))
                        });
                return if recognized {
                    Ok(())
                } else {
                    Err(format!("skip existing grammar block: {}", path.display()))
                };
            }
            // Compiled YAML scalars must never pass through a serializer.
            content.extend_from_slice(if content.ends_with(b"\n") {
                block.as_bytes()
            } else {
                block_with_newline.as_bytes()
            });
            write_file_atomic(&path, &content)
                .map_err(|error| format!("write {}: {error}", path.display()))
        })();
        if let Err(error) = result {
            eprintln!("KeyTao: warning: neutral grammar patch skipped: {error}");
        }
    }
}

pub mod engine_choice;
pub mod libjd;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", target_os = "android", target_os = "ios"))]
mod inline_ascii;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", target_os = "android", target_os = "ios"))]
pub mod jd_backend;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", target_os = "android", target_os = "ios"))]
mod runtime_libjd;
#[cfg(target_os = "android")]
pub use runtime_libjd::{android_libjd_enabled, android_libjd_ready, set_android_libjd_data_root};
#[cfg(target_os = "ios")]
pub use runtime_libjd::active_libjd_schema;
#[cfg(target_os = "linux")]
pub use runtime_libjd::LIBJD_DICTIONARY_MISSING;

// ── Native desktop engine (guarded at the module level) ──────────────────────

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
/// Operations on a single input session. The runtime owns input policy,
/// batching across policy changes, and the backend's lifecycle.
pub(crate) trait ImeBackend: Send + Sync + english_mode::EnglishBackend {
    fn input_state(&self) -> global_input::InputState;
    fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult;
    fn switcher_hotkeys(&self) -> &[SchemaShortcut] {
        &[]
    }
    fn has_declared_shortcut(&self, _keycode: u32, _mask: u32) -> bool {
        false
    }
    fn shift_switch_disabled(&self, _schema: &str, _keycode: u32) -> bool {
        false
    }
    #[allow(dead_code, reason = "The runtime applies input policy between keys in a batch")]
    fn process_keys_result(&self, keys: &[(u32, u32)]) -> Option<KeyProcessResult> {
        key_batch::process_keys(keys, |keycode, mask| self.process_key_result(keycode, mask))
    }
    fn process_enter(&self) -> KeyProcessResult;
    fn select_candidate_on_page(&self, index: usize) -> ImeState;
    fn select_candidate_global(&self, index: usize) -> ImeState;
    fn highlight_candidate_on_page(&self, index: usize) -> ImeState;
    fn candidate_is_user_phrase_on_page(&self, index: usize) -> bool;
    fn delete_candidate_on_page_result(&self, index: usize) -> (ImeState, bool);
    fn all_candidates_limited(&self, max_count: usize) -> Vec<Candidate>;
    fn change_page(&self, backward: bool) -> ImeState;
    fn reset(&self) -> ImeState;
    fn clear_composition(&self) -> ImeState;
    fn commit_composition(&self) -> ImeState;
    fn raw_input(&self) -> Option<String>;
    fn commit_raw_input(&self) -> ImeState;
    fn list_schemas(&self) -> Vec<SchemaInfo>;
    fn active_switches(&self) -> Vec<SchemaSwitchSetting>;
    fn persist_switch_options(&self, options: &[(String, bool)]);
    fn apply_switch_request(
        &self,
        request: &SchemaSwitchRequest,
        reset_only: bool,
    ) -> Result<(), String>;
    fn apply_reset_switch_request(&self, root: &Path) -> Result<(), String>;
    fn is_ascii_mode(&self) -> bool;
    fn is_composing(&self) -> bool;
    fn supports_native_inline_ascii(&self) -> bool {
        false
    }
    fn capabilities(&self) -> EngineCapabilities;
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
mod desktop {
    use super::english_mode::EnglishBackend;
    use super::*;

    use librime_sys::{
        rime_get_api, RimeApi, RimeCandidateListIterator, RimeConfig, RimeConfigIterator,
        RimeContext, RimeLeversApi, RimeUserDictIterator,
    };
    use rime_api::{create_session, KeyEvent, KeyStatus};
    use std::cell::{Cell, RefCell};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::mem::ManuallyDrop;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    /// librime makes no thread-safety promise: `Service` keeps its session map,
    /// `ConfigComponent` its config cache and `DictionaryComponent` its table and
    /// prism caches in plain unsynchronised globals. Every call into rime_api
    /// therefore runs under this process-wide lock.
    ///
    /// The lock is reentrant so that a composite operation (create session,
    /// process key + read state, deploy) can hold it across the smaller helpers
    /// that also take it. Runtime UI entry points take it without waiting,
    /// across engine rebuilding and shared input-state restoration as well.
    static RIME_API_LOCK: Mutex<()> = Mutex::new(());
    static USER_CONFIG_NEEDS_FLUSH: AtomicBool = AtomicBool::new(false);
    thread_local! {
        static RIME_API_LOCK_DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    /// Whether librime is currently initialized. `reinitialize_rime` finalizes
    /// and initializes again so that a redeploy is not served from the caches
    /// above.
    static RIME_INITIALIZED: Mutex<bool> = Mutex::new(false);
    static RIME_USER_DATA_DIR: Mutex<Option<(PathBuf, Vec<u8>)>> = Mutex::new(None);
    static RIME_RUNTIME_DIRS: Mutex<Option<RuntimeDirs>> = Mutex::new(None);
    static RIME_DEPLOYMENT_SIGNATURE: Mutex<Option<String>> = Mutex::new(None);
    static RIME_LUA_SIGNATURE: Mutex<Option<String>> = Mutex::new(None);
    /// `RimeSetup` configures process-wide librime logging and modules. In
    /// particular, calling it again reaches glog's one-time initializer and
    /// aborts the process. Finalizing the Rime service does not undo setup, so
    /// reloads must call `initialize` without repeating `setup`.
    static RIME_SETUP_COMPLETE: Mutex<bool> = Mutex::new(false);
    /// glog keeps the first setup's directory even when librime is finalized
    /// and initialized for another user root. Older librime versions iterate
    /// this directory without catching filesystem exceptions during deployment.
    static RIME_SETUP_LOG_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
    static DEPLOY_RESULT: Mutex<Option<bool>> = Mutex::new(None);

    /// Deployments serialize against each other — they rename dictionaries
    /// around, share `DEPLOY_RESULT` and must not interleave — but they must
    /// not serialize against key handling, so the librime lock is only taken
    /// for the rime_api calls themselves and never across the path walking,
    /// YAML parsing and result validation around them.
    static DEPLOY_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) struct RimeApiGuard {
        guard: Option<MutexGuard<'static, ()>>,
    }

    impl Drop for RimeApiGuard {
        fn drop(&mut self) {
            self.guard.take();
            RIME_API_LOCK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }

    /// Serialize the current thread against every other librime caller.
    pub(crate) fn rime_api_lock() -> RimeApiGuard {
        let already_held = RIME_API_LOCK_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current.saturating_add(1));
            current > 0
        });
        if already_held {
            return RimeApiGuard { guard: None };
        }
        RimeApiGuard {
            guard: Some(lock_ignore_poison(&RIME_API_LOCK)),
        }
    }

    /// UI callbacks must not wait for another session's native rebuild.
    pub(crate) fn try_rime_api_lock() -> Option<RimeApiGuard> {
        let guard = if RIME_API_LOCK_DEPTH.with(|depth| depth.get() > 0) {
            None
        } else {
            Some(try_lock_ignore_poison(&RIME_API_LOCK)?)
        };
        RIME_API_LOCK_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Some(RimeApiGuard { guard })
    }

    /// librime's `RIME_STRUCT_HAS_MEMBER` for `RimeApi`.
    ///
    /// The bindings are generated from the header that was on the build
    /// machine, but the library loaded at run time can be older — a system
    /// librime on Linux, a user-supplied one on Windows. Every member past
    /// `data_size` is then memory librime never wrote, so reading the function
    /// pointer at all is undefined behaviour, not just a null check.
    fn api_has_member(api: *const RimeApi, member_offset: usize) -> bool {
        if api.is_null() {
            return false;
        }
        // SAFETY: `data_size` is the first member of every rime struct, so it
        // is present in every ABI this crate can be loaded against.
        let data_size = unsafe { (*api).data_size };
        if data_size <= 0 {
            return false;
        }
        member_offset < std::mem::size_of::<std::ffi::c_int>() + data_size as usize
    }

    /// The `member` entry point of `api`, or `None` when the loaded librime is
    /// older than the member — see [`api_has_member`].
    ///
    /// Only valid inside an `unsafe` block that holds the librime lock.
    macro_rules! rime_api_member {
        ($api:expr, $member:ident) => {{
            let api: *const RimeApi = $api;
            if api_has_member(api, std::mem::offset_of!(RimeApi, $member)) {
                (*api).$member
            } else {
                None
            }
        }};
    }

    #[cfg(test)]
    mod api_member_tests {
        use super::{
            active_user_dictionary_text, api_has_member, context_commit_preview, ImeState,
        };
        use librime_sys::{RimeApi, RimeContext};
        use std::ffi::{c_int, CString};
        use std::mem::{offset_of, size_of};

        #[cfg(not(target_os = "android"))]
        #[test]
        fn deployment_validation_restores_the_intended_schema() {
            const CASE_ENV: &str = "KEYTAO_DEPLOY_SELECTION_TEST_CASE";
            const ROOT_ENV: &str = "KEYTAO_DEPLOY_SELECTION_TEST_ROOT";
            let cases: &[(&[&str], &str, &str)] = &[
                (&["wanxiang", "easy_en"], "easy_en", "wanxiang"),
                (&["rime_ice", "easy_en", "wanxiang"], "wanxiang", "rime_ice"),
                (&["keytao", "easy_en", "wanxiang"], "easy_en", "easy_en"),
                (&["keytao", "easy_en", "wanxiang"], "keytao", "keytao"),
                (&["keytao", "rime_ice"], "rime_ice", "rime_ice"),
            ];
            if let Ok(case) = std::env::var(CASE_ENV) {
                let root = std::path::PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
                let (schemas, selected, expected) = cases[case.parse::<usize>().unwrap()];
                let entries = schemas
                    .iter()
                    .map(|schema| format!("  - schema: {schema}\n"))
                    .collect::<String>();
                std::fs::write(
                    root.join("default.yaml"),
                    format!("config_version: '1.0'\nschema_list:\n{entries}"),
                )
                .unwrap();
                std::fs::write(
                    root.join("default.custom.yaml"),
                    format!(
                        "patch:\n  schema_list:\n  {}",
                        entries.replace('\n', "\n  ")
                    ),
                )
                .unwrap();
                for schema in schemas {
                    std::fs::write(
                        root.join(format!("{schema}.schema.yaml")),
                        format!("schema:\n  schema_id: {schema}\n  name: {schema}\n  version: '1.0'\nengine:\n  processors: [ascii_composer, express_editor]\n  segmentors: [ascii_segmentor]\n  translators: [echo_translator]\n"),
                    )
                    .unwrap();
                }
                std::fs::write(
                    root.join("user.yaml"),
                    format!("var:\n  previously_selected_schema: {selected}\n"),
                )
                .unwrap();
                super::deploy(
                    root.to_string_lossy().into_owned(),
                    root.to_string_lossy().into_owned(),
                )
                .unwrap();
                let _rime = super::rime_api_lock();
                unsafe {
                    (librime_sys::rime_get_api()
                        .as_ref()
                        .unwrap()
                        .finalize
                        .unwrap())()
                };
                let user: serde_yaml::Value =
                    serde_yaml::from_slice(&std::fs::read(root.join("user.yaml")).unwrap())
                        .unwrap();
                assert_eq!(
                    user["var"]["previously_selected_schema"].as_str(),
                    Some(expected)
                );
                assert_eq!(
                    crate::deployed_schema_id_from_dir(&root).as_deref(),
                    Some(expected)
                );
                return;
            }

            let suffix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("keytao-deploy-selection-{suffix}"));
            for index in 0..cases.len() {
                let directory = root.join(index.to_string());
                std::fs::create_dir_all(&directory).unwrap();
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "desktop::api_member_tests::deployment_validation_restores_the_intended_schema"])
                    .env(CASE_ENV, index.to_string())
                    .env(ROOT_ENV, &directory)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "case {index}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[cfg(not(target_os = "android"))]
        #[test]
        fn deployment_file_validation_requires_all_selected_schemas() {
            let suffix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("keytao-deploy-files-{suffix}"));
            std::fs::create_dir_all(dir.join("build")).unwrap();
            assert!(super::validate_deployed_schema_files(&dir).is_err());
            std::fs::write(
                dir.join("default.custom.yaml"),
                "patch:\n  schema_list:\n    - schema: first\n    - schema: second\n",
            )
            .unwrap();
            std::fs::write(dir.join("build/first.schema.yaml"), "").unwrap();
            let error = super::validate_deployed_schema_files(&dir).unwrap_err();
            assert!(error.contains("second"), "{error}");
            std::fs::write(dir.join("build/second.schema.yaml"), "").unwrap();
            assert_eq!(
                super::validate_deployed_schema_files(&dir).unwrap(),
                ["first", "second"]
            );
            std::fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn members_past_data_size_are_reported_as_missing() {
            librime_sys::rime_struct!(api: RimeApi);
            assert!(api_has_member(&api, offset_of!(RimeApi, setup)));
            assert!(api_has_member(&api, offset_of!(RimeApi, change_page)));

            // An ABI that ends right after `setup`, the way a librime older
            // than these bindings looks.
            let after_setup = offset_of!(RimeApi, setup) + size_of::<usize>();
            api.data_size = (after_setup - size_of::<c_int>()) as c_int;
            assert!(api_has_member(&api, offset_of!(RimeApi, setup)));
            assert!(!api_has_member(&api, offset_of!(RimeApi, change_page)));

            assert!(!api_has_member(std::ptr::null(), 0));
        }

        #[test]
        fn commit_preview_preserves_converted_prefix_and_requires_a_candidate() {
            librime_sys::rime_struct!(context: RimeContext);
            let preview = CString::new("已转换之").unwrap();
            context.commit_text_preview = preview.as_ptr().cast_mut();
            context.menu.num_candidates = 3;
            context.menu.highlighted_candidate_index = 1;
            assert_eq!(unsafe { context_commit_preview(&context) }, "已转换之");

            // Rime may preview raw input even when the menu has no candidate.
            let raw = CString::new("zz").unwrap();
            context.commit_text_preview = raw.as_ptr().cast_mut();
            context.menu.num_candidates = 0;
            assert_eq!(unsafe { context_commit_preview(&context) }, "");
            assert!(ImeState::empty().commit_preview.is_empty());
        }

        #[test]
        fn user_dictionary_snapshot_excludes_deleted_tombstones() {
            assert_eq!(
                active_user_dictionary_text("你好\tni hao\t3"),
                Some("你好".to_owned())
            );
            assert_eq!(active_user_dictionary_text("你好\tni hao\t-3"), None);
            assert_eq!(
                active_user_dictionary_text("ni hao \t你好\tc=3 d=1.5 t=8"),
                Some("你好".to_owned())
            );
            assert_eq!(active_user_dictionary_text("#@/tick\t8"), None);
        }
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    extern "C" {
        // Static/mobile librime builds keep plugin modules dormant until required.
        #[link_name = "_Z23rime_require_module_luav"]
        fn rime_require_module_lua();
        #[link_name = "_Z26rime_require_module_leversv"]
        fn rime_require_module_levers();
    }

    #[cfg(target_os = "android")]
    extern "C" {
        #[link_name = "_Z28rime_require_module_octagramv"]
        fn rime_require_module_octagram();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    extern "C" {
        fn dlopen(filename: *const c_char, flags: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }

    #[cfg(target_os = "macos")]
    extern "C" {
        fn dlopen_preflight(filename: *const c_char) -> bool;
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
        setup_only_for_process(user_data_dir, shared_data_dir, DeployProcess::InProcess)
    }

    fn setup_only_for_process(
        user_data_dir: String,
        shared_data_dir: String,
        process: DeployProcess,
    ) -> Result<(), String> {
        let control_root = Path::new(&user_data_dir);
        let dirs = RuntimeDirs::resolve(control_root, Path::new(&shared_data_dir))?;
        setup_resolved_runtime(control_root, &dirs, process)
    }

    pub(super) fn setup_resolved_runtime(
        control_root: &Path,
        dirs: &RuntimeDirs,
        process: DeployProcess,
    ) -> Result<(), String> {
        runtime_log::init_for_engine(control_root);
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        crate::rt_log!(Level::Info, "rime", "setup_start");
        let initialized_now = {
            let _rime = rime_api_lock();
            if dirs.active_package_key.is_none() {
                regenerate_rime_lua_for_ime(control_root);
            }
            let mut initialized = lock_ignore_poison(&RIME_INITIALIZED);
            if *initialized {
                false
            } else {
                initialize_rime(control_root, dirs, false);
                *initialized = true;
                true
            }
        };
        if initialized_now && process.needs_user_dictionary_cache() {
            initialize_user_dictionary_cache(&dirs.user_data_dir);
        }
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "setup_end",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                initialized = initialized_now,
                librime = librime_runtime_version()
            );
        }
        Ok(())
    }

    pub(crate) fn preview_deployed_candidates(
        user_dir: &Path,
        request: &PreviewRequest,
        snapshot_ready: Option<&mut dyn FnMut() -> Result<(), String>>,
    ) -> Result<CandidatePreview, String> {
        request.validate()?;
        let runtime = RuntimeDirs::resolve(user_dir, user_dir)?;
        let schema = resolved_deployed_schema_id(&runtime).ok_or("no active deployed schema")?;
        let directory = preview::PreviewDirectory::snapshot(&runtime)?;
        let _rime = rime_api_lock();
        if *lock_ignore_poison(&RIME_INITIALIZED) {
            return Err("candidate preview requires a fresh child process".into());
        }
        // Rime's user config and user dictionaries are writable only in this snapshot.
        // Compiled dictionaries/models remain in the original, read-only data search path.
        let dirs = RuntimeDirs::flat(directory.path(), &runtime.user_data_dir);
        #[cfg(target_os = "macos")]
        let live_data_stamp = || {
            use std::os::unix::fs::MetadataExt;

            let identity = |path: &Path| match std::fs::metadata(path) {
                Ok(meta) => Ok(Some((meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()))),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("inspect compiled preview data: {error}")),
            };
            let mut watched = vec![
                dirs.prebuilt_data_dir.clone(),
                runtime.user_data_dir.join(WANXIANG_GRAMMAR_FILE),
            ];
            for model in deployed_grammar_model_files(&runtime.user_data_dir) {
                if !watched.contains(&model) {
                    watched.push(model);
                }
            }
            watched
                .into_iter()
                .map(|path| identity(&path))
                .collect::<Result<Vec<_>, String>>()
        };
        #[cfg(target_os = "macos")]
        let live_data_before = snapshot_ready.is_some().then(live_data_stamp).transpose()?;
        if let Some(ready) = snapshot_ready {
            ready()?;
        }
        setup_rime(
            &dirs,
            &directory.path().join("log").to_string_lossy(),
            false,
            &runtime.user_data_dir,
        );
        *lock_ignore_poison(&RIME_INITIALIZED) = true;
        let result = preview_session(&schema, request, true);
        let finalized = finalize_rime();
        let preview = result.and_then(|preview| finalized.map(|()| preview))?;
        #[cfg(target_os = "macos")]
        if let Some(before) = live_data_before {
            if live_data_stamp()? != before {
                return Err("Compiled preview data changed during candidate preview".into());
            }
        }
        Ok(preview)
    }

    pub(crate) fn preview_session(
        schema_id: &str,
        request: &PreviewRequest,
        isolated: bool,
    ) -> Result<CandidatePreview, String> {
        request.validate()?;
        let _rime = rime_api_lock();
        // Creating a raw session restores the saved schema without ApplySchema's Save().
        let session = create_session().map_err(|e| format!("create preview session: {e:?}"))?;
        let active = session
            .status()
            .map_err(|e| format!("read preview schema: {e:?}"))?;
        if active.schema_id() != schema_id {
            if !isolated {
                return Err("active preview schema differs from the saved schema".into());
            }
            select_schema_checked(&session, schema_id)?;
        }
        unsafe {
            let api = rime_get_api();
            let set =
                rime_api_member!(api, set_option).ok_or("preview options API is unavailable")?;
            set(session.session_id, c"ascii_mode".as_ptr(), 0);
        }
        session.process_key(key_event(u32::from(b'f'), 0));
        let context = session.context().ok_or("read preview context failed")?;
        let configured = u8::try_from(context.menu().page_size)
            .ok()
            .filter(|size| *size > 0)
            .ok_or("invalid deployed candidate page size")?;
        let page_size = request.page_size.unwrap_or(configured);
        let candidates = extract_all_candidates(&session, usize::from(page_size))
            .ok_or("candidate preview iterator is unavailable")?
            .into_iter()
            .map(|candidate| PreviewCandidate {
                text: candidate.text,
                comment: candidate.comment.unwrap_or_default(),
            })
            .collect();
        unsafe {
            let api = rime_get_api();
            let clear = rime_api_member!(api, clear_composition)
                .ok_or("preview clear API is unavailable")?;
            clear(session.session_id);
        }
        Ok(CandidatePreview {
            candidates,
            page_size,
        })
    }

    /// Tear librime down, leaving the process without an instance.
    ///
    /// Only for callers that already drained every live engine — see
    /// [`crate::reinitialize`], the entry point that does.
    pub(crate) fn finalize_rime() -> Result<(), String> {
        {
            let _rime = rime_api_lock();
            let mut initialized = lock_ignore_poison(&RIME_INITIALIZED);
            if !*initialized {
                clear_user_dictionary_cache();
                return Ok(());
            }
            unsafe {
                let api = rime_get_api();
                let finalize = (*api)
                    .finalize
                    .ok_or("librime finalize API is unavailable")?;
                finalize();
            }
            *initialized = false;
            *lock_ignore_poison(&RIME_USER_DATA_DIR) = None;
            *lock_ignore_poison(&RIME_RUNTIME_DIRS) = None;
        }
        clear_user_dictionary_cache();
        Ok(())
    }

    /// Finalize librime and initialize it again.
    ///
    /// librime caches compiled config, tables and prisms behind `weak_ptr`s that
    /// are only re-read once the last reference is gone, so reloading after a
    /// deployment means tearing the whole engine down. Callers must have dropped
    /// every live session first, otherwise the sessions outlive their engine —
    /// which is why the crate-level [`crate::reinitialize`] is the only public
    /// way in.
    pub(crate) fn reinitialize_rime(
        user_data_dir: String,
        shared_data_dir: String,
    ) -> Result<(), String> {
        let control_root = Path::new(&user_data_dir);
        let dirs = RuntimeDirs::resolve(control_root, Path::new(&shared_data_dir))?;
        reinitialize_resolved_runtime(control_root, &dirs)
    }

    pub(super) fn reinitialize_resolved_runtime(
        control_root: &Path,
        dirs: &RuntimeDirs,
    ) -> Result<(), String> {
        {
            let _rime = rime_api_lock();
            if dirs.active_package_key.is_none() {
                regenerate_rime_lua_for_ime(control_root);
            }
            finalize_rime()?;
            let mut initialized = lock_ignore_poison(&RIME_INITIALIZED);
            initialize_rime(control_root, dirs, false);
            *initialized = true;
        }
        initialize_user_dictionary_cache(&dirs.user_data_dir);
        Ok(())
    }

    fn capture_deployment_log(user_dir: &Path) -> deployment_log::DeploymentLog {
        // glog keeps the first setup's directory across runtime switches.
        let directory = lock_ignore_poison(&RIME_SETUP_LOG_DIR)
            .clone()
            .unwrap_or_else(|| rime_log_dir(user_dir));
        deployment_log::DeploymentLog::capture(directory)
    }

    /// Initialize and fully deploy librime.
    /// `setup` + `initialize` run only on the first call; subsequent calls only
    /// re-run `full_deploy_and_wait` so that newly installed schemas are picked up.
    /// Android deploys one schema at a time to keep large dependency graphs from
    /// exhausting the app process while producing the same build artifacts.
    /// Blocking — run inside `tokio::task::spawn_blocking` when called from async code.
    pub fn deploy(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
        deploy_for_process(user_data_dir, shared_data_dir, DeployProcess::InProcess)
    }

    pub fn deploy_for_process(
        user_data_dir: String,
        shared_data_dir: String,
        process: DeployProcess,
    ) -> Result<(), String> {
        let control_root = Path::new(&user_data_dir);
        runtime_log::init_for_engine(control_root);
        let dirs = RuntimeDirs::resolve(control_root, Path::new(&shared_data_dir))?;
        let user_dir = dirs.user_data_dir.as_path();
        let shared_dir = dirs.shared_data_dir.as_path();
        let package_shared = dirs.package_shared();
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        let schema_count = started
            .map(|_| parse_schema_list_from_dir(user_dir).len())
            .unwrap_or_default();
        crate::rt_log!(
            Level::Info,
            "rime",
            "deploy_start",
            schema_count = schema_count
        );
        let _deploy = lock_ignore_poison(&DEPLOY_LOCK);
        let result = (|| {
            let deploy_log = capture_deployment_log(control_root);
            #[cfg(not(target_os = "android"))]
            let input_snapshot = if cfg!(target_os = "ios") {
                None
            } else {
                desktop_inputs::snapshot(user_dir, shared_dir)
            };
            #[cfg(not(target_os = "android"))]
            let selective = desktop_deploy_plan(user_dir, shared_dir);
            #[cfg(not(target_os = "android"))]
            begin_desktop_deployment(user_dir, shared_dir, selective.as_deref())?;
            #[cfg(target_os = "android")]
            begin_deployment(user_dir)?;

            #[cfg(target_os = "windows")]
            patch_windows_lua_compatibility(user_dir)?;

            setup_resolved_runtime(control_root, &dirs, process)?;

            #[cfg(target_os = "android")]
            {
                deploy_android_staged(user_dir, package_shared)
                    .map_err(|error| deploy_log.failure_message(Some(&error)))?;
            }

            #[cfg(not(target_os = "android"))]
            {
                let selective_requested = selective.is_some();
                let deployed = if let Some(changed) = selective {
                    deploy_desktop_selective(user_dir, &changed, package_shared)
                } else {
                    Ok(None)
                }
                .map_err(|error| {
                    deploy_log.failure_message(Some(&format!("依赖部署失败：{error}")))
                })?;
                let deployed = if let Some(deployed) = deployed {
                    deployed
                } else {
                    if selective_requested {
                        begin_desktop_deployment(user_dir, shared_dir, None)?;
                    }
                    prepare_missing_desktop_schemas(user_dir, shared_dir)?;
                    if !full_deploy_and_wait(user_dir)
                        .map_err(|error| deploy_log.failure_message(Some(&error)))?
                    {
                        return Err(deploy_log.failure_message(None));
                    }
                    deploy_desktop_schema_dependencies(user_dir, None, package_shared)
                        .map_err(|error| {
                            deploy_log.failure_message(Some(&format!("依赖部署失败：{error}")))
                        })?
                        .ok_or("full deployment did not finish")?
                };
                patch_neutral_grammar(user_dir);
                validate_deployed_schemas(user_dir, &deployed, dirs.active_package_key.is_some())
                    .map_err(|error| {
                        deploy_log.failure_message(Some(&format!("部署验证失败：{error}")))
                    })?;
            }
            complete_resolved_deployment(user_dir, package_shared, || {
                #[cfg(not(target_os = "android"))]
                record_desktop_deployment(user_dir, shared_dir, input_snapshot.as_ref())?;
                prune_rime_build_artifacts(user_dir, Some(shared_dir)).map(|_| ())
            })?;
            Ok(())
        })();
        if result.is_err() {
            let _ = mark_deployment_failed(user_dir);
        }
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "deploy_end",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                schema_count = schema_count,
                ok = result.is_ok()
            );
        }
        result
    }

    pub(crate) fn deploy_resolved_runtime(
        control_root: &Path,
        dirs: &RuntimeDirs,
        mut progress: impl FnMut(DeployStep),
    ) -> Result<(), String> {
        let _deploy = lock_ignore_poison(&DEPLOY_LOCK);
        let user_dir = dirs.user_data_dir.as_path();
        let shared_dir = dirs.shared_data_dir.as_path();
        let package_shared = dirs.package_shared();
        let deploy_log = capture_deployment_log(control_root);
        begin_deployment_with_invalidation(user_dir, false)?;
        #[cfg(target_os = "windows")]
        patch_windows_lua_compatibility(user_dir)?;
        {
            let _rime = rime_api_lock();
            let mut initialized = lock_ignore_poison(&RIME_INITIALIZED);
            if *initialized {
                return Err("librime is already initialized in the deployment worker".into());
            }
            // Materialization already prepared this package's Lua in the staged user directory.
            initialize_rime(control_root, dirs, false);
            *initialized = true;
        }
        let result = (|| {
            progress(DeployStep::Compile);
            if !full_deploy_and_wait(user_dir)
                .map_err(|error| deploy_log.failure_message(Some(&error)))?
            {
                return Err(deploy_log.failure_message(None));
            }
            let deployed = deploy_desktop_schema_dependencies(user_dir, None, package_shared)
                .map_err(|error| deploy_log.failure_message(Some(&error)))?
                .ok_or("full deployment did not finish")?;
            progress(DeployStep::Validate);
            validate_deployed_schemas(user_dir, &deployed, dirs.active_package_key.is_some())
                .map_err(|error| deploy_log.failure_message(Some(&error)))?;
            complete_resolved_deployment(user_dir, package_shared, || {
                progress(DeployStep::Prune);
                prune_rime_build_artifacts(user_dir, Some(shared_dir)).map(|_| ())
            })
        })();
        result.and(finalize_rime())
    }

    #[cfg(target_os = "android")]
    struct AndroidDictionaryOverride {
        source: PathBuf,
        backup: PathBuf,
    }

    #[cfg(target_os = "android")]
    impl Drop for AndroidDictionaryOverride {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.source);
            if std::fs::rename(&self.backup, &self.source).is_err()
                && std::fs::copy(&self.backup, &self.source).is_ok()
            {
                let _ = std::fs::remove_file(&self.backup);
            }
        }
    }

    #[cfg(target_os = "android")]
    fn prepare_android_auxiliary_dictionary(
        user_dir: &Path,
        schema_id: &str,
    ) -> Result<Option<AndroidDictionaryOverride>, String> {
        // LiangFen is a character-only reverse lookup dependency in XMJD and TXJX.
        // Importing essay phrases cannot affect that lookup but exceeds low-memory devices.
        if schema_id != "liangfen" {
            return Ok(None);
        }

        let source = user_dir.join("liangfen.dict.yaml");
        let backup = user_dir.join(".liangfen.dict.yaml.keytao-backup");
        if backup.is_file() {
            let current = std::fs::read_to_string(&source).unwrap_or_default();
            if current.contains(android_auxiliary_dictionary_marker()) {
                let _ = std::fs::remove_file(&source);
                std::fs::rename(&backup, &source)
                    .map_err(|error| format!("failed to restore LiangFen dictionary: {error}"))?;
            } else {
                let _ = std::fs::remove_file(&backup);
            }
        }

        let original = std::fs::read_to_string(&source)
            .map_err(|error| format!("failed to read LiangFen dictionary: {error}"))?;
        let Some(patched) = patch_android_auxiliary_dictionary(&original) else {
            return Ok(None);
        };

        std::fs::rename(&source, &backup)
            .map_err(|error| format!("failed to stage LiangFen dictionary: {error}"))?;
        if let Err(error) = std::fs::write(&source, patched) {
            let _ = std::fs::rename(&backup, &source);
            return Err(format!(
                "failed to write Android LiangFen dictionary: {error}"
            ));
        }
        Ok(Some(AndroidDictionaryOverride { source, backup }))
    }

    fn initialize_rime(control_root: &Path, dirs: &RuntimeDirs, deployer_only: bool) {
        let user_dir = dirs.user_data_dir.as_path();
        let lua_prepared = !lock_ignore_poison(&PROCESS_RIME.lua_retry_after)
            .as_ref()
            .is_some_and(|(root, _)| root == control_root);
        *lock_ignore_poison(&RIME_LUA_SIGNATURE) = lua_prepared
            .then(|| {
                rime_lua::signature_for_runtime(control_root, dirs)
                    .ok()
                    .flatten()
            })
            .flatten();
        *lock_ignore_poison(&RIME_DEPLOYMENT_SIGNATURE) =
            ReloadStamp::signature_at(&dirs.staging_dir.join(DEPLOYED_FILE));
        patch_neutral_grammar(user_dir);
        let log_dir = rime_log_dir(control_root);

        let _ = std::fs::create_dir_all(&dirs.staging_dir);
        if dirs.active_package_key.is_none() {
            let _ = std::fs::create_dir_all(&dirs.prebuilt_data_dir);
        }
        let _ = std::fs::create_dir_all(&log_dir);

        if dirs.active_package_key.as_deref().is_some_and(|key| {
            RuntimeDirs::isolated_user_data_dir(control_root, key).as_deref() == Some(user_dir)
        }) {
            ensure_installation_branding(user_dir);
        }
        setup_rime(dirs, &log_dir.to_string_lossy(), deployer_only, user_dir);
        *lock_ignore_poison(&RIME_RUNTIME_DIRS) = Some(dirs.clone());
    }

    fn ensure_installation_branding(user_dir: &Path) {
        let path = user_dir.join("installation.yaml");
        let missing = std::fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        if !missing {
            return;
        }
        // Leave device identity and sync settings to the installation and sync paths.
        let branding = b"distribution_name: KeyTao\ndistribution_code_name: keytao\n";
        #[cfg(not(target_os = "android"))]
        let result = schema_switch_request::create_file_atomic(&path, branding);
        #[cfg(target_os = "android")]
        let result = {
            // Android shared storage can reject hard links.
            use std::io::Write;
            std::fs::File::create_new(&path).and_then(|mut file| file.write_all(branding))
        };
        if let Err(error) = result {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                eprintln!("KeyTao: could not write {}: {error}", path.display());
            }
        }
    }

    pub(super) fn loaded_runtime() -> Option<RuntimeDirs> {
        lock_ignore_poison(&RIME_RUNTIME_DIRS).clone()
    }

    pub(super) fn loaded_package_changed(wanted: &RuntimeDirs) -> bool {
        lock_ignore_poison(&RIME_RUNTIME_DIRS)
            .as_ref()
            .is_some_and(|loaded| loaded.active_package_key != wanted.active_package_key)
    }

    pub(super) fn loaded_schema_root(control_root: &Path) -> PathBuf {
        lock_ignore_poison(&RIME_RUNTIME_DIRS)
            .as_ref()
            .filter(|dirs| dirs.active_package_key.is_some())
            .map_or_else(
                || control_root.to_path_buf(),
                |dirs| dirs.user_data_dir.clone(),
            )
    }

    pub(super) fn loaded_deployment_is_current(dirs: &RuntimeDirs) -> bool {
        if dirs.active_package_key.is_some()
            && lock_ignore_poison(&RIME_RUNTIME_DIRS)
                .as_ref()
                .is_none_or(|loaded| loaded.staging_dir != dirs.staging_dir)
        {
            return false;
        }
        let current = ReloadStamp::signature_at(&dirs.staging_dir.join(DEPLOYED_FILE));
        current.is_some() && *lock_ignore_poison(&RIME_DEPLOYMENT_SIGNATURE) == current
    }

    pub(super) fn loaded_lua_is_current(user_dir: &Path) -> bool {
        let signature = match loaded_runtime().filter(|dirs| dirs.active_package_key.is_some()) {
            Some(dirs) => rime_lua::signature_for_runtime(user_dir, &dirs),
            None => rime_lua::signature(user_dir),
        };
        signature.is_ok_and(|signature| *lock_ignore_poison(&RIME_LUA_SIGNATURE) == signature)
    }

    fn deploy_desktop_schema_dependencies(
        user_dir: &Path,
        changed: Option<&[String]>,
        package_shared: Option<&Path>,
    ) -> Result<Option<Vec<String>>, String> {
        let initial_schemas = parse_schema_list_from_dir(user_dir);
        if initial_schemas.is_empty() {
            return Err("no schema selected in default.custom.yaml".into());
        }

        let mut visited: HashSet<String> = initial_schemas.iter().cloned().collect();
        let mut pending = std::collections::VecDeque::new();
        let mut deployed = Vec::new();
        #[cfg(not(target_os = "android"))]
        let trusted = changed.and_then(|_| desktop_inputs::recorded_schema_ids(user_dir));
        #[cfg(target_os = "android")]
        let trusted: Option<std::collections::BTreeSet<String>> = None;
        for schema_id in &initial_schemas {
            let compiled = compiled_schema_path(user_dir, schema_id)?;
            let needs_deploy = changed.is_some_and(|ids| ids.contains(schema_id))
                || !compiled.is_file()
                || cfg!(target_os = "ios");
            if needs_deploy && !deploy_listed_schema_from(user_dir, schema_id, package_shared)? {
                continue;
            }
            require_compiled_schema(&compiled, schema_id)?;
            if changed.is_none() || needs_deploy {
                deployed.push(schema_id.clone());
            }
            pending.extend(schema_dependencies_from_file(&compiled));
        }
        require_any_compiled_schema(user_dir, &initial_schemas)?;

        while let Some(schema_id) = pending.pop_front() {
            if !visited.insert(schema_id.clone()) {
                continue;
            }

            if changed.is_some() && !trusted.as_ref().is_some_and(|ids| ids.contains(&schema_id)) {
                // New dependency edges can expose stale inactive builds, even through includes.
                return Ok(None);
            }

            let source = schema_source_path(user_dir, package_shared, &schema_id);
            if !source.is_file() {
                continue;
            }
            let compiled = compiled_schema_path(user_dir, &schema_id)?;
            if !compiled.is_file() || changed.is_some_and(|ids| ids.contains(&schema_id)) {
                deploy_schema_file(&source)?;
                deployed.push(schema_id.clone());
            } else if changed.is_none() {
                deployed.push(schema_id.clone());
            }
            require_compiled_schema(&compiled, &schema_id)?;
            pending.extend(schema_dependencies_from_file(&compiled));
        }
        Ok(Some(deployed))
    }

    #[cfg(not(target_os = "android"))]
    fn desktop_deploy_signature(user_dir: &Path, shared_dir: &Path) -> Result<String, String> {
        let dirs = RuntimeDirs::resolve(user_dir, shared_dir)?;
        Ok(format!(
            "{};librime/{};shared/{}",
            deployment_fingerprint(user_dir)?,
            librime_runtime_version().ok_or("librime version is unavailable")?,
            dirs.shared_data_dir.display()
        ))
    }

    #[cfg(not(target_os = "android"))]
    fn desktop_deploy_plan(user_dir: &Path, shared_dir: &Path) -> Option<Vec<String>> {
        let build = RuntimeDirs::resolve(user_dir, user_dir).ok()?.staging_dir;
        if cfg!(target_os = "ios")
            || build.join(DEPLOYING_FILE).exists()
            || build.join(".keytao-deploy-failed").exists()
            || !build.join(DEPLOYED_FILE).is_file()
        {
            return None;
        }
        let changed = desktop_inputs::changed_schemas(
            user_dir,
            shared_dir,
            &parse_schema_list_from_dir(user_dir),
            &desktop_deploy_signature(user_dir, shared_dir).ok()?,
        )?;
        // The granular schema deployer accepts user sources; shared-only changes use maintenance.
        changed
            .iter()
            .all(|id| user_dir.join(format!("{id}.schema.yaml")).is_file())
            .then_some(changed)
    }

    #[cfg(not(target_os = "android"))]
    fn begin_desktop_deployment(
        user_dir: &Path,
        shared_dir: &Path,
        changed: Option<&[String]>,
    ) -> Result<(), String> {
        if cfg!(target_os = "ios") {
            return begin_deployment(user_dir);
        }
        let config_current =
            changed.is_some() && desktop_inputs::default_config_is_current(user_dir, shared_dir);
        let schemas = if let Some(changed) = changed {
            changed.to_vec()
        } else {
            let mut schemas = parse_schema_list_from_dir(user_dir);
            let mut index = 0;
            while index < schemas.len() {
                for dependency in
                    schema_dependencies_from_file(&compiled_schema_path(user_dir, &schemas[index])?)
                {
                    if !schemas.contains(&dependency) {
                        schemas.push(dependency);
                    }
                }
                index += 1;
            }
            let fingerprints = schema_input_fingerprints(user_dir, &schemas)?;
            let desktop_changed = desktop_inputs::changed_schema_inputs(user_dir, shared_dir);
            schemas.retain(|id| {
                fingerprints.get(id).is_none_or(Option::is_none)
                    || desktop_changed
                        .as_ref()
                        .is_some_and(|changed| changed.contains(id))
            });
            schemas
        };
        begin_deployment_with_invalidation(user_dir, changed.is_none())?;
        invalidate_rime_build_artifacts(user_dir, &schemas, &[])?;
        // ConfigFileUpdate only checks timestamps; content receipts also catch same-mtime edits.
        if !config_current {
            remove_rime_build_artifact(
                &RuntimeDirs::resolve(user_dir, user_dir)?
                    .staging_dir
                    .join("default.yaml"),
            )?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    fn record_desktop_deployment(
        user_dir: &Path,
        shared_dir: &Path,
        snapshot: Option<&desktop_inputs::SourceSnapshot>,
    ) -> Result<(), String> {
        if cfg!(target_os = "ios") {
            return Ok(());
        }
        desktop_inputs::record(
            user_dir,
            shared_dir,
            &parse_schema_list_from_dir(user_dir),
            &desktop_deploy_signature(user_dir, shared_dir)?,
            snapshot,
        )
    }

    #[cfg(not(target_os = "android"))]
    fn prepare_missing_desktop_schemas(user_dir: &Path, shared_dir: &Path) -> Result<(), String> {
        if cfg!(target_os = "ios") {
            return Ok(());
        }
        let schemas = parse_schema_list_from_dir(user_dir);
        let mut missing = false;
        for id in &schemas {
            let name = format!("{id}.schema.yaml");
            if !user_dir.join(&name).is_file() && !shared_dir.join(&name).is_file() {
                deploy_listed_schema(user_dir, id)?;
                missing = true;
            }
        }
        if missing {
            let _rime = rime_api_lock();
            unsafe {
                let initialize = rime_api_member!(rime_get_api(), deployer_initialize)
                    .ok_or("librime deployer initialization API is unavailable")?;
                initialize(std::ptr::null_mut());
            }
            deploy_config_file("default.yaml", "config_version")?;
            write_effective_default_schema_list(
                user_dir,
                &effective_schema_list(user_dir, schemas),
            )?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    fn deploy_desktop_selective(
        user_dir: &Path,
        changed: &[String],
        package_shared: Option<&Path>,
    ) -> Result<Option<Vec<String>>, String> {
        {
            let _rime = rime_api_lock();
            prepare_deployment_log_directory()?;
            unsafe {
                let api = rime_get_api();
                let initialize = rime_api_member!(api, deployer_initialize)
                    .ok_or("librime deployer initialization API is unavailable")?;
                let run = rime_api_member!(api, run_task)
                    .ok_or("librime deployment task API is unavailable")?;
                initialize(std::ptr::null_mut());
                run(c"clean_old_log_files".as_ptr());
                if run(c"installation_update".as_ptr()) == 0 {
                    return Err("failed to update Rime installation".into());
                }
            }
        }
        // A trusted same-version full deployment already upgraded legacy userdbs;
        // new legacy databases force full maintenance. Trash cleanup only moves old files.
        update_neutral_grammar(user_dir, false);
        let result = (|| {
            deploy_config_file("default.yaml", "config_version")?;
            let deployed =
                deploy_desktop_schema_dependencies(user_dir, Some(changed), package_shared)?;
            let _rime = rime_api_lock();
            unsafe {
                let api = rime_get_api();
                let open = rime_api_member!(api, user_config_open)
                    .ok_or("librime user config API is unavailable")?;
                let close = rime_api_member!(api, config_close)
                    .ok_or("librime config close API is unavailable")?;
                let set = rime_api_member!(api, config_set_int)
                    .ok_or("librime config setter is unavailable")?;
                let mut config: RimeConfig = std::mem::zeroed();
                if open(c"user".as_ptr(), &mut config) == 0 {
                    return Err("cannot open Rime user config".into());
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i32;
                let saved = set(&mut config, c"var/last_build_time".as_ptr(), now);
                close(&mut config);
                if saved == 0 {
                    return Err("cannot save Rime last build time".into());
                }
            }
            Ok(deployed)
        })();
        patch_neutral_grammar(user_dir);
        result
    }

    fn validate_deployed_schema_files(user_dir: &Path) -> Result<Vec<String>, String> {
        let schemas = effective_schema_list(user_dir, parse_schema_list_from_dir(user_dir));
        if schemas.is_empty() {
            return Err("no schema selected in default.custom.yaml".into());
        }
        for schema_id in &schemas {
            require_compiled_schema(&compiled_schema_path(user_dir, schema_id)?, schema_id)?;
        }
        Ok(schemas)
    }

    fn validate_deployed_schemas(
        user_dir: &Path,
        deployed: &[String],
        isolated: bool,
    ) -> Result<(), String> {
        let schemas = validate_deployed_schema_files(user_dir)?;
        // Validation selections persist in user.yaml, even when validation fails.
        let schema_to_keep = if isolated {
            deployed_schema_id_from_dir(user_dir)
                .filter(|schema| schemas.contains(schema))
                .or_else(|| preferred_isolated_schema_from_list(schemas.clone()))
        } else if schemas.iter().all(|schema| is_addon_schema(schema)) {
            schemas.first().cloned()
        } else {
            deployed_schema_id_from_dir(user_dir)
                .filter(|schema| schemas.contains(schema))
                .or_else(|| preferred_schema_from_list(schemas.clone()))
        };
        let schemas = validation_schema_list(&schemas, deployed, schema_to_keep.as_deref());
        let _rime = rime_api_lock();
        let session =
            create_session().map_err(|error| format!("create validation session: {error:?}"))?;
        let result = schemas
            .iter()
            .try_for_each(|schema_id| select_schema_checked(&session, schema_id));
        let restored = schema_to_keep
            .as_deref()
            .filter(|id| result.is_err() || schemas.last().map(String::as_str) != Some(id))
            .map_or(Ok(()), |schema_id| {
                select_schema_checked(&session, schema_id)
            });
        result.and(restored)
    }

    fn validation_schema_list(
        schemas: &[String],
        deployed: &[String],
        active: Option<&str>,
    ) -> Vec<String> {
        let mut validate: Vec<_> = schemas
            .iter()
            .filter(|id| deployed.contains(id) && Some(id.as_str()) != active)
            .cloned()
            .collect();
        if let Some(active) = active {
            validate.push(active.to_owned());
        }
        validate
    }

    #[cfg(target_os = "android")]
    fn deploy_android_staged(user_dir: &Path, package_shared: Option<&Path>) -> Result<(), String> {
        deploy_config_file("default.yaml", "config_version")?;

        let initial_schemas = parse_schema_list_from_dir(user_dir);
        if initial_schemas.is_empty() {
            return Err("no schema selected in default.custom.yaml".into());
        }

        let mut pending = initial_schemas
            .iter()
            .cloned()
            .map(|schema| (schema, true))
            .collect::<std::collections::VecDeque<_>>();
        let mut deployed = HashSet::new();
        while let Some((schema_id, required)) = pending.pop_front() {
            if !deployed.insert(schema_id.clone()) {
                continue;
            }

            if required {
                if !deploy_listed_schema_from(user_dir, &schema_id, package_shared)? {
                    continue;
                }
            } else {
                let source = schema_source_path(user_dir, package_shared, &schema_id);
                if !source.is_file() {
                    continue;
                }
                let _dictionary_override =
                    if package_shared.is_none() || user_dir.join("liangfen.dict.yaml").is_file() {
                        prepare_android_auxiliary_dictionary(user_dir, &schema_id)?
                    } else {
                        None
                    };
                deploy_schema_file(&source)?;
            }

            let compiled = compiled_schema_path(user_dir, &schema_id)?;
            for dependency in schema_dependencies_from_file(&compiled) {
                if !deployed.contains(&dependency) {
                    pending.push_back((dependency, false));
                }
            }
        }
        require_any_compiled_schema(user_dir, &initial_schemas)?;
        Ok(())
    }

    #[cfg(test)]
    mod listed_schema_tests {
        include!("listed_schema_tests.rs");
    }

    #[cfg(all(
        test,
        any(target_os = "linux", target_os = "windows", target_os = "macos")
    ))]
    mod desktop_deploy_tests {
        include!("desktop_deploy_tests.rs");
    }

    #[cfg(any(not(target_os = "android"), test))]
    fn deploy_listed_schema(user_dir: &Path, schema_id: &str) -> Result<bool, String> {
        deploy_listed_schema_from(user_dir, schema_id, None)
    }

    fn deploy_listed_schema_from(
        user_dir: &Path,
        schema_id: &str,
        package_shared: Option<&Path>,
    ) -> Result<bool, String> {
        let shared_dir = if cfg!(target_os = "android") {
            let _rime = rime_api_lock();
            unsafe {
                rime_api_member!(rime_get_api(), get_shared_data_dir)
                    .and_then(|get| cstr_to_str(get()))
                    .map(PathBuf::from)
            }
        } else {
            None
        };
        let source = schema_source_path(user_dir, package_shared, schema_id);
        deploy_listed_schema_at(user_dir, schema_id, shared_dir.as_deref(), source)
    }

    #[cfg(test)]
    fn deploy_listed_schema_with_reuse(
        user_dir: &Path,
        schema_id: &str,
        shared_dir: Option<&Path>,
    ) -> Result<bool, String> {
        let source = schema_source_path(user_dir, None, schema_id);
        deploy_listed_schema_at(user_dir, schema_id, shared_dir, source)
    }

    fn deploy_listed_schema_at(
        user_dir: &Path,
        schema_id: &str,
        shared_dir: Option<&Path>,
        source: PathBuf,
    ) -> Result<bool, String> {
        let log = capture_deployment_log(user_dir);
        let compiled = compiled_schema_path(user_dir, schema_id)?;
        // librime can leave a previous build behind when a new config fails to compile.
        if !shared_dir.is_some_and(|shared| {
            schema_inputs::schema_input_is_current(user_dir, schema_id, shared)
        }) {
            remove_rime_build_artifact(&compiled)?;
        }
        #[cfg(target_os = "android")]
        let _dictionary_override =
            if source.is_file() && user_dir.join("liangfen.dict.yaml").is_file() {
                prepare_android_auxiliary_dictionary(user_dir, schema_id)?
            } else {
                None
            };
        let result = if source.is_file() {
            deploy_schema_file(&source).and_then(|()| require_compiled_schema(&compiled, schema_id))
        } else {
            Err(format!("missing schema source: {}", source.display()))
        };
        if let Err(error) = result {
            remove_rime_build_artifact(&compiled)?;
            record_skipped_schema(user_dir, schema_id, &log, error)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn record_skipped_schema(
        user_dir: &Path,
        schema_id: &str,
        log: &deployment_log::DeploymentLog,
        error: String,
    ) -> Result<(), String> {
        let detail = log.first_error().unwrap_or(error);
        let mut skipped = skipped_schemas(user_dir);
        skipped.insert(
            schema_id.to_owned(),
            format!("方案 {schema_id} 编译失败，已跳过：{detail}\n已从切换列表移除"),
        );
        let content = serde_json::to_vec(&skipped).map_err(|error| error.to_string())?;
        write_file_atomic(
            &RuntimeDirs::resolve(user_dir, user_dir)?
                .staging_dir
                .join(SKIPPED_SCHEMAS_FILE),
            &content,
        )
        .map_err(|error| format!("save skipped schemas: {error}"))
    }

    #[test]
    fn skipped_schema_reports_first_error_and_clears_after_successful_deploy() {
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "keytao-skipped-warning-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(
            root.join("default.custom.yaml"),
            "patch: {schema_list: [{schema: keytao_broken}, {schema: keytao_good}]}",
        )
        .unwrap();
        for id in ["keytao_broken", "keytao_good"] {
            std::fs::write(
                root.join(format!("{id}.schema.yaml")),
                format!("schema: {{schema_id: {id}}}"),
            )
            .unwrap();
        }
        begin_deployment_with_invalidation(&root, false).unwrap();
        let log_path = root.join("rime.keytao.log.ERROR.20261120-020000.42.log");
        std::fs::write(
            &log_path,
            "E20261120 02:00:00.000001 123 config.cc:1] old error\n",
        )
        .unwrap();
        let log = deployment_log::DeploymentLog::capture(root.clone());
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .unwrap(),
            "E20261120 02:01:00.000001 123 config.cc:1] copy on write failed; incompatible node type: commit\nE20261120 02:01:01.000001 123 config.cc:2] error building config: keytao_broken.schema"
        )
        .unwrap();
        record_skipped_schema(&root, "keytao_broken", &log, "generic failure".into()).unwrap();
        assert_eq!(
            deployment_warning(&root).as_deref(),
            Some("方案 keytao_broken 编译失败，已跳过：copy on write failed; incompatible node type: commit\n已从切换列表移除")
        );
        std::fs::copy(
            root.join("keytao_good.schema.yaml"),
            root.join("build/keytao_good.schema.yaml"),
        )
        .unwrap();
        std::fs::write(
            root.join("build/default.yaml"),
            "schema_list: [{schema: keytao_broken}, {schema: keytao_good}]",
        )
        .unwrap();
        complete_deployment(&root).unwrap();
        let state = schema_install_state(&root);
        assert!(state.deployed);
        assert_eq!(state.schemas, ["keytao_good"]);
        assert_eq!(
            parse_schema_list(&std::fs::read_to_string(root.join("build/default.yaml")).unwrap()),
            ["keytao_good"]
        );
        begin_deployment_with_invalidation(&root, false).unwrap();
        std::fs::copy(
            root.join("keytao_broken.schema.yaml"),
            root.join("build/keytao_broken.schema.yaml"),
        )
        .unwrap();
        complete_deployment(&root).unwrap();
        assert_eq!(deployment_warning(&root), None);
        assert_eq!(
            schema_install_state(&root).schemas,
            ["keytao_broken", "keytao_good"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    fn require_any_compiled_schema(user_dir: &Path, schemas: &[String]) -> Result<(), String> {
        let build = RuntimeDirs::resolve(user_dir, user_dir)?.staging_dir;
        if effective_schema_list(user_dir, schemas.to_vec())
            .iter()
            .any(|id| build.join(format!("{id}.schema.yaml")).is_file())
        {
            Ok(())
        } else {
            Err(format!(
                "没有方案编译成功\n{}",
                deployment_warning(user_dir).unwrap_or_default()
            ))
        }
    }

    fn deploy_config_file(file_name: &str, version_key: &str) -> Result<(), String> {
        let _rime = rime_api_lock();
        let file_name = CString::new(file_name).map_err(|_| "invalid config file name")?;
        let version_key = CString::new(version_key).map_err(|_| "invalid config version key")?;
        unsafe {
            let api = rime_get_api();
            let deploy = (*api)
                .deploy_config_file
                .ok_or("librime deploy_config_file API is unavailable")?;
            if deploy(file_name.as_ptr(), version_key.as_ptr()) == 0 {
                return Err("failed to deploy default.yaml".into());
            }
        }
        Ok(())
    }

    fn deploy_schema_file(path: &Path) -> Result<(), String> {
        let _rime = rime_api_lock();
        let path_string = path.to_string_lossy();
        let path = CString::new(path_string.as_bytes()).map_err(|_| "invalid schema path")?;
        unsafe {
            let api = rime_get_api();
            let deploy = (*api)
                .deploy_schema
                .ok_or("librime deploy_schema API is unavailable")?;
            if deploy(path.as_ptr()) == 0 {
                return Err(format!("failed to deploy schema: {path_string}"));
            }
        }
        Ok(())
    }

    fn parse_schema_list_from_dir(user_dir: &Path) -> Vec<String> {
        let Ok(dirs) = RuntimeDirs::resolve(user_dir, user_dir) else {
            return Vec::new();
        };
        [
            user_dir.join("default.custom.yaml"),
            user_dir.join("default-custom.yaml"),
            user_dir.join("default.yaml"),
            dirs.staging_dir.join("default.yaml"),
        ]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|content| parse_schema_list(&content))
        .find(|schemas| !schemas.is_empty())
        .unwrap_or_default()
    }

    fn schema_dependencies_from_file(path: &Path) -> Vec<String> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        parse_schema_dependencies(&content)
    }

    fn compiled_schema_path(user_dir: &Path, schema_id: &str) -> Result<PathBuf, String> {
        Ok(RuntimeDirs::resolve(user_dir, user_dir)?
            .staging_dir
            .join(format!("{schema_id}.schema.yaml")))
    }

    fn require_compiled_schema(path: &Path, schema_id: &str) -> Result<(), String> {
        if path.is_file() {
            Ok(())
        } else {
            Err(format!(
                "schema {schema_id} was not compiled to {}",
                path.display()
            ))
        }
    }

    fn setup_rime(dirs: &RuntimeDirs, log_dir: &str, deployer_only: bool, grammar_user_dir: &Path) {
        let configured_log_dir = PathBuf::from(log_dir);
        let use_octagram = !deployer_only && wanxiang_grammar_enabled(grammar_user_dir);
        let log_dir = CString::new(log_dir).expect("valid log dir");
        let distribution_name = CString::new("KeyTao").unwrap();
        let distribution_code_name = CString::new("keytao").unwrap();
        let distribution_version = CString::new("1.0.0").unwrap();
        let module_default = CString::new("default").unwrap();
        let module_lua = CString::new("lua").unwrap();
        let module_octagram = CString::new("octagram").unwrap();
        let mut modules = vec![module_default.as_ptr(), module_lua.as_ptr()];
        if use_octagram {
            modules.push(module_octagram.as_ptr());
        }
        modules.push(std::ptr::null::<c_char>());

        dirs.with_rime_traits(|mut traits| {
            traits.log_dir = log_dir.as_ptr();
            traits.distribution_name = distribution_name.as_ptr();
            traits.distribution_code_name = distribution_code_name.as_ptr();
            traits.distribution_version = distribution_version.as_ptr();
            // glog retains this pointer after setup returns.
            traits.app_name = c"rime.keytao".as_ptr();
            traits.min_log_level = 1;
            traits.modules = if deployer_only {
                std::ptr::null_mut()
            } else {
                modules.as_mut_ptr()
            };

            unsafe {
                if !deployer_only {
                    require_levers_module();
                    require_lua_module();
                    #[cfg(not(target_os = "ios"))]
                    if use_octagram {
                        require_octagram_module();
                    }
                }

                let api = rime_get_api();
                {
                    let mut setup_complete = lock_ignore_poison(&RIME_SETUP_COMPLETE);
                    if !*setup_complete {
                        if let Some(setup) = (*api).setup {
                            prune_rime_logs(&configured_log_dir);
                            setup(&mut traits);
                            *lock_ignore_poison(&RIME_SETUP_LOG_DIR) = Some(configured_log_dir);
                        }
                        *setup_complete = true;
                    }
                }
                if deployer_only {
                    if let Some(initialize) = (*api).deployer_initialize {
                        initialize(&mut traits);
                    }
                } else if let Some(initialize) = (*api).initialize {
                    initialize(&mut traits);
                }
                #[cfg(target_os = "android")]
                if let Some(deployer_initialize) = (*api).deployer_initialize {
                    traits.modules = std::ptr::null_mut();
                    deployer_initialize(&mut traits);
                }
                if let Some(set_notification_handler) = (*api).set_notification_handler {
                    set_notification_handler(Some(notification_handler), std::ptr::null_mut());
                }
                // Windows librime returns ANSI bytes here, even when the input path was UTF-8.
                *lock_ignore_poison(&RIME_USER_DATA_DIR) = rime_api_member!(api, get_user_data_dir)
                    .and_then(|get| {
                        let current = get();
                        (!current.is_null()).then(|| {
                            (
                                PathBuf::from(dirs.user_data_dir.to_string_lossy().as_ref()),
                                CStr::from_ptr(current).to_bytes().to_vec(),
                            )
                        })
                    });
            }
        });
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    unsafe fn require_levers_module() {
        rime_require_module_levers();
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    unsafe fn require_levers_module() {}

    #[cfg(any(target_os = "android", target_os = "ios"))]
    unsafe fn require_lua_module() {
        rime_require_module_lua();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    unsafe fn require_lua_module() {
        if lua_module_registered() {
            return;
        }
        if let Err(error) = load_unix_lua_plugin() {
            eprintln!(
                "KeyTao: failed to load {} librime-lua plugin: {error}",
                std::env::consts::OS
            );
        }
    }

    #[cfg(target_os = "windows")]
    unsafe fn require_lua_module() {
        if lua_module_registered() {
            return;
        }
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

    pub(super) unsafe fn lua_module_registered() -> bool {
        let api = rime_get_api();
        rime_api_member!(api, find_module)
            .is_some_and(|find_module| !find_module(c"lua".as_ptr()).is_null())
    }

    #[cfg(not(target_os = "ios"))]
    unsafe fn octagram_module_registered() -> bool {
        let api = rime_get_api();
        rime_api_member!(api, find_module)
            .is_some_and(|find_module| !find_module(c"octagram".as_ptr()).is_null())
    }

    #[cfg(target_os = "android")]
    pub(super) unsafe fn octagram_available() -> bool {
        // The Android runtime statically links octagram; requiring it would register it.
        true
    }

    #[cfg(target_os = "macos")]
    pub(super) unsafe fn octagram_available() -> bool {
        // Check architecture and dependencies without running plugin constructors.
        octagram_module_registered()
            || octagram_plugin_candidates().iter().any(|path| {
                CString::new(path.to_string_lossy().as_bytes())
                    .is_ok_and(|path| dlopen_preflight(path.as_ptr()))
            })
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub(super) unsafe fn octagram_available() -> bool {
        // Merged plugins are already registered; file presence is a fallback only.
        octagram_module_registered()
            || octagram_plugin_candidates()
                .iter()
                .any(|path| path.is_file())
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn octagram_plugin_candidates() -> Vec<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let candidates = unix_lua_plugin_candidates();
        #[cfg(target_os = "windows")]
        let candidates = windows_lua_plugin_candidates();
        #[cfg(target_os = "macos")]
        let filenames = ["librime-octagram.dylib"];
        #[cfg(target_os = "linux")]
        let filenames = ["librime-octagram.so"];
        #[cfg(target_os = "windows")]
        let filenames = ["librime-octagram.dll", "rime-octagram.dll"];
        candidates
            .iter()
            .filter_map(|candidate| candidate.parent())
            .flat_map(|dir| filenames.iter().map(move |filename| dir.join(filename)))
            .collect()
    }

    #[cfg(target_os = "android")]
    pub(super) unsafe fn require_octagram_module() -> bool {
        rime_require_module_octagram();
        octagram_module_registered()
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    pub(super) unsafe fn require_octagram_module() -> bool {
        if octagram_module_registered() {
            return true;
        }
        static LOADED_PLUGINS: LazyLock<Mutex<HashSet<PathBuf>>> =
            LazyLock::new(|| Mutex::new(HashSet::new()));
        let mut loaded = lock_ignore_poison(&LOADED_PLUGINS);
        for path in octagram_plugin_candidates() {
            if loaded.contains(&path) || !path.is_file() {
                continue;
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let handle = {
                let Ok(path) = CString::new(path.to_string_lossy().as_bytes()) else {
                    continue;
                };
                #[cfg(target_os = "macos")]
                const RTLD_GLOBAL: i32 = 0x8;
                #[cfg(target_os = "linux")]
                const RTLD_GLOBAL: i32 = 0x100;
                dlopen(path.as_ptr(), 0x2 | RTLD_GLOBAL)
            };
            #[cfg(target_os = "windows")]
            let handle = LoadLibraryW(path_to_wide(&path).as_ptr());
            if handle.is_null() {
                continue;
            }
            loaded.insert(path);
            if octagram_module_registered() {
                return true;
            }
            for symbol in [
                c"_Z28rime_require_module_octagramv",
                c"?rime_require_module_octagram@@YAXXZ",
                c"rime_require_module_octagram",
            ] {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                let require = dlsym(handle, symbol.as_ptr());
                #[cfg(target_os = "windows")]
                let require = GetProcAddress(handle, symbol.as_ptr());
                if !require.is_null() {
                    let require: unsafe extern "C" fn() = std::mem::transmute(require);
                    require();
                    if octagram_module_registered() {
                        return true;
                    }
                }
            }
        }
        false
    }

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
            if lua_module_registered() {
                return Ok(());
            }
            if let Some(require) = find_unix_lua_require_symbol(handle) {
                let require: unsafe extern "C" fn() = std::mem::transmute(require);
                require();
                if lua_module_registered() {
                    return Ok(());
                }
            }
            attempted.push(format!(
                "{display}: Lua module did not register after loading"
            ));
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
            if lua_module_registered() {
                return Ok(());
            }
            if let Some(require) = find_windows_lua_require_symbol(handle) {
                let require: unsafe extern "C" fn() = std::mem::transmute(require);
                require();
                if lua_module_registered() {
                    return Ok(());
                }
            }
            attempted.push(format!(
                "{display}: Lua module did not register after loading"
            ));
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
        let address = current_windows_module_dir as *const () as usize as *const u16;
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
            let mut result = lock_ignore_poison(&DEPLOY_RESULT);
            match message_value.as_str() {
                "success" => *result = Some(true),
                "failure" => *result = Some(false),
                _ => {}
            }
        }
    }

    fn cstr_to_str(ptr: *const c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(ptr).to_str().ok().map(str::to_owned) }
    }

    fn prepare_deployment_log_directory() -> Result<(), String> {
        // RimeFinalize does not reset glog's process-wide log directory. A
        // previous user root may have been removed after switching runtimes;
        // librime 1.10's synchronous clean_old_log_files task would then throw
        // a C++ filesystem exception across the C API into Rust.
        if let Some(log_dir) = lock_ignore_poison(&RIME_SETUP_LOG_DIR).as_ref() {
            std::fs::create_dir_all(log_dir).map_err(|error| {
                format!(
                    "Rime deployment failed: cannot prepare process-wide librime log directory {}: {error}",
                    log_dir.display()
                )
            })?;
        }
        Ok(())
    }

    fn full_deploy_and_wait(user_dir: &Path) -> Result<bool, String> {
        let _rime = rime_api_lock();
        prepare_deployment_log_directory()?;
        {
            let mut result = lock_ignore_poison(&DEPLOY_RESULT);
            *result = None;
        }
        // Keep the runtime-only grammar block out of librime's prism checksum.
        update_neutral_grammar(user_dir, false);
        let result = (|| unsafe {
            let api = rime_get_api();
            let Some(start_maintenance) = (*api).start_maintenance else {
                return Ok(false);
            };
            if start_maintenance(1) == 0 {
                return Ok(false);
            }
            if let Some(join_maintenance_thread) = (*api).join_maintenance_thread {
                join_maintenance_thread();
            }
            Ok(*lock_ignore_poison(&DEPLOY_RESULT) == Some(true))
        })();
        patch_neutral_grammar(user_dir);
        result
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    pub(super) fn sync_user_data_and_wait(user_dir: &Path) -> Result<(), String> {
        let _deploy = lock_ignore_poison(&DEPLOY_LOCK);
        let _rime = rime_api_lock();
        *lock_ignore_poison(&DEPLOY_RESULT) = None;
        // librime 1.17.0 rime_api_impl.h:127-134 schedules installation_update,
        // backup_config_files and user_dict_sync only; no workspace deployment.
        unsafe {
            let api = rime_get_api();
            let sync = rime_api_member!(api, sync_user_data)
                .ok_or("librime sync_user_data API is unavailable")?;
            let join = rime_api_member!(api, join_maintenance_thread)
                .ok_or("librime join_maintenance_thread API is unavailable")?;
            let initialize_deployer = rime_api_member!(api, deployer_initialize)
                .ok_or("librime deployer_initialize API is unavailable")?;
            // NULL preserves the configured paths and only loads task modules.
            initialize_deployer(std::ptr::null_mut());
            if sync() == 0 {
                return Err("Rime user data sync could not start".into());
            }
            join();
        }
        initialize_user_dictionary_cache(user_dir);
        // Deployer::Run aggregates task failures into the deploy notification.
        if *lock_ignore_poison(&DEPLOY_RESULT) != Some(true) {
            return Err("Rime user data sync failed".into());
        }
        Ok(())
    }

    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    mod unix_lua_plugin_tests {
        use super::*;

        #[test]
        fn loads_lua_plugin_from_configured_rime_lib_dir() {
            if std::env::var("RIME_LIB_DIR").is_err() {
                return;
            }
            let _rime = rime_api_lock();
            unsafe {
                if !lua_module_registered() {
                    load_unix_lua_plugin().expect("load librime-lua plugin");
                }
            }
            assert!(crate::librime_lua_available());
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn detects_octagram_plugin_from_configured_rime_lib_dir() {
            let Ok(lib_dir) = std::env::var("RIME_LIB_DIR") else {
                return;
            };
            if Path::new(&lib_dir)
                .join("rime-plugins/librime-octagram.dylib")
                .is_file()
            {
                let _rime = rime_api_lock();
                let registered = unsafe { octagram_module_registered() };
                assert!(crate::octagram_available());
                assert_eq!(unsafe { octagram_module_registered() }, registered);
            }
        }
    }

    struct UserDictionaryCache {
        user_data_dir: Option<PathBuf>,
        entries: HashMap<String, HashSet<String>>,
        source_available: bool,
    }

    static USER_DICTIONARY_CACHE: LazyLock<Mutex<UserDictionaryCache>> = LazyLock::new(|| {
        Mutex::new(UserDictionaryCache {
            user_data_dir: None,
            entries: HashMap::new(),
            source_available: false,
        })
    });

    #[derive(Default)]
    struct SchemaKeyPolicy {
        schema_id: String,
        chord_composer: bool,
        shortcuts: Vec<SchemaShortcut>,
        modifier_taps: Vec<u32>,
        switch_names: Option<Vec<String>>,
    }

    /// An active rime input session.
    pub struct RimeBackend {
        session: ManuallyDrop<rime_api::Session>,
        schema_keys: RefCell<SchemaKeyPolicy>,
        switcher_hotkeys: Vec<SchemaShortcut>,
        user_data_dir: Option<PathBuf>,
        user_dictionary_source_available: bool,
    }

    // SAFETY: Session holds only a usize (session_id) and librime itself is not
    // thread-safe, so every method — including `Drop`, which destroys the
    // session — serializes on `RIME_API_LOCK` before touching rime_api or the
    // per-session chord cache.
    unsafe impl Send for RimeBackend {}
    unsafe impl Sync for RimeBackend {}

    impl Drop for RimeBackend {
        fn drop(&mut self) {
            let _rime = rime_api_lock();
            // SAFETY: `session` is initialized in `new_with_user_data_dir` and
            // dropped exactly once, here, while the librime lock is held.
            unsafe { ManuallyDrop::drop(&mut self.session) };
        }
    }

    fn key_event(keycode: u32, mask: u32) -> KeyEvent {
        KeyEvent::new(keycode as _, mask as _)
    }

    /// Which of the candidate, paging and composition entry points the linked
    /// librime actually exports.
    pub(crate) fn engine_capabilities() -> EngineCapabilities {
        let _rime = rime_api_lock();
        // SAFETY: the librime lock is held and only the function pointers of
        // the static API table are read, each one gated on `data_size`.
        unsafe {
            let api = rime_get_api();
            if api.is_null() {
                return EngineCapabilities::none();
            }

            let native_paging = rime_api_member!(api, change_page).is_some();
            let candidate_highlight =
                rime_api_member!(api, highlight_candidate_on_current_page).is_some();

            EngineCapabilities {
                candidate_selection: rime_api_member!(api, select_candidate_on_current_page)
                    .is_some(),
                global_candidate_selection: rime_api_member!(api, select_candidate).is_some(),
                candidate_highlight,
                candidate_deletion: rime_api_member!(api, delete_candidate_on_current_page)
                    .is_some(),
                native_paging,
                commit_composition: rime_api_member!(api, commit_composition).is_some(),
                clear_composition: rime_api_member!(api, clear_composition).is_some(),
            }
        }
    }

    impl RimeBackend {
        pub(crate) fn prepare_switch_request(
            root: &Path,
        ) -> Result<Option<SchemaSwitchRequest>, String> {
            let schema_root = loaded_schema_root(root);
            let request = schema_switch_request::pending_for_process(&schema_root)?;
            if let Some(requested) = request.as_ref() {
                let dirs = loaded_runtime().filter(|dirs| dirs.active_package_key.is_some());
                let schema_id = requested.schema_id.clone().or_else(|| match &dirs {
                    Some(dirs) => resolved_deployed_schema_id(dirs),
                    None => deployed_schema_id_from_dir(root),
                });
                let Some(schema_id) = schema_id else {
                    return Ok(request);
                };
                if requested.schema_id.is_some()
                    && !dirs
                        .as_ref()
                        .map_or_else(|| deployed_schemas(root), resolved_deployed_schemas)
                        .iter()
                        .any(|schema| schema.id == schema_id)
                {
                    return Err(format!("schema {schema_id} is not deployed"));
                }
                let _rime = rime_api_lock();
                persist_requested_switches_locked(&schema_id, requested)?;
                if let Some(schema_id) = requested.schema_id.as_deref() {
                    persist_selected_schema(&schema_root, schema_id)?;
                }
            }
            Ok(request)
        }

        pub(crate) fn new_with_user_data_dir(user_data_dir: &Path) -> Result<Self, String> {
            // Locating and checking the compiled schema is file I/O; the
            // librime lock is only taken once the session is actually created.
            let dirs = match loaded_runtime() {
                Some(dirs)
                    if dirs.active_package_key.is_some() || dirs.user_data_dir == user_data_dir =>
                {
                    dirs
                }
                _ => RuntimeDirs::resolve(user_data_dir, user_data_dir)?,
            };
            let preferred = resolved_deployed_schema_id(&dirs);
            if let Some(schema_id) = &preferred {
                require_compiled_schema(
                    &dirs.staging_dir.join(format!("{schema_id}.schema.yaml")),
                    schema_id,
                )?;
            }
            let user_dictionary_source_available =
                user_dictionary_cache_available_for(Some(&dirs.user_data_dir));
            let _rime = rime_api_lock();
            #[cfg(all(
                test,
                any(target_os = "linux", target_os = "windows", target_os = "macos")
            ))]
            super::runtime_access_tests::pause_engine_creation();
            let session = create_session().map_err(|e| format!("{e:?}"))?;
            if let Some(schema_id) = &preferred {
                // ascii_mode is left to the schema switches and to ascii_composer;
                // a session must not silently override the user's mode.
                if USER_CONFIG_NEEDS_FLUSH.load(Ordering::Relaxed)
                    || !session
                        .status()
                        .is_ok_and(|status| !status.is_disabled && status.schema_id() == schema_id)
                {
                    select_schema_checked(&session, schema_id)?;
                    USER_CONFIG_NEEDS_FLUSH.store(false, Ordering::Relaxed);
                }
            } else {
                validate_active_schema(&session)?;
            }
            Ok(Self {
                session: ManuallyDrop::new(session),
                schema_keys: RefCell::new(SchemaKeyPolicy::default()),
                switcher_hotkeys: switcher_hotkeys_locked(),
                user_data_dir: Some(dirs.user_data_dir),
                user_dictionary_source_available,
            })
        }

        fn user_dictionary_contains(&self, dictionaries: &[String], text: &str) -> bool {
            if !self.user_dictionary_source_available {
                return false;
            }
            let cache = lock_ignore_poison(&USER_DICTIONARY_CACHE);
            if cache.user_data_dir.as_deref() != self.user_data_dir.as_deref()
                || !cache.source_available
            {
                return false;
            }
            dictionaries.iter().any(|dictionary| {
                cache
                    .entries
                    .get(dictionary)
                    .is_some_and(|words| words.contains(text))
            })
        }

        /// Fallback for ABIs without `select_candidate_on_current_page`.
        fn send_select_key(&self, index: usize) {
            let select_keys = session_select_keys(&self.session);
            let select_keys = select_keys.as_deref().unwrap_or(DEFAULT_SELECT_KEYS);
            if let Some(key) = select_keys.chars().nth(index) {
                self.session.process_key(key_event(key as u32, 0));
            }
        }

        fn remember_committed_user_phrase(&self, state: &ImeState) {
            if !self.user_dictionary_source_available {
                return;
            }
            let Some(committed) = state.committed.as_ref().filter(|text| !text.is_empty()) else {
                return;
            };
            let Ok(status) = self.session.status() else {
                return;
            };
            let dictionaries = schema_user_dictionary_names_locked(status.schema_id());
            let mut cache = lock_ignore_poison(&USER_DICTIONARY_CACHE);
            if !cache.source_available
                || cache.user_data_dir.as_deref() != self.user_data_dir.as_deref()
            {
                return;
            }
            for dictionary in dictionaries {
                cache
                    .entries
                    .entry(dictionary)
                    .or_default()
                    .insert(committed.clone());
            }
        }

        fn clear_composition_impl(&self, consume_commit: bool) -> ImeState {
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held; a missing pointer falls back to
            // the historical synthetic key.
            let handled = unsafe {
                let api = rime_get_api();
                rime_api_member!(api, clear_composition).map(|clear| {
                    clear(self.session.session_id);
                })
            };
            if handled.is_none() {
                self.session
                    .process_key(key_event(key_policy::XK_ESCAPE, 0));
            }
            if consume_commit {
                extract_state_with_commit(self)
            } else {
                extract_state_readonly(self)
            }
        }

        fn raw_input_locked(&self) -> Option<String> {
            // SAFETY: the librime lock is held; the pointer is checked and the
            // returned buffer is owned by librime and only read here.
            unsafe {
                let api = rime_get_api();
                let get_input = rime_api_member!(api, get_input)?;
                cstr_to_str(get_input(self.session.session_id))
            }
        }

        fn schema_key_policy(&self, schema_id: &str) -> std::cell::RefMut<'_, SchemaKeyPolicy> {
            let mut cached = self.schema_keys.borrow_mut();
            if cached.schema_id != schema_id {
                *cached = schema_key_policy_locked(schema_id);
                cached.schema_id = schema_id.into();
                cached.shortcuts.extend_from_slice(&self.switcher_hotkeys);
            }
            cached
        }

        fn switches_before_shortcut(&self, keycode: u32, mask: u32) -> Vec<SchemaSwitchSetting> {
            if mask & RIME_RELEASE_MASK != 0 {
                return Vec::new();
            }
            let mut cached = self.schema_keys.borrow_mut();
            if cached.schema_id.is_empty() {
                if let Ok(status) = self.session.status() {
                    *cached = schema_key_policy_locked(status.schema_id());
                    cached.schema_id = status.schema_id().into();
                    cached.shortcuts.extend_from_slice(&self.switcher_hotkeys);
                }
            }
            if !cached
                .shortcuts
                .iter()
                .any(|shortcut| !shortcut.is_switcher() && shortcut.matches_key(keycode, mask))
            {
                return Vec::new();
            }
            visible_switch_settings(
                &cached.schema_id,
                read_schema_switches_locked(&cached.schema_id),
                |name, _| Some(get_session_option(&self.session, name)),
            )
            .into_iter()
            .filter(|switch| switch.name != "ascii_mode")
            .collect()
        }
    }

    impl ImeBackend for RimeBackend {
        fn input_state(&self) -> global_input::InputState {
            let _rime = rime_api_lock();
            let Ok(status) = self.session.status() else {
                return global_input::InputState::default();
            };
            let schema_id = status.schema_id();
            let mut cached = self.schema_key_policy(schema_id);
            if cached.switch_names.is_none() {
                cached.switch_names = try_read_schema_switches_locked(schema_id)
                    .map(|switches| english_mode::schema_option_names(switches).collect());
            }
            global_input::InputState {
                schema_id: schema_id.into(),
                ascii_mode: status.is_ascii_mode,
                options: cached
                    .switch_names
                    .iter()
                    .flatten()
                    .map(|name| (name.clone(), get_session_option(&self.session, name)))
                    .collect(),
            }
        }

        fn switcher_hotkeys(&self) -> &[SchemaShortcut] {
            &self.switcher_hotkeys
        }

        fn has_declared_shortcut(&self, keycode: u32, mask: u32) -> bool {
            self.schema_keys.borrow().shortcuts.iter().any(|shortcut| {
                (shortcut.key, shortcut.modifiers) == (keycode, mask)
            })
        }

        fn shift_switch_disabled(&self, schema: &str, keycode: u32) -> bool {
            let _rime = rime_api_lock();
            let path = if keycode == 0xffe1 {
                "ascii_composer/switch_key/Shift_L"
            } else {
                "ascii_composer/switch_key/Shift_R"
            };
            let Ok(schema) = CString::new(schema) else {
                return false;
            };
            read_rime_config_locked(&schema, true, |config| {
                ascii_switch_disabled_in_config(config, path)
            })
            .flatten()
            .or_else(|| {
                read_rime_config_locked(c"default", false, |config| {
                    ascii_switch_disabled_in_config(config, path)
                })
                .flatten()
            })
            .unwrap_or(false)
        }

        fn supports_native_inline_ascii(&self) -> bool {
            true
        }

        /// What this librime can do natively, see [`EngineCapabilities`].
        fn capabilities(&self) -> EngineCapabilities {
            let mut capabilities = engine_capabilities();
            capabilities.candidate_deletion &= self.user_dictionary_source_available;
            capabilities
        }

        fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult {
            let started = runtime_log::enabled(Level::Info).then(Instant::now);
            let _rime = rime_api_lock();
            let switches = self.switches_before_shortcut(keycode, mask);
            let status = self.session.process_key(key_event(keycode, mask));
            let mut state = extract_state_with_commit(self);
            if matches!(status, KeyStatus::Accept) {
                state.switch_hint = switches
                    .into_iter()
                    .filter(|switch| switch.schema_id == state.schema_id)
                    .find_map(|switch| {
                        let get = |name| get_session_option(&self.session, name);
                        let value = if switch.options.is_empty() {
                            usize::from(get(switch.name.as_str()))
                        } else {
                            switch.options.iter().position(|name| get(name))?
                        };
                        (value != switch.value)
                            .then(|| &switch.states[value])
                            .filter(|label| !label.trim().is_empty())
                            .cloned()
                    });
            }
            self.remember_committed_user_phrase(&state);
            drop(_rime);
            let result = KeyProcessResult {
                state,
                accepted: matches!(status, KeyStatus::Accept),
            };
            if let Some(started) = started {
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                runtime_log::record_engine_key(&result.state.schema_id, ms);
                if ms > 50.0 {
                    crate::rt_log!(
                        Level::Info,
                        "rime",
                        "slow_key",
                        dur_ms = ms,
                        accepted = result.accepted,
                        preedit_len = result.state.preedit.len(),
                        cand = result.state.candidates.len()
                    );
                }
                crate::rt_log!(
                    Level::Verbose,
                    "rime",
                    "process_key",
                    dur_ms = ms,
                    accepted = result.accepted,
                    preedit_len = result.state.preedit.len(),
                    cand = result.state.candidates.len()
                );
            }
            result
        }

        /// Pick the `index`-th candidate of the current page through librime's
        /// own API, so that selection does not depend on `menu/select_keys`
        /// being long enough or on the schema's alphabet.
        fn select_candidate_on_page(&self, index: usize) -> ImeState {
            let started = runtime_log::enabled(Level::Info).then(Instant::now);
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held; the entry point is optional in
            // older ABIs, so a missing pointer falls back to the select key.
            let handled = unsafe {
                let api = rime_get_api();
                rime_api_member!(api, select_candidate_on_current_page)
                    .map(|select| select(self.session.session_id, index))
            };
            if handled.is_none() {
                self.send_select_key(index);
            }
            let state = extract_state_with_commit(self);
            self.remember_committed_user_phrase(&state);
            drop(_rime);
            if let Some(started) = started {
                crate::rt_log!(
                    Level::Info,
                    "rime",
                    "select_candidate",
                    dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                    index = index,
                    global = false
                );
            }
            state
        }

        /// Move the highlight without committing, for hover/arrow interactions.
        fn highlight_candidate_on_page(&self, index: usize) -> ImeState {
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held; the pointer is checked.
            unsafe {
                let api = rime_get_api();
                if let Some(highlight) = rime_api_member!(api, highlight_candidate_on_current_page)
                {
                    highlight(self.session.session_id, index);
                }
            }
            extract_state_with_commit(self)
        }

        /// Whether the `index`-th candidate on the current page is backed by an
        /// entry in one of the active schema's user dictionaries.
        fn candidate_is_user_phrase_on_page(&self, index: usize) -> bool {
            let candidate = {
                let _rime = rime_api_lock();
                let state = extract_state_readonly(self);
                let Some(candidate) = state.candidates.get(index) else {
                    return false;
                };
                let Ok(status) = self.session.status() else {
                    return false;
                };
                (
                    candidate.text.clone(),
                    schema_user_dictionary_names_locked(status.schema_id()),
                )
            };
            self.user_dictionary_contains(&candidate.1, &candidate.0)
        }

        /// Forget the `index`-th candidate of the current page, but only after
        /// the active user dictionary confirms that it is a learned phrase.
        fn delete_candidate_on_page_result(&self, index: usize) -> (ImeState, bool) {
            let (before, candidate_text, dictionaries) = {
                let _rime = rime_api_lock();
                let before = extract_state_readonly(self);
                let Some(candidate_text) = before
                    .candidates
                    .get(index)
                    .map(|candidate| candidate.text.clone())
                else {
                    return (before, false);
                };
                let Ok(status) = self.session.status() else {
                    return (extract_state_readonly(self), false);
                };
                let dictionaries = schema_user_dictionary_names_locked(status.schema_id());
                (before, candidate_text, dictionaries)
            };
            if !self.user_dictionary_contains(&dictionaries, &candidate_text) {
                return (before, false);
            }

            let _rime = rime_api_lock();
            let current = extract_state_readonly(self);
            if current
                .candidates
                .get(index)
                .map(|candidate| candidate.text.as_str())
                != Some(candidate_text.as_str())
            {
                return (current, false);
            }
            // SAFETY: the librime lock is held; the pointer is checked.
            let deleted = unsafe {
                let api = rime_get_api();
                if let Some(delete) = rime_api_member!(api, delete_candidate_on_current_page) {
                    delete(self.session.session_id, index) != 0
                } else {
                    false
                }
            };
            if deleted {
                let mut cache = lock_ignore_poison(&USER_DICTIONARY_CACHE);
                if cache.user_data_dir.as_deref() == self.user_data_dir.as_deref() {
                    for dictionary in dictionaries {
                        if let Some(words) = cache.entries.get_mut(&dictionary) {
                            words.remove(&candidate_text);
                        }
                    }
                }
            }
            (extract_state_with_commit(self), deleted)
        }

        fn select_candidate_global(&self, index: usize) -> ImeState {
            let started = runtime_log::enabled(Level::Info).then(Instant::now);
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held; the pointer is checked.
            unsafe {
                let api = rime_get_api();
                if let Some(select_candidate) = rime_api_member!(api, select_candidate) {
                    select_candidate(self.session.session_id, index);
                }
            }
            let state = extract_state_with_commit(self);
            self.remember_committed_user_phrase(&state);
            drop(_rime);
            if let Some(started) = started {
                crate::rt_log!(
                    Level::Info,
                    "rime",
                    "select_candidate",
                    dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                    index = index,
                    global = true
                );
            }
            state
        }

        fn all_candidates_limited(&self, max_count: usize) -> Vec<Candidate> {
            let _rime = rime_api_lock();
            extract_all_candidates(&self.session, max_count).unwrap_or_default()
        }

        /// Turn a candidate page through librime instead of replaying the
        /// `-`/`=` bindings, which only exist when a schema imports the default
        /// `key_binder` paging preset.
        fn change_page(&self, backward: bool) -> ImeState {
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held; a missing pointer falls back to
            // the historical synthetic key.
            let handled = unsafe {
                let api = rime_get_api();
                rime_api_member!(api, change_page)
                    .map(|change_page| change_page(self.session.session_id, i32::from(backward)))
                    .is_some()
            };
            if !handled {
                let kc = if backward { b'-' as u32 } else { b'=' as u32 };
                self.session.process_key(key_event(kc, 0));
            }
            extract_state_with_commit(self)
        }

        /// Drop the composition without committing anything.
        ///
        /// Kept as the historical name of [`RimeBackend::clear_composition`].
        fn reset(&self) -> ImeState {
            self.clear_composition()
        }

        /// Discard the composition through librime, so that the outcome does
        /// not depend on how a schema bound `Escape`.
        fn clear_composition(&self) -> ImeState {
            self.clear_composition_impl(true)
        }

        /// Commit whatever librime currently holds, the way a frontend has to
        /// finish a composition when the input context goes away.
        fn commit_composition(&self) -> ImeState {
            let started = runtime_log::enabled(Level::Info).then(Instant::now);
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held; a missing pointer falls back to
            // handing Return to the schema's editor.
            let handled = unsafe {
                let api = rime_get_api();
                rime_api_member!(api, commit_composition)
                    .map(|commit| commit(self.session.session_id))
            };
            if handled.is_none() {
                self.session
                    .process_key(key_event(key_policy::XK_RETURN, 0));
            }
            let state = extract_state_with_commit(self);
            drop(_rime);
            if let Some(started) = started {
                crate::rt_log!(
                    Level::Info,
                    "rime",
                    "commit",
                    dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                    committed_len = state.committed.as_ref().map_or(0, String::len)
                );
            }
            state
        }

        /// The raw input string librime is composing from, before any editor or
        /// filter rewrote it for display.
        fn raw_input(&self) -> Option<String> {
            let _rime = rime_api_lock();
            self.raw_input_locked()
        }

        /// Commit the raw input and drop the composition.
        ///
        /// This is the documented fallback for `Return` when the schema's
        /// editor passes the key back: the typed code must reach the client
        /// instead of being thrown away with the composition.
        fn commit_raw_input(&self) -> ImeState {
            let _rime = rime_api_lock();
            let raw = self.raw_input_locked().unwrap_or_default();
            if raw.is_empty() {
                return extract_state_with_commit(self);
            }
            let mut state = self.clear_composition();
            let pending = state.committed.take();
            state.committed = Some(match pending {
                Some(pending) => format!("{pending}{raw}"),
                None => raw,
            });
            state
        }

        /// `Return` handling shared by every frontend: librime decides first,
        /// and only when it passes the key while a composition exists does the
        /// raw input get committed (see [`RimeBackend::commit_raw_input`]).
        fn process_enter(&self) -> KeyProcessResult {
            let _rime = rime_api_lock();
            let composing = !extract_state_readonly(self).preedit.is_empty();
            let result = self.process_key_result(key_policy::XK_RETURN, 0);
            if result.accepted || !composing {
                return result;
            }
            let mut state = self.commit_raw_input();
            // Whatever librime had already produced still has to reach the
            // client, and it was produced before the raw input.
            if let Some(pending) = result.state.committed {
                let raw = state.committed.take();
                state.committed = Some(match raw {
                    Some(raw) => format!("{pending}{raw}"),
                    None => pending,
                });
            }
            KeyProcessResult {
                state,
                accepted: true,
            }
        }

        fn list_schemas(&self) -> Vec<SchemaInfo> {
            let Some(user_data_dir) = self.user_data_dir.as_deref() else {
                return Vec::new();
            };
            if let Some(dirs) = loaded_runtime().filter(|dirs| {
                dirs.active_package_key.is_some() && dirs.user_data_dir == user_data_dir
            }) {
                return resolved_deployed_schemas(&dirs);
            }
            deployed_schemas(user_data_dir)
        }

        fn active_switches(&self) -> Vec<SchemaSwitchSetting> {
            let _rime = rime_api_lock();
            let Ok(status) = self.session.status() else {
                return Vec::new();
            };
            let schema_id = status.schema_id().to_owned();
            if schema_id.is_empty() || schema_id == ".default" {
                return Vec::new();
            }
            let switches = read_schema_switches_locked(&schema_id);
            use crate::english_mode::EnglishBackend as _;
            visible_switch_settings(&schema_id, switches, |name, _| Some(self.get_option(name)))
        }

        fn persist_switch_options(&self, options: &[(String, bool)]) {
            if options.is_empty() {
                return;
            }
            let _rime = rime_api_lock();
            let Ok(status) = self.session.status() else {
                return;
            };
            let schema_id = status.schema_id().to_owned();
            if schema_id.is_empty() || schema_id == ".default" {
                return;
            }
            let request = SchemaSwitchRequest {
                generation: 0,
                schema_id: None,
                options: options.iter().cloned().collect(),
            };
            if let Err(error) = persist_requested_switches_locked(&schema_id, &request) {
                crate::rt_log!(Level::Info, "error", "persist_switch_options", msg = error);
            }
        }

        fn apply_switch_request(
            &self,
            request: &SchemaSwitchRequest,
            reset_only: bool,
        ) -> Result<(), String> {
            let _rime = rime_api_lock();
            let status = self
                .session
                .status()
                .map_err(|error| format!("read switch schema: {error:?}"))?;
            for switch in read_schema_switches_locked(status.schema_id()) {
                if reset_only && switch.reset.is_none() {
                    continue;
                }
                if !switch.options.is_empty() {
                    if let Some(selected) = switch
                        .options
                        .iter()
                        .position(|name| request.options.get(name) == Some(&true))
                    {
                        for (index, name) in switch.options.iter().enumerate() {
                            set_session_option(&self.session, name, index == selected);
                        }
                        continue;
                    }
                }
                for name in switch.name.iter().chain(switch.options.iter()) {
                    if let Some(&enabled) = request.options.get(name) {
                        set_session_option(&self.session, name, enabled);
                    }
                }
            }
            Ok(())
        }

        fn apply_reset_switch_request(&self, root: &Path) -> Result<(), String> {
            // A schema reset overrides save_options whenever librime loads it.
            self.apply_switch_request(&schema_switch_request::read_at(root)?, true)
        }

        fn is_ascii_mode(&self) -> bool {
            let _rime = rime_api_lock();
            self.session
                .status()
                .map(|s| s.is_ascii_mode)
                .unwrap_or(false)
        }

        fn is_composing(&self) -> bool {
            let _rime = rime_api_lock();
            self.session
                .status()
                .map(|s| s.is_composing)
                .unwrap_or(true)
        }
    }

    impl EnglishBackend for RimeBackend {
        /// Read-only snapshot: `RimeGetCommit` consumes the pending commit, so a
        /// query must never call it or the text would never reach the client.
        fn state(&self) -> ImeState {
            let _rime = rime_api_lock();
            extract_state_readonly(self)
        }

        /// Clear preedit without consuming a pending commit during a schema
        /// transition that can still fail and roll back.
        fn discard_composition(&self) {
            self.clear_composition_impl(false);
        }

        fn current_schema(&self) -> Option<SchemaInfo> {
            let _rime = rime_api_lock();
            self.session.status().ok().map(|status| SchemaInfo {
                id: status.schema_id().to_string(),
                name: status.schema_name().to_string(),
            })
        }

        fn schema_switches(&self) -> Vec<SchemaSwitch> {
            let _rime = rime_api_lock();
            let Ok(status) = self.session.status() else {
                return Vec::new();
            };
            read_schema_switches_locked(status.schema_id())
        }

        fn select_schema(&self, schema_id: &str) -> Result<ImeState, String> {
            let _rime = rime_api_lock();
            select_schema_checked(&self.session, schema_id)?;
            Ok(extract_state_readonly(self))
        }

        /// Set the session option without reading any state back, so that a
        /// pending commit stays pending (see [`RimeBackend::state`]).
        fn apply_ascii_mode(&self, enabled: bool) {
            let _rime = rime_api_lock();
            set_session_option(&self.session, "ascii_mode", enabled);
        }

        fn set_ascii_mode(&self, enabled: bool) -> ImeState {
            let _rime = rime_api_lock();
            set_session_option(&self.session, "ascii_mode", enabled);
            extract_state_with_commit(self)
        }

        fn get_option(&self, option_name: &str) -> bool {
            let _rime = rime_api_lock();
            get_session_option(&self.session, option_name)
        }

        fn set_option(&self, option_name: &str, enabled: bool) -> ImeState {
            let _rime = rime_api_lock();
            set_session_option(&self.session, option_name, enabled);
            extract_state_readonly(self)
        }
    }

    fn select_schema_checked(session: &rime_api::Session, schema_id: &str) -> Result<(), String> {
        session
            .select_schema(schema_id)
            .map_err(|error| format!("failed to select schema {schema_id}: {error:?}"))?;
        let status = session
            .status()
            .map_err(|error| format!("read schema {schema_id} status: {error:?}"))?;
        if status.is_disabled {
            return Err(format!("schema {schema_id} is disabled after selection"));
        }
        if status.schema_id() != schema_id {
            return Err(format!(
                "selected schema mismatch: expected {schema_id}, got {}",
                status.schema_id()
            ));
        }
        Ok(())
    }

    fn validate_active_schema(session: &rime_api::Session) -> Result<(), String> {
        let status = session
            .status()
            .map_err(|error| format!("read active schema status: {error:?}"))?;
        if status.is_disabled || status.schema_id().is_empty() {
            return Err("Rime session has no active schema".into());
        }
        Ok(())
    }

    fn set_session_option(session: &rime_api::Session, option_name: &str, enabled: bool) {
        let Ok(option) = CString::new(option_name) else {
            return;
        };
        unsafe {
            let api = rime_get_api();
            if let Some(set_option) = rime_api_member!(api, set_option) {
                set_option(session.session_id, option.as_ptr(), i32::from(enabled));
            }
        }
    }

    fn get_session_option(session: &rime_api::Session, option_name: &str) -> bool {
        let Ok(option) = CString::new(option_name) else {
            return false;
        };
        unsafe {
            let api = rime_get_api();
            rime_api_member!(api, get_option)
                .map(|get_option| get_option(session.session_id, option.as_ptr()) != 0)
                .unwrap_or(false)
        }
    }

    fn read_rime_config_locked<T>(
        id: &CStr,
        schema: bool,
        read: impl FnOnce(&mut RimeConfig) -> T,
    ) -> Option<T> {
        unsafe {
            let api = rime_get_api();
            let open = if schema {
                rime_api_member!(api, schema_open)?
            } else {
                rime_api_member!(api, config_open)?
            };
            let close = rime_api_member!(api, config_close)?;
            let mut config: RimeConfig = std::mem::zeroed();
            if open(id.as_ptr(), &mut config) == 0 {
                return None;
            }
            let value = read(&mut config);
            close(&mut config);
            Some(value)
        }
    }

    fn ascii_switch_disabled_in_config(config: &mut RimeConfig, path: &str) -> Option<bool> {
        if let Some(style) = config_string_locked(config, path) {
            return Some(!matches!(
                style.as_str(),
                "inline_ascii"
                    | "commit_code"
                    | "commit_text"
                    | "clear"
                    | "set_ascii_mode"
                    | "unset_ascii_mode"
            ));
        }
        unsafe {
            let api = rime_get_api();
            let begin = rime_api_member!(api, config_begin_map)?;
            let end = rime_api_member!(api, config_end)?;
            let mut iterator: RimeConfigIterator = std::mem::zeroed();
            if begin(&mut iterator, config, c"ascii_composer/switch_key".as_ptr()) == 0 {
                return None;
            }
            end(&mut iterator);
            // A schema switch_key map replaces the preset map in ascii_composer.
            Some(true)
        }
    }

    fn modifier_taps_in_config(config: &mut RimeConfig) -> Option<Vec<u32>> {
        let mut taps = Vec::new();
        for (key, path) in [
            (0xffe3, "ascii_composer/switch_key/Control_L"),
            (0xffe4, "ascii_composer/switch_key/Control_R"),
            (0xffe9, "ascii_composer/switch_key/Alt_L"),
            (0xffea, "ascii_composer/switch_key/Alt_R"),
        ] {
            if !ascii_switch_disabled_in_config(config, path)? {
                taps.push(key);
            }
        }
        Some(taps)
    }

    fn schema_key_policy_locked(schema_id: &str) -> SchemaKeyPolicy {
        let Ok(schema_id) = CString::new(schema_id) else {
            return SchemaKeyPolicy::default();
        };
        read_rime_config_locked(&schema_id, true, |config| {
            let processors = config_string_list_locked(config, "engine/processors");
            let has_chord = processors
                .iter()
                .any(|processor| processor.split('@').next() == Some("chord_composer"));
            let mut shortcuts = if processors
                .iter()
                .any(|processor| processor.split('@').next() == Some("key_binder"))
            {
                schema_switch_shortcuts_locked(config)
            } else {
                Vec::new()
            };
            if processors
                .iter()
                .any(|processor| processor.split('@').next() == Some("lua_processor"))
            {
                // Lua candidate actions declare their keys outside key_binder.
                for path in ["pinned_candidate/hotkey", "candidate_remove/hotkey"] {
                    if let Some(key) = config_string_locked(config, path) {
                        shortcuts.extend(SchemaShortcut::parse(&key, "has_menu"));
                    }
                }
            }
            let modifier_taps = if processors
                .iter()
                .any(|processor| processor.split('@').next() == Some("ascii_composer"))
            {
                modifier_taps_in_config(config)
                    .or_else(|| {
                        read_rime_config_locked(c"default", false, modifier_taps_in_config)
                            .flatten()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            SchemaKeyPolicy {
                chord_composer: has_chord,
                shortcuts,
                modifier_taps,
                ..SchemaKeyPolicy::default()
            }
        })
        .unwrap_or_default()
    }

    fn switcher_hotkeys_locked() -> Vec<SchemaShortcut> {
        read_rime_config_locked(c"default", false, |config| {
            config_string_list_locked(config, "switcher/hotkeys")
                .iter()
                .filter_map(|key| SchemaShortcut::parse_switcher(key))
                .collect()
        })
        .unwrap_or_default()
    }

    fn schema_switch_shortcuts_locked(config: &mut RimeConfig) -> Vec<SchemaShortcut> {
        let mut shortcuts = Vec::new();
        unsafe {
            let api = rime_get_api();
            let Some(begin) = rime_api_member!(api, config_begin_list) else {
                return shortcuts;
            };
            let Some(next) = rime_api_member!(api, config_next) else {
                return shortcuts;
            };
            let Some(end) = rime_api_member!(api, config_end) else {
                return shortcuts;
            };
            let mut iterator: RimeConfigIterator = std::mem::zeroed();
            if begin(&mut iterator, config, c"key_binder/bindings".as_ptr()) == 0 {
                return shortcuts;
            }
            while next(&mut iterator) != 0 {
                let Some(path) = cstr_to_str(iterator.path) else {
                    continue;
                };
                // Stock editing aliases can consume the host's Ctrl+A/Ctrl+V.
                // Admit only explicit switching actions through that boundary.
                let action = [
                    "send", "send_sequence", "toggle", "set_option", "unset_option", "select",
                ]
                    .into_iter()
                    .find(|name| config_string_locked(config, &format!("{path}/{name}")).is_some());
                if !matches!(action, Some("toggle" | "set_option" | "unset_option" | "select")) {
                    continue;
                }
                if let (Some(accept), Some(when)) = (
                    config_string_locked(config, &format!("{path}/accept")),
                    config_string_locked(config, &format!("{path}/when")),
                ) {
                    shortcuts.extend(SchemaShortcut::parse(&accept, &when));
                }
            }
            end(&mut iterator);
        }
        shortcuts
    }

    fn read_schema_switches_locked(schema_id: &str) -> Vec<SchemaSwitch> {
        try_read_schema_switches_locked(schema_id).unwrap_or_default()
    }

    fn try_read_schema_switches_locked(schema_id: &str) -> Option<Vec<SchemaSwitch>> {
        let schema_id = CString::new(schema_id).ok()?;
        unsafe {
            let api = rime_get_api();
            let schema_open = rime_api_member!(api, schema_open)?;
            let config_close = rime_api_member!(api, config_close)?;
            let config_begin_list = rime_api_member!(api, config_begin_list)?;
            let config_next = rime_api_member!(api, config_next)?;
            let config_end = rime_api_member!(api, config_end)?;

            let mut config: RimeConfig = std::mem::zeroed();
            if schema_open(schema_id.as_ptr(), &mut config) == 0 {
                return None;
            }

            let mut result = Vec::new();
            let switches_key = CString::new("switches").expect("static string has no NUL");
            let mut iterator: RimeConfigIterator = std::mem::zeroed();
            if config_begin_list(&mut iterator, &mut config, switches_key.as_ptr()) != 0 {
                while config_next(&mut iterator) != 0 {
                    let Some(path) = cstr_to_str(iterator.path) else {
                        continue;
                    };
                    let name = config_string_locked(&mut config, &format!("{path}/name"));
                    let options =
                        config_string_list_locked(&mut config, &format!("{path}/options"));
                    let states = config_string_list_locked(&mut config, &format!("{path}/states"));
                    let reset = config_int_locked(&mut config, &format!("{path}/reset"));
                    if name.is_some() || !options.is_empty() {
                        result.push(SchemaSwitch {
                            name,
                            options,
                            states,
                            reset,
                        });
                    }
                }
                config_end(&mut iterator);
            }
            config_close(&mut config);
            Some(result)
        }
    }

    pub(super) fn initialized_user_dir() -> Result<Option<PathBuf>, String> {
        let _rime = rime_api_lock();
        if !*lock_ignore_poison(&RIME_INITIALIZED) {
            return Ok(None);
        }
        let directory = lock_ignore_poison(&RIME_USER_DATA_DIR);
        let (path, expected) = directory
            .as_ref()
            .ok_or("librime user directory is unavailable")?;
        unsafe {
            let api = rime_get_api();
            let get_dir = rime_api_member!(api, get_user_data_dir)
                .ok_or("librime user directory API is unavailable")?;
            let current = get_dir();
            if current.is_null() {
                return Err("librime user directory is unavailable".into());
            }
            if CStr::from_ptr(current).to_bytes() != expected {
                return Err("librime is initialized for another user directory".into());
            }
        }
        Ok(Some(path.clone()))
    }

    pub(super) fn setup_switch_config(
        control_root: &Path,
        dirs: &RuntimeDirs,
    ) -> Result<(), String> {
        let _rime = rime_api_lock();
        if let Some(current) = initialized_user_dir()? {
            if current != dirs.user_data_dir {
                return Err("librime is initialized for another user directory".into());
            }
            return Ok(());
        }
        setup_resolved_runtime(control_root, dirs, DeployProcess::InProcess)
    }

    pub(super) fn list_switch_settings(
        control_root: &Path,
        dirs: &RuntimeDirs,
        schema_id: &str,
    ) -> Result<Vec<SchemaSwitchSetting>, String> {
        let _rime = rime_api_lock();
        let user_dir = &dirs.user_data_dir;
        let user: Value = match std::fs::read(user_dir.join("user.yaml")) {
            Ok(bytes) => serde_yaml::from_slice(&bytes)
                .map_err(|error| format!("read Rime user config: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(error) => return Err(format!("read Rime user config: {error}")),
        };
        let request = schema_switch_request::read_at(user_dir)?;
        let live = schema_switch_request::live_state(control_root, dirs, schema_id, request.generation);
        let pending = request.generation > schema_switch_applied_generation(user_dir)?;
        Ok(visible_switch_settings(
            schema_id,
            read_schema_switches_locked(schema_id),
            |name, has_reset| {
                if let Some(value) = live.as_ref().and_then(|state| state.option(name)) {
                    return Some(value);
                }
                if has_reset {
                    request.options.get(name).copied()
                } else {
                    pending
                        .then(|| request.options.get(name).copied())
                        .flatten()
                        .or_else(|| user.get("var")?.get("option")?.get(name)?.as_bool())
                }
            },
        ))
    }

    pub(super) fn set_switch_setting(
        control_root: &Path,
        dirs: &RuntimeDirs,
        schema_id: &str,
        switch_name: &str,
        value: usize,
    ) -> Result<Vec<SchemaSwitchSetting>, String> {
        let switches = list_switch_settings(control_root, dirs, schema_id)?;
        let switch = switches
            .iter()
            .find(|switch| switch.name == switch_name)
            .ok_or("Rime switch is unavailable")?;
        if value >= switch.states.len() {
            return Err("Rime switch value is out of range".into());
        }
        let values = if switch.options.is_empty() {
            vec![(switch.name.clone(), value != 0)]
        } else {
            switch
                .options
                .iter()
                .enumerate()
                .map(|(index, name)| (name.clone(), index == value))
                .collect()
        };
        let previous = schema_switch_request::read_at(&dirs.user_data_dir)?;
        let live = schema_switch_request::live_state(control_root, dirs, schema_id, previous.generation);
        // Do not replay an old App preference over a newer F4 change to another switch.
        let carry_forward = previous.options.into_keys().filter_map(|name| {
            let enabled = live.as_ref()?.option(&name)?;
            Some((name, enabled))
        });
        schema_switch_request::merge(&dirs.user_data_dir, carry_forward.chain(values))?;
        list_switch_settings(control_root, dirs, schema_id)
    }

    // RimeSetOption changes only the context. Update the IME's shared user
    // ConfigData before the next session's Switcher::SetActiveSchema saves it.
    fn persist_requested_switches_locked(
        schema_id: &str,
        request: &SchemaSwitchRequest,
    ) -> Result<(), String> {
        let names: HashSet<_> = read_schema_switches_locked(schema_id)
            .into_iter()
            .flat_map(|switch| switch.name.into_iter().chain(switch.options))
            .collect();
        let selected_schema = request
            .schema_id
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| "invalid Rime schema id")?;
        unsafe {
            let api = rime_get_api();
            let open =
                rime_api_member!(api, config_open).ok_or("librime config API is unavailable")?;
            let user_open = rime_api_member!(api, user_config_open)
                .ok_or("librime user config API is unavailable")?;
            let close = rime_api_member!(api, config_close)
                .ok_or("librime config close API is unavailable")?;
            let set = rime_api_member!(api, config_set_bool)
                .ok_or("librime config bool setter is unavailable")?;
            let set_string = if selected_schema.is_some() {
                Some(
                    rime_api_member!(api, config_set_string)
                        .ok_or("librime config string setter is unavailable")?,
                )
            } else {
                None
            };
            let mut config: RimeConfig = std::mem::zeroed();
            if open(c"default".as_ptr(), &mut config) == 0 {
                return Err("cannot open Rime switcher config".into());
            }
            let saved = config_string_list_locked(&mut config, "switcher/save_options");
            close(&mut config);
            let values = request
                .options
                .iter()
                .filter(|(name, _)| names.contains(*name) && saved.contains(name))
                .map(|(name, value)| {
                    CString::new(format!("var/option/{name}")).map(|key| (key, *value))
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid Rime switch name")?;
            if values.is_empty() && request.schema_id.is_none() {
                return Ok(());
            }
            if user_open(c"user".as_ptr(), &mut config) == 0 {
                return Err("cannot open Rime user config".into());
            }
            // Live switchers can keep ConfigData alive after config_close.
            USER_CONFIG_NEEDS_FLUSH.store(true, Ordering::Relaxed);
            let mut result = values
                .iter()
                .all(|(key, value)| set(&mut config, key.as_ptr(), i32::from(*value)) != 0);
            if let (Some(schema_id), Some(set_string)) = (selected_schema, set_string) {
                result &= set_string(
                    &mut config,
                    c"var/previously_selected_schema".as_ptr(),
                    schema_id.as_ptr(),
                ) != 0;
            }
            let closed = close(&mut config) != 0;
            if !result || !closed {
                return Err("cannot persist IME switch request".into());
            }
        }
        Ok(())
    }

    fn config_string_locked(config: &mut RimeConfig, key: &str) -> Option<String> {
        let key = CString::new(key).ok()?;
        unsafe {
            let api = rime_get_api();
            let get = rime_api_member!(api, config_get_cstring)?;
            cstr_to_str(get(config, key.as_ptr())).filter(|value| !value.is_empty())
        }
    }

    fn config_int_locked(config: &mut RimeConfig, key: &str) -> Option<i32> {
        let key = CString::new(key).ok()?;
        unsafe {
            let api = rime_get_api();
            let get = rime_api_member!(api, config_get_int)?;
            let mut value = 0;
            (get(config, key.as_ptr(), &mut value) != 0).then_some(value)
        }
    }

    fn config_bool_locked(config: &mut RimeConfig, key: &str) -> Option<bool> {
        let key = CString::new(key).ok()?;
        unsafe {
            let api = rime_get_api();
            let get = rime_api_member!(api, config_get_bool)?;
            let mut value = 0;
            (get(config, key.as_ptr(), &mut value) != 0).then_some(value != 0)
        }
    }

    fn config_string_list_locked(config: &mut RimeConfig, key: &str) -> Vec<String> {
        let Ok(key) = CString::new(key) else {
            return Vec::new();
        };
        unsafe {
            let api = rime_get_api();
            let Some(begin) = rime_api_member!(api, config_begin_list) else {
                return Vec::new();
            };
            let Some(next) = rime_api_member!(api, config_next) else {
                return Vec::new();
            };
            let Some(end) = rime_api_member!(api, config_end) else {
                return Vec::new();
            };
            let mut iterator: RimeConfigIterator = std::mem::zeroed();
            if begin(&mut iterator, config, key.as_ptr()) == 0 {
                return Vec::new();
            }
            let mut values = Vec::new();
            while next(&mut iterator) != 0 {
                if let Some(path) = cstr_to_str(iterator.path) {
                    if let Some(value) = config_string_locked(config, &path) {
                        values.push(value);
                    }
                }
            }
            end(&mut iterator);
            values
        }
    }

    fn schema_user_dictionary_names_locked(schema_id: &str) -> Vec<String> {
        let Ok(schema_id) = CString::new(schema_id) else {
            return Vec::new();
        };
        unsafe {
            let api = rime_get_api();
            let Some(schema_open) = rime_api_member!(api, schema_open) else {
                return Vec::new();
            };
            let Some(config_close) = rime_api_member!(api, config_close) else {
                return Vec::new();
            };
            let mut config: RimeConfig = std::mem::zeroed();
            if schema_open(schema_id.as_ptr(), &mut config) == 0 {
                return Vec::new();
            }

            let mut namespaces = vec!["translator".to_owned()];
            for component in config_string_list_locked(&mut config, "engine/translators") {
                if let Some((_, namespace)) = component.split_once('@') {
                    let namespace = namespace.trim();
                    if !namespace.is_empty() {
                        namespaces.push(namespace.to_owned());
                    }
                }
            }

            let mut names = Vec::new();
            for namespace in namespaces {
                if config_bool_locked(&mut config, &format!("{namespace}/enable_user_dict"))
                    == Some(false)
                {
                    continue;
                }
                let explicit_user_dict =
                    config_string_locked(&mut config, &format!("{namespace}/user_dict"));
                let name = explicit_user_dict.or_else(|| {
                    config_string_locked(&mut config, &format!("{namespace}/dictionary"))
                        .and_then(|name| name.split('.').next().map(str::to_owned))
                });
                if let Some(name) = name.filter(|name| name != "disabled") {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            config_close(&mut config);
            names
        }
    }

    fn user_dictionary_cache_available_for(user_data_dir: Option<&Path>) -> bool {
        let cache = lock_ignore_poison(&USER_DICTIONARY_CACHE);
        cache.source_available && cache.user_data_dir.as_deref() == user_data_dir
    }

    fn initialize_user_dictionary_cache(user_data_dir: &Path) {
        let result = if engine_capabilities().candidate_deletion {
            load_user_dictionary_entries()
        } else {
            Err("librime has no delete_candidate_on_current_page entry point".into())
        };
        let mut cache = lock_ignore_poison(&USER_DICTIONARY_CACHE);
        cache.user_data_dir = Some(user_data_dir.to_path_buf());
        match result {
            Ok(entries) => {
                cache.entries = entries;
                cache.source_available = true;
            }
            Err(error) => {
                cache.entries.clear();
                cache.source_available = false;
                log_user_dictionary_degrade(&error);
            }
        }
    }

    fn clear_user_dictionary_cache() {
        let mut cache = lock_ignore_poison(&USER_DICTIONARY_CACHE);
        cache.user_data_dir = None;
        cache.entries.clear();
        cache.source_available = false;
    }

    fn load_user_dictionary_entries() -> Result<HashMap<String, HashSet<String>>, String> {
        static EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

        let dictionary_names = {
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held and the module/API pointers are
            // checked before any function pointer is called.
            let levers = unsafe { user_dictionary_api_locked()? };
            let mut iterator: RimeUserDictIterator = unsafe { std::mem::zeroed() };
            let iterator_init = unsafe { (*levers).user_dict_iterator_init.unwrap() };
            let iterator_destroy = unsafe { (*levers).user_dict_iterator_destroy.unwrap() };
            let next_user_dict = unsafe { (*levers).next_user_dict.unwrap() };
            if unsafe { iterator_init(&mut iterator) } == 0 {
                return Ok(HashMap::new());
            }
            let mut dictionary_names = Vec::new();
            loop {
                let dictionary_name = unsafe { next_user_dict(&mut iterator) };
                if dictionary_name.is_null() {
                    break;
                }
                if let Some(dictionary_name) = cstr_to_str(dictionary_name) {
                    dictionary_names.push(dictionary_name);
                }
            }
            unsafe { iterator_destroy(&mut iterator) };
            dictionary_names
        };

        let mut result = HashMap::new();
        for dictionary_name in dictionary_names {
            let sequence = EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let export_path = std::env::temp_dir().join(format!(
                ".keytao-candidate-source-{}-{sequence}.txt",
                std::process::id()
            ));
            let export_file = UserDictionaryExportFile(export_path);
            export_user_dictionary(&dictionary_name, &export_file.0)?;
            // Export is the only librime operation. Reading and parsing the
            // potentially large text file must not monopolize RIME_API_LOCK.
            let contents = std::fs::read_to_string(&export_file.0).map_err(|error| {
                format!("could not read exported user dictionary {dictionary_name}: {error}")
            })?;
            result.insert(
                dictionary_name,
                contents
                    .lines()
                    .filter_map(active_user_dictionary_text)
                    .collect(),
            );
        }
        Ok(result)
    }

    struct UserDictionaryExportFile(PathBuf);

    impl Drop for UserDictionaryExportFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn export_user_dictionary(dictionary_name: &str, export_path: &Path) -> Result<(), String> {
        let dictionary_name = CString::new(dictionary_name)
            .map_err(|_| "user-dictionary name contains a NUL byte".to_string())?;
        let export_path = CString::new(export_path.to_string_lossy().as_bytes())
            .map_err(|_| "user-dictionary export path contains a NUL byte".to_string())?;
        let _rime = rime_api_lock();
        // SAFETY: the librime lock is held and the API was validated before use.
        let levers = unsafe { user_dictionary_api_locked()? };
        let export = unsafe { (*levers).export_user_dict.unwrap() };
        let exported = unsafe { export(dictionary_name.as_ptr(), export_path.as_ptr()) };
        if exported < 0 {
            Err(format!(
                "levers failed to export user dictionary {}",
                dictionary_name.to_string_lossy()
            ))
        } else {
            Ok(())
        }
    }

    unsafe fn user_dictionary_api_locked() -> Result<*const RimeLeversApi, String> {
        let api = rime_get_api();
        let find_module =
            rime_api_member!(api, find_module).ok_or("librime has no find_module entry point")?;
        let module_name = CString::new("levers").expect("static string has no NUL");
        let module = find_module(module_name.as_ptr());
        if module.is_null() {
            return Err("librime levers module is not registered".into());
        }
        let get_api = (*module)
            .get_api
            .ok_or("librime levers module has no get_api entry point")?;
        let levers = get_api() as *const RimeLeversApi;
        if levers.is_null() {
            return Err("librime levers module returned a null API".into());
        }
        for (available, name) in [
            (
                (*levers).user_dict_iterator_init.is_some(),
                "user_dict_iterator_init",
            ),
            (
                (*levers).user_dict_iterator_destroy.is_some(),
                "user_dict_iterator_destroy",
            ),
            ((*levers).next_user_dict.is_some(), "next_user_dict"),
            ((*levers).export_user_dict.is_some(), "export_user_dict"),
        ] {
            if !available {
                return Err(format!("librime levers module has no {name} entry point"));
            }
        }
        Ok(levers)
    }

    fn log_user_dictionary_degrade(error: &str) {
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::AcqRel) {
            eprintln!("KeyTao: user-dictionary candidate classification unavailable: {error}");
        }
    }

    fn active_user_dictionary_text(line: &str) -> Option<String> {
        if line.starts_with('#') {
            return None;
        }
        let mut fields = line.split('\t');
        let first = fields.next()?;
        let second = fields.next()?;
        let metadata = fields.next()?;
        let legacy_commits = metadata
            .split_whitespace()
            .find_map(|field| field.strip_prefix("c="));
        let (text, commits) = if let Some(commits) = legacy_commits {
            (second, commits.parse::<i64>().ok()?)
        } else {
            (first, metadata.trim().parse::<i64>().ok()?)
        };
        (commits >= 0 && !text.is_empty()).then(|| text.to_owned())
    }

    /// Snapshot after a state-changing call: takes the pending commit with it.
    fn extract_state_with_commit(engine: &RimeBackend) -> ImeState {
        let committed = engine.session.commit().map(|c| c.text().to_string());
        extract_state(engine, committed)
    }

    /// Snapshot for read-only queries: leaves the pending commit for the next
    /// key event so that no text is dropped on the floor.
    fn extract_state_readonly(engine: &RimeBackend) -> ImeState {
        extract_state(engine, None)
    }

    fn session_select_keys(session: &rime_api::Session) -> Option<String> {
        let ctx = session.context()?;
        ctx.menu().select_keys.map(|keys: &str| keys.to_string())
    }

    fn extract_state(engine: &RimeBackend, committed: Option<String>) -> ImeState {
        let session = &engine.session;
        let status = session.status().ok();
        let ascii_mode = status.as_ref().map(|s| s.is_ascii_mode).unwrap_or(false);
        let schema_id = status
            .as_ref()
            .map(|s| s.schema_id().to_string())
            .unwrap_or_default();
        let schema_name = status
            .as_ref()
            .map(|s| s.schema_name().to_string())
            .unwrap_or_default();
        let (chord_composer, schema_shortcuts, modifier_taps) = {
            let cached = engine.schema_key_policy(&schema_id);
            (
                cached.chord_composer,
                cached.shortcuts.clone(),
                cached.modifier_taps.clone(),
            )
        };
        let Some(ctx) = session.context() else {
            return ImeState {
                committed,
                ascii_mode,
                chord_composer,
                schema_shortcuts,
                modifier_taps,
                schema_id,
                schema_name,
                ..ImeState::empty()
            };
        };

        let comp = ctx.composition();
        let preedit = comp.preedit.unwrap_or("").to_string();
        // librime counts in UTF-8 bytes; the contract is Unicode scalars.
        let cursor = char_offset_from_utf8(&preedit, comp.cursor_pos);
        let sel_start = char_offset_from_utf8(&preedit, comp.sel_start);
        let sel_end = char_offset_from_utf8(&preedit, comp.sel_end);

        let menu = ctx.menu();
        let candidates: Vec<Candidate> = menu
            .candidates
            .iter()
            .map(|c| Candidate {
                text: c.text.to_string(),
                comment: c.comment.map(|s: &str| s.to_string()),
            })
            .collect();

        ImeState {
            preedit,
            commit_preview: unsafe { context_commit_preview(&ctx.raw()) },
            cursor,
            sel_start,
            sel_end,
            candidates,
            highlighted_candidate_index: menu.highlighted_candidate_index,
            page_size: menu.page_size,
            page: menu.page_no,
            is_last_page: menu.is_last_page,
            committed,
            select_keys: menu.select_keys.map(|s: &str| s.to_string()),
            ascii_mode,
            switch_hint: None,
            chord_composer,
            schema_shortcuts,
            modifier_taps,
            schema_id,
            schema_name,
        }
    }

    /// The context must still own the preview pointer returned by librime.
    unsafe fn context_commit_preview(context: &RimeContext) -> String {
        if context.menu.num_candidates > 0 {
            candidate_string(context.commit_text_preview)
        } else {
            String::new()
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
            let candidate_list_begin = rime_api_member!(api, candidate_list_begin)?;
            let candidate_list_next = rime_api_member!(api, candidate_list_next)?;
            let candidate_list_end = rime_api_member!(api, candidate_list_end)?;
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
pub use desktop::{deploy, deploy_for_process, setup_only};

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub(crate) use desktop::RimeBackend;

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
fn with_schema_switch_config<T>(
    user_dir: &Path,
    action: impl FnOnce(&RuntimeDirs) -> Result<T, String>,
) -> Result<T, String> {
    let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
    let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
    let dirs = initialized
        .as_ref()
        .filter(|dirs| dirs.user == user_dir)
        .cloned()
        .unwrap_or_else(|| RimeDataDirs {
            user: user_dir.into(),
            shared: default_shared_data_dir(),
        });
    desktop::initialized_user_dir()?;
    let runtime = RuntimeDirs::resolve(user_dir, Path::new(&dirs.shared))?;
    let changed = PROCESS_RIME.using_libjd() || desktop::loaded_package_changed(&runtime);
    PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs, changed)?;
    desktop::setup_switch_config(user_dir, &runtime)?;
    *initialized = Some(dirs);
    action(&runtime)
}

/// Release the App's Rime configuration runtime when switching input engines.
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub fn release_idle_rime_config(user_dir: &Path) -> Result<(), String> {
    let _build = PROCESS_RIME
        .build
        .acquire()
        .ok_or("Rime engine build timed out")?;
    let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
    let Some(dirs) = initialized
        .as_ref()
        .filter(|dirs| dirs.user == user_dir)
        .cloned()
    else {
        return Ok(());
    };
    let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
    if PROCESS_RIME.using_libjd()
        || lock_ignore_poison(&PROCESS_RIME.sessions)
            .iter()
            .any(|session| session.inner.strong_count() != 0)
    {
        return Ok(());
    }
    desktop::initialized_user_dir()?;
    PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs, true)
}

/// Read the visible F4 switches of the currently deployed schema.
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub fn list_schema_switches(user_dir: &Path) -> Result<Vec<SchemaSwitchSetting>, String> {
    if runtime_libjd::active_libjd_schema(user_dir).is_some() {
        return Ok(Vec::new());
    }
    let dirs = RuntimeDirs::resolve(user_dir, user_dir)?;
    let Some(schema_id) = deployed_schema_id_from_dir(user_dir) else {
        return Ok(Vec::new());
    };
    if !dirs
        .staging_dir
        .join(format!("{schema_id}.schema.yaml"))
        .is_file()
    {
        return Ok(Vec::new());
    }
    with_schema_switch_config(user_dir, |dirs| {
        desktop::list_switch_settings(user_dir, dirs, &schema_id)
    })
}

/// Request a visible F4 switch change for the IME process.
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub fn set_schema_switch(
    user_dir: &Path,
    schema_id: &str,
    switch_name: &str,
    value: usize,
) -> Result<Vec<SchemaSwitchSetting>, String> {
    let dirs = RuntimeDirs::resolve(user_dir, user_dir)?;
    if deployed_schema_id_from_dir(user_dir).as_deref() != Some(schema_id)
        || !dirs
            .staging_dir
            .join(format!("{schema_id}.schema.yaml"))
            .is_file()
    {
        return Err("Rime schema is no longer deployed or selected".into());
    }
    with_schema_switch_config(user_dir, |dirs| {
        desktop::set_switch_setting(user_dir, dirs, schema_id, switch_name, value)
    })
}

/// What the linked librime can do natively, see [`EngineCapabilities`].
///
/// Answerable before librime is initialized: it only inspects the ABI.
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub fn engine_capabilities() -> EngineCapabilities {
    desktop::engine_capabilities()
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
)))]
pub fn engine_capabilities() -> EngineCapabilities {
    EngineCapabilities::none()
}

// Rime modifier bits, mirroring librime's `RimeModifier` in key_table.h.
pub const RIME_MOD_SHIFT: u32 = 1 << 0;
/// CapsLock. The ASCII composer needs this bit as well as the folded keysym.
pub const RIME_MOD_LOCK: u32 = 1 << 1;
pub const RIME_MOD_CONTROL: u32 = 1 << 2;
pub const RIME_MOD_ALT: u32 = 1 << 3;
pub const RIME_MOD_SUPER: u32 = 1 << 26;
pub const RIME_MOD_HYPER: u32 = 1 << 27;
pub const RIME_MOD_META: u32 = 1 << 28;
pub const RIME_RELEASE_MASK: u32 = 1 << 30;

/// Candidate label fallback when a schema publishes no `menu/select_keys`.
///
/// Labels only: intercepting these characters as selection keys would steal
/// digits from schemas that spell codes with them, so key handling uses
/// [`key_policy::candidate_index_for_char`], which has no fallback.
pub const DEFAULT_SELECT_KEYS: &str = "1234567890";

pub mod key_policy {
    use super::{
        ImeState, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_HYPER, RIME_MOD_LOCK, RIME_MOD_META,
        RIME_MOD_SHIFT, RIME_MOD_SUPER,
    };

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
    pub const XK_BEGIN: u32 = 0xff58;
    pub const XK_DELETE: u32 = 0xffff;
    pub const XK_KP_ENTER: u32 = 0xff8d;
    pub const XK_KP_DECIMAL: u32 = 0xffae;
    pub const XK_F4: u32 = 0xffc1;

    /// X11 encodes a keysym for an arbitrary Unicode scalar as
    /// `0x01000000 | codepoint`.
    pub const XK_UNICODE_BASE: u32 = 0x0100_0000;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CandidatePanelLayout {
        Hidden,
        Horizontal,
        Vertical,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CandidateArrowAction {
        PreviousCandidate,
        NextCandidate,
        PreviousPage,
        NextPage,
    }

    impl CandidateArrowAction {
        // Private keycodes outside X11's Unicode range also travel through
        // the Windows engine pipe without changing its key-event protocol.
        fn keycode(self) -> u32 {
            0x0200_0000 + self as u32
        }

        pub(super) fn from_keycode(keycode: u32) -> Option<Self> {
            match keycode {
                0x0200_0000 => Some(Self::PreviousCandidate),
                0x0200_0001 => Some(Self::NextCandidate),
                0x0200_0002 => Some(Self::PreviousPage),
                0x0200_0003 => Some(Self::NextPage),
                _ => None,
            }
        }
    }

    pub fn candidate_arrow_action(
        sym: u32,
        mods: u32,
        state: &ImeState,
        panel: CandidatePanelLayout,
    ) -> Option<CandidateArrowAction> {
        use CandidateArrowAction::*;
        use CandidatePanelLayout::*;
        if state.candidates.is_empty()
            || super::rime_modifier_mask(mods) != 0
            || is_configured_schema_shortcut(sym, mods, state)
        {
            return None;
        }
        if state.is_package_switcher() && panel != Hidden {
            return match sym {
                XK_UP | XK_LEFT => Some(PreviousCandidate),
                XK_DOWN | XK_RIGHT => Some(NextCandidate),
                _ => None,
            };
        }
        match (panel, sym) {
            (Vertical, XK_UP) | (Horizontal, XK_LEFT) => Some(PreviousCandidate),
            (Vertical, XK_DOWN) | (Horizontal, XK_RIGHT) => Some(NextCandidate),
            (Vertical, XK_LEFT) | (Horizontal, XK_UP) => Some(PreviousPage),
            (Vertical, XK_RIGHT) | (Horizontal, XK_DOWN) => Some(NextPage),
            _ => None,
        }
    }

    /// Map visible-panel arrows to core navigation, independently of the
    /// schema's selector bindings and preedit caret position.
    pub fn candidate_arrow_key(
        sym: u32,
        mods: u32,
        state: &ImeState,
        panel: CandidatePanelLayout,
    ) -> u32 {
        candidate_arrow_action(sym, mods, state, panel).map_or(sym, CandidateArrowAction::keycode)
    }

    /// What a frontend should do with `Return`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum EnterAction {
        /// Nothing is being composed: the host application owns the key.
        Bypass,
        /// Hand `XK_Return` to librime; if the schema's editor passes it back,
        /// commit the raw input instead of dropping the composition.
        ForwardToRime,
    }

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
                ..=XK_BEGIN | XK_KP_ENTER | XK_KP_DECIMAL
        )
    }

    /// `Return` is never handled by the frontend on its own: librime decides,
    /// because only the schema's editor knows whether `Return` confirms the
    /// highlighted candidate or commits the raw code.
    pub fn enter_action(state: &ImeState) -> EnterAction {
        if state.preedit.is_empty() && state.candidates.is_empty() {
            EnterAction::Bypass
        } else {
            EnterAction::ForwardToRime
        }
    }

    /// Modifiers the window system reserves for itself (Command on macOS, the
    /// Windows/Super key elsewhere). Everything else — including Control and
    /// Alt — is offered to librime first, because Rime's switcher hotkeys and
    /// the schemas' `key_binder` bindings live there.
    pub fn is_system_reserved_modifier(mods: u32) -> bool {
        mods & (RIME_MOD_SUPER | RIME_MOD_HYPER | RIME_MOD_META) != 0
    }

    /// Whether a key must be passed straight to the application.
    ///
    /// `ascii_mode` is deliberately not part of this decision: in English mode
    /// librime's `ascii_composer` still has to see the keys to run `Shift`
    /// switching, punctuation and the schema's bindings, and it reports the
    /// key as not accepted when the application should get it.
    pub fn should_bypass_empty_composition(sym: u32, mods: u32, state: &ImeState) -> bool {
        should_bypass_empty_composition_key(is_nonstarter_key(sym), mods, state)
            && !(mods & (RIME_MOD_CONTROL | RIME_MOD_ALT) == 0
                && !is_system_reserved_modifier(mods)
                && should_forward_chord_release(sym, mods, state))
            && !is_configured_schema_shortcut(sym, mods, state)
    }

    pub fn is_configured_switcher_hotkey(sym: u32, mods: u32, state: &ImeState) -> bool {
        state.schema_shortcuts.iter().any(|shortcut| shortcut.matches_switcher(sym, mods))
    }

    pub fn is_configured_schema_shortcut(sym: u32, mods: u32, state: &ImeState) -> bool {
        !is_system_reserved_modifier(mods)
            && state.schema_shortcuts.iter().any(|shortcut| {
                shortcut.applies(state) && shortcut.matches_key(sym, mods)
            })
    }

    pub fn is_configured_modifier_tap(sym: u32, state: &ImeState) -> bool {
        state.modifier_taps.contains(&sym)
    }

    /// Chord composers must see releases even if modifiers changed after the press.
    pub fn should_forward_chord_release(sym: u32, _mods: u32, state: &ImeState) -> bool {
        state.chord_composer && !state.ascii_mode && (0x20..=0x7e).contains(&sym)
    }

    pub fn should_bypass_empty_composition_key(
        is_nonstarter: bool,
        mods: u32,
        state: &ImeState,
    ) -> bool {
        if !state.preedit.is_empty() || !state.candidates.is_empty() {
            return false;
        }
        if is_system_reserved_modifier(mods) {
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

    /// Map a typed character to a candidate index.
    ///
    /// Only the select keys the schema actually published count. There is no
    /// `1234567890` fallback here: that fallback belongs to candidate labels,
    /// and using it for key handling would swallow digits that a schema spells
    /// codes with. When a schema publishes no select keys, the key belongs to
    /// librime's own selector.
    pub fn candidate_index_for_char(ch: char, state: &ImeState) -> Option<usize> {
        if state.candidates.is_empty() {
            return None;
        }
        let keys = state.select_keys.as_deref()?;
        keys.chars().position(|candidate_key| candidate_key == ch)
    }

    pub fn candidate_index_for_select_key(sym: u32, state: &ImeState) -> Option<usize> {
        let ch = char_for_keysym(sym)?;
        candidate_index_for_char(ch, state)
    }

    /// The X11 keysym for a character, or `None` for characters that cannot be
    /// typed (control codes).
    ///
    /// Latin-1 maps onto itself, everything else uses the Unicode encoding.
    /// Sending a raw code point instead would land in the function key block:
    /// `（` (U+FF08) would arrive as `XK_BackSpace`.
    pub fn keysym_for_char(ch: char) -> Option<u32> {
        if ch.is_control() {
            return None;
        }
        let codepoint = ch as u32;
        if codepoint <= 0x00ff {
            Some(codepoint)
        } else {
            Some(XK_UNICODE_BASE | codepoint)
        }
    }

    /// The character a keysym stands for, inverse of [`keysym_for_char`].
    pub fn char_for_keysym(sym: u32) -> Option<char> {
        if sym == XK_KP_DECIMAL {
            return Some('.');
        }
        let codepoint = if sym & 0xff00_0000 == XK_UNICODE_BASE {
            sym & 0x00ff_ffff
        } else if sym <= 0x00ff {
            sym
        } else {
            return None;
        };
        char::from_u32(codepoint).filter(|ch| !ch.is_control())
    }

    /// The keysym for a piece of text a soft keyboard produced, or `None` when
    /// the text is not a single typable character and has to be committed
    /// directly instead.
    pub fn keysym_for_text(text: &str) -> Option<u32> {
        let mut chars = text.chars();
        let first = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        keysym_for_char(first)
    }

    /// Fold an active `Shift`/`CapsLock` into the keysym, preserving modifiers.
    ///
    /// Rime expects the keysym to carry the character that was actually typed
    /// and the Lock bit lets ascii_composer bypass the speller. With neither
    /// modifier, preserve the supplied character:
    /// soft keyboards and text-to-keysym callers already encode its case.
    pub fn normalize_key_for_modifiers(sym: u32, mods: u32) -> (u32, u32) {
        if mods & (RIME_MOD_LOCK | RIME_MOD_SHIFT) == 0 {
            return (sym, mods);
        }
        let uppercase = (mods & RIME_MOD_LOCK != 0) != (mods & RIME_MOD_SHIFT != 0);
        let Some(ch) = char_for_keysym(sym) else {
            return (sym, mods);
        };
        if !ch.is_ascii_alphabetic() {
            return (sym, mods);
        }
        let folded = if uppercase {
            ch.to_ascii_uppercase()
        } else {
            ch.to_ascii_lowercase()
        };
        (folded as u32, mods)
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

/// Drop everything librime has no name for (NumLock, mouse buttons, XKB
/// groups) and keep the modifiers Rime's `key_binder` can bind, including
/// Super/Hyper/Meta so that a frontend can express Command/Windows chords.
/// CapsLock is ignored for shortcut/navigation decisions; the runtime restores
/// it when dispatching to librime's ASCII composer.
pub fn rime_modifier_mask(mask: u32) -> u32 {
    mask & (RIME_MOD_SHIFT
        | RIME_MOD_CONTROL
        | RIME_MOD_ALT
        | RIME_MOD_SUPER
        | RIME_MOD_HYPER
        | RIME_MOD_META
        | RIME_RELEASE_MASK)
}

#[cfg(test)]
mod ime_runtime_tests {
    use super::{
        char_offset_from_utf8, key_policy, rime_modifier_mask, utf16_offset_from_chars, Candidate,
        ImeState, InputContextPolicy, RIME_MOD_ALT, RIME_MOD_CONTROL, RIME_MOD_LOCK, RIME_MOD_META,
        RIME_MOD_SHIFT, RIME_MOD_SUPER, RIME_RELEASE_MASK,
    };

    fn state_with_candidates(select_keys: Option<&str>) -> ImeState {
        let mut state = ImeState::empty();
        state.preedit = "ab".to_owned();
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
        state.select_keys = select_keys.map(str::to_owned);
        state
    }

    #[test]
    fn chord_releases_require_a_chord_schema_and_preserve_ascii_mode() {
        let mut state = ImeState::empty();
        assert!(!key_policy::should_forward_chord_release(
            b'a' as u32,
            0,
            &state
        ));
        state.chord_composer = true;
        for key in [b'a', b'Z', b'1', b';', b' '] {
            for mods in [0, RIME_MOD_SHIFT, RIME_MOD_LOCK, RIME_RELEASE_MASK] {
                assert!(key_policy::should_forward_chord_release(
                    key as u32, mods, &state
                ));
                assert!(!key_policy::should_bypass_empty_composition(
                    key as u32, mods, &state
                ));
            }
        }
        for mods in [
            RIME_MOD_CONTROL,
            RIME_MOD_ALT,
            RIME_MOD_SUPER,
            RIME_MOD_META,
        ] {
            for key in [b'a', b'Z', b'1', b';', b' '] {
                assert!(key_policy::should_forward_chord_release(
                    key as u32,
                    mods | RIME_RELEASE_MASK,
                    &state
                ));
            }
        }
        for key in [
            key_policy::XK_RETURN,
            key_policy::XK_BACK_SPACE,
            0xffe1,
            0x7f,
        ] {
            assert!(!key_policy::should_forward_chord_release(key, 0, &state));
        }
        state.ascii_mode = true;
        assert!(!key_policy::should_forward_chord_release(
            b'a' as u32,
            0,
            &state
        ));
        assert!(key_policy::should_bypass_empty_composition(
            key_policy::XK_SPACE,
            0,
            &state
        ));
        let mut legacy = serde_json::to_value(state).unwrap();
        legacy.as_object_mut().unwrap().remove("chord_composer");
        let legacy: ImeState = serde_json::from_value(legacy).unwrap();
        assert!(!legacy.chord_composer);
    }

    #[test]
    fn chord_release_forwarding_preserves_shortcut_press_bypass() {
        let mut state = ImeState::empty();
        state.chord_composer = true;
        for mods in [RIME_MOD_SUPER, RIME_MOD_META] {
            assert!(key_policy::should_bypass_empty_composition(
                b'a' as u32,
                mods,
                &state
            ));
        }
        for mods in [RIME_MOD_CONTROL, RIME_MOD_ALT] {
            assert!(key_policy::should_bypass_empty_composition(
                key_policy::XK_SPACE,
                mods,
                &state
            ));
        }
    }

    #[test]
    fn candidate_arrows_follow_the_visible_panel_orientation() {
        use key_policy::{CandidateArrowAction::*, CandidatePanelLayout::*, *};
        let state = state_with_candidates(None);
        for (panel, keys) in [
            (Vertical, [XK_UP, XK_DOWN, XK_LEFT, XK_RIGHT]),
            (Horizontal, [XK_LEFT, XK_RIGHT, XK_UP, XK_DOWN]),
        ] {
            for (key, action) in
                keys.into_iter()
                    .zip([PreviousCandidate, NextCandidate, PreviousPage, NextPage])
            {
                assert_eq!(candidate_arrow_action(key, 0, &state, panel), Some(action));
                assert_eq!(
                    CandidateArrowAction::from_keycode(candidate_arrow_key(key, 0, &state, panel)),
                    Some(action)
                );
            }
        }
    }

    #[test]
    fn package_switcher_arrows_navigate_items_in_both_panel_orientations() {
        use key_policy::{CandidateArrowAction::*, CandidatePanelLayout::*, *};
        let mut state = state_with_candidates(Some("12345"));
        state.preedit.clear();
        state.schema_id = super::switcher_model::SCHEMA_ID.to_owned();
        for panel in [Horizontal, Vertical] {
            for (key, action) in [
                (XK_UP, PreviousCandidate),
                (XK_LEFT, PreviousCandidate),
                (XK_DOWN, NextCandidate),
                (XK_RIGHT, NextCandidate),
            ] {
                assert_eq!(candidate_arrow_action(key, 0, &state, panel), Some(action));
                assert_eq!(
                    CandidateArrowAction::from_keycode(candidate_arrow_key(key, 0, &state, panel)),
                    Some(action)
                );
            }
        }
        for key in [XK_LEFT, XK_RIGHT, XK_UP, XK_DOWN] {
            assert_eq!(candidate_arrow_key(key, 0, &state, Hidden), key);
        }
    }

    #[test]
    fn candidate_arrows_preserve_hidden_empty_and_modified_keys() {
        use key_policy::{CandidatePanelLayout::*, *};
        let state = state_with_candidates(None);
        let mut no_candidates = state.clone();
        no_candidates.candidates.clear();
        for key in [XK_LEFT, XK_RIGHT, XK_UP, XK_DOWN] {
            for panel in [Horizontal, Vertical, Hidden] {
                for empty in [ImeState::empty(), no_candidates.clone()] {
                    assert_eq!(candidate_arrow_key(key, 0, &empty, panel), key);
                }
                for mods in [
                    RIME_MOD_SHIFT,
                    RIME_MOD_CONTROL,
                    RIME_MOD_ALT,
                    RIME_MOD_SUPER,
                    RIME_MOD_META,
                    RIME_RELEASE_MASK,
                ] {
                    assert_eq!(candidate_arrow_key(key, mods, &state, panel), key);
                }
            }
            assert_eq!(candidate_arrow_key(key, 0, &state, Hidden), key);
            assert_ne!(
                candidate_arrow_key(key, RIME_MOD_LOCK, &state, Vertical),
                key
            );
        }
        for key in [b'a' as u32, XK_SPACE, XK_RETURN, XK_PAGE_UP, XK_PAGE_DOWN] {
            assert_eq!(candidate_arrow_key(key, 0, &state, Vertical), key);
        }
    }

    #[test]
    fn rime_modifier_mask_ignores_lock_and_pointer_bits_for_navigation() {
        assert_eq!(rime_modifier_mask(0x10), 0);
        assert_eq!(
            rime_modifier_mask(0x10 | RIME_MOD_SHIFT | RIME_MOD_CONTROL),
            RIME_MOD_SHIFT | RIME_MOD_CONTROL
        );
        assert_eq!(
            rime_modifier_mask(RIME_RELEASE_MASK | 0x10),
            RIME_RELEASE_MASK
        );
        assert_eq!(rime_modifier_mask(RIME_MOD_LOCK), 0);
        assert_eq!(
            rime_modifier_mask(RIME_MOD_LOCK | RIME_MOD_SHIFT),
            RIME_MOD_SHIFT
        );
    }

    #[test]
    fn rime_modifier_mask_keeps_super_and_meta() {
        assert_eq!(
            rime_modifier_mask(RIME_MOD_SUPER | RIME_MOD_META),
            RIME_MOD_SUPER | RIME_MOD_META
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

        let mut composing = ImeState::empty();
        composing.preedit = "abc".to_owned();
        assert!(!key_policy::should_bypass_empty_composition(
            key_policy::XK_SPACE,
            0,
            &composing
        ));
    }

    #[test]
    fn key_policy_offers_control_and_alt_chords_to_rime() {
        let empty = ImeState::empty();
        // Rime's switcher hotkey and every schema binding lives here, so the
        // key must reach librime and only be forwarded when it is not accepted.
        assert!(!key_policy::should_bypass_empty_composition(
            b'`' as u32,
            RIME_MOD_CONTROL,
            &empty
        ));
        assert!(!key_policy::should_bypass_empty_composition(
            b'a' as u32,
            RIME_MOD_ALT,
            &empty
        ));
    }

    #[test]
    fn key_policy_bypasses_system_reserved_chords() {
        let empty = ImeState::empty();
        assert!(key_policy::should_bypass_empty_composition(
            b'a' as u32,
            RIME_MOD_SUPER,
            &empty
        ));
        assert!(key_policy::is_system_reserved_modifier(RIME_MOD_META));
        assert!(!key_policy::is_system_reserved_modifier(RIME_MOD_CONTROL));
    }

    #[test]
    fn key_policy_ignores_ascii_mode() {
        let mut english = ImeState::empty();
        english.ascii_mode = true;
        assert!(!key_policy::should_bypass_empty_composition(
            b'a' as u32,
            0,
            &english
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

        let mut state = state_with_candidates(Some("12"));
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
    fn key_policy_candidate_selection_requires_published_select_keys() {
        // Without select_keys the digit belongs to the schema's speller, so the
        // frontend must not steal it; librime's own selector still handles it.
        let state = state_with_candidates(None);
        assert_eq!(
            key_policy::candidate_index_for_select_key(b'2' as u32, &state),
            None
        );
        assert_eq!(
            key_policy::candidate_index_for_space_or_select_key(b'2' as u32, &state),
            None
        );
    }

    #[test]
    fn key_policy_enter_is_decided_by_rime_while_composing() {
        assert_eq!(
            key_policy::enter_action(&ImeState::empty()),
            key_policy::EnterAction::Bypass
        );
        assert_eq!(
            key_policy::enter_action(&state_with_candidates(None)),
            key_policy::EnterAction::ForwardToRime
        );
    }

    #[test]
    fn keysym_for_char_never_lands_in_the_function_key_block() {
        assert_eq!(key_policy::keysym_for_char('a'), Some(0x61));
        assert_eq!(key_policy::keysym_for_char('¥'), Some(0xa5));
        // U+FF08 must not become XK_BackSpace (0xff08).
        assert_eq!(
            key_policy::keysym_for_char('（'),
            Some(key_policy::XK_UNICODE_BASE | 0xff08)
        );
        assert_eq!(key_policy::keysym_for_char('\u{7f}'), None);
        assert_eq!(key_policy::keysym_for_char('\n'), None);
    }

    #[test]
    fn keysym_for_text_accepts_single_characters_only() {
        assert_eq!(key_policy::keysym_for_text("a"), Some(0x61));
        assert_eq!(key_policy::keysym_for_text("ab"), None);
        assert_eq!(key_policy::keysym_for_text(""), None);
        assert_eq!(
            key_policy::keysym_for_text("，"),
            Some(key_policy::XK_UNICODE_BASE | 0xff0c)
        );
    }

    #[test]
    fn char_for_keysym_is_the_inverse_of_keysym_for_char() {
        for ch in ['a', 'Z', '9', '-', '¥', '（', '中'] {
            let sym = key_policy::keysym_for_char(ch).expect("typable");
            assert_eq!(key_policy::char_for_keysym(sym), Some(ch));
        }
        assert_eq!(key_policy::char_for_keysym(key_policy::XK_BACK_SPACE), None);
        assert_eq!(key_policy::char_for_keysym(key_policy::XK_RETURN), None);
    }

    #[test]
    fn text_keys_keep_their_explicit_case_without_case_modifiers() {
        for text in ["A", "Z", "a", "z", "É", "（"] {
            let sym = key_policy::keysym_for_text(text).unwrap();
            for mods in [0, RIME_MOD_CONTROL, RIME_MOD_ALT, RIME_MOD_SUPER] {
                assert_eq!(
                    key_policy::normalize_key_for_modifiers(sym, mods),
                    (sym, mods)
                );
            }
        }
    }

    #[test]
    fn normalize_key_for_modifiers_folds_case_and_preserves_caps_lock() {
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'a' as u32, RIME_MOD_LOCK),
            (b'A' as u32, RIME_MOD_LOCK)
        );
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'a' as u32, RIME_MOD_LOCK | RIME_MOD_SHIFT),
            (b'a' as u32, RIME_MOD_LOCK | RIME_MOD_SHIFT)
        );
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'a' as u32, RIME_MOD_SHIFT),
            (b'A' as u32, RIME_MOD_SHIFT)
        );
        // Non-letters keep both their keysym and Lock bit.
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'1' as u32, RIME_MOD_LOCK),
            (b'1' as u32, RIME_MOD_LOCK)
        );
        assert_eq!(
            key_policy::normalize_key_for_modifiers(
                key_policy::XK_BACK_SPACE,
                RIME_MOD_LOCK | RIME_MOD_CONTROL
            ),
            (key_policy::XK_BACK_SPACE, RIME_MOD_LOCK | RIME_MOD_CONTROL)
        );
    }

    #[test]
    fn input_context_policy_defaults_to_composing() {
        assert_eq!(
            InputContextPolicy::default(),
            InputContextPolicy {
                composing: true,
                learning: true
            }
        );
        assert_eq!(
            InputContextPolicy::sensitive(),
            InputContextPolicy {
                composing: false,
                learning: false
            }
        );
    }

    #[test]
    fn input_context_policy_private_composes_without_learning() {
        assert_eq!(
            InputContextPolicy::private(),
            InputContextPolicy {
                composing: true,
                learning: false
            }
        );
        assert!(!InputContextPolicy::sensitive().composing);
    }

    #[test]
    fn cursor_offsets_are_converted_to_unicode_scalars() {
        let preedit = "中文ab";
        assert_eq!(char_offset_from_utf8(preedit, 0), 0);
        assert_eq!(char_offset_from_utf8(preedit, 3), 1);
        assert_eq!(char_offset_from_utf8(preedit, 6), 2);
        // Byte offsets inside a character clamp to its boundary, out of range
        // offsets clamp to the end.
        assert_eq!(char_offset_from_utf8(preedit, 4), 2);
        assert_eq!(char_offset_from_utf8(preedit, 999), 4);
        assert_eq!(char_offset_from_utf8("", 0), 0);
    }

    #[test]
    fn char_offsets_convert_to_utf16_units() {
        assert_eq!(utf16_offset_from_chars("中文ab", 2), 2);
        assert_eq!(utf16_offset_from_chars("中文ab", 4), 4);
        // Astral characters take two UTF-16 units.
        assert_eq!(utf16_offset_from_chars("𝄞a", 1), 2);
        assert_eq!(utf16_offset_from_chars("𝄞a", 9), 3);
    }

    #[test]
    fn multiline_offsets_preserve_newlines_and_surrogate_pairs() {
        for text in [
            "甲😀尾\n乙末\n丙终",
            "甲😀尾\r\n乙末\r\n丙终",
            "甲😀尾\r乙末\r丙终",
        ] {
            let mut bytes = 0;
            let mut units = 0;
            for (chars, ch) in text.chars().enumerate() {
                assert_eq!(char_offset_from_utf8(text, bytes), chars);
                assert_eq!(utf16_offset_from_chars(text, chars), units);
                bytes += ch.len_utf8();
                units += ch.len_utf16();
            }
            assert_eq!(
                utf16_offset_from_chars(text, usize::MAX),
                text.encode_utf16().count()
            );
        }
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
    /// The resolved deployment this session belongs to. Another runtime may
    /// switch the process-wide librime instance to different directories.
    dirs: RimeDataDirs,
    inner: Arc<Mutex<ImeRuntimeSessionInner>>,
    policy: Arc<SessionInputPolicy>,
    context_ascii_mode: Arc<AtomicU8>,
    pending: Arc<Mutex<PendingRuntimeInput>>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
#[derive(Default)]
struct PendingRuntimeInput {
    state_callback: Option<Arc<dyn Fn() + Send + Sync>>,
    package_menu: Option<switcher_model::Menu>,
    armed_modifier_tap: Option<(u32, Instant)>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
#[derive(Clone, Copy)]
enum RuntimeKey {
    Key(u32, u32),
    Enter,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
struct WeakRuntimeSession {
    dirs: RimeDataDirs,
    inner: Weak<Mutex<ImeRuntimeSessionInner>>,
    policy: Weak<SessionInputPolicy>,
    context_ascii_mode: Weak<AtomicU8>,
    pending: Weak<Mutex<PendingRuntimeInput>>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
impl WeakRuntimeSession {
    fn upgrade(&self) -> Option<ImeRuntimeSession> {
        Some(ImeRuntimeSession {
            dirs: self.dirs.clone(),
            inner: self.inner.upgrade()?,
            policy: self.policy.upgrade()?,
            context_ascii_mode: self.context_ascii_mode.upgrade()?,
            pending: self.pending.upgrade()?,
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
#[derive(Clone)]
struct ImeRuntimeState {
    user_data_dir: Option<PathBuf>,
    shared_data_dir: Option<String>,
    ime_host: bool,
    #[cfg(windows)]
    libjd_server: bool,
}

/// The data directories librime is running for.
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct RimeDataDirs {
    user: PathBuf,
    shared: String,
}

/// State of the single librime instance the process has.
///
/// librime keeps its service, its session map and its config, table and prism
/// caches in process globals, so initializing, finalizing and reloading are
/// process-wide events no matter which [`ImeRuntime`] triggers them. Keeping
/// the barrier, the generation and the session registry per runtime would let
/// one runtime finalize librime while another runtime's sessions still hold
/// live session ids.
///
/// Reloads lock `initialized` → write barrier → `sessions` → session inner →
/// global input → librime. UI paths pin a read barrier, release `initialized`,
/// and try their session/native locks before accessing shared input state.
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
struct ProcessRimeState {
    /// The directories librime is initialized for, `None` while it is down.
    initialized: Mutex<Option<RimeDataDirs>>,
    libjd: Mutex<Option<runtime_libjd::ActiveJd>>,
    /// Bumped whenever librime was torn down; a session whose generation is
    /// behind rebuilds its engine before its next call.
    generation: AtomicU64,
    /// Held for reading while a session touches its engine and for writing
    /// while librime is torn down, so that no session call can race a
    /// finalize/initialize cycle.
    reload_barrier: RwLock<()>,
    /// Every session handed out in this process. A teardown must drop their
    /// engines before finalizing librime; entries die with their session.
    sessions: Mutex<Vec<WeakRuntimeSession>>,
    build: EngineBuildLock,
    prewarm_pass: EngineBuildLock,
    package_reload: Mutex<Option<(PathBuf, String, u64)>>,
    prewarm_scheduled: AtomicBool,
    lua_retry_after: Mutex<Option<(PathBuf, Instant)>>,
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    global_input: Mutex<Option<global_input::GlobalInput>>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
static PROCESS_RIME: ProcessRimeState = ProcessRimeState {
    initialized: Mutex::new(None),
    libjd: Mutex::new(None),
    generation: AtomicU64::new(0),
    reload_barrier: RwLock::new(()),
    sessions: Mutex::new(Vec::new()),
    build: EngineBuildLock::new(),
    prewarm_pass: EngineBuildLock::new(),
    package_reload: Mutex::new(None),
    prewarm_scheduled: AtomicBool::new(false),
    lua_retry_after: Mutex::new(None),
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    global_input: Mutex::new(None),
};

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
impl ProcessRimeState {
    /// Drop the engine of every live session so that librime is left without a
    /// single strong reference to its cached config, tables and prisms.
    /// The ascii_mode of each engine is carried over to its replacement.
    /// Callers hold the reload barrier for writing.
    fn drop_live_engines(&self) {
        let mut sessions = lock_ignore_poison(&self.sessions);
        sessions.retain(|session| {
            if let Some(pending) = session.pending.upgrade() {
                let mut pending = lock_ignore_poison(&pending);
                pending.package_menu = None;
                pending.armed_modifier_tap = None;
            }
            let Some(session) = session.inner.upgrade() else {
                return false;
            };
            let mut inner = lock_ignore_poison(&session);
            inner.usable = None;
            if let Some(engine) = inner.engine.take() {
                inner.carried_ascii_mode = Some(engine.is_ascii_mode());
                inner.carried_english_mode = Some(inner.english_mode.is_english(engine.as_ref()));
            }
            true
        });
    }

    fn register(&self, session: &ImeRuntimeSession) {
        let mut sessions = lock_ignore_poison(&self.sessions);
        sessions.retain(|session| session.inner.strong_count() > 0);
        sessions.push(WeakRuntimeSession {
            dirs: session.dirs.clone(),
            inner: Arc::downgrade(&session.inner),
            policy: Arc::downgrade(&session.policy),
            context_ascii_mode: Arc::downgrade(&session.context_ascii_mode),
            pending: Arc::downgrade(&session.pending),
        });
    }

    /// Callers hold the reload barrier; acknowledge only after every live
    /// session has been rebuilt and received the same request.
    fn apply_switch_request(
        &self,
        root: &Path,
        extra: Option<&dyn ImeBackend>,
        request: Option<&SchemaSwitchRequest>,
    ) -> Result<(), String> {
        if request.is_none() {
            return Ok(());
        }
        let active_schema = request
            .and_then(|request| request.schema_id.clone())
            .or_else(|| current_schema_id_from_dir(root));
        let schema_root = desktop::loaded_schema_root(root);
        let schema_root = schema_root.as_path();
        let sessions = lock_ignore_poison(&self.sessions);
        let apply = |engine: &dyn ImeBackend| -> Result<(), String> {
            if let Some(request) = request {
                #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
                {
                    let mut global = lock_ignore_poison(&self.global_input);
                    let global =
                        global_input::GlobalInput::get(&mut global, engine, root, schema_root);
                    if let Some(id) = request.schema_id.as_deref() {
                        global.request_schema(id);
                    }
                    if !engine.is_composing() {
                        global.apply(engine, schema_root)?;
                    }
                    let before = global_input::InputState::read(engine);
                    engine.apply_switch_request(request, false)?;
                    global.publish(&before, &global_input::InputState::read(engine));
                }
                #[cfg(any(target_os = "android", target_os = "ios"))]
                engine.apply_switch_request(request, false)?;
            }
            Ok(())
        };
        let mut applied = false;
        let mut live = None;
        for session in sessions.iter().filter_map(WeakRuntimeSession::upgrade) {
            let mut inner = lock_ignore_poison(&session.inner);
            if !inner.ime_host || inner.user_data_dir != root {
                continue;
            }
            ImeRuntimeSession::rebuild_engine(&mut inner, root)?;
            let ImeRuntimeSessionInner {
                engine,
                applied_input_version,
                #[cfg(any(target_os = "android", target_os = "ios"))]
                pending_schema_id,
                ..
            } = &mut *inner;
            if let Some(engine) = engine.as_deref() {
                apply(engine)?;
                #[cfg(any(target_os = "android", target_os = "ios"))]
                if let Some(id) = request.and_then(|request| request.schema_id.as_deref()) {
                    if engine.is_composing() {
                        *pending_schema_id = Some(id.to_owned());
                    } else {
                        engine.select_schema(id)?;
                        engine.apply_reset_switch_request(schema_root)?;
                        engine.apply_ascii_mode(false);
                        publish_current_schema(schema_root, id);
                    }
                }
                if session.policy.current() == InputContextPolicy::default()
                    && session.context_ascii_mode.load(Ordering::Acquire) == 0
                {
                    let state = global_input::InputState::read(engine);
                    if Some(&state.schema_id) == active_schema.as_ref() {
                        live = Some(state);
                    }
                }
                *applied_input_version = 0;
                applied = true;
            }
        }
        if let Some(engine) = extra {
            apply(engine)?;
            #[cfg(any(target_os = "android", target_os = "ios"))]
            if let Some(id) = request.and_then(|request| request.schema_id.as_deref()) {
                engine.select_schema(id)?;
                engine.apply_reset_switch_request(schema_root)?;
                publish_current_schema(schema_root, id);
            }
            let state = global_input::InputState::read(engine);
            if Some(&state.schema_id) == active_schema.as_ref() {
                live = Some(state);
            }
            applied = true;
        }
        if applied {
            if let Some(request) = request {
                schema_switch_request::acknowledge(schema_root, request.generation)?;
                if let Some(state) = live {
                    self.publish_live_switches(root, schema_root, &state);
                }
            }
        }
        Ok(())
    }

    fn apply_pending_switch_request(&self, root: &Path) -> Result<(), String> {
        let request = RimeBackend::prepare_switch_request(root)?;
        self.apply_switch_request(root, None, request.as_ref())
    }

    fn publish_live_switches(&self, root: &Path, schema_root: &Path, state: &global_input::InputState) {
        if let Some(dirs) = desktop::loaded_runtime().filter(|dirs| dirs.user_data_dir == schema_root) {
            schema_switch_request::publish_live_state(root, &dirs, state);
        }
    }

    /// Tear librime down when it is up for different directories, so that the
    /// caller can bring it back up for the ones it wants. Every live engine is
    /// dropped first, exactly like a reload.
    /// The caller holds the reload barrier for writing through initialization.
    fn shutdown_for_dir_change(
        &self,
        initialized: &mut Option<RimeDataDirs>,
        wanted: &RimeDataDirs,
        package_changed: bool,
    ) -> Result<(), String> {
        match initialized.as_ref() {
            None => return Ok(()),
            Some(current) if current == wanted && !package_changed => return Ok(()),
            Some(_) => {}
        }
        self.drop_live_engines();
        if package_changed {
            for session in lock_ignore_poison(&self.sessions)
                .iter()
                .filter_map(WeakRuntimeSession::upgrade)
            {
                let mut inner = lock_ignore_poison(&session.inner);
                inner.carried_ascii_mode = None;
                inner.carried_english_mode = None;
                inner.applied_input_version = 0;
                inner.last_input_state = None;
                #[cfg(any(target_os = "android", target_os = "ios"))]
                {
                    inner.pending_schema_id = None;
                }
            }
            *lock_ignore_poison(&self.lua_retry_after) = None;
            *lock_ignore_poison(&rime_lua::SAVE_RETRY) = None;
            rime_lua::RELOAD_PENDING.store(false, Ordering::Release);
        }
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        {
            *lock_ignore_poison(&self.global_input) = None;
        }
        *initialized = None;
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.finalize_backend()
    }
}

/// Finalize librime and initialize it again for these directories.
///
/// Every engine handed out by an [`ImeRuntime`] in this process is dropped
/// first: librime only re-reads a deployment once the last session holding its
/// cached config and dictionaries is gone, and a session that survived the
/// finalize would be talking to a torn-down engine.
#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub fn reinitialize(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
    let _build = PROCESS_RIME
        .build
        .acquire()
        .ok_or("Rime engine build timed out")?;
    let dirs = RimeDataDirs {
        user: PathBuf::from(&user_data_dir),
        shared: shared_data_dir.clone(),
    };
    let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
    let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
    if deployment_in_progress(&dirs.user) {
        return Err("Rime deployment is busy".into());
    }
    let runtime = RuntimeDirs::resolve(&dirs.user, Path::new(&dirs.shared))?;
    let changed = PROCESS_RIME.using_libjd() || desktop::loaded_package_changed(&runtime);
    if changed {
        PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs, true)?;
    } else {
        PROCESS_RIME.drop_live_engines();
        PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
    }
    let result = desktop::reinitialize_resolved_runtime(&dirs.user, &runtime);
    *initialized = result.is_ok().then_some(dirs);
    result
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
pub(crate) fn with_rime_finalized(
    action: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let _build = PROCESS_RIME
        .build
        .acquire()
        .ok_or("Rime engine build timed out")?;
    let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
    let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
    PROCESS_RIME.drop_live_engines();
    *initialized = None;
    PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
    PROCESS_RIME.finalize_backend()?;
    action()
}

/// Release every session and mapped build before a deployment child writes them.
#[cfg(target_os = "windows")]
pub fn prepare_server_deploy() -> Result<(), String> {
    with_rime_finalized(|| Ok(()))
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
struct ImeRuntimeSessionInner {
    user_data_dir: PathBuf,
    ime_host: bool,
    #[cfg(windows)]
    libjd_server: bool,
    /// `None` after teardown. Pending switch requests rebuild all live engines
    /// before acknowledgment; other reloads rebuild lazily on the next access.
    engine: Option<Box<dyn ImeBackend>>,
    usable: Option<Arc<()>>,
    generation: u64,
    /// ascii_mode of the engine that was dropped, restored on the rebuilt one.
    carried_ascii_mode: Option<bool>,
    carried_english_mode: Option<bool>,
    english_mode: english_mode::EnglishModeController,
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pending_schema_id: Option<String>,
    applied_input_version: u64,
    last_input_state: Option<global_input::InputState>,
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
            user_data_dir,
            shared_data_dir,
            ime_host: false,
            #[cfg(windows)]
            libjd_server: false,
        }))
    }

    /// Allow only an IME host process to apply and acknowledge switch requests.
    /// App previews, test input and deployment runtimes must keep the default.
    pub fn with_ime_host(mut self) -> Self {
        Arc::make_mut(&mut self.0).ime_host = true;
        self
    }

    #[cfg(windows)]
    pub fn with_libjd_server(mut self) -> Self {
        Arc::make_mut(&mut self.0).libjd_server = true;
        self
    }

    pub fn switch_package(&self, key: &str) -> Result<(), String> {
        let dirs = self.data_dirs()?;
        switch_ready_package(&dirs.user, Path::new(&dirs.shared), key)
    }

    fn switch_package_from_ime(
        &self,
        key: &str,
        schema_id: Option<&str>,
        finish_composition: impl FnOnce(),
    ) -> Result<(), String> {
        // Keep a local stamp watcher from reloading between publishing the
        // switch and bringing up the selected package's runtime.
        let mut applied = try_lock_ignore_poison(&PROCESS_RIME.package_reload)
            .ok_or("Rime package reload is busy")?;
        let dirs = self.data_dirs()?;
        if !engine_choice::is_enabled(&dirs.user) {
            return Err("The input engine is disabled".into());
        }
        if deployment_in_progress(&dirs.user) {
            return Err("Rime deployment is busy".into());
        }
        isolated_deploy::try_switch_ready_package_with_schema(
            &dirs.user,
            Path::new(&dirs.shared),
            key,
            schema_id,
            PROCESS_RIME.using_libjd(),
        )?;
        let signature = ReloadStamp::current_signature(&dirs.user);
        finish_composition();
        self.reload_without_deploy_inner()?;
        let target = RuntimeDirs::isolated_user_data_dir(&dirs.user, key);
        if !PROCESS_RIME.using_libjd()
            && target.as_deref() != Some(desktop::loaded_schema_root(&dirs.user).as_path())
        {
            return Err(PACKAGE_SWITCH_PENDING.into());
        }
        *applied = signature.map(|signature| (dirs.user, signature, self.generation()));
        Ok(())
    }

    pub fn has_applied_package_switch(&self, signature: &str) -> bool {
        lock_ignore_poison(&PROCESS_RIME.package_reload)
            .as_ref()
            .is_some_and(|(root, applied, generation)| {
                applied == signature
                    && *generation == self.generation()
                    && self.data_dirs().is_ok_and(|dirs| dirs.user == *root)
            })
    }

    fn apply_pending_switch_request(&self, root: &Path) -> Result<(), String> {
        if self.0.ime_host {
            PROCESS_RIME.apply_pending_switch_request(root)?;
        }
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    fn apply_pending_user_data_sync(&self, root: &Path, runtime_root: &Path) -> Result<bool, String> {
        if !self.0.ime_host {
            return Ok(false);
        }
        let Some(generation) = user_data_sync::claim(runtime_root)? else {
            return Ok(false);
        };
        // RimeSyncUserData destroys all native sessions before releasing userdbs.
        PROCESS_RIME.drop_live_engines();
        let outcome = desktop::sync_user_data_and_wait(runtime_root);
        PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
        let rebuilt = PROCESS_RIME.apply_pending_switch_request(root);
        user_data_sync::finish(runtime_root, generation, outcome.and(rebuilt))?;
        Ok(true)
    }

    /// What the linked librime can do natively, see [`EngineCapabilities`].
    pub fn capabilities(&self) -> EngineCapabilities {
        engine_capabilities()
    }

    /// Whether the process-wide librime instance is running for this runtime's
    /// directories. Waits for an in-flight initialization/reload so callers
    /// deciding whether to retire handles do not mistake contention for teardown.
    pub fn is_initialized(&self) -> bool {
        let Ok(dirs) = self.data_dirs() else {
            return false;
        };
        lock_ignore_poison(&PROCESS_RIME.initialized).as_ref() == Some(&dirs)
    }

    /// Release this runtime before removing its isolated data directory.
    pub fn shutdown(&self) -> Result<(), String> {
        let dirs = self.data_dirs()?;
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        if initialized.as_ref() != Some(&dirs) {
            return Ok(());
        }
        let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
        PROCESS_RIME.drop_live_engines();
        *initialized = None;
        PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
        PROCESS_RIME.finalize_backend()
    }

    fn stop_if_disabled(&self, dirs: &RimeDataDirs) -> Result<bool, String> {
        if engine_choice::is_enabled(&dirs.user) {
            return Ok(false);
        }
        self.shutdown()?;
        Ok(true)
    }

    /// Initialize and deploy. Idempotent per process: librime can only run for
    /// one pair of directories, so a second runtime asking for the ones it is
    /// already running for is a no-op instead of a second deployment.
    pub fn init(&self) -> Result<(), String> {
        let _build = PROCESS_RIME
            .build
            .acquire()
            .ok_or("Rime engine build timed out")?;
        let dirs = self.data_dirs()?;
        if self.0.ime_host && self.stop_if_disabled(&dirs)? {
            return Ok(());
        }
        if self.activate_libjd(&dirs, false)? {
            return Ok(());
        }
        if deployment_in_progress(&dirs.user)
            && !deployment_state::failed_deployment_owned_by_process(&dirs.user)
        {
            return Err("Rime deployment is busy".into());
        }
        runtime_log::init_for_engine(&dirs.user);
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        let mut deployed = false;
        let result = (|| {
            let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
            let runtime = RuntimeDirs::resolve(&dirs.user, Path::new(&dirs.shared))?;
            let changed = PROCESS_RIME.using_libjd() || desktop::loaded_package_changed(&runtime);
            if initialized.as_ref() == Some(&dirs) && !changed {
                return Ok(());
            }

            let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
            if deployment_in_progress(&dirs.user)
                && !deployment_state::failed_deployment_owned_by_process(&dirs.user)
            {
                return Err("Rime deployment is busy".into());
            }
            PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs, changed)?;
            deploy(
                dirs.user.to_string_lossy().into_owned(),
                dirs.shared.clone(),
            )?;
            // Deployment may have just enabled or disabled Wanxiang grammar.
            desktop::reinitialize_rime(
                dirs.user.to_string_lossy().into_owned(),
                dirs.shared.clone(),
            )?;
            deployed = true;
            *initialized = Some(dirs);
            Ok(())
        })();
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "init",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                deployed = deployed,
                ok = result.is_ok()
            );
        }
        result
    }

    pub fn init_without_deploy(&self) -> Result<(), String> {
        let _build = PROCESS_RIME
            .build
            .acquire()
            .ok_or("Rime engine build timed out")?;
        let dirs = self.data_dirs()?;
        if self.stop_if_disabled(&dirs)? {
            return Ok(());
        }
        if self.activate_libjd(&dirs, false)? {
            return Ok(());
        }
        runtime_log::init_for_engine(&dirs.user);
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        let result = (|| {
            let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
            let runtime = RuntimeDirs::resolve(&dirs.user, Path::new(&dirs.shared))?;
            let changed = PROCESS_RIME.using_libjd() || desktop::loaded_package_changed(&runtime);
            if initialized.as_ref() == Some(&dirs) && !changed {
                return Ok(());
            }
            if deployment_in_progress(&dirs.user) {
                return Err("Rime deployment is busy".into());
            }

            let schema_state = resolved_schema_install_state(&runtime);
            if !schema_state.installed {
                return Err(
                    "no KeyTao scheme is installed; install one in the KeyTao app first".into(),
                );
            }
            if !schema_state.deployed {
                return Err(
                    "the installed KeyTao scheme has not been deployed in the KeyTao app".into(),
                );
            }

            #[cfg(target_os = "windows")]
            {
                if windows_rime_build_repair_required(&dirs.user) {
                    return Err(
                        "Windows RIME build repair is pending; open the KeyTao app to finish it"
                            .into(),
                    );
                }
                patch_windows_lua_compatibility(&runtime.user_data_dir)?;
            }

            let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
            PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs, changed)?;
            desktop::setup_resolved_runtime(&dirs.user, &runtime, DeployProcess::InProcess)?;
            *initialized = Some(dirs);
            Ok(())
        })();
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "init",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                deployed = false,
                ok = result.is_ok()
            );
        }
        result
    }

    /// Inspect a private session in the existing mobile runtime without saving a schema.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn preview_candidates(&self, page_size: Option<u8>) -> Result<CandidatePreview, String> {
        let dirs = self.data_dirs()?;
        let initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        let _barrier = PROCESS_RIME
            .reload_barrier
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if initialized.is_none() {
            // An App can open settings before its keyboard/IME has started librime.
            return desktop::preview_deployed_candidates(
                &dirs.user,
                &PreviewRequest { page_size },
                None,
            );
        }
        if initialized.as_ref() != Some(&dirs) {
            return Err("candidate preview runtime is not initialized".into());
        }
        let schema = deployed_schema_id_from_dir(&dirs.user).ok_or("no active deployed schema")?;
        desktop::preview_session(&schema, &PreviewRequest { page_size }, false)
    }

    pub fn reload_without_deploy(&self) -> Result<(), String> {
        self.reload_without_deploy_changed().map(|_| ())
    }

    pub fn reload_without_deploy_changed(&self) -> Result<bool, String> {
        let applied = lock_ignore_poison(&PROCESS_RIME.package_reload);
        if applied.as_ref().is_some_and(|(root, signature, generation)| {
            *generation == self.generation()
                && self.data_dirs().is_ok_and(|dirs| dirs.user == *root)
                && ReloadStamp::current_signature(root).as_deref() == Some(signature.as_str())
        }) {
            return Ok(false);
        }
        self.reload_without_deploy_inner()
    }

    fn reload_without_deploy_inner(&self) -> Result<bool, String> {
        let _build = PROCESS_RIME
            .build
            .acquire()
            .ok_or("Rime engine build timed out")?;
        let dirs = self.data_dirs()?;
        if self.stop_if_disabled(&dirs)? {
            return Ok(true);
        }
        if self.activate_libjd(&dirs, true)? {
            return Ok(true);
        }
        if deployment_in_progress(&dirs.user) {
            return Err("Rime deployment is busy".into());
        }
        runtime_log::init_for_engine(&dirs.user);
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        let result = (|| {
            let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
            if deployment_in_progress(&dirs.user) {
                return Err("Rime deployment is busy".into());
            }
            let runtime = RuntimeDirs::resolve(&dirs.user, Path::new(&dirs.shared))?;
            let package_changed = PROCESS_RIME.using_libjd() || desktop::loaded_package_changed(&runtime);
            if self.0.ime_host
                && runtime.active_package_key.is_some()
                && ReloadStamp::package_key(&ReloadStamp::path(&dirs.user))
                    .is_some_and(|key| runtime.active_package_key.as_deref() != Some(key.as_str()))
            {
                return Err(PACKAGE_SWITCH_PENDING.into());
            }
            let schema_state = resolved_schema_install_state(&runtime);
            if self.0.ime_host
                && !schema_state.deployed
                && initialized.as_ref() == Some(&dirs)
                && PROCESS_RIME.using_libjd()
                && engine_choice::preferred(&dirs.user) == engine_choice::PackageEngine::Rime
            {
                // An explicit mode switch must take effect even without a Rime build.
                let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
                if deployment_in_progress(&dirs.user) {
                    return Err("Rime deployment is busy".into());
                }
                PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs, true)?;
                return Ok(true);
            }
            if schema_state.schemas.is_empty() {
                if runtime.active_package_key.is_some() {
                    return Err("请在键道 App 部署此方案包".into());
                }
                if initialized.as_ref() == Some(&dirs) {
                    let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
                    PROCESS_RIME.drop_live_engines();
                    *initialized = None;
                    PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
                    PROCESS_RIME.finalize_backend()?;
                }
                return Ok(true);
            }
            if !schema_state.deployed {
                return Err(
                    "the installed KeyTao scheme has not been deployed in the KeyTao app".into(),
                );
            }

            let barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
            if deployment_in_progress(&dirs.user) {
                return Err("Rime deployment is busy".into());
            }
            if runtime.active_package_key.is_none() {
                regenerate_rime_lua_for_ime(&dirs.user);
            }
            // A schema signal can replace a deployment signal before the IME sees it.
            if self.0.ime_host
                && initialized.as_ref() == Some(&dirs)
                && !package_changed
                && desktop::loaded_deployment_is_current(&runtime)
                && desktop::loaded_lua_is_current(&dirs.user)
            {
                #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
                if self.apply_pending_user_data_sync(&dirs.user, &runtime.user_data_dir)? {
                    return Ok(true);
                }
                let schema_root = desktop::loaded_schema_root(&dirs.user);
                let request = schema_switch_request::read_at(&schema_root)?;
                let schema_stamp = std::fs::read_to_string(ReloadStamp::path(&dirs.user))
                    .ok()
                    .is_some_and(|stamp| {
                        stamp.starts_with(&format!("schema:{}:", request.generation))
                    });
                if schema_stamp || (request.schema_id.is_some()
                    && schema_switch_request::pending_for_process(&schema_root)?.is_some()) {
                    self.apply_pending_switch_request(&dirs.user)?;
                    return Ok(request.schema_id.is_some());
                }
            }
            if package_changed {
                PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs, true)?;
            } else {
                PROCESS_RIME.drop_live_engines();
                PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
            }
            let result = desktop::reinitialize_resolved_runtime(&dirs.user, &runtime);
            *initialized = result.is_ok().then(|| dirs.clone());
            let result = result.and_then(|()| self.apply_pending_switch_request(&dirs.user));
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            let result = result.and_then(|()| {
                self.apply_pending_user_data_sync(&dirs.user, &runtime.user_data_dir)
                    .map(|_| ())
            });
            drop(barrier);
            result.map(|()| true)
        })();
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "reload",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                deployed = false,
                ok = result.is_ok()
            );
        }
        result
    }

    pub fn generation(&self) -> u64 {
        PROCESS_RIME.generation.load(Ordering::SeqCst)
    }

    /// Rebuild the focused engine first on the caller's worker. Rime also
    /// rebuilds its peers, while ime-jd can restart idle sessions as needed.
    /// Keys can take the build gate between session rebuilds.
    /// Returns whether any session was rebuilt.
    pub fn prewarm(&self, focused: Option<&ImeRuntimeSession>) -> Result<bool, String> {
        let dirs = self.data_dirs()?;
        let pass = PROCESS_RIME
            .prewarm_pass
            .acquire()
            .ok_or("Rime engine build timed out")?;
        PROCESS_RIME
            .prewarm_scheduled
            .store(true, Ordering::Release);
        let mut changed = Vec::new();
        let result = (|| {
            let acquire = || {
                PROCESS_RIME
                    .build
                    .acquire_background()
                    .ok_or("Rime engine build timed out")
            };
            let mut worker = acquire()?;
            let initial_generation = self.generation();
            let mut generation = initial_generation;
            if self.stop_if_disabled(&dirs)? {
                return Ok(self.generation() != initial_generation);
            }
            if deployment_in_progress(&dirs.user) {
                return Err("Rime deployment is busy".into());
            }
            let libjd = PROCESS_RIME.using_libjd();
            let mut focused = focused.cloned();
            loop {
                let mut sessions: Vec<_> = lock_ignore_poison(&PROCESS_RIME.sessions)
                    .iter()
                    .filter_map(WeakRuntimeSession::upgrade)
                    .filter(|session| session.dirs == dirs)
                    .collect();
                if let Some(first) = focused.take().filter(|session| session.dirs == dirs) {
                    sessions.retain(|session| !Arc::ptr_eq(&session.inner, &first.inner));
                    sessions.insert(0, first);
                }
                if libjd {
                    // Each jd_init scans the whole dictionary even when contexts share its bytes.
                    sessions.truncate(1);
                }
                if self.generation() != generation {
                    return Err("Rime generation changed".into());
                }
                // Lua refresh may finalize librime; never do it while holding
                // the pending-input mutex needed by host callbacks.
                if let Some(session) = sessions.first() {
                    session.try_finish_lua_reload();
                    generation = self.generation();
                }
                drop(worker);
                let mut worked = false;
                for session in sessions {
                    let _worker = acquire()?;
                    if self.generation() != generation {
                        return Err("Rime generation changed".into());
                    }
                    let initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
                    if initialized.as_ref() != Some(&dirs) {
                        return Err("librime is running for other data directories".into());
                    }
                    let barrier = PROCESS_RIME
                        .reload_barrier
                        .read()
                        .unwrap_or_else(PoisonError::into_inner);
                    drop(initialized);
                    let mut inner = lock_ignore_poison(&session.inner);
                    if inner.generation != generation || inner.engine.is_none() {
                        let rime = (!libjd).then(desktop::rime_api_lock);
                        if self.generation() != generation {
                            return Err("Rime generation changed".into());
                        }
                        if deployment_in_progress(&dirs.user) {
                            return Err("Rime deployment is busy".into());
                        }
                        ImeRuntimeSession::rebuild_engine(&mut inner, &dirs.user)?;
                        drop(rime);
                        changed.push(session.clone());
                        worked = true;
                    }
                    drop(inner);
                    drop(barrier);
                }
                if !worked || libjd {
                    return Ok(generation != initial_generation || !changed.is_empty());
                }
                worker = acquire()?;
            }
        })();
        PROCESS_RIME
            .prewarm_scheduled
            .store(false, Ordering::Release);
        drop(pass);
        for session in changed {
            let callback = lock_ignore_poison(&session.pending).state_callback.clone();
            if let Some(callback) = callback {
                callback();
            }
        }
        result
    }

    pub fn reload(&self) -> Result<(), String> {
        let _build = PROCESS_RIME
            .build
            .acquire()
            .ok_or("Rime engine build timed out")?;
        let dirs = self.data_dirs()?;
        if self.stop_if_disabled(&dirs)? {
            return Ok(());
        }
        if self.activate_libjd(&dirs, true)? {
            return Ok(());
        }
        if deployment_in_progress(&dirs.user)
            && !deployment_state::failed_deployment_owned_by_process(&dirs.user)
        {
            return Err("Rime deployment is busy".into());
        }
        runtime_log::init_for_engine(&dirs.user);
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);

        let barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
        if deployment_in_progress(&dirs.user)
            && !deployment_state::failed_deployment_owned_by_process(&dirs.user)
        {
            return Err("Rime deployment is busy".into());
        }
        PROCESS_RIME.drop_live_engines();
        if PROCESS_RIME.clear_libjd() {
            *initialized = None;
        }
        // Deploying while an old session keeps librime's config and dictionary
        // caches alive would rebuild the artifacts but keep serving the stale
        // ones, so the engine is torn down first.
        let result = (|| {
            if initialized.is_some() {
                desktop::reinitialize_rime(
                    dirs.user.to_string_lossy().into_owned(),
                    dirs.shared.clone(),
                )?;
            }
            deploy(
                dirs.user.to_string_lossy().into_owned(),
                dirs.shared.clone(),
            )?;
            desktop::reinitialize_rime(
                dirs.user.to_string_lossy().into_owned(),
                dirs.shared.clone(),
            )
        })();
        PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
        *initialized = result.is_ok().then(|| dirs.clone());
        let result = result.and_then(|()| self.apply_pending_switch_request(&dirs.user));
        drop(barrier);
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "reload",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                deployed = result.is_ok(),
                ok = result.is_ok()
            );
        }
        result
    }

    fn data_dirs(&self) -> Result<RimeDataDirs, String> {
        let user = self
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
        Ok(RimeDataDirs { user, shared })
    }

    /// Create a session after a worker initialized this runtime. UI callers
    /// wait for an in-flight engine build, or receive an error after 20 seconds.
    pub fn create_session(&self) -> Result<ImeRuntimeSession, String> {
        self.create_session_inner(false)
    }

    pub fn try_create_spare_session(&self) -> Result<ImeRuntimeSession, String> {
        self.create_session_inner(true)
    }

    fn create_session_inner(&self, spare: bool) -> Result<ImeRuntimeSession, String> {
        let dirs = self.data_dirs()?;
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        let result = (|| {
            let _build = if spare {
                PROCESS_RIME
                    .build
                    .try_acquire_background()
                    .ok_or("Rime engine build is busy")?
            } else {
                PROCESS_RIME
                    .build
                    .acquire()
                    .ok_or("Rime engine build timed out")?
            };
            let disabled = if spare {
                if !engine_choice::is_enabled(&dirs.user) || deployment_in_progress(&dirs.user) {
                    return Err("Rime is disabled or deployment is busy".into());
                }
                false
            } else {
                self.stop_if_disabled(&dirs)?
            };
            // Pin the read barrier before releasing the directory check, so
            // teardown cannot race creation or block peers behind this session.
            let initialized = try_lock_ignore_poison(&PROCESS_RIME.initialized)
                .ok_or("librime initialization or reload is busy")?;
            let has_backend = initialized.as_ref() == Some(&dirs);
            // Frontends keep a session while the engine is off or ime-jd is waiting for a scheme.
            if !has_backend
                && !disabled
                && (spare || initialized.is_some() || !self.libjd_requested(&dirs.user))
            {
                return Err("librime is running for other data directories".into());
            }
            let barrier = try_read_ignore_poison(&PROCESS_RIME.reload_barrier)
                .ok_or("librime initialization or reload is busy")?;
            drop(initialized);
            let generation = PROCESS_RIME.generation.load(Ordering::SeqCst);
            let libjd = has_backend && PROCESS_RIME.using_libjd();
            let rime = if has_backend && !libjd {
                Some(desktop::try_rime_api_lock().ok_or("librime session is busy")?)
            } else {
                None
            };
            let mut english_mode = english_mode::EnglishModeController::default();
            let engine = if has_backend {
                let schema_root = PROCESS_RIME.schema_root(&dirs.user);
                let request = if self.0.ime_host && !libjd {
                    // Schema requests need the registered session's Lua reload barrier first.
                    let pending = schema_switch_request::pending_for_process(&schema_root)?;
                    if pending
                        .as_ref()
                        .is_some_and(|request| request.schema_id.is_some())
                    {
                        None
                    } else {
                        RimeBackend::prepare_switch_request(&dirs.user)?
                    }
                } else {
                    None
                };
                let engine = PROCESS_RIME.create_backend(&dirs.user)?;
                let mode = english_mode::read_english_mode(&dirs.user);
                let switch_key = english_mode::read_english_switch_key(&dirs.user);
                let schemas = engine.list_schemas();
                if self.0.ime_host {
                    engine.apply_reset_switch_request(&schema_root)?;
                }
                #[cfg(any(target_os = "android", target_os = "ios"))]
                {
                    // Mobile keyboards own Chinese/English state via their dedicated 中/英 key;
                    // ignore upstream desktop defaults like `ascii_mode: reset: 1`.
                    engine.apply_ascii_mode(false);
                }
                #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
                if self.0.ime_host {
                    let mut global = lock_ignore_poison(&PROCESS_RIME.global_input);
                    let global = global_input::GlobalInput::get(
                        &mut global,
                        engine.as_ref(),
                        &dirs.user,
                        &schema_root,
                    );
                    global.english_mode.configure_with_switch_key(
                        mode,
                        switch_key.clone(),
                        schemas.clone(),
                    );
                    global.apply(engine.as_ref(), &schema_root)?;
                }
                english_mode.configure_with_switch_key(mode, switch_key, schemas);
                if request.is_some() {
                    PROCESS_RIME.apply_switch_request(
                        &dirs.user,
                        Some(engine.as_ref()),
                        request.as_ref(),
                    )?;
                }
                Some(engine)
            } else {
                None
            };
            let usable = engine
                .is_some()
                .then(|| deployment_state::engine_loaded(&dirs.user));
            let inner = Arc::new(Mutex::new(ImeRuntimeSessionInner {
                user_data_dir: dirs.user.clone(),
                ime_host: self.0.ime_host,
                #[cfg(windows)]
                libjd_server: self.0.libjd_server,
                engine,
                usable,
                generation,
                carried_ascii_mode: None,
                carried_english_mode: None,
                english_mode,
                #[cfg(any(target_os = "android", target_os = "ios"))]
                pending_schema_id: None,
                applied_input_version: 0,
                last_input_state: None,
            }));
            let session = ImeRuntimeSession {
                dirs,
                inner,
                policy: Arc::new(SessionInputPolicy::new(InputContextPolicy::default())),
                context_ascii_mode: Arc::new(AtomicU8::new(0)),
                pending: Arc::new(Mutex::new(PendingRuntimeInput::default())),
            };
            PROCESS_RIME.register(&session);
            drop(rime);
            drop(barrier);
            Ok(session)
        })();
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "create_session",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                ok = result.is_ok()
            );
        }
        result
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
        self.with_package_menu(|menu| menu.state())
            .or_else(|| {
                self.with_engine_access(false, false, |engine, _, mode, _, _| {
                    let mut state = engine.state();
                    mode.switch_key.apply_to_state(&mut state);
                    state
                })
            })
            .unwrap_or_else(ImeState::empty)
    }

    fn with_package_menu<T>(&self, action: impl FnOnce(&mut switcher_model::Menu) -> T) -> Option<T> {
        let mut pending = lock_ignore_poison(&self.pending);
        if pending.package_menu.is_some()
            && self.input_policy().composing
            && engine_choice::is_enabled(&self.dirs.user)
        {
            pending.package_menu.as_mut().map(action)
        } else {
            pending.package_menu = None;
            None
        }
    }

    fn close_package_menu(&self) {
        lock_ignore_poison(&self.pending).package_menu = None;
    }

    fn decide_package_menu(
        &self,
        decide: impl FnOnce(&mut switcher_model::Menu) -> switcher_model::Decision,
    ) -> Option<KeyProcessResult> {
        let decision = {
            let mut pending = lock_ignore_poison(&self.pending);
            pending.package_menu.as_ref()?;
            if !self.input_policy().composing || !engine_choice::is_enabled(&self.dirs.user) {
                pending.package_menu = None;
                return None;
            }
            let decision = decide(pending.package_menu.as_mut()?);
            if decision != switcher_model::Decision::Stay {
                pending.package_menu = None;
            }
            decision
        };
        self.apply_package_decision(decision)
    }

    pub fn should_forward_consumed_shortcut(&self, key: u32, mask: u32) -> bool {
        key_policy::should_forward_consumed_shortcut(key, mask)
            && !(switcher_model::is_hotkey(key, mask)
                && engine_choice::is_enabled(&self.dirs.user)
                && !RuntimeDirs::needs_migration(&self.dirs.user))
    }

    fn has_package_hotkey(&self, keys: &[(u32, u32)]) -> bool {
        try_lock_ignore_poison(&self.inner).is_some_and(|inner| {
            inner.ime_host
                && keys.iter().any(|&(key, mask)| {
                    switcher_model::is_hotkey(key, mask)
                        || inner.engine.as_ref().is_some_and(|engine| {
                            engine
                                .switcher_hotkeys()
                                .iter()
                                .any(|hotkey| hotkey.matches_switcher(key, mask))
                        })
                })
        }) && engine_choice::is_enabled(&self.dirs.user)
    }

    /// Called on the prewarm worker, without engine locks. Frontends can post
    /// their existing reload notification to refresh candidates on the UI thread.
    pub fn set_state_callback(&self, callback: Option<Arc<dyn Fn() + Send + Sync>>) {
        lock_ignore_poison(&self.pending).state_callback = callback;
    }

    /// Inspect candidates without waiting for or rebuilding a busy engine.
    pub fn peek_state(&self) -> ImeState {
        if let Some(state) = self.with_package_menu(|menu| menu.state()) {
            return state;
        }
        let state = self.with_engine_access_mode(false, false, false, |engine, _, mode, _, _| {
            let mut state = engine.state();
            mode.switch_key.apply_to_state(&mut state);
            state
        });
        if state.is_none() && self.runtime_can_resume() {
            self.schedule_prewarm();
        }
        state.unwrap_or_else(ImeState::empty)
    }

    pub fn process_key_result(&self, keycode: u32, mask: u32) -> Option<KeyProcessResult> {
        self.process_runtime_key(RuntimeKey::Key(keycode, mask))
    }

    /// Process one client batch under a single runtime/session access.
    pub fn process_keys_result(&self, keys: &[(u32, u32)]) -> Option<KeyProcessResult> {
        if keys.is_empty() {
            return None;
        }
        if !self.input_policy().composing {
            return Some(KeyProcessResult {
                state: ImeState::empty(),
                accepted: false,
            });
        }
        self.process_keys_now(keys)
    }

    fn process_runtime_key(&self, key: RuntimeKey) -> Option<KeyProcessResult> {
        if !self.input_policy().composing {
            return Some(KeyProcessResult {
                state: ImeState::empty(),
                accepted: false,
            });
        }
        self.process_runtime_key_now(key)
    }

    fn runtime_can_resume(&self) -> bool {
        try_lock_ignore_poison(&PROCESS_RIME.initialized)
            .map(|initialized| initialized.as_ref() == Some(&self.dirs))
            .unwrap_or(true)
            || PROCESS_RIME.prewarm_scheduled.load(Ordering::Acquire)
    }

    fn schedule_prewarm(&self) {
        if self.lua_reload_deferred() {
            return;
        }
        if PROCESS_RIME.prewarm_scheduled.swap(true, Ordering::AcqRel) {
            return;
        }
        let focused = self.clone();
        std::thread::spawn(move || {
            #[cfg(target_os = "macos")]
            {
                extern "C" {
                    fn pthread_set_qos_class_self_np(class: u32, priority: i32) -> i32;
                }
                const QOS_CLASS_UTILITY: u32 = 0x11;
                unsafe { pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) };
            }
            let runtime =
                ImeRuntime::with_dirs(focused.dirs.user.clone(), focused.dirs.shared.clone());
            if let Err(error) = runtime.prewarm(Some(&focused)) {
                crate::rt_log!(Level::Info, "error", "prewarm", msg = error);
            }
        });
    }

    fn process_runtime_key_now(&self, key: RuntimeKey) -> Option<KeyProcessResult> {
        match key {
            RuntimeKey::Key(keycode, mask) => self.process_key_now(keycode, mask),
            RuntimeKey::Enter => self.process_enter_now(),
        }
    }

    fn process_key_now(&self, keycode: u32, mask: u32) -> Option<KeyProcessResult> {
        self.process_keys_now(&[(keycode, mask)])
    }

    fn process_keys_now(&self, keys: &[(u32, u32)]) -> Option<KeyProcessResult> {
        if self.with_package_menu(|_| ()).is_none() && !self.has_package_hotkey(keys) {
            return self.process_native_keys(keys);
        }
        if RuntimeDirs::needs_migration(&self.dirs.user) {
            self.close_package_menu();
            if keys.iter().all(|&(key, _)| key != switcher_model::OPEN_PACKAGE_MENU) {
                return self.process_native_keys(keys);
            }
        }
        key_batch::process_keys(keys, |key, mask| {
            self.process_package_key(key, mask)
                .or_else(|| self.process_native_keys(&[(key, mask)]))
                .unwrap_or_else(|| KeyProcessResult { state: self.peek_state(), accepted: false })
        })
    }

    fn process_package_key(&self, key: u32, mask: u32) -> Option<KeyProcessResult> {
        if let Some(result) = self.decide_package_menu(|menu| menu.key(key, mask)) {
            return Some(result);
        }
        if self.has_package_hotkey(&[(key, mask)]) {
            let libjd_active = PROCESS_RIME.using_libjd();
            let opened = self.with_engine_and_policy(|engine, policy| {
                let state = engine.state();
                if !policy.composing || state.schema_id == ".default" {
                    return None;
                }
                let active_schemas = engine.list_schemas();
                let active_switches = engine.active_switches();
                let menu = switcher_model::Menu::load(
                    &self.dirs.user,
                    Path::new(&self.dirs.shared),
                    state,
                    active_schemas,
                    active_switches,
                    libjd_active,
                )?;
                let state = menu.state();
                lock_ignore_poison(&self.pending).package_menu = Some(menu);
                Some(KeyProcessResult {
                    state,
                    accepted: true,
                })
            });
            if let Some(result) = opened.flatten() {
                return Some(result);
            }
        }
        (key == switcher_model::OPEN_PACKAGE_MENU)
            .then(|| KeyProcessResult { state: self.peek_state(), accepted: false })
    }

    fn apply_package_decision(&self, decision: switcher_model::Decision) -> Option<KeyProcessResult> {
        use switcher_model::Decision;
        match decision {
            Decision::Pass => return None,
            Decision::SelectActiveSchema(schema_id) => {
                let committed = self
                    .with_engine_and_policy(|engine, policy| {
                        if !policy.composing || !engine.is_composing() {
                            return None;
                        }
                        let raw = engine.raw_input().filter(|raw| !raw.is_empty());
                        engine.commit_composition().committed.or(raw)
                    })
                    .flatten();
                let mut state = match self.select_schema(&schema_id) {
                    Ok(state) => state,
                    Err(error) => {
                        crate::rt_log!(Level::Info, "error", "schema_switch", msg = error);
                        self.state()
                    }
                };
                if let Some(mut text) = committed {
                    if let Some(next) = state.committed.take() {
                        text.push_str(&next);
                    }
                    state.committed = Some(text);
                }
                return Some(KeyProcessResult {
                    state,
                    accepted: true,
                });
            }
            Decision::SwitchPackage {
                package_key,
                schema_id,
            } => {
                let mut result = self.switch_package_selection(&package_key, schema_id.as_deref());
                // Once the desktop menu handles a key, even a rejected switch consumes it.
                result.accepted = true;
                return Some(result);
            }
            Decision::ToggleSwitch {
                name,
                options,
                next_value,
            } => {
                let state = if options.is_empty() {
                    let enabled = next_value != 0;
                    let state = if name == "ascii_mode" {
                        self.set_ascii_mode(enabled)
                    } else {
                        self.set_option(&name, enabled)
                    };
                    self.with_engine(|engine| {
                        engine.persist_switch_options(&[(name.clone(), enabled)]);
                    });
                    state
                } else {
                    let pairs: Vec<_> = options
                        .iter()
                        .enumerate()
                        .map(|(index, opt)| (opt.clone(), index == next_value))
                        .collect();
                    let mut last = None;
                    for (opt, enabled) in &pairs {
                        last = self.set_option(opt, *enabled);
                    }
                    self.with_engine(|engine| {
                        engine.persist_switch_options(&pairs);
                    });
                    last
                };
                return Some(KeyProcessResult {
                    state: state.unwrap_or_else(|| self.state()),
                    accepted: true,
                });
            }
            Decision::Close | Decision::Stay => {}
        }
        Some(KeyProcessResult { state: self.state(), accepted: true })
    }

    fn switch_package_selection(
        &self,
        package_key: &str,
        schema_id: Option<&str>,
    ) -> KeyProcessResult {
        let runtime = ImeRuntime::with_dirs(&self.dirs.user, &self.dirs.shared).with_ime_host();
        #[cfg(windows)]
        let runtime = if lock_ignore_poison(&self.inner).libjd_server {
            runtime.with_libjd_server()
        } else {
            runtime
        };
        let mut committed = None;
        let switched = runtime.switch_package_from_ime(package_key, schema_id, || {
            committed = self
                .with_engine_and_policy(|engine, policy| {
                    if !policy.composing {
                        return None;
                    }
                    let raw = engine.raw_input().filter(|raw| !raw.is_empty());
                    engine.commit_composition().committed.or(raw)
                })
                .flatten();
        });
        let accepted = switched.is_ok();
        let mut state = match switched {
            Ok(()) => match schema_id {
                Some(id)
                    if !PROCESS_RIME.using_libjd()
                        && self.current_schema().as_ref().map(|s| s.id.as_str()) != Some(id) =>
                {
                    self.select_schema(id).unwrap_or_else(|_| self.state())
                }
                _ => self.state(),
            },
            Err(error) => {
                crate::rt_log!(Level::Info, "error", "package_switch", msg = error);
                self.state()
            }
        };
        if let Some(mut text) = committed {
            if let Some(native) = state.committed.take() {
                text.push_str(&native);
            }
            state.committed = Some(text);
        }
        KeyProcessResult { state, accepted }
    }

    fn process_native_keys(&self, keys: &[(u32, u32)]) -> Option<KeyProcessResult> {
        // Reading the policy and handling the key happen in one critical
        // section: a context that turns sensitive between the two would
        // otherwise still hand this key to librime.
        let caps = keys
            .iter()
            .any(|&(keycode, mask)| keycode == 0xffe5 && mask & RIME_RELEASE_MASK == 0);
        if caps {
            // Physical Caps Lock supersedes a host's context-only language override.
            self.context_ascii_mode.store(0, Ordering::Release);
        }
        self.with_engine_access_mode(!caps, caps, true, |engine, _, mode, ime_host, input| {
            key_batch::process_keys(keys, |keycode, mask| {
                // TSF may pass an unshifted accept name from a declared binding.
                let (keycode, mask) =
                    if cfg!(windows) && mask != 0 && engine.has_declared_shortcut(keycode, mask) {
                        (keycode, mask)
                    } else {
                        key_policy::normalize_key_for_modifiers(keycode, mask)
                    };
                let mask = rime_modifier_mask(mask) | (mask & RIME_MOD_LOCK);
                let (policy, clear_composition) = self.policy.take_for_engine();
                if clear_composition {
                    engine.discard_composition();
                }
                if policy.composing {
                    if cfg!(not(any(target_os = "android", target_os = "ios")))
                        && !mode.switch_key.is_schema()
                    {
                        if let Some(mut intercepted) = self.intercept_custom_english_switch_key(
                            engine,
                            mode,
                            keycode,
                            mask,
                            ime_host,
                            input,
                        ) {
                            mode.switch_key.apply_to_state(&mut intercepted.state);
                            return intercepted;
                        }
                    }
                    let desktop_dictionary =
                        cfg!(not(any(target_os = "android", target_os = "ios")))
                            && mode.mode == english_mode::EnglishMode::Schema
                            && keycode != 0xffe5;
                    if desktop_dictionary
                        && matches!(keycode, 0xffe1 | 0xffe2)
                        && rime_modifier_mask(mask) & !(RIME_MOD_SHIFT | RIME_RELEASE_MASK) == 0
                        && mode.is_english(engine)
                        && mode
                            .chinese_schema()
                            .is_some_and(|schema| engine.shift_switch_disabled(schema, keycode))
                    {
                        return KeyProcessResult {
                            state: engine.state(),
                            accepted: false,
                        };
                    }
                    if keycode == key_policy::XK_KP_DECIMAL
                        && mask & !(RIME_MOD_LOCK | RIME_MOD_SHIFT) == 0
                    {
                        let current = engine.state();
                        if current.preedit.is_empty() && current.candidates.is_empty() {
                            return KeyProcessResult {
                                state: current,
                                accepted: false,
                            };
                        }
                        let raw = engine.raw_input().filter(|raw| !raw.is_empty());
                        let mut committed_state = engine.commit_composition();
                        let mut text = committed_state
                            .committed
                            .take()
                            .or(raw)
                            .unwrap_or_default();
                        text.push('.');
                        committed_state.committed = Some(text);
                        mode.switch_key.apply_to_state(&mut committed_state);
                        return KeyProcessResult {
                            state: committed_state,
                            accepted: true,
                        };
                    }
                    let before = engine.is_ascii_mode();
                    let mut result = if mask & !RIME_MOD_LOCK == 0 {
                        key_policy::CandidateArrowAction::from_keycode(keycode)
                            .map(|action| {
                                use key_policy::CandidateArrowAction::*;
                                let state = engine.state();
                                let Some(index) = key_policy::highlighted_candidate_index(&state)
                                else {
                                    return KeyProcessResult {
                                        state,
                                        accepted: false,
                                    };
                                };
                                let state = match action {
                                    PreviousCandidate if index > 0 => {
                                        engine.highlight_candidate_on_page(index - 1)
                                    }
                                    NextCandidate if index + 1 < state.candidates.len() => {
                                        engine.highlight_candidate_on_page(index + 1)
                                    }
                                    PreviousCandidate | NextCandidate
                                        if (action == PreviousCandidate && state.page > 0)
                                            || (action == NextCandidate && !state.is_last_page) =>
                                    {
                                        let backward = action == PreviousCandidate;
                                        let page = engine.change_page(backward);
                                        let target = if backward {
                                            page.candidates.len().saturating_sub(1)
                                        } else {
                                            0
                                        };
                                        // Native paging may preserve the page-local offset.
                                        if page.page != state.page
                                            && page.committed.is_none()
                                            && !page.candidates.is_empty()
                                            && page.highlighted_candidate_index != target
                                        {
                                            engine.highlight_candidate_on_page(target)
                                        } else {
                                            page
                                        }
                                    }
                                    PreviousPage if state.page > 0 => engine.change_page(true),
                                    NextPage if !state.is_last_page => engine.change_page(false),
                                    _ => state,
                                };
                                KeyProcessResult {
                                    state,
                                    accepted: true,
                                }
                            })
                            .unwrap_or_else(|| engine.process_key_result(keycode, mask))
                    } else {
                        engine.process_key_result(keycode, mask)
                    };
                    if before != result.state.ascii_mode {
                        self.context_ascii_mode.store(0, Ordering::Release);
                    }
                    // Mobile keyboards already select schemas themselves. Desktop
                    // ascii_composer toggles become dictionary-mode switches only
                    // when the user explicitly selected the schema preference.
                    if desktop_dictionary {
                        if let Err(error) = mode.reconcile_ascii_toggle(engine, before, &mut result)
                        {
                            crate::rt_log!(
                                Level::Info,
                                "error",
                                "english_mode_switch",
                                msg = error
                            );
                        }
                    }
                    if let Err(error) =
                        self.apply_reset_switch_request(engine, ime_host, input, &mut result.state)
                    {
                        crate::rt_log!(Level::Info, "error", "schema_switch_reset", msg = error);
                    }
                    mode.switch_key.apply_to_state(&mut result.state);
                    result
                } else {
                    // A sensitive context never reaches librime: no composition, no
                    // candidates and nothing for the user dictionary to learn.
                    let mut state = engine.state();
                    mode.switch_key.apply_to_state(&mut state);
                    KeyProcessResult {
                        state,
                        accepted: false,
                    }
                }
            })
        })
        .flatten()
    }

    fn trigger_custom_english_toggle(
        &self,
        engine: &dyn ImeBackend,
        mode: &mut english_mode::EnglishModeController,
        target_english: bool,
        accepted: bool,
        show_hint: bool,
        ime_host: bool,
        input: Option<&global_input::InputState>,
    ) -> KeyProcessResult {
        self.context_ascii_mode.store(0, Ordering::Release);
        let committed = if engine.is_composing() {
            engine.commit_composition().committed
        } else {
            None
        };
        let mut state = match mode.switch(engine, target_english) {
            Ok(state) => state,
            Err(error) => {
                crate::rt_log!(Level::Info, "error", "english_mode_switch", msg = error);
                engine.set_ascii_mode(target_english)
            }
        };
        if let Some(prefix) = committed {
            state.committed = Some(format!("{prefix}{}", state.committed.unwrap_or_default()));
        }
        if let Err(error) = self.apply_reset_switch_request(engine, ime_host, input, &mut state) {
            crate::rt_log!(Level::Info, "error", "schema_switch_reset", msg = error);
        }
        if show_hint {
            state.switch_hint = Some(if state.is_english_mode() {
                "英".into()
            } else {
                "中".into()
            });
        }
        KeyProcessResult { state, accepted }
    }

    fn intercept_custom_english_switch_key(
        &self,
        engine: &dyn ImeBackend,
        mode: &mut english_mode::EnglishModeController,
        keycode: u32,
        mask: u32,
        ime_host: bool,
        input: Option<&global_input::InputState>,
    ) -> Option<KeyProcessResult> {
        let own_modifier = match keycode {
            0xffe1 | 0xffe2 => Some(RIME_MOD_SHIFT),
            0xffe3 | 0xffe4 => Some(RIME_MOD_CONTROL),
            0xffe9 | 0xffea => Some(RIME_MOD_ALT),
            _ => None,
        };
        if let Some(own_modifier) = own_modifier {
            let other = rime_modifier_mask(mask) & !(own_modifier | RIME_RELEASE_MASK);
            let mut pending = lock_ignore_poison(&self.pending);
            if mask & RIME_RELEASE_MASK == 0 {
                pending.armed_modifier_tap = (other == 0).then_some((keycode, Instant::now()));
                return Some(KeyProcessResult {
                    state: engine.state(),
                    accepted: false,
                });
            }
            let armed = pending.armed_modifier_tap.take();
            drop(pending);
            if other == 0
                && armed.is_some_and(|(key, started)| {
                    key == keycode && started.elapsed() < Duration::from_millis(500)
                })
                && mode.switch_key.matches_modifier_tap(keycode)
            {
                let target_english = !mode.is_english(engine);
                return Some(self.trigger_custom_english_toggle(
                    engine,
                    mode,
                    target_english,
                    true,
                    false,
                    ime_host,
                    input,
                ));
            }
            return Some(KeyProcessResult {
                state: engine.state(),
                accepted: false,
            });
        }

        if keycode == 0xffe5 {
            lock_ignore_poison(&self.pending).armed_modifier_tap = None;
            if mask & RIME_RELEASE_MASK == 0 && mode.switch_key.matches_caps_lock() {
                let target_english = mask & RIME_MOD_LOCK == 0;
                return Some(self.trigger_custom_english_toggle(
                    engine,
                    mode,
                    target_english,
                    false,
                    false,
                    ime_host,
                    input,
                ));
            }
            return Some(KeyProcessResult {
                state: engine.state(),
                accepted: false,
            });
        }

        if mask & RIME_RELEASE_MASK == 0 {
            lock_ignore_poison(&self.pending).armed_modifier_tap = None;
        }
        if mode.switch_key.matches_shortcut(keycode, mask) {
            let target_english = !mode.is_english(engine);
            return Some(self.trigger_custom_english_toggle(
                engine,
                mode,
                target_english,
                true,
                true,
                ime_host,
                input,
            ));
        }
        None
    }

    /// `Return` handling shared by every frontend, see [`RimeBackend::process_enter`].
    pub fn process_enter(&self) -> Option<KeyProcessResult> {
        self.process_runtime_key(RuntimeKey::Enter)
    }

    fn process_enter_now(&self) -> Option<KeyProcessResult> {
        if !self.input_policy().composing {
            return Some(KeyProcessResult {
                state: ImeState::empty(),
                accepted: false,
            });
        }
        if let Some(result) = self.process_package_key(key_policy::XK_RETURN, 0) {
            return Some(result);
        }
        self.with_engine_access_mode(true, false, true, |engine, policy, mode, ime_host, before| {
            let mut result = if policy.composing {
                let mut result = engine.process_enter();
                if let Err(error) =
                    self.apply_reset_switch_request(engine, ime_host, before, &mut result.state)
                {
                    crate::rt_log!(Level::Info, "error", "schema_switch_reset", msg = error);
                }
                result
            } else {
                KeyProcessResult {
                    state: engine.state(),
                    accepted: false,
                }
            };
            mode.switch_key.apply_to_state(&mut result.state);
            result
        })
    }

    /// What the current input context allows.
    pub fn input_policy(&self) -> InputContextPolicy {
        self.policy.current()
    }

    /// Declare what the current input context allows, e.g. on focus change.
    ///
    /// Turning composing off discards whatever was being composed, so the
    /// returned state is the one the frontend has to apply. A busy engine
    /// defers the discard, but records the policy immediately; later keys
    /// cannot pass the sensitive-context check using an older policy.
    ///
    /// The policy is recorded even when no engine can be built, otherwise a
    /// context that turned sensitive while librime was down would come back as
    /// a composing one.
    pub fn set_input_policy(&self, policy: InputContextPolicy) -> Option<ImeState> {
        self.policy.set(policy);
        if !policy.composing {
            self.close_package_menu();
        } else if let Some(state) = self.with_package_menu(|menu| menu.state()) {
            return Some(state);
        }
        self.with_engine_access_mode(false, false, false, |engine, policy, mode, _, _| {
            let mut state = if !policy.composing {
                engine.clear_composition()
            } else {
                engine.state()
            };
            mode.switch_key.apply_to_state(&mut state);
            state
        })
    }

    pub fn select_candidate_on_page(&self, index: usize) -> Option<ImeState> {
        if let Some(result) = self.decide_package_menu(|menu| {
            menu.page_index(index).map_or(switcher_model::Decision::Stay, |index| menu.select(index))
        }) {
            return Some(result.state);
        }
        self.with_engine_and_mode(|engine, _, _, ime_host, before| {
            let mut state = engine.select_candidate_on_page(index);
            if let Err(error) =
                self.apply_reset_switch_request(engine, ime_host, before, &mut state)
            {
                crate::rt_log!(Level::Info, "error", "schema_switch_reset", msg = error);
            }
            state
        })
    }

    pub fn highlight_candidate_on_page(&self, index: usize) -> Option<ImeState> {
        if let Some(state) = self.with_package_menu(|menu| {
            menu.highlight(index);
            menu.state()
        }) {
            return Some(state);
        }
        self.with_engine(|engine| engine.highlight_candidate_on_page(index))
    }

    pub fn candidate_is_user_phrase_on_page(&self, index: usize) -> Option<bool> {
        if self.with_package_menu(|_| ()).is_some() {
            return Some(false);
        }
        self.with_engine(|engine| engine.candidate_is_user_phrase_on_page(index))
    }

    pub fn delete_candidate_on_page_result(&self, index: usize) -> Option<(ImeState, bool)> {
        if let Some(state) = self.with_package_menu(|menu| menu.state()) {
            return Some((state, false));
        }
        self.with_engine(|engine| engine.delete_candidate_on_page_result(index))
    }

    pub fn select_candidate_global(&self, index: usize) -> Option<ImeState> {
        if let Some(result) = self.decide_package_menu(|menu| menu.select(index)) {
            return Some(result.state);
        }
        self.with_engine_and_mode(|engine, _, _, ime_host, before| {
            let mut state = engine.select_candidate_global(index);
            if let Err(error) =
                self.apply_reset_switch_request(engine, ime_host, before, &mut state)
            {
                crate::rt_log!(Level::Info, "error", "schema_switch_reset", msg = error);
            }
            state
        })
    }

    /// Commit whatever is being composed; the path a frontend takes when the
    /// input context ends and the composition must not be lost.
    pub fn commit_composition(&self) -> Option<ImeState> {
        self.with_input_change(|engine| engine.commit_composition())
    }

    /// Discard whatever is being composed; the path a frontend takes when the
    /// input context ends and the composition must not reach the client.
    pub fn clear_composition(&self) -> Option<ImeState> {
        self.with_input_change(|engine| engine.clear_composition())
    }

    pub fn commit_raw_input(&self) -> Option<ImeState> {
        self.with_input_change(|engine| engine.commit_raw_input())
    }

    pub fn raw_input(&self) -> Option<String> {
        self.with_engine(|engine| engine.raw_input()).flatten()
    }

    pub fn all_candidates_limited(&self, max_count: usize) -> Option<Vec<Candidate>> {
        if let Some(candidates) = self.with_package_menu(|menu| menu.candidates(max_count)) {
            return Some(candidates);
        }
        self.with_engine(|engine| engine.all_candidates_limited(max_count))
    }

    pub fn list_schemas(&self) -> Option<Vec<SchemaInfo>> {
        self.with_engine(|engine| engine.list_schemas())
    }

    pub fn list_switcher_packages(&self) -> Vec<switcher_model::Package> {
        if !self.can_use_mobile_package_switcher() {
            return Vec::new();
        }
        switcher_model::mobile_packages(
            &self.dirs.user,
            Path::new(&self.dirs.shared),
            PROCESS_RIME.using_libjd(),
        )
    }

    pub fn select_package_schema(
        &self,
        package_key: &str,
        schema_id: &str,
    ) -> Option<KeyProcessResult> {
        if schema_id.is_empty() || !self.can_use_mobile_package_switcher() {
            return None;
        }
        self.close_package_menu();
        Some(self.switch_package_selection(package_key, Some(schema_id)))
    }

    fn can_use_mobile_package_switcher(&self) -> bool {
        self.policy.current().composing
            && try_lock_ignore_poison(&self.inner).is_some_and(|inner| inner.ime_host)
            && engine_choice::is_enabled(&self.dirs.user)
    }

    pub fn current_schema(&self) -> Option<SchemaInfo> {
        self.with_engine(|engine| engine.current_schema()).flatten()
    }

    pub fn schema_switches(&self) -> Option<Vec<SchemaSwitch>> {
        self.with_engine(|engine| engine.schema_switches())
    }

    pub fn select_schema(&self, schema_id: &str) -> Result<ImeState, String> {
        self.close_package_menu();
        self.context_ascii_mode.store(0, Ordering::Release);
        self.with_engine_and_mode(|engine, _, mode, ime_host, before| {
            let mut state = match engine.select_schema(schema_id) {
                Ok(state) => state,
                Err(error) => {
                    if let Some(before) = before {
                        before.restore(engine, &PROCESS_RIME.schema_root(&self.dirs.user))?;
                    }
                    return Err(error);
                }
            };
            let schema_root = PROCESS_RIME.schema_root(&self.dirs.user);
            if schema_root != self.dirs.user {
                persist_selected_schema(&schema_root, schema_id)?;
                write_current_schema(&schema_root, schema_id)?;
            }
            self.apply_reset_switch_request(engine, ime_host, before, &mut state)?;
            #[cfg(any(target_os = "android", target_os = "ios"))]
            {
                engine.apply_ascii_mode(false);
                state.ascii_mode = false;
            }
            mode.switch_key.apply_to_state(&mut state);
            Ok(state)
        })
        .ok_or_else(|| "Rime session is unavailable".to_string())?
    }

    pub fn change_page(&self, backward: bool) -> Option<ImeState> {
        if let Some(state) = self.with_package_menu(|menu| {
            menu.change_page(backward);
            menu.state()
        }) {
            return Some(state);
        }
        self.with_input_change(|engine| engine.change_page(backward))
    }

    pub fn reset(&self) -> Option<ImeState> {
        lock_ignore_poison(&self.pending).armed_modifier_tap = None;
        self.with_input_change(|engine| engine.reset())
    }

    pub fn is_ascii_mode(&self) -> bool {
        self.with_engine(|engine| engine.is_ascii_mode()).unwrap_or(false)
    }

    pub fn set_ascii_mode(&self, enabled: bool) -> Option<ImeState> {
        self.context_ascii_mode.store(0, Ordering::Release);
        self.with_input_change(|engine| engine.set_ascii_mode(enabled))
    }

    /// Host conversion compartments are local to a context, until its next focus.
    pub fn set_context_ascii_mode(&self, enabled: Option<bool>) -> Option<ImeState> {
        self.context_ascii_mode.store(
            enabled.map_or(0, |enabled| 1 + u8::from(enabled)),
            Ordering::Release,
        );
        self.with_engine(|engine| {
            enabled.map_or_else(|| engine.state(), |enabled| engine.set_ascii_mode(enabled))
        })
    }

    /// Logical English mode, including a dictionary schema with ASCII off.
    /// Keep `is_ascii_mode` for deciding whether keys should pass through.
    pub fn is_english_mode(&self) -> bool {
        self.with_engine_access(false, false, |engine, _, mode, _, _| {
            mode.is_english(engine)
        })
        .unwrap_or(false)
    }

    /// Switch desktop input using the saved ASCII/dictionary preference.
    /// Mobile clients that manage schemas themselves keep using set_ascii_mode.
    pub fn set_english_mode(&self, enabled: bool) -> Result<ImeState, String> {
        self.context_ascii_mode.store(0, Ordering::Release);
        self.with_engine_and_mode(|engine, _, mode, ime_host, before| {
            let mut state = mode.switch(engine, enabled)?;
            self.apply_reset_switch_request(engine, ime_host, before, &mut state)?;
            mode.switch_key.apply_to_state(&mut state);
            Ok(state)
        })
        .ok_or_else(|| "Rime session is unavailable".to_string())?
    }

    fn apply_reset_switch_request(
        &self,
        engine: &dyn ImeBackend,
        ime_host: bool,
        before: Option<&global_input::InputState>,
        state: &mut ImeState,
    ) -> Result<(), String> {
        if ime_host
            && state.schema_id != ".default"
            && before.is_none_or(|before| before.schema_id != state.schema_id)
        {
            engine.apply_reset_switch_request(&PROCESS_RIME.schema_root(&self.dirs.user))?;
            let committed = state.committed.take();
            *state = engine.state();
            state.committed = committed;
        }
        Ok(())
    }

    pub fn get_option(&self, option_name: &str) -> bool {
        self.with_engine(|engine| engine.get_option(option_name))
            .unwrap_or(false)
    }

    pub fn set_option(&self, option_name: &str, enabled: bool) -> Option<ImeState> {
        self.context_ascii_mode.store(0, Ordering::Release);
        self.with_input_change(|engine| engine.set_option(option_name, enabled))
    }

    /// What the linked librime can do natively, see [`EngineCapabilities`].
    pub fn capabilities(&self) -> EngineCapabilities {
        self.with_engine(|engine| engine.capabilities())
            .unwrap_or_else(EngineCapabilities::none)
    }

    /// Whether [`ImeRuntimeSession::change_page`] pages through librime instead
    /// of replaying the `-`/`=` bindings a schema may not have.
    pub fn supports_native_paging(&self) -> bool {
        self.capabilities().supports_native_paging()
    }

    /// Whether [`ImeRuntimeSession::select_candidate_on_page`] selects through
    /// librime instead of sending a select key the schema may not have.
    pub fn supports_candidate_selection(&self) -> bool {
        self.capabilities().supports_candidate_selection()
    }

    /// Run `action` on an engine that matches the current generation,
    /// rebuilding it first when a reload invalidated the previous one.
    /// Lock order: reload barrier → session → librime.
    fn with_engine<T>(&self, action: impl FnOnce(&dyn ImeBackend) -> T) -> Option<T> {
        self.with_engine_and_policy(|engine, _| action(engine))
    }

    fn with_input_change(&self, action: impl FnOnce(&dyn ImeBackend) -> ImeState) -> Option<ImeState> {
        self.close_package_menu();
        self.with_engine_and_mode(|engine, _, mode, _, _| {
            let mut state = action(engine);
            mode.switch_key.apply_to_state(&mut state);
            state
        })
    }

    /// Same as [`ImeRuntimeSession::with_engine`], but the action also sees the
    /// input context policy, so that policy checks and the librime call they
    /// guard cannot be split by another thread.
    fn with_engine_and_policy<T>(
        &self,
        action: impl FnOnce(&dyn ImeBackend, InputContextPolicy) -> T,
    ) -> Option<T> {
        self.with_engine_access(false, false, |engine, policy, _, _, _| {
            action(engine, policy)
        })
    }

    fn with_engine_and_mode<T>(
        &self,
        action: impl FnOnce(
            &dyn ImeBackend,
            InputContextPolicy,
            &mut english_mode::EnglishModeController,
            bool,
            Option<&global_input::InputState>,
        ) -> T,
    ) -> Option<T> {
        self.with_engine_access(true, false, action)
    }

    fn with_engine_access<T>(
        &self,
        publish: bool,
        invalidate_peers: bool,
        action: impl FnOnce(
            &dyn ImeBackend,
            InputContextPolicy,
            &mut english_mode::EnglishModeController,
            bool,
            Option<&global_input::InputState>,
        ) -> T,
    ) -> Option<T> {
        self.with_engine_access_mode(publish, invalidate_peers, true, action)
    }

    fn with_engine_access_mode<T>(
        &self,
        publish: bool,
        invalidate_peers: bool,
        rebuild: bool,
        action: impl FnOnce(
            &dyn ImeBackend,
            InputContextPolicy,
            &mut english_mode::EnglishModeController,
            bool,
            Option<&global_input::InputState>,
        ) -> T,
    ) -> Option<T> {
        #[cfg(all(
            test,
            any(target_os = "windows", target_os = "macos", target_os = "linux")
        ))]
        if !runtime_access_tests::allow_engine_access() {
            return None;
        }
        // Wait before taking any lock needed by the builder.
        // Keep the loaded schema usable while another process compiles its replacement.
        let use_loaded_schema = deployment_state::cached_deployment_in_progress(&self.dirs.user)
            && try_lock_ignore_poison(&self.inner).is_some_and(|inner| {
                inner.engine.is_some()
                    && inner.generation == PROCESS_RIME.generation.load(Ordering::SeqCst)
            });
        let _build = if rebuild && !use_loaded_schema {
            Some(PROCESS_RIME.build.acquire()?)
        } else {
            None
        };
        let ime_host = try_lock_ignore_poison(&self.inner)?.ime_host && !PROCESS_RIME.using_libjd();
        let pending_lua = !rebuild && ime_host && self.lua_reload_needed();
        let lua_ready = !ime_host
            || if rebuild {
                self.try_finish_lua_reload()
            } else {
                !deployment_state::cached_deployment_in_progress(&self.dirs.user)
                    && !self.lua_reload_deferred()
                    && (!pending_lua || self.try_finish_lua_reload_mode(false))
            };
        if pending_lua && !lua_ready {
            self.schedule_prewarm();
            return None;
        }
        let result =
            self.with_engine_access_inner(publish, invalidate_peers, lua_ready, rebuild, action);
        // F4 and mobile selection publish while holding session locks.
        if ime_host && rebuild {
            self.try_finish_lua_reload();
        }
        result
    }

    fn lua_reload_needed(&self) -> bool {
        if deployment_state::cached_deployment_in_progress(&self.dirs.user)
            || self.lua_reload_deferred()
            || desktop::loaded_schema_root(&self.dirs.user) != self.dirs.user
        {
            return false;
        }
        rime_lua::RELOAD_PENDING.load(Ordering::Acquire)
            || schema_switch_request::pending_for_process(&self.dirs.user)
                .ok()
                .flatten()
                .is_some_and(|request| request.schema_id.is_some())
            || !desktop::loaded_lua_is_current(&self.dirs.user)
    }

    fn lua_reload_deferred(&self) -> bool {
        if PROCESS_RIME.using_libjd() {
            return false;
        }
        lock_ignore_poison(&PROCESS_RIME.lua_retry_after)
            .as_ref()
            .is_some_and(|(root, after)| root == &self.dirs.user && Instant::now() < *after)
    }

    fn try_finish_lua_reload(&self) -> bool {
        self.try_finish_lua_reload_mode(true)
    }

    fn try_finish_lua_reload_mode(&self, reinitialize: bool) -> bool {
        if deployment_state::cached_deployment_in_progress(&self.dirs.user) {
            return false;
        }
        let Some(initialized) = try_lock_ignore_poison(&PROCESS_RIME.initialized) else {
            return false;
        };
        if initialized.as_ref() != Some(&self.dirs) {
            return false;
        }
        if PROCESS_RIME.using_libjd() {
            return true;
        }
        if desktop::loaded_schema_root(&self.dirs.user) != self.dirs.user {
            return true;
        }
        drop(initialized);
        if lock_ignore_poison(&PROCESS_RIME.lua_retry_after)
            .as_ref()
            .is_some_and(|(root, after)| root == &self.dirs.user && Instant::now() < *after)
        {
            return false;
        }
        let result =
            schema_switch_request::pending_for_process(&self.dirs.user).and_then(|request| {
                self.finish_lua_reload(request.and_then(|r| r.schema_id).as_deref(), reinitialize)
            });
        match result {
            Ok(()) => {
                let ready = !rime_lua::RELOAD_PENDING.load(Ordering::Acquire);
                if ready {
                    *lock_ignore_poison(&PROCESS_RIME.lua_retry_after) = None;
                }
                ready
            }
            Err(error) => {
                if reinitialize
                    && try_lock_ignore_poison(&PROCESS_RIME.initialized)
                        .is_some_and(|initialized| initialized.as_ref() == Some(&self.dirs))
                {
                    defer_rime_lua_reload(&self.dirs.user, &error);
                }
                false
            }
        }
    }

    fn finish_lua_reload(&self, requested: Option<&str>, reinitialize: bool) -> Result<(), String> {
        let mut initialized =
            try_lock_ignore_poison(&PROCESS_RIME.initialized).ok_or("Rime is busy")?;
        if initialized.as_ref() != Some(&self.dirs) {
            return Err("Rime session belongs to another user directory".into());
        }
        if requested.is_none()
            && !rime_lua::RELOAD_PENDING.load(Ordering::Acquire)
            && desktop::loaded_lua_is_current(&self.dirs.user)
        {
            return Ok(());
        }
        let _barrier = PROCESS_RIME
            .reload_barrier
            .try_write()
            .map_err(|_| "Rime is busy")?;
        if deployment_in_progress(&self.dirs.user) {
            return Err("Rime deployment is busy".into());
        }
        let retry = lock_ignore_poison(&rime_lua::SAVE_RETRY).clone();
        let retry = retry.filter(|(root, _)| root == &self.dirs.user);
        let save_error = retry.as_ref().and_then(|(root, id)| {
            match persist_selected_schema(root, id).and_then(|()| write_current_schema(root, id)) {
                Ok(()) => {
                    *lock_ignore_poison(&rime_lua::SAVE_RETRY) = None;
                    None
                }
                Err(error) => Some(error),
            }
        });
        let requested = if save_error.is_some() {
            retry.as_ref().map(|(_, id)| id.as_str())
        } else {
            requested
        };
        match requested {
            Some(id) => {
                rime_lua::select(&self.dirs.user, id)?;
            }
            None => regenerate_active_rime_lua(&self.dirs.user)?,
        }
        if !desktop::loaded_lua_is_current(&self.dirs.user) {
            if !reinitialize {
                return Err("Rime Lua reload requires background rebuilding".into());
            }
            PROCESS_RIME.drop_live_engines();
            let result = desktop::reinitialize_rime(
                self.dirs.user.to_string_lossy().into_owned(),
                self.dirs.shared.clone(),
            );
            PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
            *initialized = result.is_ok().then(|| self.dirs.clone());
            result?;
            if save_error.is_none() {
                PROCESS_RIME.apply_pending_switch_request(&self.dirs.user)?;
            }
        }
        // Consume under the directory lock and write barrier, excluding publishers.
        if let Some(error) = save_error {
            defer_rime_lua_reload(&self.dirs.user, &error);
        } else {
            rime_lua::RELOAD_PENDING.store(false, Ordering::Release);
        }
        Ok(())
    }

    fn with_engine_access_inner<T>(
        &self,
        publish: bool,
        invalidate_peers: bool,
        lua_ready: bool,
        rebuild: bool,
        action: impl FnOnce(
            &dyn ImeBackend,
            InputContextPolicy,
            &mut english_mode::EnglishModeController,
            bool,
            Option<&global_input::InputState>,
        ) -> T,
    ) -> Option<T> {
        // A surviving session must not rebuild against another runtime's
        // deployment. Take locks in the same order as initialization, and keep
        // the read barrier pinned before releasing the directory check.
        let initialized = try_lock_ignore_poison(&PROCESS_RIME.initialized)?;
        if initialized.as_ref() != Some(&self.dirs) {
            return None;
        }
        let libjd = PROCESS_RIME.using_libjd();
        let schema_root = PROCESS_RIME.schema_root(&self.dirs.user);
        if !libjd && lua_ready
            && self
                .inner
                .try_lock()
                .ok()
                .is_some_and(|inner| inner.ime_host)
            && schema_switch_request::pending_for_process(&schema_root)
                .ok()
                .flatten()
                .is_some()
        {
            if let Ok(_barrier) = PROCESS_RIME.reload_barrier.try_write() {
                if !rebuild {
                    let generation = PROCESS_RIME.generation.load(Ordering::SeqCst);
                    let sessions = try_lock_ignore_poison(&PROCESS_RIME.sessions)?;
                    let current = sessions
                        .iter()
                        .filter_map(|session| session.inner.upgrade())
                        .all(|session| {
                            try_lock_ignore_poison(&session).is_some_and(|inner| {
                                inner.user_data_dir != self.dirs.user
                                    || (inner.generation == generation && inner.engine.is_some())
                            })
                        });
                    if !current {
                        self.schedule_prewarm();
                        return None;
                    }
                }
                let _rime = desktop::try_rime_api_lock()?;
                PROCESS_RIME
                    .apply_pending_switch_request(&self.dirs.user)
                    .ok()?;
            }
        }
        // Builds have already been awaited without holding runtime locks.
        let _barrier = try_read_ignore_poison(&PROCESS_RIME.reload_barrier)?;
        drop(initialized);
        let mut inner = try_lock_ignore_poison(&self.inner)?;
        let _rime = if libjd { None } else { Some(desktop::try_rime_api_lock()?) };
        if !rebuild
            && (inner.engine.is_none()
                || inner.generation != PROCESS_RIME.generation.load(Ordering::SeqCst))
        {
            return None;
        }
        self.refresh_if_needed(&mut inner).ok()?;
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some(id) = inner.pending_schema_id.clone() {
            let engine = inner.engine.as_deref()?;
            if !engine.is_composing() {
                engine.select_schema(&id).ok()?;
                engine.apply_reset_switch_request(&schema_root).ok()?;
                publish_current_schema(&schema_root, &id);
                inner.pending_schema_id = None;
            }
        }
        let ime_host = inner.ime_host;
        let (policy, clear_composition) = self.policy.take_for_engine();
        let ImeRuntimeSessionInner {
            engine,
            english_mode,
            applied_input_version,
            last_input_state,
            #[cfg(any(target_os = "android", target_os = "ios"))]
            pending_schema_id,
            ..
        } = &mut *inner;
        let engine = engine.as_deref()?;
        let first_input = last_input_state.is_none();
        if clear_composition {
            engine.discard_composition();
        }
        let context_ascii_mode = self.context_ascii_mode.load(Ordering::Acquire);
        if context_ascii_mode != 0 {
            engine.apply_ascii_mode(context_ascii_mode == 2);
        }
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        if ime_host {
            let mut global = lock_ignore_poison(&PROCESS_RIME.global_input);
            let global =
                global_input::GlobalInput::get(&mut global, engine, &self.dirs.user, &schema_root);
            if policy.composing && context_ascii_mode == 0 {
                if *applied_input_version != global.version && !engine.is_composing() {
                    global.apply(engine, &schema_root).ok()?;
                    *applied_input_version = global.version;
                }
                let before = publish
                    .then(|| global_input::InputState::read(engine))
                    .and_then(|state| {
                        if state.is_switcher() {
                            last_input_state.clone()
                        } else {
                            Some(state)
                        }
                    });
                if let Some(before) = &before {
                    *last_input_state = Some(before.clone());
                }
                let result = action(
                    engine,
                    policy,
                    &mut global.english_mode,
                    ime_host,
                    before.as_ref(),
                );
                if let Some(before) = before {
                    let after = global_input::InputState::read(engine);
                    let changed = global.publish(&before, &after);
                    if (changed || first_input) && policy.learning && !libjd {
                        PROCESS_RIME.publish_live_switches(&self.dirs.user, &schema_root, global.state());
                    }
                    if !after.is_switcher() {
                        *last_input_state = Some(after.clone());
                    }
                    if global.matches(&after) {
                        *applied_input_version = global.version;
                    }
                } else if invalidate_peers {
                    // Caps Lock's ASCII state belongs to this composer's internal toggle.
                    // Invalidate peers without turning it into a global language preference.
                    global.version += 1;
                    *applied_input_version = global.version;
                }
                return Some(result);
            }
            *applied_input_version = 0;
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let _ = (publish, invalidate_peers, applied_input_version);
        let before = (ime_host && publish)
            .then(|| global_input::InputState::read(engine))
            .and_then(|state| {
                if state.is_switcher() {
                    last_input_state.clone()
                } else {
                    Some(state)
                }
            });
        if let Some(before) = &before {
            *last_input_state = Some(before.clone());
        }
        let result = action(engine, policy, english_mode, ime_host, before.as_ref());
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some(before) = before {
            let after = global_input::InputState::read(engine);
            if !after.is_switcher() {
                if before.schema_id != after.schema_id {
                    *pending_schema_id = None;
                    publish_current_schema(&schema_root, &after.schema_id);
                }
                if policy == InputContextPolicy::default()
                    && context_ascii_mode == 0
                    && !libjd
                    && (first_input || before != after)
                {
                    PROCESS_RIME.publish_live_switches(&self.dirs.user, &schema_root, &after);
                }
                *last_input_state = Some(after);
            }
        }
        Some(result)
    }

    fn refresh_if_needed(&self, inner: &mut ImeRuntimeSessionInner) -> Result<(), String> {
        Self::rebuild_engine(inner, &self.dirs.user)
    }

    fn rebuild_engine(inner: &mut ImeRuntimeSessionInner, root: &Path) -> Result<(), String> {
        let current = PROCESS_RIME.generation.load(Ordering::SeqCst);
        if inner.generation == current && inner.engine.is_some() {
            return Ok(());
        }
        let started = runtime_log::enabled(Level::Info).then(Instant::now);
        let result = (|| {
            // Drop the previous engine before creating the replacement: librime
            // hands a new session the cached config and dictionaries as long as any
            // other session still references them.
            inner.usable = None;
            if let Some(engine) = inner.engine.take() {
                inner.carried_ascii_mode = Some(engine.is_ascii_mode());
                inner.carried_english_mode = Some(inner.english_mode.is_english(engine.as_ref()));
            }
            let engine = PROCESS_RIME.create_backend(root)?;
            let schema_root = PROCESS_RIME.schema_root(root);
            let mode = english_mode::read_english_mode(root);
            let switch_key = english_mode::read_english_switch_key(root);
            let schemas = engine.list_schemas();
            inner
                .english_mode
                .configure_with_switch_key(mode, switch_key.clone(), schemas.clone());
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            if inner.ime_host {
                let mut global = lock_ignore_poison(&PROCESS_RIME.global_input);
                let global =
                    global_input::GlobalInput::get(&mut global, engine.as_ref(), root, &schema_root);
                global
                    .english_mode
                    .configure_with_switch_key(mode, switch_key, schemas);
                global.apply(engine.as_ref(), &schema_root)?;
                inner.carried_ascii_mode = None;
                inner.carried_english_mode = None;
            }
            // Restore live language state before applying any new app request.
            let carried_ascii = inner.carried_ascii_mode.take();
            if let Some(ascii_mode) = carried_ascii {
                engine.apply_ascii_mode(ascii_mode);
            }
            if cfg!(not(any(target_os = "android", target_os = "ios"))) {
                if let Some(english) = inner.carried_english_mode.take() {
                    if let Err(error) = inner.english_mode.restore(engine.as_ref(), english) {
                        // A removed or broken optional dictionary must not
                        // prevent the rebuilt session from accepting input.
                        engine.apply_ascii_mode(english);
                        crate::rt_log!(Level::Info, "error", "english_mode_restore", msg = error);
                    }
                }
            }
            if inner.ime_host {
                engine.apply_reset_switch_request(&schema_root)?;
            }
            #[cfg(any(target_os = "android", target_os = "ios"))]
            if carried_ascii.is_none() {
                engine.apply_ascii_mode(false);
            }
            inner.engine = Some(engine);
            inner.generation = current;
            inner.applied_input_version = 0;
            inner.last_input_state = None;
            inner.usable = Some(deployment_state::engine_loaded(root));
            Ok(())
        })();
        if let Some(started) = started {
            crate::rt_log!(
                Level::Info,
                "rime",
                "engine_rebuild",
                dur_ms = started.elapsed().as_secs_f64() * 1000.0,
                ok = result.is_ok()
            );
        }
        result
    }
}

#[cfg(test)]
fn preferred_schema_location(user_data_dir: &Path) -> Option<(PathBuf, String)> {
    deployed_schema_id_from_dir(user_data_dir).map(|schema| (user_data_dir.to_path_buf(), schema))
}

pub fn deployed_schemas(root: &Path) -> Vec<SchemaInfo> {
    if let Some(schema) = runtime_libjd::active_libjd_schema(root) {
        return vec![schema];
    }
    let Ok(dirs) = RuntimeDirs::resolve(root, root) else {
        return Vec::new();
    };
    resolved_deployed_schemas(&dirs)
}

pub fn package_switchable_schemas(
    control_root: &Path,
    shared: &Path,
    data_root: Option<&Path>,
    package_key: &str,
) -> Vec<SchemaInfo> {
    if let Some(schema) = runtime_libjd::package_libjd_schema_in_store(
        control_root,
        data_root,
        Some(package_key),
    ) {
        return vec![schema];
    }
    let Some(dirs) = data_root
        .and_then(|data| RuntimeDirs::for_package_in_store(control_root, shared, data, package_key))
        .or_else(|| RuntimeDirs::for_package(control_root, shared, package_key))
    else {
        return Vec::new();
    };
    let deployed = resolved_deployed_schemas(&dirs);
    if !deployed.is_empty() {
        return deployed;
    }

    let mut ids = dirs
        .shared_data_dir
        .parent()
        .and_then(|pkg_dir| runtime_dirs::read_regular_file(&pkg_dir.join("package.json")))
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|meta| meta.get("selected_schemas")?.as_array().cloned())
        .map(|arr| {
            arr.into_iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if ids.is_empty() {
        for name in ["default.custom.yaml", "default-custom.yaml", "default.yaml"] {
            if let Ok(content) = std::fs::read_to_string(dirs.shared_data_dir.join(name)) {
                let parsed = parse_schema_list(&content);
                if !parsed.is_empty() {
                    ids = parsed;
                    break;
                }
            }
        }
    }

    if ids.is_empty() {
        let normalized = package_key.strip_prefix("builtin/").unwrap_or(package_key);
        if dirs
            .shared_data_dir
            .join(format!("{normalized}.schema.yaml"))
            .is_file()
        {
            ids.push(normalized.to_owned());
        } else if let Ok(entries) = std::fs::read_dir(&dirs.shared_data_dir) {
            let mut sources = std::collections::BTreeSet::new();
            let mut deps = std::collections::BTreeSet::new();
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if let Some(id) = name.strip_suffix(".schema.yaml") {
                    sources.insert(id.to_owned());
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        deps.extend(parse_schema_dependencies(&content));
                    }
                }
            }
            ids.extend(sources.into_iter().filter(|id| !deps.contains(id)));
        }
    }

    ids.into_iter()
        .filter(|id| valid_rime_build_id(id))
        .map(|id| SchemaInfo {
            name: resolved_schema_name(&dirs, &id).unwrap_or_else(|| id.clone()),
            id,
        })
        .collect()
}

fn resolved_deployed_schemas(dirs: &RuntimeDirs) -> Vec<SchemaInfo> {
    let schemas = std::fs::read_to_string(dirs.staging_dir.join("default.yaml"))
        .ok()
        .map(|content| parse_schema_list(&content))
        .filter(|schemas| !schemas.is_empty())
        .unwrap_or_else(|| resolved_configured_schema_list(dirs));
    effective_schema_list(&dirs.user_data_dir, schemas)
        .into_iter()
        .map(|id| SchemaInfo {
            name: resolved_schema_name(dirs, &id).unwrap_or_else(|| id.clone()),
            id,
        })
        .collect()
}

pub fn current_schema_id_from_dir(root: &Path) -> Option<String> {
    if let Some(schema) = runtime_libjd::active_libjd_schema(root) {
        return Some(schema.id);
    }
    let dirs = RuntimeDirs::resolve(root, root).ok()?;
    std::fs::read_to_string(dirs.user_data_dir.join(".keytao-current-schema"))
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| {
            resolved_deployed_schemas(&dirs)
                .iter()
                .any(|schema| &schema.id == id)
        })
        .or_else(|| resolved_deployed_schema_id(&dirs))
}

pub fn write_current_schema(root: &Path, id: &str) -> Result<(), String> {
    if id == ".default" || !valid_rime_build_id(id) {
        return Err(format!("invalid current schema id: {id}"));
    }
    let dirs = RuntimeDirs::resolve(root, root)?;
    write_current_schema_in_dirs(&dirs, id)
}

fn write_current_schema_in_dirs(dirs: &RuntimeDirs, id: &str) -> Result<(), String> {
    if id == ".default" || !valid_rime_build_id(id) {
        return Err(format!("invalid current schema id: {id}"));
    }
    std::fs::create_dir_all(&dirs.user_data_dir)
        .map_err(|error| format!("save current schema: {error}"))?;
    write_file_atomic(
        &dirs.user_data_dir.join(".keytao-current-schema"),
        id.as_bytes(),
    )
    .map_err(|error| format!("save current schema: {error}"))
}

fn persist_selected_schema(root: &Path, id: &str) -> Result<(), String> {
    let dirs = RuntimeDirs::resolve(root, root)?;
    persist_selected_schema_only(&dirs, id)
}

pub(crate) fn persist_selected_schema_in_dirs(dirs: &RuntimeDirs, id: &str) -> Result<(), String> {
    persist_selected_schema_only(dirs, id)?;
    write_current_schema_in_dirs(dirs, id)
}

fn persist_selected_schema_only(dirs: &RuntimeDirs, id: &str) -> Result<(), String> {
    if id == ".default" || !valid_rime_build_id(id) {
        return Err(format!("invalid current schema id: {id}"));
    }
    let path = dirs.user_data_dir.join("user.yaml");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("refuse symbolic link: {}", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    }
    let mut value = match std::fs::read(&path) {
        Ok(bytes) => serde_yaml::from_slice::<Value>(&bytes)
            .map_err(|error| format!("parse {}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Value::Mapping(Mapping::new())
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let vars = value
        .as_mapping_mut()
        .ok_or("user.yaml must be a YAML mapping")?
        .entry(Value::String("var".into()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .ok_or("user.yaml var must be a YAML mapping")?;
    if vars
        .get(Value::String("previously_selected_schema".into()))
        .and_then(Value::as_str)
        == Some(id)
    {
        return Ok(());
    }
    vars.insert(
        Value::String("previously_selected_schema".into()),
        Value::String(id.into()),
    );
    let content = serde_yaml::to_string(&value).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&dirs.user_data_dir)
        .map_err(|error| format!("save {}: {error}", path.display()))?;
    write_file_atomic(&path, content.as_bytes())
        .map_err(|error| format!("save {}: {error}", path.display()))
}

fn publish_current_schema(root: &Path, id: &str) {
    if PROCESS_RIME.using_libjd() {
        return;
    }
    if let Err(error) =
        persist_selected_schema(root, id).and_then(|()| write_current_schema(root, id))
    {
        *lock_ignore_poison(&rime_lua::SAVE_RETRY) = Some((root.to_path_buf(), id.to_owned()));
        rime_lua::RELOAD_PENDING.store(true, Ordering::Release);
        crate::rt_log!(Level::Info, "error", "schema_selection_save", msg = error);
    } else {
        let mut retry = lock_ignore_poison(&rime_lua::SAVE_RETRY);
        if retry
            .as_ref()
            .is_some_and(|(saved_root, _)| saved_root == root)
        {
            *retry = None;
        }
    }
    match rime_lua::select(root, id) {
        Ok(true) => {
            #[cfg(any(
                target_os = "linux",
                target_os = "windows",
                target_os = "macos",
                target_os = "android",
                target_os = "ios"
            ))]
            {
                *lock_ignore_poison(&PROCESS_RIME.lua_retry_after) = None;
            }
            rime_lua::RELOAD_PENDING.store(true, Ordering::Release);
            if let Err(error) = ReloadStamp::write(root) {
                crate::rt_log!(Level::Info, "error", "schema_lua_reload", msg = error);
            }
        }
        Ok(false) => {}
        Err(error) => {
            rime_lua::RELOAD_PENDING.store(true, Ordering::Release);
            crate::rt_log!(Level::Info, "error", "schema_lua_prepare", msg = error);
        }
    }
}

pub fn deployed_schema_id_from_dir(dir: &Path) -> Option<String> {
    resolved_deployed_schema_id(&RuntimeDirs::resolve(dir, dir).ok()?)
}

fn resolved_deployed_schema_id(dirs: &RuntimeDirs) -> Option<String> {
    let dir = dirs.user_data_dir.as_path();
    let configured = effective_schema_list(dir, resolved_configured_schema_list(dirs));
    if configured.is_empty() {
        return None;
    }
    let deployed = std::fs::read_to_string(dirs.staging_dir.join("default.yaml"))
        .ok()
        .map(|content| parse_schema_list(&content))
        .filter(|schemas| !schemas.is_empty());
    let schemas = effective_schema_list(dir, deployed.as_ref().cloned().unwrap_or(configured));
    // A live selection still applies when saving user.yaml must be retried.
    let selected = lock_ignore_poison(&rime_lua::SAVE_RETRY)
        .as_ref()
        .filter(|(root, _)| root == dir)
        .map(|(_, id)| id.clone())
        .or_else(|| {
            std::fs::read_to_string(dirs.user_data_dir.join("user.yaml"))
                .ok()
                .and_then(|content| serde_yaml::from_str::<Value>(&content).ok())
                .and_then(|value| {
                    value["var"]["previously_selected_schema"]
                        .as_str()
                        .map(str::to_owned)
                })
                .or_else(|| {
                    if dirs.active_package_key.is_some() {
                        return None;
                    }
                    std::fs::read_to_string(dir.join(".keytao-current-schema"))
                        .ok()
                        .map(|id| id.trim().to_owned())
                })
        });
    if let Some(schema) = selected.filter(|schema| {
        valid_rime_build_id(schema)
            && schemas.contains(schema)
            && dirs
                .staging_dir
                .join(format!("{schema}.schema.yaml"))
                .is_file()
    }) {
        return Some(schema);
    }
    if !skipped_schemas(dir).is_empty() {
        return schemas.into_iter().next();
    }
    let pick = if dirs.active_package_key.is_some() {
        preferred_isolated_schema_from_list
    } else {
        preferred_schema_from_list
    };
    match deployed {
        Some(_) => pick(schemas),
        None => pick(resolved_configured_schema_list(dirs)),
    }
}

#[cfg(test)]
fn preferred_schema_id_from_dir(dir: &Path) -> Option<String> {
    preferred_schema_from_list(configured_schema_list_from_dir(dir))
}

fn preferred_isolated_schema_from_list(schemas: Vec<String>) -> Option<String> {
    let mut first_easy_en = None;
    for schema in schemas {
        if schema.trim().is_empty() {
            continue;
        }
        if schema == "easy_en" {
            if first_easy_en.is_none() {
                first_easy_en = Some(schema);
            }
            continue;
        }
        return Some(schema);
    }
    first_easy_en
}

fn preferred_schema_from_list(schemas: Vec<String>) -> Option<String> {
    let mut first_schema = None;
    let mut first_addon = None;
    for schema in schemas {
        if schema.trim().is_empty() {
            continue;
        }
        if is_addon_schema(&schema) {
            if first_addon.is_none() {
                first_addon = Some(schema);
            }
            continue;
        }
        if first_schema.is_none() {
            first_schema = Some(schema.clone());
        }
        if is_keytao_managed_schema(&schema) {
            return Some(schema);
        }
    }
    first_schema.or(first_addon)
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
    if let Ok(value) = serde_yaml::from_str::<Value>(content) {
        if let Some(list) = value
            .get("patch")
            .and_then(|patch| patch.get("schema_list"))
            .or_else(|| value.get("schema_list"))
        {
            return schema_list_from_yaml(Some(list));
        }
    }
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

pub fn parse_schema_name(content: &str) -> Option<String> {
    serde_yaml::from_str::<serde_json::Value>(content)
        .ok()?
        .get("schema")?
        .get("name")?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn resolved_schema_name(dirs: &RuntimeDirs, schema_id: &str) -> Option<String> {
    [
        dirs.staging_dir.join(format!("{schema_id}.schema.yaml")),
        schema_source_path(&dirs.user_data_dir, dirs.package_shared(), schema_id),
    ]
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .find_map(|content| parse_schema_name(&content))
}

pub fn parse_schema_dependencies(content: &str) -> Vec<String> {
    let Ok(root) = serde_yaml::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    let Some(dependencies) = root
        .get("schema")
        .and_then(|schema| schema.get("dependencies"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    dependencies
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|dependency| !dependency.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

const WINDOWS_RIME_BUILD_REPAIR_MARKER: &str = ".keytao-windows-build-repair-v1";
const WINDOWS_RIME_BUILD_REPAIR_MARKER_CONTENT: &str = "complete-v1\n";
const WINDOWS_BUILD_ARTIFACT_REMOVE_RETRIES: usize = 100;
const WINDOWS_BUILD_ARTIFACT_REMOVE_RETRY_DELAY: Duration = Duration::from_millis(100);

fn retryable_windows_build_artifact_error(error: &std::io::Error) -> bool {
    cfg!(target_os = "windows")
        && (error.kind() == std::io::ErrorKind::PermissionDenied
            || matches!(error.raw_os_error(), Some(5 | 32 | 33 | 1224)))
}

fn remove_rime_build_artifact(path: &Path) -> Result<bool, String> {
    let mut retries = 0;
    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error)
                if retryable_windows_build_artifact_error(&error)
                    && retries < WINDOWS_BUILD_ARTIFACT_REMOVE_RETRIES =>
            {
                retries += 1;
                std::thread::sleep(WINDOWS_BUILD_ARTIFACT_REMOVE_RETRY_DELAY);
            }
            Err(error) => {
                return Err(format!(
                    "remove {} after {retries} retries: {error}",
                    path.display()
                ));
            }
        }
    }
}

fn valid_rime_build_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn configured_schema_list_from_dir(user_data_dir: &Path) -> Vec<String> {
    let Ok(dirs) = RuntimeDirs::resolve(user_data_dir, user_data_dir) else {
        return Vec::new();
    };
    resolved_configured_schema_list(&dirs)
}

fn resolved_configured_schema_list(dirs: &RuntimeDirs) -> Vec<String> {
    [
        dirs.user_data_dir.join("default.custom.yaml"),
        dirs.user_data_dir.join("default-custom.yaml"),
        dirs.staging_dir.join("default.yaml"),
        dirs.user_data_dir.join("default.yaml"),
    ]
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .find_map(|content| {
        let schemas = parse_schema_list(&content);
        let has_empty_schema_list = serde_yaml::from_str::<Value>(&content).is_ok_and(|value| {
            value
                .get("patch")
                .and_then(|patch| patch.get("schema_list"))
                .or_else(|| value.get("schema_list"))
                .is_some_and(|list| list.is_null() || list.as_sequence().is_some_and(Vec::is_empty))
        });
        (!schemas.is_empty() || has_empty_schema_list).then_some(schemas)
    })
    .unwrap_or_default()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaInstallState {
    pub installed: bool,
    pub deployed: bool,
    pub schemas: Vec<String>,
}

pub const ACTIVE_SCHEME_MANIFEST: &str = ".keytao-active-scheme.json";

/// Rebase a verified package snapshot after copying it into an isolated runtime.
pub fn rebase_active_scheme_manifest(root: &Path) -> Result<(), String> {
    rime_lua::rebase_manifest(root)
}

fn legacy_active_local_manifest(data_dir: &Path) -> Option<serde_json::Value> {
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data_dir.join("app-state.json")).ok()?).ok()?;
    let key = state["active_scheme"].as_str()?;
    if !key.starts_with("local-") || !state["installed_packages"].as_object()?.contains_key(key) {
        return None;
    }
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package_store::active_manifest(data_dir)).ok()?)
            .ok()?;
    if manifest["scheme_key"]
        .as_str()
        .is_some_and(|actual| actual != key)
    {
        return None;
    }
    manifest
        .as_object_mut()?
        .insert("scheme_key".into(), key.into());
    Some(manifest)
}

fn active_package_schema_ids(user_data_dir: &Path) -> HashSet<String> {
    let read = || -> Option<HashSet<String>> {
        let manifest = match std::fs::read(user_data_dir.join(ACTIVE_SCHEME_MANIFEST)) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).ok()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if default_user_data_dir().as_deref() != Some(user_data_dir) {
                    return None;
                }
                legacy_active_local_manifest(&dirs::data_local_dir()?.join("ink.rea.keytao-app"))?
            }
            Err(_) => return None,
        };
        if Path::new(manifest["root"].as_str()?) != user_data_dir.canonicalize().ok()? {
            return None;
        }
        if let Some(packages) = manifest["packages"].as_object() {
            return Some(
                packages
                    .values()
                    .filter_map(|package| package["schemas"].as_array())
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .filter(|id| valid_rime_build_id(id))
                    .map(str::to_owned)
                    .collect(),
            );
        }
        if manifest["scheme_key"].as_str()?.is_empty() {
            return None;
        }
        Some(
            manifest["manifest"]
                .as_object()?
                .keys()
                .filter_map(|name| name.strip_suffix(".schema.yaml"))
                .filter(|id| valid_rime_build_id(id))
                .map(str::to_owned)
                .collect(),
        )
    };
    read().unwrap_or_default()
}

const DEPLOYING_FILE: &str = ".keytao-deploying";
const DEPLOYED_FILE: &str = ".keytao-deployed";
const SKIPPED_SCHEMAS_FILE: &str = ".keytao-skipped-schemas.json";

fn skipped_schemas(user_data_dir: &Path) -> std::collections::BTreeMap<String, String> {
    skipped_schemas_with_shared(user_data_dir, user_data_dir)
}

fn skipped_schemas_with_shared(
    user_data_dir: &Path,
    shared_dir: &Path,
) -> std::collections::BTreeMap<String, String> {
    let Ok(dirs) = RuntimeDirs::resolve(user_data_dir, shared_dir) else {
        return std::collections::BTreeMap::new();
    };
    std::fs::read(dirs.staging_dir.join(SKIPPED_SCHEMAS_FILE))
        .ok()
        .and_then(|content| serde_json::from_slice(&content).ok())
        .unwrap_or_default()
}

/// Non-blocking warnings from the latest deployment, including deploy children.
pub fn deployment_warning(user_data_dir: &Path) -> Option<String> {
    deployment_warning_with_shared(user_data_dir, user_data_dir)
}

pub fn deployment_warning_with_shared(user_data_dir: &Path, shared_dir: &Path) -> Option<String> {
    let warnings = skipped_schemas_with_shared(user_data_dir, shared_dir);
    (!warnings.is_empty()).then(|| warnings.into_values().collect::<Vec<_>>().join("\n"))
}

fn effective_schema_list(user_data_dir: &Path, schemas: Vec<String>) -> Vec<String> {
    let skipped = skipped_schemas(user_data_dir);
    schemas
        .into_iter()
        .filter(|id| !skipped.contains_key(id))
        .collect()
}

#[derive(Serialize, Deserialize)]
struct DeploymentMarker {
    fingerprint: String,
    schemas: Vec<String>,
}

fn deployment_fingerprint(user_data_dir: &Path) -> Result<String, String> {
    // Mobile apps publish the bundle version only after refreshing shared assets.
    let bundled_version =
        match std::fs::read_to_string(user_data_dir.join("rime-data/.bundled-version")) {
            Ok(version) => version,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("read bundled version: {error}")),
        };
    Ok(format!(
        "keytao-core/{};bundled/{bundled_version}",
        env!("CARGO_PKG_VERSION")
    ))
}

fn schema_source_path(user_data_dir: &Path, package_shared: Option<&Path>, id: &str) -> PathBuf {
    let name = format!("{id}.schema.yaml");
    let path = user_data_dir.join(&name);
    match package_shared {
        Some(shared) if !path.is_file() => shared.join(name),
        _ => path,
    }
}

fn managed_primary_schemas(user_data_dir: &Path, skip_failed: bool, isolated: bool) -> Vec<String> {
    let schemas: Vec<_> = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find_map(|name| std::fs::read_to_string(user_data_dir.join(name)).ok())
        .map(|content| parse_schema_list(&content))
        .unwrap_or_default()
        .into_iter()
        .filter(|schema| valid_rime_build_id(schema))
        .collect();
    let schemas = if skip_failed {
        effective_schema_list(user_data_dir, schemas)
    } else {
        schemas
    };
    if isolated {
        schemas
    } else {
        managed_primary_schemas_from_list(user_data_dir, schemas)
    }
}

fn managed_primary_schemas_from_list(user_data_dir: &Path, schemas: Vec<String>) -> Vec<String> {
    let package_schemas = active_package_schema_ids(user_data_dir);
    let has_primary = schemas
        .iter()
        .any(|schema| is_keytao_managed_schema(schema) || package_schemas.contains(schema));
    schemas
        .into_iter()
        .filter(|schema| {
            is_keytao_managed_schema(schema)
                || package_schemas.contains(schema)
                || (!has_primary && is_addon_schema(schema))
        })
        .collect()
}

fn begin_deployment(user_data_dir: &Path) -> Result<(), String> {
    begin_deployment_with_invalidation(user_data_dir, true)
}

fn begin_deployment_with_invalidation(
    user_data_dir: &Path,
    invalidate: bool,
) -> Result<(), String> {
    let build = RuntimeDirs::resolve(user_data_dir, user_data_dir)?.staging_dir;
    std::fs::create_dir_all(&build)
        .map_err(|error| format!("create {}: {error}", build.display()))?;
    let sentinel = build.join(DEPLOYING_FILE);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::fs::write(&sentinel, format!("{timestamp}/{}\n", std::process::id()))
        .map_err(|error| format!("write {}: {error}", sentinel.display()))?;
    for name in [DEPLOYED_FILE, SKIPPED_SCHEMAS_FILE, ".keytao-deploy-failed"] {
        let marker = build.join(name);
        match std::fs::remove_file(&marker) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("remove {}: {error}", marker.display())),
        }
    }
    if invalidate {
        schema_inputs::invalidate_changed_deploy_schemas(
            user_data_dir,
            &configured_schema_list_from_dir(user_data_dir),
        )?;
    }
    regenerate_active_rime_lua(user_data_dir)
}

fn write_deployed_marker(user_data_dir: &Path, schemas: &[String]) -> Result<(), String> {
    let build = RuntimeDirs::resolve(user_data_dir, user_data_dir)?.staging_dir;
    let marker = DeploymentMarker {
        fingerprint: deployment_fingerprint(user_data_dir)?,
        schemas: schemas.to_vec(),
    };
    let content = serde_json::to_vec(&marker).map_err(|error| error.to_string())?;
    write_file_atomic(&build.join(DEPLOYED_FILE), &content)
        .map_err(|error| format!("publish deployment marker: {error}"))
}

fn persist_standalone_addon_selection(user_data_dir: &Path) -> Result<(), String> {
    let schemas = effective_schema_list(
        user_data_dir,
        configured_schema_list_from_dir(user_data_dir),
    );
    let Some(schema) = schemas.first().filter(|_| {
        schemas.iter().all(|schema| is_addon_schema(schema))
            || !skipped_schemas(user_data_dir).is_empty()
    }) else {
        return Ok(());
    };
    let path = user_data_dir.join("user.yaml");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("refuse symbolic link: {}", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    }
    let mut value = match std::fs::read(&path) {
        Ok(bytes) => serde_yaml::from_slice::<Value>(&bytes)
            .map_err(|error| format!("parse {}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Value::Mapping(Mapping::new())
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mapping = value
        .as_mapping_mut()
        .ok_or("user.yaml must be a YAML mapping")?;
    let vars = mapping
        .entry(Value::String("var".into()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .ok_or("user.yaml var must be a YAML mapping")?;
    if !schemas.iter().all(|schema| is_addon_schema(schema))
        && vars
            .get(Value::String("previously_selected_schema".into()))
            .and_then(Value::as_str)
            .is_some_and(|selected| schemas.iter().any(|schema| schema == selected))
    {
        return Ok(());
    }
    vars.insert(
        Value::String("previously_selected_schema".into()),
        Value::String(schema.clone()),
    );
    let content = serde_yaml::to_string(&value).map_err(|error| error.to_string())?;
    write_file_atomic(&path, content.as_bytes())
        .map_err(|error| format!("save {}: {error}", path.display()))
}

/// Publish readiness only after every deployment step has succeeded.
pub fn complete_deployment(user_data_dir: &Path) -> Result<(), String> {
    complete_deployment_with_cleanup(user_data_dir, || Ok(()))
}

/// Keep deployment gated until verification and compiled-output cleanup finish.
pub fn complete_deployment_with_cleanup(
    user_data_dir: &Path,
    cleanup: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let dirs = RuntimeDirs::resolve(user_data_dir, user_data_dir)?;
    complete_resolved_deployment(&dirs.user_data_dir, dirs.package_shared(), cleanup)
}

fn complete_resolved_deployment(
    user_data_dir: &Path,
    package_shared: Option<&Path>,
    cleanup: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let build = RuntimeDirs::resolve(user_data_dir, user_data_dir)?.staging_dir;
    let schemas = managed_primary_schemas(user_data_dir, true, package_shared.is_some());
    if schemas.is_empty()
        || !schemas.iter().all(|schema| {
            schema_source_path(user_data_dir, package_shared, schema).is_file()
                && build.join(format!("{schema}.schema.yaml")).is_file()
        })
    {
        return Err("managed primary schemas are not deployed".into());
    }
    let effective = effective_schema_list(
        user_data_dir,
        configured_schema_list_from_dir(user_data_dir),
    );
    write_effective_default_schema_list(user_data_dir, &effective)?;
    patch_neutral_grammar(user_data_dir);
    persist_standalone_addon_selection(user_data_dir)?;
    if let Err(error) = schema_inputs::record_resolved_schema_input_fingerprints(
        user_data_dir,
        &effective,
        package_shared,
    ) {
        crate::rt_log!(
            Level::Info,
            "rime",
            "schema_input_cache_write_failed",
            msg = error
        );
    }
    cleanup()?;
    write_deployed_marker(user_data_dir, &schemas)?;
    let sentinel = build.join(DEPLOYING_FILE);
    match std::fs::remove_file(&sentinel) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", sentinel.display())),
    }
}

fn write_effective_default_schema_list(
    user_data_dir: &Path,
    effective: &[String],
) -> Result<(), String> {
    let default_path = RuntimeDirs::resolve(user_data_dir, user_data_dir)?
        .staging_dir
        .join("default.yaml");
    if default_path.is_file() {
        let content = std::fs::read(&default_path).map_err(|error| error.to_string())?;
        let mut default: Value =
            serde_yaml::from_slice(&content).map_err(|error| error.to_string())?;
        if schema_list_from_yaml(default.get("schema_list")) != effective {
            default
                .as_mapping_mut()
                .ok_or("deployed default.yaml must be a mapping")?
                .insert(
                    Value::String("schema_list".into()),
                    make_schema_list_value(effective),
                );
            let content = serde_yaml::to_string(&default).map_err(|error| error.to_string())?;
            write_file_atomic(&default_path, content.as_bytes())
                .map_err(|error| format!("save effective schema list: {error}"))?;
        }
    }
    Ok(())
}

pub fn schema_install_state(user_data_dir: &Path) -> SchemaInstallState {
    schema_install_state_with_shared(user_data_dir, user_data_dir)
}

/// Report deployment freshness against the same bundled assets used for deployment.
pub fn schema_install_state_with_shared(
    user_data_dir: &Path,
    shared_dir: &Path,
) -> SchemaInstallState {
    if let Some(schema) = runtime_libjd::active_libjd_schema(user_data_dir) {
        return SchemaInstallState {
            installed: true,
            deployed: true,
            schemas: vec![schema.id],
        };
    }
    let Ok(dirs) = RuntimeDirs::resolve(user_data_dir, shared_dir) else {
        return SchemaInstallState::default();
    };
    let state = resolved_schema_install_state(&dirs);
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    if state.deployed
        && dirs.active_package_key.is_some()
        && !isolated_deploy::package_receipt_matches(user_data_dir, &dirs)
    {
        return SchemaInstallState {
            deployed: false,
            ..state
        };
    }
    state
}

/// The IME can keep using the last committed build while source edits await deployment.
pub fn schema_build_is_available(user_data_dir: &Path) -> bool {
    RuntimeDirs::resolve(user_data_dir, user_data_dir)
        .is_ok_and(|dirs| resolved_schema_install_state(&dirs).deployed)
}

pub fn isolated_ime_schema_state(control_root: &Path, shared: &Path) -> Option<SchemaInstallState> {
    match RuntimeDirs::resolve(control_root, shared) {
        Ok(dirs) if dirs.active_package_key.is_some() => Some(resolved_schema_install_state(&dirs)),
        Ok(dirs) if dirs.user_data_dir == control_root => None,
        _ => Some(SchemaInstallState::default()),
    }
}

fn resolved_schema_install_state(dirs: &RuntimeDirs) -> SchemaInstallState {
    let package_shared = dirs.package_shared();
    let user_data_dir = dirs.user_data_dir.as_path();
    let build = &dirs.staging_dir;
    let marker = std::fs::read(build.join(DEPLOYED_FILE))
        .ok()
        .and_then(|content| serde_json::from_slice::<DeploymentMarker>(&content).ok());
    let committed = marker.as_ref().filter(|marker| {
        package_shared.is_some()
            && !build.join(DEPLOYING_FILE).exists()
            && build.join("default.yaml").is_file()
            && !marker.schemas.is_empty()
            && marker.schemas.iter().all(|id| {
                valid_rime_build_id(id) && build.join(format!("{id}.schema.yaml")).is_file()
            })
    });
    if let Some(marker) = committed {
        // A new source selection can fail to compile while the published build is still usable.
        return SchemaInstallState {
            installed: true,
            deployed: true,
            schemas: marker.schemas.clone(),
        };
    }
    let installed_schemas = managed_primary_schemas(user_data_dir, false, package_shared.is_some());
    let schemas = managed_primary_schemas(user_data_dir, true, package_shared.is_some());
    let skipped = skipped_schemas(user_data_dir);

    let installed = !installed_schemas.is_empty()
        && installed_schemas.iter().all(|schema| {
            schema_source_path(user_data_dir, package_shared, schema).is_file()
                || skipped.contains_key(schema)
        });
    let deployed = installed
        && !schemas.is_empty()
        && schemas
            .iter()
            .all(|schema| build.join(format!("{schema}.schema.yaml")).is_file())
        && !build.join(DEPLOYING_FILE).exists()
        && marker.is_some_and(|marker| {
            deployment_fingerprint(user_data_dir).is_ok_and(|current| marker.fingerprint == current)
                && schemas.iter().all(|schema| marker.schemas.contains(schema))
        });

    SchemaInstallState {
        installed,
        deployed,
        schemas,
    }
}

fn collect_schema_dictionary_ids(value: &Value, dictionary_ids: &mut HashSet<String>) {
    match value {
        Value::Mapping(mapping) => {
            for (key, value) in mapping {
                let is_dictionary = key
                    .as_str()
                    .is_some_and(|key| key == "dictionary" || key.ends_with("/dictionary"));
                if is_dictionary {
                    if let Some(id) = value
                        .as_str()
                        .map(str::trim)
                        .filter(|id| valid_rime_build_id(id))
                    {
                        dictionary_ids.insert(id.to_string());
                    }
                }
                collect_schema_dictionary_ids(value, dictionary_ids);
            }
        }
        Value::Sequence(values) => {
            for value in values {
                collect_schema_dictionary_ids(value, dictionary_ids);
            }
        }
        _ => {}
    }
}

/// Remove compiled outputs owned by schemas and dictionaries that are about to
/// be replaced. librime will recreate them during the next app-owned deploy.
pub fn invalidate_rime_build_artifacts(
    user_data_dir: &Path,
    schema_ids: &[String],
    dictionary_ids: &[String],
) -> Result<Vec<String>, String> {
    let build_dir = RuntimeDirs::resolve(user_data_dir, user_data_dir)?.staging_dir;
    if !build_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut relative_paths = HashSet::new();
    if !schema_ids.is_empty() || !dictionary_ids.is_empty() {
        relative_paths.insert("default.yaml".to_string());
    }
    for id in schema_ids
        .iter()
        .map(String::as_str)
        .filter(|id| valid_rime_build_id(id))
    {
        relative_paths.insert(format!("{id}.schema.yaml"));
        relative_paths.insert(format!("{id}.prism.bin"));
    }
    for id in dictionary_ids
        .iter()
        .map(String::as_str)
        .filter(|id| valid_rime_build_id(id))
    {
        relative_paths.insert(format!("{id}.table.bin"));
        relative_paths.insert(format!("{id}.reverse.bin"));
    }

    let mut removed = Vec::new();
    for relative in relative_paths {
        let path = build_dir.join(&relative);
        if remove_rime_build_artifact(&path)? {
            removed.push(format!("build/{relative}"));
        }
    }
    removed.sort();
    Ok(removed)
}

/// Returns whether a Windows host-provided caret extent has usable bounds.
///
/// Width may be zero: Chromium returns width-0 rectangles for a collapsed
/// composition extent, so this must never test `right > left`. Zero or negative
/// height is the unambiguous signal that the host had no bounds.
pub fn caret_extent_is_usable(left: i32, top: i32, right: i32, bottom: i32) -> bool {
    let _ = (left, right);
    bottom > top
}

pub fn windows_rime_build_repair_required(user_data_dir: &Path) -> bool {
    let Ok(dirs) = RuntimeDirs::resolve(user_data_dir, user_data_dir) else {
        return true;
    };
    if dirs.active_package_key.is_some()
        || std::fs::read_to_string(user_data_dir.join(WINDOWS_RIME_BUILD_REPAIR_MARKER))
            .is_ok_and(|content| content == WINDOWS_RIME_BUILD_REPAIR_MARKER_CONTENT)
    {
        return false;
    }

    configured_schema_list_from_dir(user_data_dir)
        .into_iter()
        .any(|schema| {
            is_keytao_managed_schema(&schema)
                && user_data_dir
                    .join(format!("{schema}.schema.yaml"))
                    .is_file()
        })
}

pub fn clear_windows_rime_build_repair_marker(user_data_dir: &Path) -> Result<(), String> {
    let marker = user_data_dir.join(WINDOWS_RIME_BUILD_REPAIR_MARKER);
    match std::fs::remove_file(&marker) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", marker.display())),
    }
}

pub fn mark_windows_rime_build_repair_complete(user_data_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(user_data_dir)
        .map_err(|error| format!("create {}: {error}", user_data_dir.display()))?;
    let marker = user_data_dir.join(WINDOWS_RIME_BUILD_REPAIR_MARKER);
    write_file_atomic(&marker, WINDOWS_RIME_BUILD_REPAIR_MARKER_CONTENT.as_bytes())
        .map_err(|error| format!("write {}: {error}", marker.display()))
}

/// Invalidate the selected managed schemas and their dependency graph before a
/// one-time Windows repair deployment.
pub fn invalidate_active_windows_rime_build(user_data_dir: &Path) -> Result<Vec<String>, String> {
    let mut pending: VecDeque<String> = configured_schema_list_from_dir(user_data_dir)
        .into_iter()
        .filter(|schema| is_keytao_managed_schema(schema))
        .collect();
    let mut schema_ids = HashSet::new();
    let mut dictionary_ids = HashSet::new();

    while let Some(schema_id) = pending.pop_front() {
        if !valid_rime_build_id(&schema_id) || !schema_ids.insert(schema_id.clone()) {
            continue;
        }
        let source = user_data_dir.join(format!("{schema_id}.schema.yaml"));
        let Ok(content) = std::fs::read_to_string(&source) else {
            continue;
        };
        if let Ok(value) = serde_yaml::from_str::<Value>(&content) {
            collect_schema_dictionary_ids(&value, &mut dictionary_ids);
        }
        for dependency in parse_schema_dependencies(&content) {
            if !schema_ids.contains(&dependency) {
                pending.push_back(dependency);
            }
        }
    }

    let mut schemas: Vec<String> = schema_ids.into_iter().collect();
    schemas.sort();
    let mut dictionaries: Vec<String> = dictionary_ids.into_iter().collect();
    dictionaries.sort();
    invalidate_rime_build_artifacts(user_data_dir, &schemas, &dictionaries)
}

#[cfg(any(target_os = "android", test))]
fn android_auxiliary_dictionary_marker() -> &'static str {
    "KeyTao Android auxiliary deployment"
}

#[cfg(any(target_os = "android", test))]
fn patch_android_auxiliary_dictionary(content: &str) -> Option<String> {
    let marker = android_auxiliary_dictionary_marker();
    let mut changed = false;
    let mut output = String::with_capacity(content.len() + marker.len());
    for line in content.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, ""), |body| (body, "\n"));
        if !changed && body.trim() == "use_preset_vocabulary: true" {
            let indent = &body[..body.len() - body.trim_start().len()];
            output.push_str(indent);
            output.push_str("use_preset_vocabulary: false # ");
            output.push_str(marker);
            output.push_str(newline);
            changed = true;
        } else {
            output.push_str(line);
        }
    }
    changed.then_some(output)
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
    trimmed
        .split_once('#')
        .map_or(trimmed, |(head, _)| head)
        .trim()
        .to_string()
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
    schema == "snow_jiandao"
        || ["keytao", "txjx", "xmjd6", "keydo"]
            .iter()
            .any(|prefix| schema.starts_with(prefix))
}

pub fn is_addon_schema(schema: &str) -> bool {
    matches!(
        schema,
        "easy_en"
            | "wanxiang"
            | "rime_ice"
            | "rime_mint"
            | "rime_frost"
            | "triple_pinyin_lssp"
            | "melt_eng"
            | "radical_pinyin"
            | "wubi98_mint"
            | "rime_mint_melt_eng"
            | "rime_mint_radical_pinyin"
            | "rime_mint_stroke"
            | "rime_frost_aux"
            | "rime_frost_melt_eng"
            | "rime_frost_radical_pinyin"
            | "lssp_stroke"
    )
}

fn dedupe_schemas(schemas: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    schemas
        .into_iter()
        .filter(|schema| !schema.trim().is_empty())
        .filter(|schema| seen.insert(schema.clone()))
        .collect()
}

fn merge_schema_lists(existing_schemas: Vec<String>, package_schemas: Vec<String>) -> Vec<String> {
    let user_schemas = existing_schemas
        .iter()
        .filter(|schema| !is_addon_schema(schema))
        .cloned();
    let package_schemas_without_addons = package_schemas
        .iter()
        .filter(|schema| !is_addon_schema(schema))
        .cloned();
    let addon_schemas = existing_schemas
        .iter()
        .chain(package_schemas.iter())
        .filter(|schema| is_addon_schema(schema))
        .cloned();
    dedupe_schemas(
        user_schemas
            .chain(package_schemas_without_addons)
            .chain(addon_schemas),
    )
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
                let existing_schemas = schema_list_from_yaml(Some(existing_value));
                let merged_schemas = merge_schema_lists(existing_schemas, package_schemas);
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
    let existing_schemas = existing.map(parse_schema_list).unwrap_or_default();
    let user_schemas: Vec<String> = existing_schemas
        .iter()
        .filter(|schema| !is_keytao_managed_schema(schema) && !is_addon_schema(schema))
        .cloned()
        .collect();
    let merged_schemas = merge_schema_lists(existing_schemas, package_schemas);

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
        .filter(|schema| !is_keytao_managed_schema(schema) && !is_addon_schema(schema))
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

fn parse_rime_lua_require_bindings(content: &str) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    let mut in_block_comment = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if in_block_comment {
            if trimmed.contains("--]]") {
                in_block_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("--[[") {
            in_block_comment = true;
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }

        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            continue;
        }
        if let Some(module) = extract_lua_require(trimmed) {
            bindings.insert(name.to_string(), module);
        }
    }
    bindings
}

fn rewrite_lua_component_autoloads(
    schema_content: &str,
    bindings: &HashMap<String, String>,
) -> Option<String> {
    let mut ordered_bindings: Vec<_> = bindings.iter().collect();
    ordered_bindings.sort_by(|(left, _), (right, _)| {
        right.len().cmp(&left.len()).then_with(|| left.cmp(right))
    });

    let mut changed = false;
    let mut output = String::with_capacity(schema_content.len());
    for line in schema_content.split_inclusive('\n') {
        let mut rewritten = line.to_string();
        for component in [
            "lua_processor",
            "lua_translator",
            "lua_filter",
            "lua_segmentor",
        ] {
            for (name, module) in &ordered_bindings {
                let old = format!("{component}@{name}");
                let replacement = format!("{component}@*{module}");
                let mut search_from = 0;
                while let Some(relative) = rewritten[search_from..].find(&old) {
                    let start = search_from + relative;
                    let end = start + old.len();
                    let has_boundary = rewritten[end..].chars().next().is_none_or(|character| {
                        !character.is_ascii_alphanumeric() && character != '_'
                    });
                    if has_boundary {
                        rewritten.replace_range(start..end, &replacement);
                        search_from = start + replacement.len();
                    } else {
                        search_from = end;
                    }
                }
            }
        }
        changed |= rewritten != line;
        output.push_str(&rewritten);
    }

    changed.then_some(output)
}

fn patch_keydo_helpers(content: &str) -> Option<String> {
    const VULNERABLE: &str = "    local target_key = string.char(key_event.keycode) -- 当前按键对应字符\n\n    -- 若无目标键位，则交由其它函数进行判断\n    if not key then\n        return true\n    end";
    const PATCHED: &str = "    -- A wildcard match does not need a printable character conversion.\n    if not key then\n        return true\n    end\n\n    local keycode = key_event.keycode\n    if type(keycode) ~= \"number\" or keycode < 0x20 or keycode >= 0x7f then\n        return false\n    end\n    local target_key = string.char(keycode) -- 当前按键对应字符";

    content
        .contains("local function is_key(key, key_event)")
        .then(|| content.replace(VULNERABLE, PATCHED))
        .filter(|patched| patched != content)
}

/// Source adjustments required by Windows hosts that keep a Lua VM alive.
pub struct WindowsLuaCompatibility {
    bindings: HashMap<String, String>,
}

impl WindowsLuaCompatibility {
    pub fn new(entry: &str) -> Self {
        Self {
            bindings: parse_rime_lua_require_bindings(entry),
        }
    }

    pub fn patch_source(&self, path: &str, content: &[u8]) -> Option<String> {
        if self.bindings.is_empty() {
            return None;
        }
        if path.eq_ignore_ascii_case("lua/helpers.lua") {
            patch_keydo_helpers(std::str::from_utf8(content).ok()?)
        } else if path.ends_with(".schema.yaml") && !path.contains('/') {
            rewrite_lua_component_autoloads(std::str::from_utf8(content).ok()?, &self.bindings)
        } else {
            None
        }
    }
}

/// Convert package-global Lua components to librime-lua's lazy module form.
///
/// Windows loads a TSF DLL into long-lived host processes such as Explorer.
/// Replacing `rime.lua` on disk does not update the Lua VM in those processes,
/// so a newly installed scheme must not depend on new global variables there.
pub fn patch_windows_lua_compatibility(user_data_dir: &Path) -> Result<Vec<String>, String> {
    let dirs = RuntimeDirs::resolve(user_data_dir, user_data_dir)?;
    let rime_lua_path = dirs.user_data_dir.join("rime.lua");
    let rime_lua = match std::fs::read_to_string(&rime_lua_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", rime_lua_path.display())),
    };
    let compatibility = WindowsLuaCompatibility::new(&rime_lua);
    if compatibility.bindings.is_empty() {
        return Ok(Vec::new());
    }

    let mut changed = Vec::new();
    let entries = std::fs::read_dir(user_data_dir)
        .map_err(|error| format!("read {}: {error}", user_data_dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_file() || !file_name.ends_with(".schema.yaml") {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let Some(patched) = compatibility.patch_source(file_name, content.as_bytes()) else {
            continue;
        };
        // A deployment stage can share this file with the live package via a hard link.
        write_file_atomic(&path, patched.as_bytes())
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        changed.push(file_name.to_string());

        let compiled_path = dirs.staging_dir.join(file_name);
        match std::fs::read_to_string(&compiled_path) {
            Ok(compiled) => {
                if let Some(patched) = compatibility.patch_source(file_name, compiled.as_bytes()) {
                    write_file_atomic(&compiled_path, patched.as_bytes())
                        .map_err(|error| format!("write {}: {error}", compiled_path.display()))?;
                    changed.push(format!("build/{file_name}"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("read {}: {error}", compiled_path.display())),
        }
    }

    let helpers_path = user_data_dir.join("lua").join("helpers.lua");
    if let Ok(content) = std::fs::read_to_string(&helpers_path) {
        if let Some(patched) = compatibility.patch_source("lua/helpers.lua", content.as_bytes()) {
            write_file_atomic(&helpers_path, patched.as_bytes())
                .map_err(|error| format!("write {}: {error}", helpers_path.display()))?;
            changed.push("lua/helpers.lua".to_string());
        }
    }

    changed.sort();
    Ok(changed)
}

/// Add-ons use built-in components or lazy Lua modules, without root rime.lua lines.
pub fn installed_addon_rime_lua_lines(_user_data_dir: &Path) -> &'static [&'static str] {
    &[]
}

/// Restore the active package snapshot before any platform starts deployment.
pub fn regenerate_active_rime_lua(user_data_dir: &Path) -> Result<(), String> {
    rime_lua::regenerate(user_data_dir)
}

/// Prepare an isolated package stage before its selected schema has been built.
pub fn regenerate_active_rime_lua_for_schema(
    user_data_dir: &Path,
    schema_id: &str,
) -> Result<(), String> {
    rime_lua::select(user_data_dir, schema_id).map(|_| ())
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
fn defer_rime_lua_reload(root: &Path, error: &str) {
    rime_lua::RELOAD_PENDING.store(true, Ordering::Release);
    *lock_ignore_poison(&PROCESS_RIME.lua_retry_after) =
        Some((root.to_path_buf(), Instant::now() + Duration::from_secs(1)));
    crate::rt_log!(Level::Info, "error", "schema_lua_reload", msg = error);
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
fn regenerate_rime_lua_for_ime(root: &Path) -> bool {
    match regenerate_active_rime_lua(root) {
        Ok(()) => {
            *lock_ignore_poison(&PROCESS_RIME.lua_retry_after) = None;
            true
        }
        Err(error) => {
            defer_rime_lua_reload(root, &error);
            false
        }
    }
}

fn regenerate_legacy_rime_lua(user_data_dir: &Path) -> Result<(), String> {
    let manifest_path = user_data_dir.join(ACTIVE_SCHEME_MANIFEST);
    let rime_lua_path = RuntimeDirs::resolve(user_data_dir, user_data_dir)?
        .user_data_dir
        .join("rime.lua");
    for path in [&manifest_path, &rime_lua_path] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("refuse symbolic link: {}", path.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("inspect {}: {error}", path.display())),
        }
    }
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", manifest_path.display())),
    };
    let manifest: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse {}: {error}", manifest_path.display()))?;
    let Some(package_content) = manifest.get("rime_lua") else {
        return Ok(());
    };
    let root = user_data_dir
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", user_data_dir.display()))?;
    if manifest["root"].as_str().map(Path::new) != Some(root.as_path()) {
        return Err(format!(
            "active scheme root does not match {}",
            root.display()
        ));
    }
    let package_content = match package_content {
        serde_json::Value::String(content) => Some(content.as_str()),
        serde_json::Value::Null => None,
        _ => return Err("active scheme rime_lua must be a string or null".into()),
    };
    let generated = generate_rime_lua_content(
        package_content,
        installed_addon_rime_lua_lines(user_data_dir),
    );
    preserve_rime_lua_user_edits(user_data_dir, generated.as_deref())?;
    match generated {
        Some(content) => write_file_atomic(&rime_lua_path, content.as_bytes())
            .map_err(|error| format!("write {}: {error}", rime_lua_path.display())),
        None => match std::fs::remove_file(&rime_lua_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("remove {}: {error}", rime_lua_path.display())),
        },
    }
}

/// Keep manual entry-point changes once before restoring the active package snapshot.
pub fn preserve_rime_lua_user_edits(
    user_data_dir: &Path,
    expected: Option<&str>,
) -> Result<Option<std::path::PathBuf>, String> {
    let source = RuntimeDirs::resolve(user_data_dir, user_data_dir)?
        .user_data_dir
        .join("rime.lua");
    match std::fs::symlink_metadata(&source) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("refuse symbolic link: {}", source.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let current = match std::fs::read(source) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read rime.lua: {error}")),
    };
    if current.is_empty() || expected.is_some_and(|value| value.as_bytes() == current) {
        return Ok(None);
    }
    for index in 0.. {
        let path = user_data_dir.join(format!(".keytao-rime-user-{index}.lua"));
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => {
                if std::fs::read(&path).map_err(|error| error.to_string())? == current {
                    return Ok(None);
                }
            }
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|error| error.to_string())?;
                file.write_all(&current)
                    .and_then(|_| file.sync_all())
                    .map_err(|error| error.to_string())?;
                eprintln!("Preserved manual rime.lua changes: {}", path.display());
                return Ok(Some(path));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!()
}

/// Regenerate the active package's Lua entry point with explicit add-on registrations.
/// Add-on lines must be single-line assignments to global identifiers.
pub fn generate_rime_lua_content(
    package_content: Option<&str>,
    addon_lines: &[&str],
) -> Option<String> {
    if addon_lines.is_empty() {
        return package_content
            .filter(|content| !content.is_empty())
            .map(str::to_owned);
    }

    fn global_names(line: &str) -> Vec<&str> {
        let names = if let Some(function) = line
            .strip_prefix("function")
            .filter(|rest| rest.starts_with(char::is_whitespace))
        {
            function.split_once('(').map(|(name, _)| name)
        } else {
            line.split_once('=').map(|(names, _)| names)
        };
        names
            .into_iter()
            .flat_map(|names| names.split(','))
            .map(str::trim)
            .filter(|name| {
                name.chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                    && name
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_')
            })
            .collect()
    }

    let package_content = package_content.unwrap_or_default();
    let mut content = package_content.to_string();
    let mut scheme_globals = HashSet::new();
    let mut seen_lines = HashSet::new();
    let mut in_block_comment = false;
    for line in package_content.lines().map(str::trim) {
        if in_block_comment {
            in_block_comment = !line.contains("]]");
            continue;
        }
        if let Some(comment) = line.strip_prefix("--[[") {
            in_block_comment = !comment.contains("]]");
            continue;
        }
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        scheme_globals.extend(global_names(line));
        seen_lines.insert(line);
    }

    for line in addon_lines.iter().map(|line| line.trim()) {
        let bindings = global_names(line);
        if bindings.is_empty()
            || bindings.iter().any(|name| scheme_globals.contains(name))
            || !seen_lines.insert(line)
        {
            continue;
        }
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(line);
        content.push('\n');
    }
    (!content.is_empty()).then_some(content)
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

// ── Reload stamp ─────────────────────────────────────────────────────────────

/// Name of the reload signal the app writes into the user data directory.
pub const RELOAD_STAMP_FILE_NAME: &str = "keytao-ime.reload";

/// The single implementation of the reload signal: path, write format and
/// change detection. Frontends must not invent their own signature, otherwise
/// the same deployment reloads on some platforms and not on others.
///
/// The signature detects both rewrites and publication of a migration marker.
pub struct ReloadStamp;

/// The package manifest has been published but its reload stamp is still pending.
pub const PACKAGE_SWITCH_PENDING: &str = "Rime package switch is pending";

impl ReloadStamp {
    pub fn path(user_data_dir: &Path) -> PathBuf {
        user_data_dir.join(RELOAD_STAMP_FILE_NAME)
    }

    pub fn default_path() -> Option<PathBuf> {
        default_user_data_dir().map(|dir| Self::path(&dir))
    }

    /// Request a reload from every running frontend watching `user_data_dir`.
    pub fn write(user_data_dir: &Path) -> Result<PathBuf, String> {
        if !RuntimeDirs::needs_migration(user_data_dir) {
            if let Some(key) = RuntimeDirs::active_package_key(user_data_dir) {
                return Self::write_package(user_data_dir, &key);
            }
        }
        std::fs::create_dir_all(user_data_dir)
            .map_err(|error| format!("failed to create {}: {error}", user_data_dir.display()))?;
        let stamp = Self::path(user_data_dir);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        std::fs::write(&stamp, format!("{now}\n"))
            .map_err(|error| format!("failed to write {}: {error}", stamp.display()))?;
        Ok(stamp)
    }

    pub fn write_package(control_root: &Path, key: &str) -> Result<PathBuf, String> {
        RuntimeDirs::isolated_user_data_dir(control_root, key).ok_or("invalid package key")?;
        let stamp = Self::path(control_root);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        write_file_atomic(&stamp, format!("package:{key}:{now}\n").as_bytes())
            .map_err(|error| format!("failed to write {}: {error}", stamp.display()))?;
        Ok(stamp)
    }

    pub fn package_key(path: &Path) -> Option<String> {
        let stamp = std::fs::read_to_string(path).ok()?;
        let (key, time) = stamp.trim_end().strip_prefix("package:")?.split_once(':')?;
        time.parse::<u128>().ok()?;
        Some(key.to_owned())
    }

    pub fn write_schema(user_data_dir: &Path) -> Result<PathBuf, String> {
        let dirs = RuntimeDirs::resolve(user_data_dir, user_data_dir)?;
        let request = schema_switch_request::read_at(&dirs.user_data_dir)?;
        if request.schema_id.is_none() && request.options.is_empty() {
            return Err("no schema switch request".into());
        }
        let stamp = Self::path(user_data_dir);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        write_file_atomic(
            &stamp,
            format!("schema:{}:{now}\n", request.generation).as_bytes(),
        )
        .map_err(|error| format!("failed to write {}: {error}", stamp.display()))?;
        Ok(stamp)
    }

    pub fn write_default() -> Result<PathBuf, String> {
        let dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        Self::write(&dir)
    }

    /// `None` when no stamp has been written yet.
    pub fn current_signature(user_data_dir: &Path) -> Option<String> {
        Self::signature_at(&Self::path(user_data_dir))
    }

    pub fn signature_at(path: &Path) -> Option<String> {
        // A migration publishes its stamp before committing the marker.
        // Observe both so even a reader that sees the early stamp reloads again.
        let migrated = path.parent().is_some_and(RuntimeDirs::migration_complete);
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let content_hash = std::fs::read(path)
            .map(|bytes| fnv1a64(&bytes))
            .unwrap_or(0);
        Some(Self::format_signature(metadata.len(), modified, content_hash, migrated))
    }

    fn format_signature(len: u64, modified: u128, content_hash: u64, migrated: bool) -> String {
        let migration = if migrated { ":migrated" } else { "" };
        format!("{len}:{modified}:{content_hash:016x}{migration}")
    }
}

/// Peek/commit view of the reload signal. A failed reload leaves its request
/// pending; committing the loaded signature also preserves newer requests.
pub struct ReloadStampGate {
    path: PathBuf,
    loaded_signature: Option<String>,
}

impl ReloadStampGate {
    /// Seed with the deployment the frontend just loaded at startup.
    pub fn new(user_data_dir: &Path) -> Self {
        let path = ReloadStamp::path(user_data_dir);
        let loaded_signature = ReloadStamp::signature_at(&path);
        Self {
            path,
            loaded_signature,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A missing stamp is not a reload request, so an uninstalled or not yet
    /// deployed app never triggers one. Peeking does not consume a request.
    pub fn pending(&self) -> Option<String> {
        let current = ReloadStamp::signature_at(&self.path)?;
        (self.loaded_signature.as_deref() != Some(current.as_str())).then_some(current)
    }

    /// Record exactly the signature a successful reload loaded.
    pub fn commit(&mut self, signature: String) {
        self.loaded_signature = Some(signature);
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{
        caret_extent_is_usable, clear_windows_rime_build_repair_marker, generate_rime_lua_content,
        invalidate_active_windows_rime_build, invalidate_rime_build_artifacts,
        mark_windows_rime_build_repair_complete, merge_default_custom_content,
        parse_rime_lua_requires, parse_schema_dependencies, parse_schema_list, parse_schema_name,
        patch_android_auxiliary_dictionary, patch_windows_lua_compatibility,
        preferred_schema_from_list, preferred_schema_id_from_dir, rime_build_dirs, rime_log_dir,
        schema_install_state, windows_rime_build_repair_required, ReloadStamp, ReloadStampGate,
        RELOAD_STAMP_FILE_NAME,
    };
    #[cfg(target_os = "windows")]
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn waiting_foreground_build_precedes_the_next_background_build() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        let lock = super::EngineBuildLock::new();
        let first = lock.acquire_background().unwrap();
        let ran = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _foreground = lock.acquire().unwrap();
                ran.store(true, Ordering::Release);
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while lock.foreground_waiters.load(Ordering::Relaxed) == 0 {
                assert!(
                    Instant::now() < deadline,
                    "foreground did not wait for the build gate"
                );
                std::thread::yield_now();
            }
            drop(first);
            let _second = lock.acquire_background().unwrap();
            assert!(ran.load(Ordering::Acquire));
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_deploy_lock_handles_free_held_and_released_lock() {
        use std::time::{Duration, Instant};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-deploy-lock-{suffix}"));
        let lock = super::linux_deploy_lock(&dir, Duration::ZERO)
            .unwrap()
            .expect("a free lock must be acquired");
        assert!(super::linux_deploy_lock(&dir, Duration::ZERO)
            .unwrap()
            .is_none());
        let timeout = Duration::from_millis(20);
        let started = Instant::now();
        assert!(super::linux_deploy_lock(&dir, timeout).unwrap().is_none());
        assert!(started.elapsed() >= timeout);
        drop(lock);
        let lock = super::linux_deploy_lock(&dir, Duration::ZERO)
            .unwrap()
            .expect("a released lock must be acquired");
        drop(lock);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn caret_extent_accepts_zero_width_collapsed_rect() {
        assert!(caret_extent_is_usable(1010, 316, 1010, 336));
    }

    #[test]
    fn caret_extent_rejects_zero_height() {
        assert!(!caret_extent_is_usable(73, 59, 73, 59));
    }

    #[test]
    fn caret_extent_rejects_all_zero() {
        assert!(!caret_extent_is_usable(0, 0, 0, 0));
    }

    #[test]
    fn caret_extent_rejects_inverted() {
        assert!(!caret_extent_is_usable(10, 40, 20, 20));
    }

    #[test]
    fn reload_stamp_signature_tracks_rewrites() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-reload-stamp-test-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(ReloadStamp::path(&dir), dir.join(RELOAD_STAMP_FILE_NAME));
        assert!(
            ReloadStamp::current_signature(&dir).is_none(),
            "a missing stamp has no signature"
        );

        let mut gate = ReloadStampGate::new(&dir);
        assert_eq!(gate.path(), ReloadStamp::path(&dir));
        assert_eq!(gate.pending(), None, "a missing stamp is not a request");

        let stamp = ReloadStamp::write(&dir).unwrap();
        assert_eq!(stamp, ReloadStamp::path(&dir));
        gate.commit(gate.pending().expect("the first stamp is a request"));
        assert_eq!(gate.pending(), None, "a loaded stamp fires once");

        // Same length and same mtime resolution: only the content hash differs.
        let signature_before = ReloadStamp::current_signature(&dir).unwrap();
        std::fs::write(&stamp, "0123456789\n").unwrap();
        let signature_after = ReloadStamp::current_signature(&dir).unwrap();
        assert_ne!(signature_before, signature_after);
        assert!(gate.pending().is_some(), "a rewritten stamp is a request");

        std::fs::remove_file(&stamp).unwrap();
        assert_eq!(gate.pending(), None, "a removed stamp is not a request");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reload_stamp_failed_reload_keeps_request_pending() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-reload-stamp-retry-{suffix}"));
        let mut gate = ReloadStampGate::new(&dir);
        ReloadStamp::write(&dir).unwrap();

        let requested = gate.pending().expect("the first stamp is a request");
        assert_eq!(
            gate.pending().as_deref(),
            Some(requested.as_str()),
            "a failed reload must leave the request pending",
        );
        gate.commit(requested);
        assert_eq!(gate.pending(), None, "a loaded stamp fires once");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reload_stamp_commit_preserves_a_newer_request() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-reload-stamp-overlap-{suffix}"));
        ReloadStamp::write(&dir).unwrap();
        let mut gate = ReloadStampGate::new(&dir);
        assert_eq!(gate.pending(), None, "startup adopts the loaded deployment");

        let stamp = ReloadStamp::path(&dir);
        std::fs::write(&stamp, "next deployment\n").unwrap();
        let requested = gate.pending().expect("a rewrite is a request");
        std::fs::write(&stamp, "newer deployment\n").unwrap();
        gate.commit(requested.clone());
        let newer = gate.pending().expect("a newer request must remain pending");
        assert_ne!(requested, newer);
        gate.commit(newer);
        assert_eq!(gate.pending(), None);

        std::fs::remove_dir_all(dir).ok();
    }

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
    fn parse_schema_name_reads_the_display_name() {
        let content = "schema:\n  schema_id: keytao\n  name: 键道\n";
        assert_eq!(parse_schema_name(content).as_deref(), Some("键道"));
    }

    #[test]
    fn schema_install_state_requires_source_and_matching_deployment() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-schema-state-test-{suffix}"));
        let build = dir.join("build");
        std::fs::create_dir_all(&build).unwrap();

        let empty = schema_install_state(&dir);
        assert!(!empty.installed);
        assert!(!empty.deployed);
        assert!(empty.schemas.is_empty());

        std::fs::write(build.join("keytao.schema.yaml"), "schema: {}\n").unwrap();
        let build_only = schema_install_state(&dir);
        assert!(
            !build_only.installed,
            "build residue is not an installation"
        );
        assert!(
            !build_only.deployed,
            "build residue cannot be usable by itself"
        );

        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keydo\n",
        )
        .unwrap();
        std::fs::write(dir.join("keydo.schema.yaml"), "schema: {}\n").unwrap();
        let source_only = schema_install_state(&dir);
        assert!(source_only.installed);
        assert!(
            !source_only.deployed,
            "a newly installed scheme needs manual deployment"
        );
        assert_eq!(source_only.schemas, vec!["keydo"]);

        std::fs::write(build.join("keydo.schema.yaml"), "schema: {}\n").unwrap();
        super::complete_deployment(&dir).unwrap();
        let ready = schema_install_state(&dir);
        assert!(ready.installed);
        assert!(ready.deployed);
        assert_eq!(ready.schemas, vec!["keydo"]);

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn schema_install_state_tracks_standalone_addons_until_main_scheme_is_installed() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-addon-schema-state-{suffix}"));
        std::fs::create_dir_all(dir.join("build")).unwrap();
        for addon in [
            "easy_en",
            "wanxiang",
            "rime_ice",
            "rime_mint",
            "rime_frost",
            "triple_pinyin_lssp",
        ] {
            std::fs::write(
                dir.join("default.custom.yaml"),
                format!("patch:\n  schema_list:\n    - schema: {addon}\n"),
            )
            .unwrap();
            std::fs::write(dir.join(format!("{addon}.schema.yaml")), "schema: {}\n").unwrap();
            let installed = schema_install_state(&dir);
            assert!(installed.installed);
            assert!(!installed.deployed);
            assert_eq!(installed.schemas, [addon]);
            std::fs::write(
                dir.join(format!("build/{addon}.schema.yaml")),
                "schema: {}\n",
            )
            .unwrap();
            super::complete_deployment(&dir).unwrap();
            assert!(schema_install_state(&dir).deployed);

            std::fs::write(
                dir.join("default.custom.yaml"),
                format!("patch:\n  schema_list:\n    - schema: keytao\n    - schema: {addon}\n"),
            )
            .unwrap();
            std::fs::write(dir.join("keytao.schema.yaml"), "schema: {}\n").unwrap();
            std::fs::write(dir.join("build/keytao.schema.yaml"), "schema: {}\n").unwrap();
            std::fs::remove_file(dir.join(format!("{addon}.schema.yaml"))).unwrap();
            super::complete_deployment(&dir).unwrap();
            let main = schema_install_state(&dir);
            assert!(main.installed);
            assert!(main.deployed);
            assert_eq!(main.schemas, ["keytao"]);
        }
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list: []\n",
        )
        .unwrap();
        let empty = schema_install_state(&dir);
        assert!(!empty.installed);
        assert!(!empty.deployed);
        assert!(empty.schemas.is_empty());
        assert!(super::complete_deployment(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn complete_deployment_persists_standalone_default_without_changing_main_selection() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-addon-selection-{suffix}"));
        std::fs::create_dir_all(dir.join("build")).unwrap();
        for schema in ["wanxiang", "easy_en", "keytao"] {
            std::fs::write(dir.join(format!("{schema}.schema.yaml")), "schema: {}\n").unwrap();
            std::fs::write(
                dir.join(format!("build/{schema}.schema.yaml")),
                "schema: {}\n",
            )
            .unwrap();
        }
        let standalone = "patch:\n  schema_list:\n    - schema: wanxiang\n    - schema: easy_en\n";
        std::fs::write(dir.join("default.custom.yaml"), standalone).unwrap();
        let user_path = dir.join("user.yaml");
        let user =
            "custom_key: keep\nvar:\n  previously_selected_schema: easy_en\n  option: true\n";
        std::fs::write(&user_path, user).unwrap();
        super::complete_deployment(&dir).unwrap();
        let selected: serde_yaml::Value =
            serde_yaml::from_slice(&std::fs::read(&user_path).unwrap()).unwrap();
        assert_eq!(
            selected["var"]["previously_selected_schema"].as_str(),
            Some("wanxiang")
        );
        assert_eq!(selected["var"]["option"].as_bool(), Some(true));
        assert_eq!(selected["custom_key"].as_str(), Some("keep"));

        std::fs::write(dir.join("default.custom.yaml"), "patch:\n  schema_list:\n    - schema: keytao\n    - schema: easy_en\n    - schema: wanxiang\n").unwrap();
        std::fs::write(&user_path, user).unwrap();
        super::complete_deployment(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&user_path).unwrap(), user);

        std::fs::write(dir.join("default.custom.yaml"), standalone).unwrap();
        for invalid in ["[invalid", "scalar", "var: scalar\n"] {
            std::fs::write(&user_path, invalid).unwrap();
            super::begin_deployment(&dir).unwrap();
            assert!(super::complete_deployment(&dir).is_err());
            assert_eq!(std::fs::read_to_string(&user_path).unwrap(), invalid);
            assert!(!dir.join("build").join(super::DEPLOYED_FILE).exists());
            assert!(dir.join("build").join(super::DEPLOYING_FILE).exists());
        }
        #[cfg(unix)]
        {
            std::fs::remove_file(&user_path).unwrap();
            let target = dir.join("original-user.yaml");
            std::fs::write(&target, user).unwrap();
            std::os::unix::fs::symlink(&target, &user_path).unwrap();
            assert!(super::complete_deployment(&dir)
                .unwrap_err()
                .contains("symbolic link"));
            assert_eq!(std::fs::read_to_string(target).unwrap(), user);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_install_state_accepts_legacy_installed_local_manifest() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-legacy-local-state-{suffix}"));
        std::fs::create_dir_all(super::package_store::root(&dir)).unwrap();
        let mut state = serde_json::json!({
            "active_scheme": "local-example", "installed_packages": {"local-example": {}}
        });
        let state_path = dir.join("app-state.json");
        std::fs::write(&state_path, state.to_string()).unwrap();
        let manifest = serde_json::json!({
            "root": dir.canonicalize().unwrap(), "manifest": {"example.schema.yaml": "hash"}
        });
        std::fs::write(
            super::package_store::active_manifest(&dir),
            manifest.to_string(),
        )
        .unwrap();
        let restored = super::legacy_active_local_manifest(&dir).unwrap();
        assert_eq!(restored["scheme_key"], "local-example");
        std::fs::write(
            dir.join(super::ACTIVE_SCHEME_MANIFEST),
            restored.to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: example\n",
        )
        .unwrap();
        std::fs::write(dir.join("example.schema.yaml"), "schema: {}\n").unwrap();
        assert!(schema_install_state(&dir).installed);
        state["installed_packages"] = serde_json::json!({});
        std::fs::write(&state_path, state.to_string()).unwrap();
        assert!(super::legacy_active_local_manifest(&dir).is_none());
        state["active_scheme"] = serde_json::json!("keytao");
        state["installed_packages"] = serde_json::json!({"keytao": {}});
        std::fs::write(&state_path, state.to_string()).unwrap();
        assert!(super::legacy_active_local_manifest(&dir).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_install_state_ignores_shared_and_stale_user_schemas() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-shared-schema-state-{suffix}"));
        std::fs::create_dir_all(dir.join("shared")).unwrap();
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(dir.join("shared/luna_pinyin.schema.yaml"), "schema: {}\n").unwrap();
        std::fs::write(dir.join("keytao.schema.yaml"), "schema: {}\n").unwrap();
        std::fs::write(dir.join("build/keytao.schema.yaml"), "schema: {}\n").unwrap();
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keytao\n    - schema: luna_pinyin\n    - schema: stale\n",
        ).unwrap();
        super::complete_deployment(&dir).unwrap();
        let state = schema_install_state(&dir);
        assert!(state.installed);
        assert!(state.deployed);
        assert_eq!(state.schemas, ["keytao"]);
        std::fs::remove_file(dir.join("keytao.schema.yaml")).unwrap();
        assert!(!schema_install_state(&dir).installed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_install_state_counts_only_registered_active_package_schemas() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-local-schema-state-{suffix}"));
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: example\n    - schema: auxiliary\n    - schema: luna_pinyin\n    - schema: stale\n",
        ).unwrap();
        std::fs::write(dir.join("example.schema.yaml"), "schema: {}\n").unwrap();
        let mut manifest = serde_json::json!({
            "root": dir.canonicalize().unwrap(), "scheme_key": "local-example",
            "manifest": {"example.schema.yaml": "hash", "auxiliary.schema.yaml": "hash", "nested/stale.schema.yaml": "hash"}
        });
        let marker = dir.join(super::ACTIVE_SCHEME_MANIFEST);
        assert!(!schema_install_state(&dir).installed);
        std::fs::write(&marker, manifest.to_string()).unwrap();
        let incomplete = schema_install_state(&dir);
        assert_eq!(incomplete.schemas, ["example", "auxiliary"]);
        assert!(!incomplete.installed);
        std::fs::write(dir.join("auxiliary.schema.yaml"), "schema: {}\n").unwrap();
        let installed = schema_install_state(&dir);
        assert!(installed.installed);
        assert!(!installed.deployed);
        for id in ["example", "auxiliary"] {
            std::fs::write(dir.join(format!("build/{id}.schema.yaml")), "schema: {}\n").unwrap();
        }
        super::complete_deployment(&dir).unwrap();
        assert!(schema_install_state(&dir).deployed);
        manifest["scheme_key"] = serde_json::json!("keytao");
        std::fs::write(&marker, manifest.to_string()).unwrap();
        assert!(schema_install_state(&dir).installed);
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch: {schema_list: [{schema: example}]}",
        )
        .unwrap();
        std::fs::remove_file(dir.join("auxiliary.schema.yaml")).unwrap();
        assert_eq!(schema_install_state(&dir).schemas, ["example"]);
        assert!(schema_install_state(&dir).installed);
        manifest["scheme_key"] = serde_json::json!("local-example");
        manifest["root"] = serde_json::json!(dir.join("other"));
        std::fs::write(&marker, manifest.to_string()).unwrap();
        assert!(!schema_install_state(&dir).installed);
        std::fs::write(&marker, "invalid json").unwrap();
        assert!(!schema_install_state(&dir).installed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parse_schema_dependencies_reads_flow_and_block_sequences() {
        let flow = "schema:\n  schema_id: txjx\n  dependencies: [txjx.cx, txjx.danzi]\n";
        assert_eq!(
            parse_schema_dependencies(flow),
            vec!["txjx.cx", "txjx.danzi"]
        );

        let block =
            "schema:\n  schema_id: xmjd6\n  dependencies:\n    - pinyin_simp\n    - liangfen\n";
        assert_eq!(
            parse_schema_dependencies(block),
            vec!["pinyin_simp", "liangfen"]
        );
    }

    #[test]
    fn invalidating_package_build_artifacts_preserves_unrelated_outputs() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-build-invalidate-test-{suffix}"));
        let build = dir.join("build");
        std::fs::create_dir_all(&build).unwrap();
        for name in [
            "default.yaml",
            "keydo.schema.yaml",
            "keydo.prism.bin",
            "keydo.table.bin",
            "keydo.reverse.bin",
            "keytao.schema.yaml",
            "keytao.table.bin",
        ] {
            std::fs::write(build.join(name), name).unwrap();
        }

        let removed =
            invalidate_rime_build_artifacts(&dir, &["keydo".to_string()], &["keydo".to_string()])
                .unwrap();

        assert_eq!(
            removed,
            vec![
                "build/default.yaml",
                "build/keydo.prism.bin",
                "build/keydo.reverse.bin",
                "build/keydo.schema.yaml",
                "build/keydo.table.bin",
            ]
        );
        assert!(build.join("keytao.schema.yaml").is_file());
        assert!(build.join("keytao.table.bin").is_file());
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn invalidating_build_artifact_waits_for_windows_file_lock() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-build-lock-test-{suffix}"));
        let build = dir.join("build");
        std::fs::create_dir_all(&build).unwrap();
        let table = build.join("keydo.table.bin");
        std::fs::write(&table, "locked").unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&table)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(locked);
        });

        let removed = invalidate_rime_build_artifacts(&dir, &[], &["keydo".to_string()]).unwrap();

        release.join().unwrap();
        assert!(removed.contains(&"build/keydo.table.bin".to_string()));
        assert!(!table.exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn windows_build_repair_invalidates_selected_schema_graph() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-windows-repair-test-{suffix}"));
        let build = dir.join("build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: keydo\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("keydo.schema.yaml"),
            concat!(
                "schema:\n",
                "  schema_id: keydo\n",
                "  dependencies: [pinyin_simp]\n",
                "translator:\n",
                "  dictionary: keydo\n",
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("pinyin_simp.schema.yaml"),
            "schema:\n  schema_id: pinyin_simp\ntranslator:\n  dictionary: pinyin_simp\n",
        )
        .unwrap();
        for name in [
            "default.yaml",
            "keydo.schema.yaml",
            "keydo.prism.bin",
            "keydo.table.bin",
            "keydo.reverse.bin",
            "pinyin_simp.schema.yaml",
            "pinyin_simp.prism.bin",
            "pinyin_simp.table.bin",
            "pinyin_simp.reverse.bin",
            "unrelated.table.bin",
        ] {
            std::fs::write(build.join(name), name).unwrap();
        }

        assert!(windows_rime_build_repair_required(&dir));
        let removed = invalidate_active_windows_rime_build(&dir).unwrap();
        assert!(removed.contains(&"build/keydo.table.bin".to_string()));
        assert!(removed.contains(&"build/pinyin_simp.table.bin".to_string()));
        assert!(build.join("unrelated.table.bin").is_file());

        mark_windows_rime_build_repair_complete(&dir).unwrap();
        assert!(!windows_rime_build_repair_required(&dir));
        clear_windows_rime_build_repair_marker(&dir).unwrap();
        assert!(windows_rime_build_repair_required(&dir));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn android_auxiliary_dictionary_disables_preset_vocabulary() {
        let content = "---\nname: liangfen\nuse_preset_vocabulary: true\n...\n";
        let patched = patch_android_auxiliary_dictionary(content).expect("patched dictionary");
        assert!(
            patched.contains("use_preset_vocabulary: false # KeyTao Android auxiliary deployment")
        );
        assert!(patch_android_auxiliary_dictionary(&patched).is_none());
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn preferred_schema_location_does_not_fall_back_to_default_root() {
        const ROOT_ENV: &str = "KEYTAO_SCHEMA_LOCATION_TEST_ROOT";
        if let Some(root) = std::env::var_os(ROOT_ENV) {
            let root = std::path::PathBuf::from(root);
            let default = super::default_user_data_dir().unwrap();
            assert!(default.starts_with(&root));
            std::fs::create_dir_all(&default).unwrap();
            std::fs::write(
                default.join("default.custom.yaml"),
                "patch:\n  schema_list:\n    - schema: keytao\n",
            )
            .unwrap();
            let custom = root.join("custom");
            std::fs::create_dir_all(&custom).unwrap();
            assert!(super::preferred_schema_location(&custom).is_none());
            return;
        }

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("keytao-schema-root-{suffix}"));
        std::fs::create_dir_all(&root).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::preferred_schema_location_does_not_fall_back_to_default_root",
            ])
            .env(ROOT_ENV, &root)
            .env("HOME", root.join("home"))
            .env("XDG_DATA_HOME", root.join("xdg"))
            .output()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
    fn preferred_schema_honors_compiled_previous_selection_and_falls_back_when_missing() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-current-schema-{suffix}"));
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(
            dir.join("build/default.yaml"),
            "schema_list:\n  - schema: keytao\n  - schema: selected\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("user.yaml"),
            "var:\n  previously_selected_schema: selected\n",
        )
        .unwrap();
        assert_eq!(
            super::deployed_schema_id_from_dir(&dir).as_deref(),
            Some("keytao")
        );
        std::fs::write(dir.join("build/selected.schema.yaml"), "").unwrap();
        assert_eq!(
            super::deployed_schema_id_from_dir(&dir).as_deref(),
            Some("selected")
        );
        std::fs::remove_file(dir.join("build/selected.schema.yaml")).unwrap();
        assert_eq!(
            super::deployed_schema_id_from_dir(&dir).as_deref(),
            Some("keytao")
        );
        std::fs::write(dir.join("build/selected.schema.yaml"), "").unwrap();
        std::fs::write(
            dir.join("build/default.yaml"),
            "schema_list:\n  - schema: keytao\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - schema: selected\n",
        )
        .unwrap();
        assert_eq!(
            super::deployed_schema_id_from_dir(&dir).as_deref(),
            Some("keytao")
        );
        std::fs::remove_file(dir.join("build/default.yaml")).unwrap();
        assert_eq!(
            super::deployed_schema_id_from_dir(&dir).as_deref(),
            Some("selected")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleared_schema_list_does_not_restore_stale_compiled_addon() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-empty-schema-{suffix}"));
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(
            dir.join("build/default.yaml"),
            "schema_list:\n  - schema: easy_en\n",
        )
        .unwrap();
        std::fs::write(dir.join("build/easy_en.schema.yaml"), "").unwrap();
        std::fs::write(
            dir.join("user.yaml"),
            "var:\n  previously_selected_schema: easy_en\n",
        )
        .unwrap();
        assert_eq!(
            super::deployed_schema_id_from_dir(&dir).as_deref(),
            Some("easy_en")
        );
        for content in ["patch:\n  schema_list: []\n", "patch:\n  schema_list:\n"] {
            std::fs::write(dir.join("default.custom.yaml"), content).unwrap();
            assert!(preferred_schema_id_from_dir(&dir).is_none());
            assert!(super::deployed_schema_id_from_dir(&dir).is_none());
            assert!(super::configured_schema_list_from_dir(&dir).is_empty());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn merge_default_custom_keeps_user_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: user_schema\n    - schema: keytao_old\n    - schema: txjx\n";
        let package = "patch:\n  schema_list:\n    - schema: keytao\n    - schema: keytao-dz\n";
        let (merged, user) = merge_default_custom_content(Some(existing), package).unwrap();
        assert_eq!(user, vec!["user_schema"]);
        assert!(merged.contains("- schema: user_schema"));
        assert!(merged.contains("- schema: keytao"));
        assert!(merged.contains("- schema: keytao-dz"));
        assert!(merged.contains("- schema: keytao_old"));
        assert!(merged.contains("- schema: txjx"));
    }

    #[test]
    fn merge_default_custom_preserves_addon_after_package_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: user_schema\n    - schema: easy_en\n    - schema: wanxiang\n    - schema: rime_ice\n    - schema: rime_mint\n    - schema: rime_frost\n    - schema: snow_jiandao\n    - schema: triple_pinyin_lssp\n    - schema: keydo\n";
        let package = "patch:\n  schema_list:\n    - schema: keytao\n    - schema: wanxiang\n    - schema: rime_ice\n    - schema: rime_mint\n    - schema: rime_frost\n    - schema: snow_jiandao\n    - schema: triple_pinyin_lssp\n    - schema: keytao-dz\n";
        let (merged, user) = merge_default_custom_content(Some(existing), package).unwrap();
        let schemas = parse_schema_list(&merged);

        assert_eq!(user, vec!["user_schema"]);
        assert_eq!(
            schemas,
            vec![
                "user_schema",
                "snow_jiandao",
                "keydo",
                "keytao",
                "keytao-dz",
                "easy_en",
                "wanxiang",
                "rime_ice",
                "rime_mint",
                "rime_frost",
                "triple_pinyin_lssp"
            ]
        );
    }

    #[test]
    fn preferred_schema_uses_addon_only_without_other_schemas() {
        for addon in [
            "easy_en",
            "wanxiang",
            "rime_ice",
            "rime_mint",
            "rime_frost",
            "triple_pinyin_lssp",
        ] {
            assert_eq!(
                preferred_schema_from_list(vec![addon.into(), "user_schema".into()]),
                Some("user_schema".into())
            );
            assert_eq!(
                preferred_schema_from_list(vec![addon.into(), "keytao".into()]),
                Some("keytao".into())
            );
            assert_eq!(
                preferred_schema_from_list(vec![addon.into()]),
                Some(addon.into())
            );
        }
        assert_eq!(
            preferred_schema_from_list(vec!["wanxiang".into(), "easy_en".into()]),
            Some("wanxiang".into())
        );
    }

    #[test]
    fn merge_default_custom_accepts_non_keytao_package_schemas() {
        let existing = "patch:\n  schema_list:\n    - schema: user_schema\n    - schema: keytao\n    - schema: xmjd6\n";
        let package = "patch:\n  schema_list:\n    - schema: txjx\n";
        let (merged, user) = merge_default_custom_content(Some(existing), package).unwrap();
        assert_eq!(user, vec!["user_schema"]);
        assert!(merged.contains("- schema: user_schema"));
        assert!(merged.contains("- schema: txjx"));
        assert!(merged.contains("- schema: keytao"));
        assert!(merged.contains("- schema: xmjd6"));
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
        assert!(merged.contains("- schema: keydo"));
    }

    #[test]
    fn parse_rime_lua_requires_skips_block_comments() {
        let content = "--[[\nfoo = require(\"bar\")\n--]]\nreal = require(\"real\")\n";
        assert_eq!(parse_rime_lua_requires(content), vec!["real"]);
    }

    #[test]
    fn generate_rime_lua_preserves_package_verbatim() {
        let package = "-- Package settings\r\ncollectgarbage('setpause', 150)\r\ncollectgarbage('setstepmul', 200)";
        assert_eq!(
            generate_rime_lua_content(Some(package), &[]).as_deref(),
            Some(package)
        );
        assert_eq!(generate_rime_lua_content(None, &[]), None);
        assert_eq!(generate_rime_lua_content(Some(""), &[]), None);
    }

    #[test]
    fn generate_rime_lua_appends_each_addon_registration_once() {
        let package = "scheme = require('scheme')";
        let addon = "addon = require('addon')";
        let generated = generate_rime_lua_content(
            Some(package),
            &[addon, "  addon = require('addon')  ", package],
        )
        .unwrap();
        assert_eq!(generated, format!("{package}\n{addon}\n"));
        assert_eq!(
            generate_rime_lua_content(None, &[addon]).unwrap(),
            format!("{addon}\n")
        );
        assert_eq!(
            generate_rime_lua_content(Some(&generated), &[addon]).unwrap(),
            generated,
        );
    }

    #[test]
    fn generate_rime_lua_keeps_scheme_globals_over_addons() {
        let package = concat!(
            "number_translator = require('xnumber')\n",
            "flag = true\n",
            "first, second = 1, 2\n",
            "function translator(input, seg)\nend\n",
        );
        let addon_lines = [
            "number_translator = require('translators.xnumber')",
            "flag = false",
            "second = require('addon')",
            "translator = require('addon')",
            "addon = require('addon')",
        ];
        assert_eq!(
            generate_rime_lua_content(Some(package), &addon_lines).unwrap(),
            format!("{package}addon = require('addon')\n"),
        );
    }

    #[test]
    fn generate_rime_lua_ignores_commented_scheme_bindings() {
        let package = "--[[\naddon = false\n]]\n-- addon = false\n";
        let addon = "addon = require('addon')";
        assert_eq!(
            generate_rime_lua_content(Some(package), &[addon]).unwrap(),
            format!("{package}{addon}\n"),
        );
    }

    #[test]
    fn deployment_regenerates_active_rime_lua_idempotently() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-active-lua-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rime.lua");
        let manifest_path = dir.join(super::ACTIVE_SCHEME_MANIFEST);
        let package = "number_translator = require('xnumber')\ncollectgarbage('setpause', 150)";
        let mut manifest = serde_json::json!({
            "root": dir.canonicalize().unwrap(), "rime_lua": package,
        });
        std::fs::write(
            &path,
            "number_translator = require('translators.xnumber')\n",
        )
        .unwrap();
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        for _ in 0..2 {
            super::begin_deployment(&dir).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), package);
        }
        manifest["rime_lua"] = serde_json::Value::Null;
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        for _ in 0..2 {
            super::begin_deployment(&dir).unwrap();
            assert!(!path.exists());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deployment_keeps_legacy_lua_without_snapshot() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-legacy-active-lua-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rime.lua");
        let manifest_path = dir.join(super::ACTIVE_SCHEME_MANIFEST);
        let existing = "existing = true\r\n";
        let manifest = serde_json::json!({
            "root": dir.canonicalize().unwrap(), "scheme_key": "keytao", "manifest": {},
        })
        .to_string();
        std::fs::write(&path, existing).unwrap();
        std::fs::write(&manifest_path, &manifest).unwrap();
        for _ in 0..2 {
            super::begin_deployment(&dir).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
            assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), manifest);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lua_user_edits_are_preserved_once_while_package_changes() {
        let dir = std::env::temp_dir().join(format!(
            "keytao-lua-user-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let user = "require('old-package')\nmanual = true\r\n";
        std::fs::write(dir.join("rime.lua"), user).unwrap();
        let preserved = super::preserve_rime_lua_user_edits(&dir, Some("require('old-package')\n"))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&preserved).unwrap(), user);
        assert!(
            super::preserve_rime_lua_user_edits(&dir, Some("require('old-package')\n"))
                .unwrap()
                .is_none()
        );
        let manifest = serde_json::json!({"root": dir.canonicalize().unwrap(),
            "rime_lua": "require('new-package')\n"});
        std::fs::write(
            dir.join(super::ACTIVE_SCHEME_MANIFEST),
            manifest.to_string(),
        )
        .unwrap();
        super::regenerate_active_rime_lua(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("rime.lua")).unwrap(),
            "require('new-package')\n"
        );
        assert_eq!(std::fs::read_to_string(preserved).unwrap(), user);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deployment_keeps_lua_until_snapshot_is_valid() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-invalid-active-lua-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rime.lua");
        let manifest_path = dir.join(super::ACTIVE_SCHEME_MANIFEST);
        let existing = "existing = true\n";
        std::fs::write(&path, existing).unwrap();
        super::begin_deployment(&dir).unwrap();
        let deployed_marker = dir.join("build").join(super::DEPLOYED_FILE);
        std::fs::write(&deployed_marker, "previous deployment").unwrap();
        for manifest in [
            serde_json::json!({"root": dir.canonicalize().unwrap(), "rime_lua": 7}),
            serde_json::json!({"root": dir.join("other"), "rime_lua": "bad = true"}),
            serde_json::json!({"rime_lua": null}),
        ] {
            std::fs::write(&manifest_path, manifest.to_string()).unwrap();
            assert!(super::begin_deployment(&dir).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
        }
        assert!(!deployed_marker.exists());
        std::fs::write(&manifest_path, "invalid json").unwrap();
        assert!(super::begin_deployment(&dir).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
        std::fs::remove_file(&manifest_path).unwrap();
        std::fs::create_dir(&manifest_path).unwrap();
        assert!(super::begin_deployment(&dir).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn deployment_rejects_symlinked_active_lua_or_manifest() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-symlink-active-lua-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rime.lua");
        let target = dir.join("other.lua");
        let manifest_path = dir.join(super::ACTIVE_SCHEME_MANIFEST);
        let manifest = serde_json::json!({
            "root": dir.canonicalize().unwrap(), "rime_lua": "scheme = true\n",
        });
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        std::fs::write(&target, "preserve = true\n").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(super::begin_deployment(&dir).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "preserve = true\n"
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&manifest_path).unwrap();
        std::os::unix::fs::symlink(&target, &manifest_path).unwrap();
        assert!(super::begin_deployment(&dir).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "preserve = true\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prune_rime_logs_keeps_newest_twenty_and_preserves_other_files() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-glog-prune-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        let levels = ["INFO", "WARNING", "ERROR", "FATAL"];
        let mut names = Vec::new();
        for index in 0..25 {
            let name = format!("rime.keytao.log.{}.{:02}", levels[index % 4], 99 - index);
            let file = std::fs::File::create(dir.join(&name)).unwrap();
            file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(index as u64 + 1))
                .unwrap();
            names.push(name);
        }
        for name in [
            "keytao-app.log",
            "keytao-ime.log",
            "rime.log.INFO",
            "other.txt",
        ] {
            std::fs::write(dir.join(name), "keep").unwrap();
        }
        std::fs::create_dir(dir.join("directory.log.INFO.keep")).unwrap();
        super::prune_rime_logs(&dir);
        for (index, name) in names.iter().enumerate() {
            assert_eq!(dir.join(name).exists(), index >= 5, "{name}");
        }
        for name in [
            "keytao-app.log",
            "keytao-ime.log",
            "rime.log.INFO",
            "other.txt",
        ] {
            assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), "keep");
        }
        assert!(dir.join("directory.log.INFO.keep").is_dir());
        super::prune_rime_logs(&dir.join("missing"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn windows_lua_compatibility_replaces_staged_files_without_changing_hard_links() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-lua-compat-test-{suffix}"));
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(
            dir.join("rime.lua"),
            concat!(
                "-- keydo_select_processor = require(\"ignored\")\n",
                "keydo_select_processor = require(\"keydo.processors.select\")\n",
                "keydo_date_time_translator = require('keydo.translators.date_time')\n",
                "keydo_cand_filter = require(\"keydo.filters.cand\")\n",
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("keydo.schema.yaml"),
            concat!(
                "engine:\n",
                "  processors:\n",
                "    - lua_processor@keydo_select_processor\n",
                "  translators:\n",
                "    - lua_translator@keydo_date_time_translator\n",
                "  filters:\n",
                "    - lua_filter@keydo_cand_filter\n",
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("build/keydo.schema.yaml"),
            concat!(
                "engine:\n",
                "  processors:\n",
                "    - lua_processor@keydo_select_processor\n",
                "  translators:\n",
                "    - lua_translator@keydo_date_time_translator\n",
                "  filters:\n",
                "    - lua_filter@keydo_cand_filter\n",
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("lua/helpers.lua"),
            concat!(
                "local function is_key(key, key_event)\n",
                "    local target_key = string.char(key_event.keycode) -- 当前按键对应字符\n\n",
                "    -- 若无目标键位，则交由其它函数进行判断\n",
                "    if not key then\n",
                "        return true\n",
                "    end\n",
                "    return target_key == key\n",
                "end\n",
            ),
        )
        .unwrap();

        let original_files = [
            "keydo.schema.yaml",
            "build/keydo.schema.yaml",
            "lua/helpers.lua",
        ]
        .map(|relative| {
            let path = dir.join(relative);
            let original = path.with_extension("original");
            std::fs::hard_link(&path, &original).unwrap();
            (original, std::fs::read(path).unwrap())
        });

        let changed = patch_windows_lua_compatibility(&dir).unwrap();
        assert_eq!(
            changed,
            vec![
                "build/keydo.schema.yaml",
                "keydo.schema.yaml",
                "lua/helpers.lua"
            ]
        );
        let schema = std::fs::read_to_string(dir.join("keydo.schema.yaml")).unwrap();
        assert!(schema.contains("lua_processor@*keydo.processors.select"));
        assert!(schema.contains("lua_translator@*keydo.translators.date_time"));
        assert!(schema.contains("lua_filter@*keydo.filters.cand"));
        let compiled = std::fs::read_to_string(dir.join("build/keydo.schema.yaml")).unwrap();
        assert!(compiled.contains("lua_processor@*keydo.processors.select"));
        assert!(compiled.contains("lua_translator@*keydo.translators.date_time"));
        assert!(compiled.contains("lua_filter@*keydo.filters.cand"));
        let helpers = std::fs::read_to_string(dir.join("lua/helpers.lua")).unwrap();
        assert!(helpers.contains("if not key then\n        return true"));
        assert!(helpers.contains("keycode >= 0x7f"));
        assert_eq!(
            patch_windows_lua_compatibility(&dir).unwrap(),
            Vec::<String>::new()
        );
        for (original, bytes) in original_files {
            assert_eq!(std::fs::read(original).unwrap(), bytes);
        }

        let _ = std::fs::remove_dir_all(dir);
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
