//! Pure librime engine wrapper — no Tauri, no D-Bus, no platform I/O.
//! Every platform frontend (Tauri app, ibus engine, macOS IMKit, Windows TSF)
//! links against this crate as its rime back-end.

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
    },
    time::Duration,
};

/// A poisoned lock only means some other caller panicked while holding it; the
/// input method must keep working, so the data is recovered instead of
/// propagating the panic (a panic across an FFI boundary aborts the process).
fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read_ignore_poison<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_ignore_poison<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

// ── Public types ──────────────────────────────────────────────────────────────

pub const WINDOWS_IME_ENGINE_INIT_MUTEX_NAME: &str = "Local\\KeyTao.WindowsIme.EngineInit";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImeState {
    pub preedit: String,
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

/// What the librime the process is linked against can do through its own
/// entry points, as opposed to through synthesized key strokes.
///
/// The vendored iOS librime is 1.8.5 and predates paging and highlighting, and
/// a user-supplied system librime can be anything, so a frontend must be able
/// to ask instead of assume: a capability that is missing degrades to a
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
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    use librime_sys::RimeFindModule;
    use librime_sys::{
        rime_get_api, RimeApi, RimeCandidateListIterator, RimeConfig, RimeConfigIterator,
        RimeLeversApi, RimeTraits, RimeUserDictIterator,
    };
    use rime_api::{create_session, KeyEvent, KeyStatus};
    use std::cell::Cell;
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
    /// that also take it. It is never held across platform I/O outside librime.
    static RIME_API_LOCK: Mutex<()> = Mutex::new(());
    thread_local! {
        static RIME_API_LOCK_DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    /// Whether librime is currently initialized. `reinitialize_rime` finalizes
    /// and initializes again so that a redeploy is not served from the caches
    /// above.
    static RIME_INITIALIZED: Mutex<bool> = Mutex::new(false);
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

    // `change_page` only exists in the headers of librime 1.9 and newer, see
    // [`engine_capabilities`].
    #[cfg(all(test, not(target_os = "ios")))]
    mod api_member_tests {
        use super::{active_user_dictionary_text, api_has_member};
        use librime_sys::RimeApi;
        use std::ffi::c_int;
        use std::mem::{offset_of, size_of};

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
        let initialized_now = {
            let _rime = rime_api_lock();
            let mut initialized = lock_ignore_poison(&RIME_INITIALIZED);
            if *initialized {
                false
            } else {
                initialize_rime(&user_data_dir, &shared_data_dir);
                *initialized = true;
                true
            }
        };
        if initialized_now {
            initialize_user_dictionary_cache(Path::new(&user_data_dir));
        }
        Ok(())
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
        {
            let _rime = rime_api_lock();
            finalize_rime()?;
            let mut initialized = lock_ignore_poison(&RIME_INITIALIZED);
            initialize_rime(&user_data_dir, &shared_data_dir);
            *initialized = true;
        }
        initialize_user_dictionary_cache(Path::new(&user_data_dir));
        Ok(())
    }

    /// Initialize and fully deploy librime.
    /// `setup` + `initialize` run only on the first call; subsequent calls only
    /// re-run `full_deploy_and_wait` so that newly installed schemas are picked up.
    /// Android deploys one schema at a time to keep large dependency graphs from
    /// exhausting the app process while producing the same build artifacts.
    /// Blocking — run inside `tokio::task::spawn_blocking` when called from async code.
    pub fn deploy(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
        let _deploy = lock_ignore_poison(&DEPLOY_LOCK);
        let log_dir = rime_log_dir(Path::new(&user_data_dir));

        #[cfg(target_os = "windows")]
        patch_windows_lua_compatibility(Path::new(&user_data_dir))?;

        #[cfg(target_os = "android")]
        {
            setup_only(user_data_dir.clone(), shared_data_dir)?;
            return deploy_android_staged(&user_data_dir).map_err(|error| {
                format!(
                    "Rime deployment failed: {error}. See librime logs in {}",
                    log_dir.display()
                )
            });
        }

        #[cfg(not(target_os = "android"))]
        {
            setup_only(user_data_dir.clone(), shared_data_dir)?;
            if !full_deploy_and_wait() {
                return Err(format!(
                    "Rime deployment failed. See librime logs in {}",
                    log_dir.display()
                ));
            }

            let user_dir = Path::new(&user_data_dir);
            deploy_desktop_schema_dependencies(user_dir).map_err(|error| {
                format!(
                    "Rime dependency deployment failed: {error}. See librime logs in {}",
                    log_dir.display()
                )
            })?;
            validate_deployed_schemas(user_dir).map_err(|error| {
                format!(
                    "Rime deployment validation failed: {error}. See librime logs in {}",
                    log_dir.display()
                )
            })
        }
    }

    #[cfg(target_os = "android")]
    pub fn deploy_android_config(
        user_data_dir: String,
        shared_data_dir: String,
    ) -> Result<Vec<String>, String> {
        let _deploy = lock_ignore_poison(&DEPLOY_LOCK);
        setup_only(user_data_dir.clone(), shared_data_dir)?;
        deploy_config_file("default.yaml", "config_version")?;
        let schemas = parse_schema_list_from_dir(Path::new(&user_data_dir));
        if schemas.is_empty() {
            Err("no schema selected in default.custom.yaml".into())
        } else {
            Ok(schemas)
        }
    }

    #[cfg(target_os = "android")]
    pub fn deploy_android_schema(
        user_data_dir: String,
        shared_data_dir: String,
        schema_id: String,
    ) -> Result<Vec<String>, String> {
        let _deploy = lock_ignore_poison(&DEPLOY_LOCK);
        setup_only(user_data_dir.clone(), shared_data_dir)?;
        let source = Path::new(&user_data_dir).join(format!("{schema_id}.schema.yaml"));
        if !source.is_file() {
            return Err(format!("missing schema source: {}", source.display()));
        }
        let _dictionary_override =
            prepare_android_auxiliary_dictionary(Path::new(&user_data_dir), &schema_id)?;
        deploy_schema_file(&source)?;
        let compiled = Path::new(&user_data_dir)
            .join("build")
            .join(format!("{schema_id}.schema.yaml"));
        Ok(schema_dependencies_from_file(&compiled))
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

    fn initialize_rime(user_data_dir: &str, shared_data_dir: &str) {
        let user_dir = Path::new(user_data_dir);
        let shared_dir = Path::new(shared_data_dir);
        let (staging_dir, prebuilt_dir) = rime_build_dirs(user_dir, shared_dir);
        let log_dir = rime_log_dir(user_dir);

        let _ = std::fs::create_dir_all(&staging_dir);
        let _ = std::fs::create_dir_all(&prebuilt_dir);
        let _ = std::fs::create_dir_all(&log_dir);

        setup_rime(
            user_data_dir,
            shared_data_dir,
            &staging_dir.to_string_lossy(),
            &prebuilt_dir.to_string_lossy(),
            &log_dir.to_string_lossy(),
        );
    }

    #[cfg(not(target_os = "android"))]
    fn deploy_desktop_schema_dependencies(user_dir: &Path) -> Result<(), String> {
        let initial_schemas = parse_schema_list_from_dir(user_dir);
        if initial_schemas.is_empty() {
            return Err("no schema selected in default.custom.yaml".into());
        }

        let mut visited: HashSet<String> = initial_schemas.iter().cloned().collect();
        let mut pending = std::collections::VecDeque::new();
        for schema_id in &initial_schemas {
            let compiled = compiled_schema_path(user_dir, schema_id);
            if !compiled.is_file() {
                let source = user_dir.join(format!("{schema_id}.schema.yaml"));
                if !source.is_file() {
                    return Err(format!("missing schema source: {}", source.display()));
                }
                deploy_schema_file(&source)?;
            }
            require_compiled_schema(&compiled, schema_id)?;
            pending.extend(schema_dependencies_from_file(&compiled));
        }

        while let Some(schema_id) = pending.pop_front() {
            if !visited.insert(schema_id.clone()) {
                continue;
            }

            let source = user_dir.join(format!("{schema_id}.schema.yaml"));
            if !source.is_file() {
                continue;
            }
            deploy_schema_file(&source)?;

            let compiled = compiled_schema_path(user_dir, &schema_id);
            require_compiled_schema(&compiled, &schema_id)?;
            pending.extend(schema_dependencies_from_file(&compiled));
        }
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    fn validate_deployed_schemas(user_dir: &Path) -> Result<(), String> {
        let schemas = parse_schema_list_from_dir(user_dir);
        if schemas.is_empty() {
            return Err("no schema selected in default.custom.yaml".into());
        }
        // Reading the build directory is plain file I/O and stays out of the
        // librime lock; only the validation session below needs it.
        for schema_id in &schemas {
            require_compiled_schema(&compiled_schema_path(user_dir, schema_id), schema_id)?;
        }

        let _rime = rime_api_lock();
        let session =
            create_session().map_err(|error| format!("create validation session: {error:?}"))?;
        for schema_id in &schemas {
            select_schema_checked(&session, schema_id)?;
        }
        Ok(())
    }

    #[cfg(target_os = "android")]
    fn deploy_android_staged(user_data_dir: &str) -> Result<(), String> {
        deploy_config_file("default.yaml", "config_version")?;

        let user_dir = Path::new(user_data_dir);
        let initial_schemas = parse_schema_list_from_dir(user_dir);
        if initial_schemas.is_empty() {
            return Err("no schema selected in default.custom.yaml".into());
        }

        let mut pending = initial_schemas
            .into_iter()
            .map(|schema| (schema, true))
            .collect::<std::collections::VecDeque<_>>();
        let mut deployed = HashSet::new();
        while let Some((schema_id, required)) = pending.pop_front() {
            if !deployed.insert(schema_id.clone()) {
                continue;
            }

            let source = user_dir.join(format!("{schema_id}.schema.yaml"));
            if !source.is_file() {
                if required {
                    return Err(format!("missing schema source: {}", source.display()));
                }
                continue;
            }
            let _dictionary_override = prepare_android_auxiliary_dictionary(user_dir, &schema_id)?;
            deploy_schema_file(&source)?;

            let compiled = user_dir
                .join("build")
                .join(format!("{schema_id}.schema.yaml"));
            for dependency in schema_dependencies_from_file(&compiled) {
                if !deployed.contains(&dependency) {
                    pending.push_back((dependency, false));
                }
            }
        }
        Ok(())
    }

    #[cfg(target_os = "android")]
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
        [
            "default.custom.yaml",
            "default-custom.yaml",
            "default.yaml",
            "build/default.yaml",
        ]
        .iter()
        .filter_map(|name| std::fs::read_to_string(user_dir.join(name)).ok())
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

    fn compiled_schema_path(user_dir: &Path, schema_id: &str) -> PathBuf {
        user_dir
            .join("build")
            .join(format!("{schema_id}.schema.yaml"))
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
            require_levers_module();
            require_lua_module();

            let api = rime_get_api();
            if let Some(setup) = (*api).setup {
                setup(&mut traits);
            }
            if let Some(initialize) = (*api).initialize {
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
        }
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

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    unsafe fn lua_module_registered() -> bool {
        !RimeFindModule(c"lua".as_ptr()).is_null()
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

    #[cfg(not(target_os = "android"))]
    fn full_deploy_and_wait() -> bool {
        let _rime = rime_api_lock();
        {
            let mut result = lock_ignore_poison(&DEPLOY_RESULT);
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
        *lock_ignore_poison(&DEPLOY_RESULT) == Some(true)
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

    /// An active rime input session.
    pub struct Engine {
        session: ManuallyDrop<rime_api::Session>,
        user_data_dir: Option<PathBuf>,
        user_dictionary_source_available: bool,
    }

    // SAFETY: Session holds only a usize (session_id) and librime itself is not
    // thread-safe, so every method — including `Drop`, which destroys the
    // session — serializes on `RIME_API_LOCK` before touching rime_api.
    unsafe impl Send for Engine {}
    unsafe impl Sync for Engine {}

    impl Drop for Engine {
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
    ///
    /// `vendor/librime/ios` is built from librime 1.8.5, whose `RimeApi` has no
    /// `change_page` and no `highlight_candidate_on_current_page` field at all,
    /// so those two are answered at compile time. Remove the `cfg` once the
    /// vendored iOS librime is rebuilt from 1.9 or newer.
    pub(crate) fn engine_capabilities() -> EngineCapabilities {
        let _rime = rime_api_lock();
        // SAFETY: the librime lock is held and only the function pointers of
        // the static API table are read, each one gated on `data_size`.
        unsafe {
            let api = rime_get_api();
            if api.is_null() {
                return EngineCapabilities::none();
            }

            #[cfg(target_os = "ios")]
            let (native_paging, candidate_highlight) = (false, false);
            #[cfg(not(target_os = "ios"))]
            let (native_paging, candidate_highlight) = (
                rime_api_member!(api, change_page).is_some(),
                rime_api_member!(api, highlight_candidate_on_current_page).is_some(),
            );

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

    impl Engine {
        /// Create a new session. `deploy()` must have succeeded first.
        pub fn new() -> Result<Self, String> {
            Self::new_with_user_data_dir(None)
        }

        /// What this librime can do natively, see [`EngineCapabilities`].
        pub fn capabilities(&self) -> EngineCapabilities {
            let mut capabilities = engine_capabilities();
            capabilities.candidate_deletion &= self.user_dictionary_source_available;
            capabilities
        }

        /// Whether [`Engine::change_page`] pages through librime instead of
        /// replaying the `-`/`=` bindings.
        pub fn supports_native_paging(&self) -> bool {
            engine_capabilities().supports_native_paging()
        }

        /// Whether [`Engine::select_candidate_on_page`] selects through librime
        /// instead of sending a select key.
        pub fn supports_candidate_selection(&self) -> bool {
            engine_capabilities().supports_candidate_selection()
        }

        pub(crate) fn new_with_user_data_dir(user_data_dir: Option<&Path>) -> Result<Self, String> {
            // Locating and checking the compiled schema is file I/O; the
            // librime lock is only taken once the session is actually created.
            let preferred = preferred_schema_location(user_data_dir);
            if let Some((dir, schema_id)) = &preferred {
                require_compiled_schema(&compiled_schema_path(dir, schema_id), schema_id)?;
            }
            let effective_user_data_dir = user_data_dir
                .map(Path::to_path_buf)
                .or_else(|| preferred.as_ref().map(|(dir, _)| dir.clone()))
                .or_else(default_user_data_dir);
            let user_dictionary_source_available =
                user_dictionary_cache_available_for(effective_user_data_dir.as_deref());
            let _rime = rime_api_lock();
            let session = create_session().map_err(|e| format!("{e:?}"))?;
            if let Some((_, schema_id)) = &preferred {
                // ascii_mode is left to the schema switches and to ascii_composer;
                // a session must not silently override the user's mode.
                select_schema_checked(&session, schema_id)?;
            } else {
                validate_active_schema(&session)?;
            }
            Ok(Self {
                session: ManuallyDrop::new(session),
                user_data_dir: effective_user_data_dir,
                user_dictionary_source_available,
            })
        }

        pub fn process_key(&self, keycode: u32, mask: u32) -> ImeState {
            self.process_key_result(keycode, mask).state
        }

        pub fn process_key_result(&self, keycode: u32, mask: u32) -> KeyProcessResult {
            let _rime = rime_api_lock();
            let status = self.session.process_key(key_event(keycode, mask));
            let state = extract_state_with_commit(&self.session);
            self.remember_committed_user_phrase(&state);
            KeyProcessResult {
                state,
                accepted: matches!(status, KeyStatus::Accept),
            }
        }

        /// Read-only snapshot: `RimeGetCommit` consumes the pending commit, so a
        /// query must never call it or the text would never reach the client.
        pub fn state(&self) -> ImeState {
            let _rime = rime_api_lock();
            extract_state_readonly(&self.session)
        }

        /// Pick the `index`-th candidate of the current page.
        ///
        /// Kept as the historical name of [`Engine::select_candidate_on_page`].
        pub fn select_candidate(&self, index: usize) -> ImeState {
            self.select_candidate_on_page(index)
        }

        /// Pick the `index`-th candidate of the current page through librime's
        /// own API, so that selection does not depend on `menu/select_keys`
        /// being long enough or on the schema's alphabet.
        pub fn select_candidate_on_page(&self, index: usize) -> ImeState {
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
            let state = extract_state_with_commit(&self.session);
            self.remember_committed_user_phrase(&state);
            state
        }

        /// Move the highlight without committing, for hover/arrow interactions.
        pub fn highlight_candidate_on_page(&self, index: usize) -> ImeState {
            let _rime = rime_api_lock();
            // `vendor/librime/ios` is built from librime 1.8.5, whose `RimeApi`
            // predates `highlight_candidate_on_current_page` (added in 1.9), so
            // the field does not exist at compile time and moving the highlight
            // without committing degrades to a no-op. Remove the gate once the
            // vendored iOS librime is rebuilt from 1.9 or newer.
            #[cfg(target_os = "ios")]
            let _ = index;
            // SAFETY: the librime lock is held; the pointer is checked.
            #[cfg(not(target_os = "ios"))]
            unsafe {
                let api = rime_get_api();
                if let Some(highlight) = rime_api_member!(api, highlight_candidate_on_current_page)
                {
                    highlight(self.session.session_id, index);
                }
            }
            extract_state_with_commit(&self.session)
        }

        /// Whether the `index`-th candidate on the current page is backed by an
        /// entry in one of the active schema's user dictionaries.
        pub fn candidate_is_user_phrase_on_page(&self, index: usize) -> bool {
            let candidate = {
                let _rime = rime_api_lock();
                let state = extract_state_readonly(&self.session);
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

        /// Forget the `index`-th candidate of the current page, but only after
        /// the active user dictionary confirms that it is a learned phrase.
        pub fn delete_candidate_on_page_result(&self, index: usize) -> (ImeState, bool) {
            let (before, candidate_text, dictionaries) = {
                let _rime = rime_api_lock();
                let before = extract_state_readonly(&self.session);
                let Some(candidate_text) = before
                    .candidates
                    .get(index)
                    .map(|candidate| candidate.text.clone())
                else {
                    return (before, false);
                };
                let Ok(status) = self.session.status() else {
                    return (extract_state_readonly(&self.session), false);
                };
                let dictionaries = schema_user_dictionary_names_locked(status.schema_id());
                (before, candidate_text, dictionaries)
            };
            if !self.user_dictionary_contains(&dictionaries, &candidate_text) {
                return (before, false);
            }

            let _rime = rime_api_lock();
            let current = extract_state_readonly(&self.session);
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
            (extract_state_with_commit(&self.session), deleted)
        }

        pub fn delete_candidate_on_page(&self, index: usize) -> ImeState {
            self.delete_candidate_on_page_result(index).0
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

        pub fn select_candidate_global(&self, index: usize) -> ImeState {
            let _rime = rime_api_lock();
            // SAFETY: the librime lock is held; the pointer is checked.
            unsafe {
                let api = rime_get_api();
                if let Some(select_candidate) = rime_api_member!(api, select_candidate) {
                    select_candidate(self.session.session_id, index);
                }
            }
            let state = extract_state_with_commit(&self.session);
            self.remember_committed_user_phrase(&state);
            state
        }

        pub fn all_candidates(&self) -> Vec<Candidate> {
            self.all_candidates_limited(usize::MAX)
        }

        pub fn all_candidates_limited(&self, max_count: usize) -> Vec<Candidate> {
            let _rime = rime_api_lock();
            extract_all_candidates(&self.session, max_count).unwrap_or_default()
        }

        /// Turn a candidate page through librime instead of replaying the
        /// `-`/`=` bindings, which only exist when a schema imports the default
        /// `key_binder` paging preset.
        pub fn change_page(&self, backward: bool) -> ImeState {
            let _rime = rime_api_lock();
            // `vendor/librime/ios` is built from librime 1.8.5, whose `RimeApi`
            // predates `change_page` (added in 1.9), so the field does not exist
            // at compile time and paging always takes the synthetic-key path.
            // Remove the gate once the vendored iOS librime is rebuilt from 1.9
            // or newer.
            #[cfg(target_os = "ios")]
            let handled = false;
            // SAFETY: the librime lock is held; a missing pointer falls back to
            // the historical synthetic key.
            #[cfg(not(target_os = "ios"))]
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
            extract_state_with_commit(&self.session)
        }

        /// Drop the composition without committing anything.
        ///
        /// Kept as the historical name of [`Engine::clear_composition`].
        pub fn reset(&self) -> ImeState {
            self.clear_composition()
        }

        /// Discard the composition through librime, so that the outcome does
        /// not depend on how a schema bound `Escape`.
        pub fn clear_composition(&self) -> ImeState {
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
            extract_state_with_commit(&self.session)
        }

        /// Commit whatever librime currently holds, the way a frontend has to
        /// finish a composition when the input context goes away.
        pub fn commit_composition(&self) -> ImeState {
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
            extract_state_with_commit(&self.session)
        }

        /// The raw input string librime is composing from, before any editor or
        /// filter rewrote it for display.
        pub fn raw_input(&self) -> Option<String> {
            let _rime = rime_api_lock();
            self.raw_input_locked()
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

        /// Commit the raw input and drop the composition.
        ///
        /// This is the documented fallback for `Return` when the schema's
        /// editor passes the key back: the typed code must reach the client
        /// instead of being thrown away with the composition.
        pub fn commit_raw_input(&self) -> ImeState {
            let _rime = rime_api_lock();
            let raw = self.raw_input_locked().unwrap_or_default();
            if raw.is_empty() {
                return extract_state_with_commit(&self.session);
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
        /// raw input get committed (see [`Engine::commit_raw_input`]).
        pub fn process_enter(&self) -> KeyProcessResult {
            let _rime = rime_api_lock();
            let composing = !extract_state_readonly(&self.session).preedit.is_empty();
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

        pub fn current_schema_name(&self) -> String {
            self.current_schema()
                .map(|schema| schema.name)
                .unwrap_or_else(|| "unknown".to_string())
        }

        pub fn list_schemas(&self) -> Vec<SchemaInfo> {
            let Some(user_data_dir) = self.user_data_dir.as_deref() else {
                return Vec::new();
            };
            parse_schema_list_from_dir(user_data_dir)
                .into_iter()
                .map(|id| SchemaInfo {
                    name: schema_name_from_dir(user_data_dir, &id).unwrap_or_else(|| id.clone()),
                    id,
                })
                .collect()
        }

        pub fn current_schema(&self) -> Option<SchemaInfo> {
            let _rime = rime_api_lock();
            self.session.status().ok().map(|status| SchemaInfo {
                id: status.schema_id().to_string(),
                name: status.schema_name().to_string(),
            })
        }

        pub fn schema_switches(&self) -> Vec<SchemaSwitch> {
            let _rime = rime_api_lock();
            let Ok(status) = self.session.status() else {
                return Vec::new();
            };
            read_schema_switches_locked(status.schema_id())
        }

        pub fn select_schema(&self, schema_id: &str) -> Result<ImeState, String> {
            let _rime = rime_api_lock();
            select_schema_checked(&self.session, schema_id)?;
            Ok(extract_state_readonly(&self.session))
        }

        pub fn is_ascii_mode(&self) -> bool {
            let _rime = rime_api_lock();
            self.session
                .status()
                .map(|s| s.is_ascii_mode)
                .unwrap_or(false)
        }

        /// Set the session option without reading any state back, so that a
        /// pending commit stays pending (see [`Engine::state`]).
        pub fn apply_ascii_mode(&self, enabled: bool) {
            let _rime = rime_api_lock();
            set_session_option(&self.session, "ascii_mode", enabled);
        }

        pub fn set_ascii_mode(&self, enabled: bool) -> ImeState {
            let _rime = rime_api_lock();
            set_session_option(&self.session, "ascii_mode", enabled);
            extract_state_with_commit(&self.session)
        }

        pub fn get_option(&self, option_name: &str) -> bool {
            let _rime = rime_api_lock();
            get_session_option(&self.session, option_name)
        }

        pub fn set_option(&self, option_name: &str, enabled: bool) -> ImeState {
            let _rime = rime_api_lock();
            set_session_option(&self.session, option_name, enabled);
            extract_state_readonly(&self.session)
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

    fn read_schema_switches_locked(schema_id: &str) -> Vec<SchemaSwitch> {
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
            let Some(config_begin_list) = rime_api_member!(api, config_begin_list) else {
                return Vec::new();
            };
            let Some(config_next) = rime_api_member!(api, config_next) else {
                return Vec::new();
            };
            let Some(config_end) = rime_api_member!(api, config_end) else {
                return Vec::new();
            };

            let mut config: RimeConfig = std::mem::zeroed();
            if schema_open(schema_id.as_ptr(), &mut config) == 0 {
                return Vec::new();
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
            result
        }
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
    fn extract_state_with_commit(session: &rime_api::Session) -> ImeState {
        let committed = session.commit().map(|c| c.text().to_string());
        extract_state(session, committed)
    }

    /// Snapshot for read-only queries: leaves the pending commit for the next
    /// key event so that no text is dropped on the floor.
    fn extract_state_readonly(session: &rime_api::Session) -> ImeState {
        extract_state(session, None)
    }

    fn session_select_keys(session: &rime_api::Session) -> Option<String> {
        let ctx = session.context()?;
        ctx.menu().select_keys.map(|keys: &str| keys.to_string())
    }

    fn extract_state(session: &rime_api::Session, committed: Option<String>) -> ImeState {
        let Some(ctx) = session.context() else {
            return ImeState {
                committed,
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

        let status = session.status().ok();
        let ascii_mode = status.as_ref().map(|s| s.is_ascii_mode).unwrap_or(false);
        let schema_name = status
            .as_ref()
            .map(|s| s.schema_name().to_string())
            .unwrap_or_default();

        ImeState {
            preedit,
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
pub use desktop::{deploy, setup_only, Engine};
#[cfg(target_os = "android")]
pub use desktop::{deploy_android_config, deploy_android_schema};

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
/// CapsLock. Never forwarded to librime — see
/// [`key_policy::normalize_key_for_modifiers`], which folds it into the keysym.
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
        ImeState, RIME_MOD_CONTROL, RIME_MOD_HYPER, RIME_MOD_LOCK, RIME_MOD_META, RIME_MOD_SHIFT,
        RIME_MOD_SUPER,
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
    pub const XK_F4: u32 = 0xffc1;

    /// X11 encodes a keysym for an arbitrary Unicode scalar as
    /// `0x01000000 | codepoint`.
    pub const XK_UNICODE_BASE: u32 = 0x0100_0000;

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
                ..=XK_BEGIN | XK_KP_ENTER
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

    /// Fold `Shift`/`CapsLock` into the keysym and drop the Lock bit.
    ///
    /// Rime expects the keysym to carry the character that was actually typed
    /// while the mask carries only real modifiers, so a locked `a` must arrive
    /// as `A` and never as `a` plus a Lock bit that librime would treat as an
    /// unknown modifier.
    pub fn normalize_key_for_modifiers(sym: u32, mods: u32) -> (u32, u32) {
        let uppercase = (mods & RIME_MOD_LOCK != 0) != (mods & RIME_MOD_SHIFT != 0);
        let mods = mods & !RIME_MOD_LOCK;
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
/// CapsLock is folded into the keysym instead, see
/// [`key_policy::normalize_key_for_modifiers`].
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
        assert_eq!(rime_modifier_mask(RIME_MOD_LOCK), 0);
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
    fn normalize_key_for_modifiers_folds_caps_lock_into_the_keysym() {
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'a' as u32, RIME_MOD_LOCK),
            (b'A' as u32, 0)
        );
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'a' as u32, RIME_MOD_LOCK | RIME_MOD_SHIFT),
            (b'a' as u32, RIME_MOD_SHIFT)
        );
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'a' as u32, RIME_MOD_SHIFT),
            (b'A' as u32, RIME_MOD_SHIFT)
        );
        // Non-letters keep their keysym; only the Lock bit is dropped.
        assert_eq!(
            key_policy::normalize_key_for_modifiers(b'1' as u32, RIME_MOD_LOCK),
            (b'1' as u32, 0)
        );
        assert_eq!(
            key_policy::normalize_key_for_modifiers(
                key_policy::XK_BACK_SPACE,
                RIME_MOD_LOCK | RIME_MOD_CONTROL
            ),
            (key_policy::XK_BACK_SPACE, RIME_MOD_CONTROL)
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
    user_data_dir: Option<PathBuf>,
    shared_data_dir: Option<String>,
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
/// Lock order: `initialized` → `reload_barrier` → `sessions` → session inner →
/// librime.
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
    /// Bumped whenever librime was torn down; a session whose generation is
    /// behind rebuilds its engine before its next call.
    generation: AtomicU64,
    /// Held for reading while a session touches its engine and for writing
    /// while librime is torn down, so that no session call can race a
    /// finalize/initialize cycle.
    reload_barrier: RwLock<()>,
    /// Every session handed out in this process. A teardown must drop their
    /// engines before finalizing librime; entries die with their session.
    sessions: Mutex<Vec<Weak<Mutex<ImeRuntimeSessionInner>>>>,
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
    generation: AtomicU64::new(0),
    reload_barrier: RwLock::new(()),
    sessions: Mutex::new(Vec::new()),
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
            let Some(session) = session.upgrade() else {
                return false;
            };
            let mut inner = lock_ignore_poison(&session);
            if let Some(engine) = inner.engine.take() {
                inner.carried_ascii_mode = Some(engine.is_ascii_mode());
            }
            true
        });
    }

    fn register(&self, session: &Arc<Mutex<ImeRuntimeSessionInner>>) {
        let mut sessions = lock_ignore_poison(&self.sessions);
        sessions.retain(|session| session.strong_count() > 0);
        sessions.push(Arc::downgrade(session));
    }

    /// Tear librime down when it is up for different directories, so that the
    /// caller can bring it back up for the ones it wants. Every live engine is
    /// dropped first, exactly like a reload.
    fn shutdown_for_dir_change(
        &self,
        initialized: &mut Option<RimeDataDirs>,
        wanted: &RimeDataDirs,
    ) -> Result<(), String> {
        match initialized.as_ref() {
            None => return Ok(()),
            Some(current) if current == wanted => return Ok(()),
            Some(_) => {}
        }
        let _barrier = write_ignore_poison(&self.reload_barrier);
        self.drop_live_engines();
        *initialized = None;
        self.generation.fetch_add(1, Ordering::SeqCst);
        desktop::finalize_rime()
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
    let dirs = RimeDataDirs {
        user: PathBuf::from(&user_data_dir),
        shared: shared_data_dir.clone(),
    };
    let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
    let _barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
    PROCESS_RIME.drop_live_engines();
    let result = desktop::reinitialize_rime(user_data_dir, shared_data_dir);
    PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
    *initialized = result.is_ok().then_some(dirs);
    result
}

#[cfg(target_os = "android")]
pub fn reinitialize_android(user_data_dir: String, shared_data_dir: String) -> Result<(), String> {
    reinitialize(user_data_dir, shared_data_dir)
}

#[cfg(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "android",
    target_os = "ios"
))]
struct ImeRuntimeSessionInner {
    /// `None` between a reload and the next access; the engine is rebuilt
    /// lazily so that the old one is already gone when librime re-reads the
    /// deployment.
    engine: Option<Engine>,
    generation: u64,
    /// ascii_mode of the engine that was dropped, restored on the rebuilt one.
    carried_ascii_mode: Option<bool>,
    /// What the current input context allows; survives engine rebuilds.
    policy: InputContextPolicy,
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
        }))
    }

    /// What the linked librime can do natively, see [`EngineCapabilities`].
    pub fn capabilities(&self) -> EngineCapabilities {
        engine_capabilities()
    }

    /// Initialize and deploy. Idempotent per process: librime can only run for
    /// one pair of directories, so a second runtime asking for the ones it is
    /// already running for is a no-op instead of a second deployment.
    pub fn init(&self) -> Result<(), String> {
        let dirs = self.data_dirs()?;
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        if initialized.as_ref() == Some(&dirs) {
            return Ok(());
        }

        PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs)?;
        deploy(
            dirs.user.to_string_lossy().into_owned(),
            dirs.shared.clone(),
        )?;
        *initialized = Some(dirs);
        Ok(())
    }

    pub fn init_without_deploy(&self) -> Result<(), String> {
        let dirs = self.data_dirs()?;
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        if initialized.as_ref() == Some(&dirs) {
            return Ok(());
        }

        let user_dir = dirs.user.clone();
        let schema_state = schema_install_state(&user_dir);
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
            if windows_rime_build_repair_required(&user_dir) {
                return Err(
                    "Windows RIME build repair is pending; open the KeyTao app to finish it".into(),
                );
            }
            patch_windows_lua_compatibility(&user_dir)?;
        }

        PROCESS_RIME.shutdown_for_dir_change(&mut initialized, &dirs)?;
        setup_only(
            dirs.user.to_string_lossy().into_owned(),
            dirs.shared.clone(),
        )?;
        *initialized = Some(dirs);
        Ok(())
    }

    pub fn reload_without_deploy(&self) -> Result<(), String> {
        let dirs = self.data_dirs()?;
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        let schema_state = schema_install_state(&dirs.user);
        if !schema_state.deployed {
            return Err(
                "the installed KeyTao scheme has not been deployed in the KeyTao app".into(),
            );
        }

        let barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
        PROCESS_RIME.drop_live_engines();
        let result = desktop::reinitialize_rime(
            dirs.user.to_string_lossy().into_owned(),
            dirs.shared.clone(),
        );
        PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
        *initialized = result.is_ok().then_some(dirs);
        drop(barrier);
        result
    }

    pub fn reload(&self) -> Result<(), String> {
        let dirs = self.data_dirs()?;
        let mut initialized = lock_ignore_poison(&PROCESS_RIME.initialized);

        let barrier = write_ignore_poison(&PROCESS_RIME.reload_barrier);
        PROCESS_RIME.drop_live_engines();
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
            )
        })();
        PROCESS_RIME.generation.fetch_add(1, Ordering::SeqCst);
        *initialized = result.is_ok().then(|| dirs.clone());
        drop(barrier);
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

    pub fn create_session(&self) -> Result<ImeRuntimeSession, String> {
        let dirs = self.data_dirs()?;
        if lock_ignore_poison(&PROCESS_RIME.initialized).as_ref() != Some(&dirs) {
            self.init_without_deploy()?;
        }

        // Every teardown takes `initialized` before the reload barrier, so
        // holding it here keeps librime from being finalized between the check
        // and the session that is about to be created against it.
        let initialized = lock_ignore_poison(&PROCESS_RIME.initialized);
        if initialized.as_ref() != Some(&dirs) {
            return Err("librime is running for other data directories".into());
        }
        let barrier = read_ignore_poison(&PROCESS_RIME.reload_barrier);
        let generation = PROCESS_RIME.generation.load(Ordering::SeqCst);
        let inner = Arc::new(Mutex::new(ImeRuntimeSessionInner {
            engine: Some(Engine::new_with_user_data_dir(
                self.0.user_data_dir.as_deref(),
            )?),
            generation,
            carried_ascii_mode: None,
            policy: InputContextPolicy::default(),
        }));
        PROCESS_RIME.register(&inner);
        drop(barrier);
        drop(initialized);
        Ok(ImeRuntimeSession {
            shared: self.0.clone(),
            inner,
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
        self.with_engine(Engine::state)
            .unwrap_or_else(ImeState::empty)
    }

    pub fn process_key_result(&self, keycode: u32, mask: u32) -> Option<KeyProcessResult> {
        let (keycode, mask) = key_policy::normalize_key_for_modifiers(keycode, mask);
        let mask = rime_modifier_mask(mask);
        // Reading the policy and handling the key happen in one critical
        // section: a context that turns sensitive between the two would
        // otherwise still hand this key to librime.
        self.with_engine_and_policy(|engine, policy| {
            if policy.composing {
                engine.process_key_result(keycode, mask)
            } else {
                // A sensitive context never reaches librime: no composition, no
                // candidates and nothing for the user dictionary to learn.
                KeyProcessResult {
                    state: engine.state(),
                    accepted: false,
                }
            }
        })
    }

    /// `Return` handling shared by every frontend, see [`Engine::process_enter`].
    pub fn process_enter(&self) -> Option<KeyProcessResult> {
        self.with_engine_and_policy(|engine, policy| {
            if policy.composing {
                engine.process_enter()
            } else {
                KeyProcessResult {
                    state: engine.state(),
                    accepted: false,
                }
            }
        })
    }

    /// What the current input context allows.
    pub fn input_policy(&self) -> InputContextPolicy {
        lock_ignore_poison(&self.inner).policy
    }

    /// Declare what the current input context allows, e.g. on focus change.
    ///
    /// Turning composing off discards whatever was being composed, so the
    /// returned state is the one the frontend has to apply. The switch and the
    /// discard share one critical section, so a key being handled concurrently
    /// is either fully before or fully after the switch.
    ///
    /// The policy is recorded even when no engine can be built, otherwise a
    /// context that turned sensitive while librime was down would come back as
    /// a composing one.
    pub fn set_input_policy(&self, policy: InputContextPolicy) -> Option<ImeState> {
        let _barrier = read_ignore_poison(&PROCESS_RIME.reload_barrier);
        let mut inner = lock_ignore_poison(&self.inner);
        let previous = std::mem::replace(&mut inner.policy, policy);
        self.refresh_if_needed(&mut inner).ok()?;
        let engine = inner.engine.as_ref()?;
        Some(if previous.composing && !policy.composing {
            engine.clear_composition()
        } else {
            engine.state()
        })
    }

    pub fn select_candidate(&self, index: usize) -> Option<ImeState> {
        self.with_engine(|engine| engine.select_candidate_on_page(index))
    }

    pub fn select_candidate_on_page(&self, index: usize) -> Option<ImeState> {
        self.with_engine(|engine| engine.select_candidate_on_page(index))
    }

    pub fn highlight_candidate_on_page(&self, index: usize) -> Option<ImeState> {
        self.with_engine(|engine| engine.highlight_candidate_on_page(index))
    }

    pub fn delete_candidate_on_page(&self, index: usize) -> Option<ImeState> {
        self.with_engine(|engine| engine.delete_candidate_on_page(index))
    }

    pub fn candidate_is_user_phrase_on_page(&self, index: usize) -> Option<bool> {
        self.with_engine(|engine| engine.candidate_is_user_phrase_on_page(index))
    }

    pub fn delete_candidate_on_page_result(&self, index: usize) -> Option<(ImeState, bool)> {
        self.with_engine(|engine| engine.delete_candidate_on_page_result(index))
    }

    pub fn select_candidate_global(&self, index: usize) -> Option<ImeState> {
        self.with_engine(|engine| engine.select_candidate_global(index))
    }

    /// Commit whatever is being composed; the path a frontend takes when the
    /// input context ends and the composition must not be lost.
    pub fn commit_composition(&self) -> Option<ImeState> {
        self.with_engine(Engine::commit_composition)
    }

    /// Discard whatever is being composed; the path a frontend takes when the
    /// input context ends and the composition must not reach the client.
    pub fn clear_composition(&self) -> Option<ImeState> {
        self.with_engine(Engine::clear_composition)
    }

    pub fn commit_raw_input(&self) -> Option<ImeState> {
        self.with_engine(Engine::commit_raw_input)
    }

    pub fn raw_input(&self) -> Option<String> {
        self.with_engine(Engine::raw_input).flatten()
    }

    pub fn all_candidates(&self) -> Option<Vec<Candidate>> {
        self.with_engine(Engine::all_candidates)
    }

    pub fn all_candidates_limited(&self, max_count: usize) -> Option<Vec<Candidate>> {
        self.with_engine(|engine| engine.all_candidates_limited(max_count))
    }

    pub fn list_schemas(&self) -> Option<Vec<SchemaInfo>> {
        self.with_engine(Engine::list_schemas)
    }

    pub fn current_schema(&self) -> Option<SchemaInfo> {
        self.with_engine(Engine::current_schema).flatten()
    }

    pub fn schema_switches(&self) -> Option<Vec<SchemaSwitch>> {
        self.with_engine(Engine::schema_switches)
    }

    pub fn select_schema(&self, schema_id: &str) -> Result<ImeState, String> {
        self.with_engine(|engine| engine.select_schema(schema_id))
            .ok_or_else(|| "Rime session is unavailable".to_string())?
    }

    pub fn change_page(&self, backward: bool) -> Option<ImeState> {
        self.with_engine(|engine| engine.change_page(backward))
    }

    pub fn reset(&self) -> Option<ImeState> {
        self.with_engine(Engine::reset)
    }

    pub fn is_ascii_mode(&self) -> bool {
        self.with_engine(Engine::is_ascii_mode).unwrap_or(false)
    }

    pub fn set_ascii_mode(&self, enabled: bool) -> Option<ImeState> {
        self.with_engine(|engine| engine.set_ascii_mode(enabled))
    }

    pub fn get_option(&self, option_name: &str) -> bool {
        self.with_engine(|engine| engine.get_option(option_name))
            .unwrap_or(false)
    }

    pub fn set_option(&self, option_name: &str, enabled: bool) -> Option<ImeState> {
        self.with_engine(|engine| engine.set_option(option_name, enabled))
    }

    /// What the linked librime can do natively, see [`EngineCapabilities`].
    pub fn capabilities(&self) -> EngineCapabilities {
        self.with_engine(Engine::capabilities)
            .unwrap_or_else(EngineCapabilities::none)
    }

    /// Whether [`ImeRuntimeSession::change_page`] pages through librime instead
    /// of replaying the `-`/`=` bindings a schema may not have.
    pub fn supports_native_paging(&self) -> bool {
        engine_capabilities().supports_native_paging()
    }

    /// Whether [`ImeRuntimeSession::select_candidate_on_page`] selects through
    /// librime instead of sending a select key the schema may not have.
    pub fn supports_candidate_selection(&self) -> bool {
        engine_capabilities().supports_candidate_selection()
    }

    /// Run `action` on an engine that matches the current generation,
    /// rebuilding it first when a reload invalidated the previous one.
    /// Lock order: reload barrier → session → librime.
    fn with_engine<T>(&self, action: impl FnOnce(&Engine) -> T) -> Option<T> {
        self.with_engine_and_policy(|engine, _| action(engine))
    }

    /// Same as [`ImeRuntimeSession::with_engine`], but the action also sees the
    /// input context policy, so that policy checks and the librime call they
    /// guard cannot be split by another thread.
    fn with_engine_and_policy<T>(
        &self,
        action: impl FnOnce(&Engine, InputContextPolicy) -> T,
    ) -> Option<T> {
        let _barrier = read_ignore_poison(&PROCESS_RIME.reload_barrier);
        let mut inner = lock_ignore_poison(&self.inner);
        self.refresh_if_needed(&mut inner).ok()?;
        let policy = inner.policy;
        Some(action(inner.engine.as_ref()?, policy))
    }

    fn refresh_if_needed(&self, inner: &mut ImeRuntimeSessionInner) -> Result<(), String> {
        let current = PROCESS_RIME.generation.load(Ordering::SeqCst);
        if inner.generation == current && inner.engine.is_some() {
            return Ok(());
        }
        // Drop the previous engine before creating the replacement: librime
        // hands a new session the cached config and dictionaries as long as any
        // other session still references them.
        if let Some(engine) = inner.engine.take() {
            inner.carried_ascii_mode = Some(engine.is_ascii_mode());
        }
        let engine = Engine::new_with_user_data_dir(self.shared.user_data_dir.as_deref())?;
        if let Some(ascii_mode) = inner.carried_ascii_mode.take() {
            engine.apply_ascii_mode(ascii_mode);
        }
        inner.engine = Some(engine);
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

fn preferred_schema_location(user_data_dir: Option<&Path>) -> Option<(PathBuf, String)> {
    if let Some(dir) = user_data_dir {
        if let Some(schema) = preferred_schema_id_from_dir(dir) {
            return Some((dir.to_path_buf(), schema));
        }
    }
    default_user_data_dir()
        .and_then(|dir| preferred_schema_id_from_dir(&dir).map(|schema| (dir, schema)))
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

fn parse_schema_name(content: &str) -> Option<String> {
    let Value::Mapping(root) = serde_yaml::from_str::<Value>(content).ok()? else {
        return None;
    };
    let Value::Mapping(schema) = root.get(Value::String("schema".into()))? else {
        return None;
    };
    schema
        .get(Value::String("name".into()))?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn schema_name_from_dir(user_data_dir: &Path, schema_id: &str) -> Option<String> {
    [
        user_data_dir.join(format!("{schema_id}.schema.yaml")),
        user_data_dir
            .join("build")
            .join(format!("{schema_id}.schema.yaml")),
    ]
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .find_map(|content| parse_schema_name(&content))
}

fn parse_schema_dependencies(content: &str) -> Vec<String> {
    let Ok(Value::Mapping(root)) = serde_yaml::from_str::<Value>(content) else {
        return Vec::new();
    };
    let Some(Value::Mapping(schema)) = root.get(Value::String("schema".into())) else {
        return Vec::new();
    };
    let Some(Value::Sequence(dependencies)) = schema.get(Value::String("dependencies".into()))
    else {
        return Vec::new();
    };
    dependencies
        .iter()
        .filter_map(Value::as_str)
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
    [
        "default.custom.yaml",
        "default-custom.yaml",
        "build/default.yaml",
        "default.yaml",
    ]
    .into_iter()
    .filter_map(|name| std::fs::read_to_string(user_data_dir.join(name)).ok())
    .map(|content| parse_schema_list(&content))
    .find(|schemas| !schemas.is_empty())
    .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaInstallState {
    pub installed: bool,
    pub deployed: bool,
    pub schemas: Vec<String>,
}

pub fn schema_install_state(user_data_dir: &Path) -> SchemaInstallState {
    let schemas = ["default.custom.yaml", "default-custom.yaml"]
        .into_iter()
        .find_map(|name| std::fs::read_to_string(user_data_dir.join(name)).ok())
        .map(|content| parse_schema_list(&content))
        .unwrap_or_default()
        .into_iter()
        .filter(|schema| valid_rime_build_id(schema) && is_keytao_managed_schema(schema))
        .collect::<Vec<_>>();

    let installed = !schemas.is_empty()
        && schemas.iter().all(|schema| {
            user_data_dir
                .join(format!("{schema}.schema.yaml"))
                .is_file()
        });
    let deployed = installed
        && schemas.iter().all(|schema| {
            user_data_dir
                .join("build")
                .join(format!("{schema}.schema.yaml"))
                .is_file()
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
    let build_dir = user_data_dir.join("build");
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
    if std::fs::read_to_string(user_data_dir.join(WINDOWS_RIME_BUILD_REPAIR_MARKER))
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
    let temporary = user_data_dir.join(format!(
        "{WINDOWS_RIME_BUILD_REPAIR_MARKER}.{}.tmp",
        std::process::id()
    ));
    std::fs::write(&temporary, WINDOWS_RIME_BUILD_REPAIR_MARKER_CONTENT)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    if marker.exists() {
        std::fs::remove_file(&marker)
            .map_err(|error| format!("remove {}: {error}", marker.display()))?;
    }
    std::fs::rename(&temporary, &marker).map_err(|error| {
        format!(
            "rename {} to {}: {error}",
            temporary.display(),
            marker.display()
        )
    })
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
    const VULNERABLE: &str =
        "    local target_key = string.char(key_event.keycode) -- 当前按键对应字符\n\n    -- 若无目标键位，则交由其它函数进行判断\n    if not key then\n        return true\n    end";
    const PATCHED: &str =
        "    -- A wildcard match does not need a printable character conversion.\n    if not key then\n        return true\n    end\n\n    local keycode = key_event.keycode\n    if type(keycode) ~= \"number\" or keycode < 0x20 or keycode >= 0x7f then\n        return false\n    end\n    local target_key = string.char(keycode) -- 当前按键对应字符";

    content
        .contains("local function is_key(key, key_event)")
        .then(|| content.replace(VULNERABLE, PATCHED))
        .filter(|patched| patched != content)
}

/// Convert package-global Lua components to librime-lua's lazy module form.
///
/// Windows loads a TSF DLL into long-lived host processes such as Explorer.
/// Replacing `rime.lua` on disk does not update the Lua VM in those processes,
/// so a newly installed scheme must not depend on new global variables there.
pub fn patch_windows_lua_compatibility(user_data_dir: &Path) -> Result<Vec<String>, String> {
    let rime_lua_path = user_data_dir.join("rime.lua");
    let rime_lua = match std::fs::read_to_string(&rime_lua_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", rime_lua_path.display())),
    };
    let bindings = parse_rime_lua_require_bindings(&rime_lua);
    if bindings.is_empty() {
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
        let Some(patched) = rewrite_lua_component_autoloads(&content, &bindings) else {
            continue;
        };
        std::fs::write(&path, patched)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        changed.push(file_name.to_string());

        let compiled_path = user_data_dir.join("build").join(file_name);
        match std::fs::read_to_string(&compiled_path) {
            Ok(compiled) => {
                if let Some(patched) = rewrite_lua_component_autoloads(&compiled, &bindings) {
                    std::fs::write(&compiled_path, patched)
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
        if let Some(patched) = patch_keydo_helpers(&content) {
            std::fs::write(&helpers_path, patched)
                .map_err(|error| format!("write {}: {error}", helpers_path.display()))?;
            changed.push("lua/helpers.lua".to_string());
        }
    }

    changed.sort();
    Ok(changed)
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
    let mut seen_lines: HashSet<String> = package_content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("--"))
        .map(ToOwned::to_owned)
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
                if seen_lines.insert(new_line.trim().to_string()) {
                    extra_lines.push(new_line);
                }
            } else if seen_lines.insert(t.to_string()) {
                extra_lines.push(line.to_string());
            }
        } else if seen_lines.insert(t.to_string()) {
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

// ── Reload stamp ─────────────────────────────────────────────────────────────

/// Name of the reload signal the app writes into the user data directory.
pub const RELOAD_STAMP_FILE_NAME: &str = "keytao-ime.reload";

/// The single implementation of the reload signal: path, write format and
/// change detection. Frontends must not invent their own signature, otherwise
/// the same deployment reloads on some platforms and not on others.
///
/// The signature is `<len>:<mtime_nanos>:<content_hash>`, so a rewrite is
/// detected even when the timestamp resolution is coarse or the content is
/// stable.
pub struct ReloadStamp;

impl ReloadStamp {
    pub fn path(user_data_dir: &Path) -> PathBuf {
        user_data_dir.join(RELOAD_STAMP_FILE_NAME)
    }

    pub fn default_path() -> Option<PathBuf> {
        default_user_data_dir().map(|dir| Self::path(&dir))
    }

    /// Request a reload from every running frontend watching `user_data_dir`.
    pub fn write(user_data_dir: &Path) -> Result<PathBuf, String> {
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

    pub fn write_default() -> Result<PathBuf, String> {
        let dir = default_user_data_dir().ok_or("cannot determine keytao data directory")?;
        Self::write(&dir)
    }

    /// `None` when no stamp has been written yet.
    pub fn current_signature(user_data_dir: &Path) -> Option<String> {
        Self::signature_at(&Self::path(user_data_dir))
    }

    pub fn signature_at(path: &Path) -> Option<String> {
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
        Some(format!("{}:{modified}:{content_hash:016x}", metadata.len()))
    }

    /// Watcher seeded with the current signature: the first check only reports a
    /// change if the stamp was rewritten after this call.
    pub fn watcher(user_data_dir: &Path) -> ReloadStampWatcher {
        ReloadStampWatcher::new(user_data_dir)
    }
}

/// Remembers the last observed signature so frontends do not each keep their
/// own idea of what "changed" means.
pub struct ReloadStampWatcher {
    path: PathBuf,
    last_signature: Option<String>,
}

impl ReloadStampWatcher {
    pub fn new(user_data_dir: &Path) -> Self {
        let path = ReloadStamp::path(user_data_dir);
        let last_signature = ReloadStamp::signature_at(&path);
        Self {
            path,
            last_signature,
        }
    }

    /// Watcher that has seen nothing yet: an existing stamp counts as a change
    /// on the first check. Use it when the frontend must reload after a restart.
    pub fn unseen(user_data_dir: &Path) -> Self {
        Self {
            path: ReloadStamp::path(user_data_dir),
            last_signature: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn signature(&self) -> Option<&str> {
        self.last_signature.as_deref()
    }

    /// A missing stamp is not a reload request, so an uninstalled or not yet
    /// deployed app never triggers one.
    pub fn has_changed(&mut self) -> bool {
        let Some(current) = ReloadStamp::signature_at(&self.path) else {
            return false;
        };
        if self.last_signature.as_deref() == Some(current.as_str()) {
            return false;
        }
        self.last_signature = Some(current);
        true
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
        caret_extent_is_usable, clear_windows_rime_build_repair_marker,
        invalidate_active_windows_rime_build, invalidate_rime_build_artifacts,
        mark_windows_rime_build_repair_complete, merge_default_custom_content,
        merge_rime_lua_content, parse_rime_lua_requires, parse_schema_dependencies,
        parse_schema_list, parse_schema_name, patch_android_auxiliary_dictionary,
        patch_windows_lua_compatibility, preferred_schema_id_from_dir, rime_build_dirs,
        rime_log_dir, schema_install_state, windows_rime_build_repair_required, ReloadStamp,
        ReloadStampWatcher, RELOAD_STAMP_FILE_NAME,
    };
    use std::collections::HashSet;
    #[cfg(target_os = "windows")]
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

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

        let mut watcher = ReloadStampWatcher::new(&dir);
        assert!(!watcher.has_changed(), "a missing stamp is not a request");

        let stamp = ReloadStamp::write(&dir).unwrap();
        assert_eq!(stamp, ReloadStamp::path(&dir));
        assert!(watcher.has_changed(), "the first stamp is a request");
        assert!(!watcher.has_changed(), "an unchanged stamp fires once");

        // Same length and same mtime resolution: only the content hash differs.
        let signature_before = ReloadStamp::current_signature(&dir).unwrap();
        std::fs::write(&stamp, "0123456789\n").unwrap();
        let signature_after = ReloadStamp::current_signature(&dir).unwrap();
        assert_ne!(signature_before, signature_after);
        assert!(watcher.has_changed(), "a rewritten stamp is a request");

        std::fs::remove_file(&stamp).unwrap();
        assert!(!watcher.has_changed(), "a removed stamp is not a request");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reload_stamp_unseen_watcher_reports_existing_stamp() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keytao-reload-stamp-unseen-{suffix}"));
        ReloadStamp::write(&dir).unwrap();

        let mut watcher = ReloadStampWatcher::unseen(&dir);
        assert!(watcher.has_changed());
        assert!(!watcher.has_changed());
        assert_eq!(
            watcher.signature().map(str::to_owned),
            ReloadStamp::current_signature(&dir)
        );

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
        let ready = schema_install_state(&dir);
        assert!(ready.installed);
        assert!(ready.deployed);
        assert_eq!(ready.schemas, vec!["keydo"]);

        std::fs::remove_dir_all(dir).ok();
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
    fn merge_rime_lua_deduplicates_runtime_statements() {
        let local = concat!(
            "collectgarbage(\"setpause\", 50)\n",
            "collectgarbage(\"setpause\", 50)\n",
            "user_flag = true\n",
            "user_flag = true\n",
        );
        let package = "package_module = require(\"package_module\")\n";
        let (merged, _) = merge_rime_lua_content(Some(local), package, &HashSet::new());
        assert_eq!(merged.matches("collectgarbage").count(), 1);
        assert_eq!(merged.matches("user_flag = true").count(), 1);
    }

    #[test]
    fn windows_lua_compatibility_patches_source_and_compiled_schema() {
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
