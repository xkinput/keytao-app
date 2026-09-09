//! Bounded, opt-out runtime diagnostics. Never pass input text or key values.
use serde_json::{Map, Value};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// Re-export for macros invoked by crates without a direct serde_json import.
#[doc(hidden)]
pub use serde_json as json;

const CAPACITY: usize = 2048;
const ROTATE_BYTES: u64 = 2 * 1024 * 1024;
// Reserve space for the fixed session header in a freshly rotated file.
const MAX_RECORD_BYTES: usize = ROTATE_BYTES as usize - 4096;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const SETTINGS_INTERVAL: Duration = Duration::from_secs(5);
static LEVEL: AtomicU8 = AtomicU8::new(0);
static LOGGER: OnceLock<Arc<Logger>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Level {
    Off = 0,
    Info = 1,
    Verbose = 2,
}

#[inline]
pub fn enabled(level: Level) -> bool {
    let current = LEVEL.load(Ordering::Relaxed);
    level != Level::Off && current >= level as u8
}

/// Start at most one writer. All filesystem work happens on that thread.
/// An initialization or write failure is terminal for this process.
pub fn init(user_data_dir: &Path, tag: &str) {
    LOGGER.get_or_init(|| {
        let logger = Arc::new(Logger::new(&LEVEL));
        let _ = logger.start(user_data_dir.to_owned(), tag.to_owned(), log_dir);
        logger
    });
}

/// Core entry points also cover frontends whose lifecycle hooks land later.
/// Resolve the executable/process name on the writer, never on the key path.
#[doc(hidden)]
pub fn init_for_engine(user_data_dir: &Path) {
    init(user_data_dir, "engine");
}

pub fn log_event(level: Level, cat: &str, ev: &str, dur_ms: Option<f64>, kv: Map<String, Value>) {
    if !enabled(level) {
        return;
    }
    if let Some(logger) = LOGGER.get() {
        logger.event(level, cat, ev, dur_ms, kv);
    }
}

/// Wait for the queue watermark observed on entry, without fsync.
pub fn flush_blocking(timeout: Duration) {
    if let Some(logger) = LOGGER.get() {
        logger.flush(timeout);
    }
}

#[macro_export]
macro_rules! rt_log {
    ($lvl:expr, $cat:literal, $ev:literal, dur_ms = $dur:expr $(, $k:ident = $v:expr)* $(,)?) => {{
        let level = $lvl;
        if $crate::runtime_log::enabled(level) {
            #[allow(unused_mut)]
            let mut kv = $crate::runtime_log::json::Map::new();
            $(kv.insert(stringify!($k).trim_start_matches("r#").into(), $crate::runtime_log::json::json!($v));)*
            $crate::runtime_log::log_event(level, $cat, $ev, Some($dur), kv);
        }
    }};
    ($lvl:expr, $cat:literal, $ev:literal $(, $k:ident = $v:expr)* $(,)?) => {{
        let level = $lvl;
        if $crate::runtime_log::enabled(level) {
            #[allow(unused_mut)]
            let mut kv = $crate::runtime_log::json::Map::new();
            $(kv.insert(stringify!($k).trim_start_matches("r#").into(), $crate::runtime_log::json::json!($v));)*
            $crate::runtime_log::log_event(level, $cat, $ev, None, kv);
        }
    }};
}

