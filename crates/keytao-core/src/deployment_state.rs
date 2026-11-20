use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HINT_DURATION: Duration = Duration::from_millis(1500);
const HINT_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
static LOADED_BUILD: Mutex<Option<(PathBuf, Option<String>, u64)>> = Mutex::new(None);
static USABLE_SESSIONS: Mutex<Vec<(PathBuf, Option<u64>, Weak<()>)>> = Mutex::new(Vec::new());
static DEPLOYING: LazyLock<Mutex<HashMap<PathBuf, bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn cached_deployment_in_progress(root: &Path) -> bool {
    crate::lock_ignore_poison(&DEPLOYING)
        .get(root)
        .copied()
        .unwrap_or(false)
}

fn cache_deployment(root: &Path, active: bool) {
    crate::lock_ignore_poison(&DEPLOYING).insert(root.to_path_buf(), active);
}

fn engine_rebuilding() -> bool {
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    {
        crate::PROCESS_RIME
            .prewarm_scheduled
            .load(std::sync::atomic::Ordering::Acquire)
            || crate::try_lock_ignore_poison(&crate::PROCESS_RIME.build.busy)
                .is_none_or(|busy| *busy)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    )))]
    false
}

fn engine_generation() -> u64 {
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    ))]
    {
        crate::PROCESS_RIME
            .generation
            .load(std::sync::atomic::Ordering::Acquire)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android",
        target_os = "ios"
    )))]
    0
}

pub(crate) fn has_usable_input_session(root: &Path) -> bool {
    crate::lock_ignore_poison(&USABLE_SESSIONS)
        .iter()
        .any(|(loaded_root, generation, session)| {
            loaded_root == root
                && generation.is_none_or(|generation| generation == engine_generation())
                && session.strong_count() > 0
        })
}

/// Keep this token alive only while a validated local or remote input session is usable.
pub fn track_usable_input_session(root: &Path) -> Arc<()> {
    track_input_session(root, None)
}

fn track_input_session(root: &Path, generation: Option<u64>) -> Arc<()> {
    let session = Arc::new(());
    let mut sessions = crate::lock_ignore_poison(&USABLE_SESSIONS);
    sessions.retain(|(_, _, session)| session.strong_count() > 0);
    sessions.push((
        root.to_path_buf(),
        generation,
        Arc::downgrade(&session),
    ));
    session
}

pub(crate) fn engine_loaded(root: &Path) -> Arc<()> {
    let session = track_input_session(root, Some(engine_generation()));
    let marker = sentinel(root).is_none().then(|| marker(root)).flatten();
    *crate::lock_ignore_poison(&LOADED_BUILD) =
        Some((root.to_path_buf(), marker, engine_generation()));
    session
}

/// Both native deployers (nanoseconds) and older Android deployers (milliseconds)
/// publish a timestamp and the process that owns the build directory.
pub fn deployment_in_progress(root: &Path) -> bool {
    let active = sentinel_in_progress(sentinel(root).as_deref());
    cache_deployment(root, active);
    active
}

fn sentinel_in_progress(content: Option<&str>) -> bool {
    let Some(content) = content else {
        return false;
    };
    let Some((started, pid)) = content.trim().split_once('/') else {
        return false;
    };
    let (Ok(started), Ok(pid)) = (started.parse::<u128>(), pid.parse::<u32>()) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let started = if started < 100_000_000_000_000 {
        started.saturating_mul(1_000_000)
    } else {
        started
    };
    now.as_nanos()
        .checked_sub(started)
        .is_some_and(|age| age <= Duration::from_secs(600).as_nanos())
        && process_alive(pid)
}

fn sentinel(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join("build/.keytao-deploying")).ok()
}

/// Keep the owner sentinel intact: a failed/timed-out deploy worker may still
/// be writing artifacts, so failure must not permit runtime teardown.
pub fn mark_deployment_failed(root: &Path) -> Result<(), String> {
    let Some(token) = sentinel(root) else {
        return Ok(());
    };
    crate::write_file_atomic(&root.join("build/.keytao-deploy-failed"), token.as_bytes())
        .map_err(|error| format!("publish deployment failure: {error}"))
}