#[derive(Default)]
struct Queue {
    lines: VecDeque<String>,
    dropped: u64,
    submitted: u64,
    completed: u64,
    flush: bool,
    failed: bool,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn numeric_thread_id() -> u64 {
    extern "C" {
        fn gettid() -> std::os::raw::c_int;
    }
    // SAFETY: gettid takes no arguments and always returns the caller's ID.
    unsafe { gettid() as u64 }
}

#[cfg(target_vendor = "apple")]
fn numeric_thread_id() -> u64 {
    extern "C" {
        fn pthread_threadid_np(thread: *mut std::ffi::c_void, id: *mut u64) -> std::os::raw::c_int;
    }
    let mut id = 0;
    // SAFETY: a null thread selects the caller; id points to a writable u64.
    if unsafe { pthread_threadid_np(std::ptr::null_mut(), &mut id) } == 0 {
        id
    } else {
        rust_thread_id()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn numeric_thread_id() -> u64 {
    rust_thread_id()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn rust_thread_id() -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    // ThreadId::as_u64 is unstable; hashing preserves a numeric, per-thread ID.
    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

impl Queue {
    fn push(&mut self, line: String) {
        self.submitted += 1;
        if line.len() > MAX_RECORD_BYTES {
            self.dropped += 1;
            return;
        }
        if self.lines.len() == CAPACITY {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }
}

struct Logger {
    level: &'static AtomicU8,
    start: Instant,
    queue: Mutex<Queue>,
    wake: Condvar,
    home: Option<String>,
}

impl Logger {
    fn new(level: &'static AtomicU8) -> Self {
        Self {
            level,
            start: Instant::now(),
            queue: Mutex::new(Queue {
                lines: VecDeque::with_capacity(CAPACITY),
                ..Queue::default()
            }),
            wake: Condvar::new(),
            home: dirs::home_dir().map(|path| path.to_string_lossy().into_owned()),
        }
    }

    fn start(
        self: &Arc<Self>,
        user_dir: PathBuf,
        tag: String,
        directory: fn(&Path) -> Option<PathBuf>,
    ) -> io::Result<std::thread::JoinHandle<()>> {
        let worker = Arc::clone(self);
        self.level.store(Level::Info as u8, Ordering::Relaxed);
        let result = std::thread::Builder::new()
            .name("keytao-log".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.run(&user_dir, &tag, directory)
                }));
                if !matches!(result, Ok(Ok(()))) {
                    worker.disable();
                }
            });
        if result.is_err() {
            self.disable();
        }
        result
    }

    fn event(
        &self,
        level: Level,
        cat: &str,
        ev: &str,
        dur_ms: Option<f64>,
        kv: Map<String, Value>,
    ) {
        if level == Level::Off || self.level.load(Ordering::Relaxed) < level as u8 {
            return;
        }
        if !matches!(
            cat,
            "lifecycle" | "input" | "rime" | "render" | "memory" | "ui" | "error"
        ) || !identifier(ev)
        {
            return;
        }
        let line = self.line(level, cat, ev, dur_ms, kv);
        let mut queue = super::lock_ignore_poison(&self.queue);
        if !queue.failed && self.level.load(Ordering::Relaxed) >= level as u8 {
            queue.push(line);
            if queue.lines.len() > 256 {
                self.wake.notify_one();
            }
        }
    }

    fn line(
        &self,
        level: Level,
        cat: &str,
        ev: &str,
        dur_ms: Option<f64>,
        mut kv: Map<String, Value>,
    ) -> String {
        let truncated = sanitize_map(&mut kv, self.home.as_deref());
        kv.insert("ts".into(), utc_timestamp(SystemTime::now()).into());
        kv.insert("mono".into(), self.start.elapsed().as_secs_f64().into());
        kv.insert(
            "lvl".into(),
            if level == Level::Verbose {
                "verbose"
            } else {
                "info"
            }
            .into(),
        );
        kv.insert("cat".into(), cat.into());
        kv.insert("ev".into(), ev.into());
        kv.insert("pid".into(), std::process::id().into());
        // Thread names are application-defined, unlike OS/user host names.
        let thread = std::thread::current();
        let mut tid = thread
            .name()
            .map(Value::from)
            .unwrap_or_else(|| numeric_thread_id().into());
        let tid_truncated = sanitize_value(&mut tid, self.home.as_deref());
        kv.insert("tid".into(), tid);
        if let Some(ms) = dur_ms.filter(|ms| ms.is_finite() && *ms >= 0.0) {
            kv.insert("dur_ms".into(), ((ms * 100.0).round() / 100.0).into());
        }
        if truncated || tid_truncated {
            kv.insert("_trunc".into(), true.into());
        }
        let mut line = Value::Object(kv).to_string();
        line.push('\n');
        line
    }

    fn disable(&self) {
        self.level.store(Level::Off as u8, Ordering::Relaxed);
        let mut queue = super::lock_ignore_poison(&self.queue);
        queue.failed = true;
        queue.lines.clear();
        self.wake.notify_all();
    }

    fn flush(&self, timeout: Duration) {
        let start = Instant::now();
        let mut queue = super::lock_ignore_poison(&self.queue);
        let target = queue.submitted;
        queue.flush = true;
        self.wake.notify_all();
        while queue.completed < target && !queue.failed {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            let (next, result) = self
                .wake
                .wait_timeout(queue, remaining)
                .unwrap_or_else(|e| e.into_inner());
            queue = next;
            if result.timed_out() {
                break;
            }
        }
    }

    fn run(
        &self,
        user_dir: &Path,
        tag: &str,
        directory: fn(&Path) -> Option<PathBuf>,
    ) -> io::Result<()> {
        let tag = if tag == "engine" { process_tag() } else { tag };
        if !matches!(
            tag,
            "android-ime"
                | "android-app"
                | "android-deploy"
                | "ios-ime"
                | "macos-ime"
                | "windows-ime"
                | "linux-ime"
                | "desktop-app"
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid logger tag",
            ));
        }
        let mut settings = Settings::new(user_dir.join("runtime-log.json"));
        let initial_level = settings.read();
        {
            let mut queue = super::lock_ignore_poison(&self.queue);
            self.level.store(initial_level as u8, Ordering::Relaxed);
            // Startup is asynchronous. A persisted Off switch must discard any
            // events buffered before the initial configuration was available.
            if initial_level == Level::Off {
                queue.lines.clear();
                queue.dropped = 0;
                queue.completed = queue.submitted;
                self.wake.notify_all();
            }
        }
        let dir = directory(user_dir)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no log directory"))?;
        let header = self.header(tag);
        let mut writer = LogFile::new(dir.join(format!("keytao-{tag}.log")), header)?;
        let mut checked = Instant::now();
        loop {
            let (lines, dropped, target) = {
                let mut queue = super::lock_ignore_poison(&self.queue);
                if queue.lines.len() <= 256 && !queue.flush && !queue.failed {
                    queue = self
                        .wake
                        .wait_timeout(queue, FLUSH_INTERVAL)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
                if queue.failed {
                    return Ok(());
                }
                queue.flush = false;
                (
                    queue.lines.drain(..).collect::<Vec<_>>(),
                    std::mem::take(&mut queue.dropped),
                    queue.submitted,
                )
            };
            if checked.elapsed() >= SETTINGS_INTERVAL {
                if let Some(level) = settings.refresh() {
                    self.level.store(level as u8, Ordering::Relaxed);
                }
                checked = Instant::now();
            }
            // Settings changes govern new producers, not previously accepted
            // lines. In particular a flush watermark must not discard a batch.
            let dropped_line = if dropped > 0 {
                let mut kv = Map::new();
                kv.insert("n".into(), dropped.into());
                Some(self.line(Level::Info, "error", "log_dropped", None, kv))
            } else {
                None
            };
            writer.write_batch(dropped_line.into_iter().chain(lines))?;
            let mut queue = super::lock_ignore_poison(&self.queue);
            queue.completed = target;
            self.wake.notify_all();
        }
    }