pub(crate) fn failed_deployment_owned_by_process(root: &Path) -> bool {
    // Android's coordinator can time out while its deploy worker still writes.
    if cfg!(target_os = "android") {
        return false;
    }
    let Some(token) = sentinel(root) else {
        return false;
    };
    token
        .trim()
        .split_once('/')
        .and_then(|(_, pid)| pid.parse::<u32>().ok())
        == Some(std::process::id())
        && std::fs::read_to_string(root.join("build/.keytao-deploy-failed")).ok() == Some(token)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    pid > 0
        && pid <= i32::MAX as u32
        && (unsafe { kill(pid as i32, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(1))
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    if pid == 0 {
        return false;
    }
    let handle = unsafe { OpenProcess(0x1000, 0, pid) };
    if handle.is_null() {
        return std::io::Error::last_os_error().raw_os_error() == Some(5);
    }
    let mut code = 0;
    let running = unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == 259;
    unsafe { CloseHandle(handle) };
    running
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> bool {
    false
}

/// Per-IME presentation state; success means this IME loaded the new build.
#[derive(Clone, Default)]
pub struct DeploymentState {
    hint_requested: bool,
    observed: bool,
    loaded_marker: Option<String>,
    pending: Option<String>,
    failed_sentinel: Option<String>,
    transient: Option<(&'static str, Instant)>,
    refreshed_at: Option<Instant>,
    hint: Option<&'static str>,
}

impl DeploymentState {
    pub fn visible_hint(hint: Option<&str>, has_candidates: bool) -> Option<&str> {
        hint.filter(|hint| *hint == "正在部署中" || !has_candidates)
    }

    /// Call before a key can wait for deployment or the following engine rebuild.
    pub fn key_pressed(&mut self, root: &Path) {
        self.key_pressed_with_session(root, has_usable_input_session(root));
    }

    pub(crate) fn key_pressed_with_session(&mut self, root: &Path, usable: bool) {
        if usable {
            self.hint = self.hint.filter(|hint| *hint == "部署成功");
            return;
        }
        self.hint_at_with_session(root, Instant::now(), engine_rebuilding(), usable);
        if self.pending.is_some() && self.pending != self.failed_sentinel {
            self.hint_requested = true;
            self.hint = Some("正在部署中");
        } else if self.failed_sentinel.is_some() {
            self.transient = Some(("部署失败", Instant::now() + HINT_DURATION));
            self.hint = Some("部署失败");
        }
    }

    /// Cached decision shared by native key gates; never reads deployment files.
    pub fn blocks_input(&self, root: &Path) -> bool {
        self.blocks_input_with_session(has_usable_input_session(root))
    }

    pub(crate) fn blocks_input_with_session(&self, usable: bool) -> bool {
        !usable && self.pending.is_some()
    }

    pub fn reset_input(&mut self) {
        self.hint_requested = false;
        self.transient = None;
        self.hint = None;
        self.refreshed_at = None;
    }

    pub fn hint(&mut self, root: &Path) -> Option<&'static str> {
        if has_usable_input_session(root) {
            if self.hint_requested {
                self.cached_hint_at(root, Instant::now());
            }
            self.hint = self.hint.filter(|hint| *hint == "部署成功");
            return self.cached_hint();
        }
        self.cached_hint_at(root, Instant::now())
    }

    pub(crate) fn cached_hint_at(&mut self, root: &Path, now: Instant) -> Option<&'static str> {
        if self
            .refreshed_at
            .is_none_or(|checked| now.saturating_duration_since(checked) >= HINT_REFRESH_INTERVAL)
        {
            self.hint_at(root, now);
        }
        self.hint.filter(|hint| {
            self.transient
                .is_none_or(|(transient, until)| *hint != transient || now < until)
        })
    }

    pub fn cached_hint(&self) -> Option<&'static str> {
        self.hint.filter(|hint| {
            self.transient
                .is_none_or(|(transient, until)| *hint != transient || Instant::now() < until)
        })
    }

    pub(crate) fn hint_at(&mut self, root: &Path, now: Instant) -> Option<&'static str> {
        self.hint_at_with_build(root, now, engine_rebuilding())
    }

    pub(crate) fn hint_at_with_build(
        &mut self,
        root: &Path,
        now: Instant,
        rebuilding: bool,
    ) -> Option<&'static str> {
        self.hint_at_with_session(root, now, rebuilding, has_usable_input_session(root))
    }

    pub(crate) fn hint_at_with_session(
        &mut self,
        root: &Path,
        now: Instant,
        rebuilding: bool,
        usable: bool,
    ) -> Option<&'static str> {
        self.hint = self.refresh_hint_at(root, now, rebuilding).filter(|hint| {
            (*hint != "正在部署中" || self.hint_requested) && (*hint == "部署成功" || !usable)
        });
        self.refreshed_at = Some(now);
        self.hint
    }

    fn refresh_hint_at(
        &mut self,
        root: &Path,
        now: Instant,
        rebuilding: bool,
    ) -> Option<&'static str> {
        if !self.observed {
            self.loaded_marker = marker(root);
            self.observed = true;
        }
        let current = sentinel(root);
        let active = sentinel_in_progress(current.as_deref());
        cache_deployment(root, active);
        let previously_loaded = current.is_none()
            && crate::lock_ignore_poison(&LOADED_BUILD)
                .as_ref()
                .is_some_and(|(loaded_root, _, _)| loaded_root == root);
        if current.is_none() {
            let loaded = crate::lock_ignore_poison(&LOADED_BUILD)
                .as_ref()
                .is_some_and(|(loaded_root, loaded_marker, generation)| {
                    loaded_root == root
                        && loaded_marker
                            .as_ref()
                            .is_some_and(|loaded| marker(root).as_ref() == Some(loaded))
                        && *generation == engine_generation()
                });
            if loaded && !rebuilding {
                self.loaded(root);
            }
            // A loaded runtime handles reloads through its existing engine-build wait.
            if previously_loaded {
                self.pending = None;
            }
        }
        if current.is_some()
            && current != self.failed_sentinel
            && std::fs::read_to_string(root.join("build/.keytao-deploy-failed")).ok() == current
        {
            self.pending = current.clone();
            self.failed();
        }
        if current.is_some() && current != self.failed_sentinel {
            if active {
                if self.pending != current {
                    self.hint_requested = false;
                }
                self.pending = current;
                self.transient = None;
                return Some("正在部署中");
            }
            if self.pending != current {
                self.pending = current.clone();
            }
            self.failed();
            self.failed_sentinel = current;
        } else if current.is_none() {
            let marker = marker(root);
            if !previously_loaded
                && self.pending.is_none()
                && marker.is_some()
                && marker != self.loaded_marker
            {
                self.pending = marker.clone();
                self.hint_requested = false;
                self.transient = None;
            }
            if self.pending.is_none() {
                return self
                    .transient
                    .filter(|(_, until)| now < *until)
                    .map(|(hint, _)| hint);
            }
            if marker.is_none() {
                self.failed();
            } else {
                return Some("正在部署中");
            }
        }
        self.transient
            .filter(|(_, until)| now < *until)
            .map(|(hint, _)| hint)
    }

    pub fn loaded(&mut self, root: &Path) {
        if deployment_in_progress(root) {
            return;
        }
        let marker = marker(root);
        if marker.is_none() && self.pending.is_some() {
            self.failed();
            return;
        }
        let changed = self.pending.is_some()
            || (self.observed && marker.is_some() && marker != self.loaded_marker);
        self.observed = true;
        self.loaded_marker = marker;
        self.pending = None;
        self.failed_sentinel = None;
        if changed && self.hint_requested {
            self.transient = Some(("部署成功", Instant::now() + HINT_DURATION));
            self.hint = Some("部署成功");
        }
        self.hint_requested = false;
        self.refreshed_at = None;
    }

    pub fn failed(&mut self) {
        self.failed_sentinel = self.pending.take();
        if self.hint_requested {
            self.transient = Some(("部署失败", Instant::now() + HINT_DURATION));
            self.hint = Some("部署失败");
        }
        self.hint_requested = false;
        self.refreshed_at = None;
    }
}

fn marker(root: &Path) -> Option<String> {
    crate::ReloadStamp::signature_at(&root.join("build/.keytao-deployed"))
}