    fn header(&self, tag: &str) -> String {
        let mut kv = Map::new();
        let version = include_str!("../../../Cargo.toml")
            .split("[workspace.package]")
            .nth(1)
            .and_then(|section| {
                section
                    .lines()
                    .find_map(|line| line.strip_prefix("version = \""))
            })
            .and_then(|line| line.split('"').next())
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        kv.insert("app_ver".into(), version.into());
        kv.insert("os".into(), std::env::consts::OS.into());
        kv.insert("abi".into(), std::env::consts::ARCH.into());
        // Unknown is explicit; never infer a model from a user-supplied device name.
        kv.insert("device".into(), Value::Null);
        kv.insert("mem_class_mb".into(), Value::Null);
        kv.insert("locale".into(), Value::Null);
        kv.insert("librime".into(), super::librime_runtime_version().into());
        kv.insert("tag".into(), tag.into());
        self.line(Level::Info, "lifecycle", "session_begin", None, kv)
    }
}

fn process_tag() -> &'static str {
    #[cfg(target_os = "android")]
    {
        let command = fs::read("/proc/self/cmdline").unwrap_or_default();
        let name = command.split(|byte| *byte == 0).next().unwrap_or_default();
        return if name.ends_with(b":rime_deployer") {
            "android-deploy"
        } else if name.ends_with(b":ime") || name.is_empty() {
            "android-ime"
        } else {
            "android-app"
        };
    }
    #[cfg(not(target_os = "android"))]
    {
        if std::env::current_exe()
            .ok()
            .and_then(|path| path.file_stem().map(|s| s == "keytao-app"))
            .unwrap_or(false)
        {
            return "desktop-app";
        }
        match std::env::consts::OS {
            "ios" => "ios-ime",
            "macos" => "macos-ime",
            "windows" => "windows-ime",
            "linux" => "linux-ime",
            _ => "desktop-app",
        }
    }
}

fn log_dir(user_dir: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let _ = user_dir;
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
            .map(|base| base.join("keytao/log"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some(user_dir.join("log"))
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            }
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

struct LogFile {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    header: String,
}

impl LogFile {
    fn new(path: PathBuf, header: String) -> io::Result<Self> {
        create_private_dir(
            path.parent()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))?,
        )?;
        let mut writer = Self {
            path,
            file: None,
            size: 0,
            header,
        };
        writer.open()?;
        Ok(writer)
    }

    fn open(&mut self) -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(0o600);
            self.file = Some(options.open(&self.path)?);
            self.file
                .as_ref()
                .unwrap()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        {
            self.file = Some(options.open(&self.path)?);
        }
        self.size = self.file.as_ref().unwrap().metadata()?.len();
        Ok(())
    }

    fn rotated(&self, index: u8) -> PathBuf {
        let mut path = self.path.as_os_str().to_owned();
        path.push(format!(".{index}"));
        path.into()
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.take();
        for index in [3, 2] {
            match fs::remove_file(self.rotated(index)) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
        match fs::rename(self.rotated(1), self.rotated(2)) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        fs::rename(&self.path, self.rotated(1))?;
        self.open()
    }

    fn write_batch(&mut self, lines: impl Iterator<Item = String>) -> io::Result<()> {
        let mut lines = lines.peekable();
        if lines.peek().is_some() && !self.path.try_exists()? {
            self.file.take();
            self.open()?;
        }
        let mut batch = String::new();
        for line in lines {
            if line.len() + self.header.len() > ROTATE_BYTES as usize {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "log record exceeds file limit",
                ));
            }
            if self.size + batch.len() as u64 + line.len() as u64 > ROTATE_BYTES
                && self.size + batch.len() as u64 > 0
            {
                self.write(&batch)?;
                batch.clear();
                self.rotate()?;
            }
            if self.size == 0 && batch.is_empty() {
                batch.push_str(&self.header);
            }
            batch.push_str(&line);
        }
        self.write(&batch)
    }

    fn write(&mut self, batch: &str) -> io::Result<()> {
        if !batch.is_empty() {
            self.file
                .as_mut()
                .ok_or_else(|| io::Error::other("log file closed"))?
                .write_all(batch.as_bytes())?;
            self.size += batch.len() as u64;
        }
        Ok(())
    }
}

struct Settings {
    path: PathBuf,
    stamp: Option<(SystemTime, u64)>,
}

impl Settings {
    fn new(path: PathBuf) -> Self {
        Self { path, stamp: None }
    }
    fn stamp(&self) -> Option<(SystemTime, u64)> {
        fs::metadata(&self.path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())))
    }
    fn read(&mut self) -> Level {
        self.read_with_windows_diagnostics(
            cfg!(target_os = "windows")
                && std::env::var("KEYTAO_WINDOWS_IME_DIAGNOSTICS").as_deref() == Ok("1"),
        )
    }
    fn read_with_windows_diagnostics(&mut self, diagnostics: bool) -> Level {
        self.stamp = self.stamp();
        let level = match fs::read(&self.path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Level::Info,
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) if value.get("enabled").and_then(Value::as_bool) == Some(true) => {
                    match value.get("level").and_then(Value::as_str) {
                        Some("info") => Level::Info,
                        Some("verbose") => Level::Verbose,
                        _ => Level::Off,
                    }
                }
                _ => Level::Off,
            },
            Err(_) => Level::Off,
        };
        if diagnostics && level == Level::Info {
            Level::Verbose
        } else {
            level
        }
    }
    fn refresh(&mut self) -> Option<Level> {
        (self.stamp() != self.stamp).then(|| self.read())
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn sanitize_map(map: &mut Map<String, Value>, home: Option<&str>) -> bool {
    let mut truncated = false;
    map.retain(|key, value| {
        if !identifier(key)
            || matches!(
                key.as_str(),
                "ts" | "mono" | "lvl" | "cat" | "ev" | "pid" | "tid" | "dur_ms" | "_trunc"
            )
        {
            return false;
        }
        // Also protect the bridge against accidentally passing a whole IME state.
        let normalized = key.replace('_', "").to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "preedit"
                | "candidate"
                | "candidates"
                | "candidatetext"
                | "committed"
                | "commit"
                | "committext"
                | "clipboard"
                | "clipboardtext"
                | "key"
                | "keycode"
                | "keysym"
                | "keyvalue"
                | "editorinfo"
                | "text"
        ) {
            return false;
        }
        truncated |= sanitize_value(value, home);
        true
    });
    truncated
}

fn sanitize_value(value: &mut Value, home: Option<&str>) -> bool {
    match value {
        Value::String(text) => {
            if let Some(home) = home.filter(|home| !home.is_empty()) {
                if text.contains(home) {
                    *text = text.replace(home, "<user>");
                }
            }
            if let Some((end, _)) = text.char_indices().nth(128) {
                text.truncate(end);
                true
            } else {
                false
            }
        }
        Value::Array(values) => values.iter_mut().fold(false, |truncated, value| {
            sanitize_value(value, home) || truncated
        }),
        Value::Object(map) => sanitize_map(map, home),
        _ => false,
    }
}

fn utc_timestamp(time: SystemTime) -> String {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seconds = millis / 1000;
    let days = (seconds / 86400) as i64;
    // Gregorian civil date from days since the Unix epoch, using 400-year eras.
    let shifted = days + 719468;
    let era = shifted / 146097;
    let day_of_era = shifted - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    let year = era * 400 + year_of_era + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        millis % 1000
    )
}

/// Approximate duration percentiles in eleven logarithmic buckets.
pub struct DurationHistogram {
    data: Mutex<HistogramData>,
}

struct HistogramData {
    buckets: [u32; 11],
    max: f64,
    count: u32,
}

const BUCKETS: [f64; 11] = [
    0.25,
    0.5,
    1.0,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
    64.0,
    128.0,
    f64::INFINITY,
];

impl Default for DurationHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl DurationHistogram {
    pub const fn new() -> Self {
        Self {
            data: Mutex::new(HistogramData {
                buckets: [0; 11],
                max: 0.0,
                count: 0,
            }),
        }
    }

    pub fn record(&self, ms: f64) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        let mut data = super::lock_ignore_poison(&self.data);
        let bucket = BUCKETS.iter().position(|bound| ms <= *bound).unwrap_or(10);
        data.buckets[bucket] = data.buckets[bucket].saturating_add(1);
        data.count = data.count.saturating_add(1);
        data.max = data.max.max(ms);
    }

    fn take(&self) -> Option<Map<String, Value>> {
        let mut data = super::lock_ignore_poison(&self.data);
        if data.count == 0 {
            return None;
        }
        let percentile = |percent: u64| {
            let rank = (u64::from(data.count) * percent).div_ceil(100);
            let mut cumulative = 0u64;
            for (index, count) in data.buckets.iter().enumerate() {
                cumulative += u64::from(*count);
                if cumulative >= rank {
                    return BUCKETS[index].min(data.max);
                }
            }
            data.max
        };
        let mut kv = Map::new();
        kv.insert("n".into(), data.count.into());
        kv.insert("p50".into(), percentile(50).into());
        kv.insert("p95".into(), percentile(95).into());
        kv.insert("max".into(), data.max.into());
        *data = HistogramData {
            buckets: [0; 11],
            max: 0.0,
            count: 0,
        };
        Some(kv)
    }

    pub fn drain_into(&self, ev: &str) {
        if !enabled(Level::Info) {
            return;
        }
        if let Some(kv) = self.take() {
            log_event(
                Level::Info,
                if ev == "draw" { "render" } else { "rime" },
                ev,
                None,
                kv,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::atomic::AtomicU64,
    };

    struct CountingAllocator;
    thread_local! {
        static COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }
    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = COUNT_ALLOCATIONS.try_with(|count| {
                if count.get() {
                    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
                }
            });
            System.alloc(layout)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout);
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let _ = COUNT_ALLOCATIONS.try_with(|count| {
                if count.get() {
                    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
                }
            });
            System.realloc(ptr, layout, size)
        }
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let path = std::env::temp_dir().join(format!(
                    "keytao-runtime-log-{}-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("temp directory: {e}"),
                }
            }
        }
        fn file(&self) -> PathBuf {
            self.0.join("log/keytao-test.log")
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn local_logger() -> Logger {
        Logger::new(Box::leak(Box::new(AtomicU8::new(Level::Info as u8))))
    }

    #[test]
    fn thread_identity_preserves_named_threads() {
        let tid = std::thread::Builder::new()
            .name("keytao-named-test".into())
            .spawn(|| {
                let line = local_logger().line(Level::Info, "rime", "test", None, Map::new());
                serde_json::from_str::<Value>(&line).unwrap()["tid"].clone()
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(tid, "keytao-named-test");
    }

    #[test]
    fn thread_identity_distinguishes_unnamed_threads() {
        let barrier = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let record = || {
                assert!(std::thread::current().name().is_none());
                let logger = local_logger();
                let read_tid = || {
                    let line = logger.line(Level::Info, "rime", "test", None, Map::new());
                    serde_json::from_str::<Value>(&line).unwrap()["tid"].clone()
                };
                let first = read_tid();
                let second = read_tid();
                // Keep both threads alive so the OS cannot reuse their IDs.
                barrier.wait();
                assert_eq!(first, second);
                first.as_u64().expect("unnamed thread ID must be numeric")
            };
            let first = scope.spawn(record);
            let second = scope.spawn(record);
            barrier.wait();
            let first = first.join().unwrap();
            let second = second.join().unwrap();
            assert_ne!(first, 0);
            assert_ne!(second, 0);
            assert_ne!(first, second);
        });
    }

    #[test]
    fn disabled_macro_allocates_nothing_and_short_circuits() {
        // Logger tests use isolated levels; the process logger stays Off in
        // this unit-test binary (Rime lifecycle tests are integration binaries).
        assert_eq!(LEVEL.load(Ordering::Relaxed), Level::Off as u8);
        let evaluated = Cell::new(false);
        ALLOCATIONS.with(|n| n.set(0));
        COUNT_ALLOCATIONS.with(|count| count.set(true));
        for _ in 0..100 {
            assert!(!enabled(Level::Off));
            assert!(!enabled(Level::Info));
            assert!(!enabled(Level::Verbose));
            crate::rt_log!(
                Level::Info,
                "rime",
                "disabled",
                msg = {
                    evaluated.set(true);
                    "unused".repeat(200)
                }
            );
            crate::rt_log!(
                Level::Verbose,
                "rime",
                "disabled",
                dur_ms = {
                    evaluated.set(true);
                    1.0
                },
                msg = "unused".repeat(200)
            );
            crate::rt_log!(
                Level::Off,
                "rime",
                "disabled",
                msg = {
                    evaluated.set(true);
                    "unused".repeat(200)
                }
            );
            crate::rt_log!(
                Level::Off,
                "rime",
                "disabled",
                dur_ms = {
                    evaluated.set(true);
                    1.0
                },
                msg = "unused".repeat(200)
            );
        }
        COUNT_ALLOCATIONS.with(|count| count.set(false));
        assert!(!evaluated.get());
        assert_eq!(ALLOCATIONS.with(Cell::get), 0);
        let logger = local_logger();
        logger.level.store(0, Ordering::Relaxed);
        logger.event(Level::Info, "rime", "disabled", None, Map::new());
        assert!(super::super::lock_ignore_poison(&logger.queue)
            .lines
            .is_empty());
    }

    #[test]
    fn overflow_drops_oldest_and_reports_exact_count_before_batch() {
        let dir = TempDir::new();
        let logger = local_logger();
        let mut queue = Queue::default();
        for index in 0..CAPACITY + 7 {
            queue.push(format!("{{\"index\":{index}}}\n"));
        }
        assert_eq!(queue.lines.len(), CAPACITY);
        assert_eq!(queue.dropped, 7);
        assert_eq!(queue.lines.front().unwrap(), "{\"index\":7}\n");
        assert_eq!(
            queue.lines.back().unwrap(),
            &format!("{{\"index\":{}}}\n", CAPACITY + 6)
        );
        let mut kv = Map::new();
        kv.insert("n".into(), queue.dropped.into());
        let dropped = logger.line(Level::Info, "error", "log_dropped", None, kv);
        let mut writer = LogFile::new(dir.file(), "{\"ev\":\"session_begin\"}\n".into()).unwrap();
        writer
            .write_batch(std::iter::once(dropped).chain(queue.lines))
            .unwrap();
        let contents = fs::read_to_string(dir.file()).unwrap();
        let record: Value = serde_json::from_str(contents.lines().nth(1).unwrap()).unwrap();
        assert_eq!(record["ev"], "log_dropped");
        assert_eq!(record["n"], 7);
        assert_eq!(contents.lines().nth(2).unwrap(), "{\"index\":7}");
    }

    #[test]
    fn oversized_record_is_dropped_without_disabling_the_queue() {
        let mut queue = Queue::default();
        queue.push("x".repeat(MAX_RECORD_BYTES + 1));
        queue.push("{\"ev\":\"next\"}\n".into());
        assert_eq!(queue.submitted, 2);
        assert_eq!(queue.dropped, 1);
        assert_eq!(queue.lines.len(), 1);
        assert!(!queue.failed);
        assert_eq!(queue.lines.front().unwrap(), "{\"ev\":\"next\"}\n");
    }

    #[test]
    fn rotation_keeps_three_files_and_removes_log_3() {
        let dir = TempDir::new();
        let header = "{\"ev\":\"session_begin\"}\n";
        let mut writer = LogFile::new(dir.file(), header.into()).unwrap();
        fs::write(writer.rotated(3), "stale").unwrap();
        for index in 0..4 {
            let prefix = format!("{{\"file\":{index},\"padding\":\"");
            let line = format!(
                "{}{}\"}}\n",
                prefix,
                "x".repeat(ROTATE_BYTES as usize - header.len() - prefix.len() - 4)
            );
            writer.write_batch(std::iter::once(line)).unwrap();
            assert!(fs::metadata(dir.file()).unwrap().len() <= ROTATE_BYTES);
        }
        for (path, expected) in [
            (dir.file(), 3),
            (writer.rotated(1), 2),
            (writer.rotated(2), 1),
        ] {
            let contents = fs::read_to_string(path).unwrap();
            assert_eq!(contents.lines().count(), 2);
            assert!(contents.starts_with(header));
            let record: Value = serde_json::from_str(contents.lines().nth(1).unwrap()).unwrap();
            assert_eq!(record["file"], expected);
        }
        assert!(!writer.rotated(3).exists());
    }

    #[test]
    fn cleared_log_is_recreated_only_for_a_nonempty_batch() {
        let dir = TempDir::new();
        let header = "{\"ev\":\"session_begin\"}\n";
        let mut writer = LogFile::new(dir.file(), header.into()).unwrap();
        writer
            .write_batch(std::iter::once("{\"ev\":\"before_clear\"}\n".into()))
            .unwrap();
        fs::remove_file(dir.file()).unwrap();
        writer.write_batch(std::iter::empty()).unwrap();
        assert!(!dir.file().exists());
        let next = "{\"ev\":\"after_clear\"}\n";
        writer.write_batch(std::iter::once(next.into())).unwrap();
        assert_eq!(
            fs::read_to_string(dir.file()).unwrap(),
            format!("{header}{next}")
        );
        assert_eq!(writer.size, (header.len() + next.len()) as u64);
    }

    #[test]
    fn unwritable_dir_turns_off_without_panic_or_retry() {
        let dir = TempDir::new();
        let blocked = dir.0.join("blocked");
        fs::write(&blocked, "not a directory").unwrap();
        let logger = Arc::new(local_logger());
        // Exercise the same asynchronous start/error boundary used by init.
        let handle = logger
            .start(blocked, "ios-ime".into(), |root| Some(root.join("log")))
            .unwrap();
        assert!(handle.join().is_ok());
        logger.event(Level::Info, "error", "ignored", None, Map::new());
        logger.flush(Duration::from_millis(10));
        assert_eq!(logger.level.load(Ordering::Relaxed), 0);
        let queue = super::super::lock_ignore_poison(&logger.queue);
        assert!(queue.failed);
        assert!(queue.lines.is_empty());
    }

    #[test]
    fn writer_flushes_accepted_batch_even_after_level_turns_off() {
        let dir = TempDir::new();
        let logger = Arc::new(local_logger());
        let handle = logger
            .start(dir.0.clone(), "ios-ime".into(), |root| {
                Some(root.join("log"))
            })
            .unwrap();
        let path = dir.0.join("log/keytao-ios-ime.log");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        logger.event(Level::Info, "rime", "accepted", Some(1.0), Map::new());
        logger.level.store(0, Ordering::Relaxed);
        logger.flush(Duration::from_secs(2));
        logger.disable();
        handle.join().unwrap();
        let content = fs::read_to_string(path).unwrap();
        let lines: Vec<Value> = content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["ev"], "session_begin");
        assert_eq!(lines[0]["tag"], "ios-ime");
        assert_eq!(lines[1]["ev"], "accepted");
    }

    #[test]
    fn persisted_off_discards_events_buffered_before_initial_read() {
        let dir = TempDir::new();
        fs::write(
            dir.0.join("runtime-log.json"),
            r#"{"enabled":false,"level":"info"}"#,
        )
        .unwrap();
        let logger = Arc::new(local_logger());
        logger.event(Level::Info, "rime", "startup", None, Map::new());
        let handle = logger
            .start(dir.0.clone(), "ios-ime".into(), |root| {
                Some(root.join("log"))
            })
            .unwrap();
        logger.flush(Duration::from_secs(2));
        let level = logger.level.load(Ordering::Relaxed);
        logger.disable();
        handle.join().unwrap();
        assert_eq!(level, 0);
        let content = fs::read_to_string(dir.0.join("log/keytao-ios-ime.log")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn long_values_are_unicode_truncated_and_paths_redacted_recursively() {
        let mut kv = serde_json::json!({
            "msg": "字".repeat(129), "nested": {"values": ["🙂".repeat(200)]},
            "path": "/Users/example/data", "preedit": "private", "keysym": 99,
            "cat": "override", "dur_ms": 999
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(sanitize_map(&mut kv, Some("/Users/example")));
        assert_eq!(kv["msg"].as_str().unwrap().chars().count(), 128);
        assert_eq!(
            kv["nested"]["values"][0].as_str().unwrap().chars().count(),
            128
        );
        assert_eq!(kv["path"], "<user>/data");
        for key in ["preedit", "keysym", "cat", "dur_ms"] {
            assert!(!kv.contains_key(key));
        }
        let logger = local_logger();
        kv.insert("long".into(), "x".repeat(129).into());
        let line: Value =
            serde_json::from_str(&logger.line(Level::Info, "rime", "truncation", Some(1.236), kv))
                .unwrap();
        assert_eq!(line["_trunc"], true);
        assert_eq!(line["dur_ms"], 1.24);
        assert_eq!(line["cat"], "rime");
    }

    #[test]
    fn settings_default_disable_reload_and_delete() {
        let dir = TempDir::new();
        let path = dir.0.join("runtime-log.json");
        let mut settings = Settings::new(path.clone());
        assert_eq!(settings.read(), Level::Info);
        fs::write(&path, r#"{"enabled":false,"level":"info"}"#).unwrap();
        assert_eq!(settings.refresh(), Some(Level::Off));
        assert_eq!(settings.refresh(), None);
        fs::write(&path, r#"{"enabled":true,"level":"verbose"}"#).unwrap();
        assert_eq!(settings.refresh(), Some(Level::Verbose));
        fs::remove_file(path).unwrap();
        assert_eq!(settings.refresh(), Some(Level::Info));
    }

    #[test]
    fn windows_diagnostics_raises_info_without_overriding_disabled_settings() {
        let dir = TempDir::new();
        let path = dir.0.join("runtime-log.json");
        let mut settings = Settings::new(path.clone());
        assert_eq!(settings.read_with_windows_diagnostics(false), Level::Info);
        assert_eq!(settings.read_with_windows_diagnostics(true), Level::Verbose);
        for (contents, expected) in [
            (r#"{"enabled":true,"level":"info"}"#, Level::Verbose),
            (r#"{"enabled":true,"level":"verbose"}"#, Level::Verbose),
            (r#"{"enabled":false,"level":"info"}"#, Level::Off),
            (r#"{"enabled":false,"level":"verbose"}"#, Level::Off),
            (r#"{"enabled":true,"level":"invalid"}"#, Level::Off),
            ("invalid json", Level::Off),
        ] {
            fs::write(&path, contents).unwrap();
            assert_eq!(settings.read_with_windows_diagnostics(true), expected);
        }
    }

    #[test]
    fn histogram_drains_counts_percentiles_and_max() {
        let histogram = DurationHistogram::new();
        for _ in 0..95 {
            histogram.record(1.0);
        }
        for _ in 0..5 {
            histogram.record(80.0);
        }
        histogram.record(f64::NAN);
        let data = histogram.take().unwrap();
        assert_eq!(data["n"], 100);
        assert_eq!(data["p50"], 1.0);
        assert_eq!(data["p95"], 1.0);
        assert_eq!(data["max"], 80.0);
        assert!(histogram.take().is_none());
    }

    #[test]
    fn utc_format_handles_epoch_leap_day_and_milliseconds() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + Duration::from_millis(1709251199123)),
            "2024-02-29T23:59:59.123Z"
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_and_file_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let writer = LogFile::new(dir.file(), String::new()).unwrap();
        assert_eq!(
            fs::metadata(dir.file().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(writer.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
