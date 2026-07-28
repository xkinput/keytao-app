//! Entry point: detect display server, deploy librime, launch correct backend.

#[cfg(target_os = "linux")]
mod engine;
#[cfg(target_os = "linux")]
mod gnome_ibus_engine;
#[cfg(target_os = "linux")]
mod ibus_backend;
#[cfg(target_os = "linux")]
mod ibus_shared;
#[cfg(target_os = "linux")]
mod kimpanel;
#[cfg(target_os = "linux")]
mod panel;
#[cfg(target_os = "linux")]
mod reload_bus;
#[cfg(target_os = "linux")]
mod tray;
#[cfg(target_os = "linux")]
mod wayland_backend;
#[cfg(target_os = "linux")]
mod wayland_backend_kde;
#[cfg(target_os = "linux")]
mod x11_backend;

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default)]
struct BackendSelection {
    wayland: bool,
    xim: bool,
    ibus: bool,
    /// Run as a standalone IBus engine that connects to an existing ibus-daemon
    /// (used on GNOME or when launched by ibus-daemon via a component XML).
    ibus_engine: bool,
}

#[cfg(target_os = "linux")]
impl BackendSelection {
    fn from_args(args: &[String]) -> Result<Self, String> {
        let mut selection = Self::default();
        let mut explicit = false;

        for arg in args {
            if arg == "--ibus-engine" {
                // Launched by ibus-daemon as a standalone engine process.
                // Override everything and run in IBus engine mode only.
                return Ok(Self {
                    ibus_engine: true,
                    ..Default::default()
                });
            }
            if let Some(value) = arg.strip_prefix("--backend=") {
                explicit = true;
                selection = Self::parse_list(value)?;
            } else if arg == "--wayland" {
                explicit = true;
                selection.wayland = true;
            } else if arg == "--xim" {
                explicit = true;
                selection.xim = true;
            } else if arg == "--ibus" {
                explicit = true;
                selection.ibus = true;
            }
        }

        if explicit {
            if !selection.any() {
                return Err("no backends selected".into());
            }
            Ok(selection)
        } else {
            Ok(Self::default())
        }
    }

    fn parse_list(value: &str) -> Result<Self, String> {
        let mut selection = Self::default();
        for raw in value.split(',') {
            let item = raw.trim();
            if item.is_empty() {
                continue;
            }
            match item {
                "wayland" => selection.wayland = true,
                "xim" | "x11" => selection.xim = true,
                "ibus" => selection.ibus = true,
                other => return Err(format!("unknown backend '{other}'")),
            }
        }
        if !selection.any() {
            return Err("no backends selected".into());
        }
        Ok(selection)
    }

    fn for_session(has_wayland: bool, has_x11: bool, is_gnome: bool, is_kde: bool) -> Self {
        if is_gnome {
            // GNOME does not support zwp_input_method_manager_v2.
            // Use the IBus engine backend which connects to GNOME's ibus-daemon,
            // plus XIM for any XWayland apps.
            return Self {
                ibus_engine: true,
                xim: has_x11,
                ..Default::default()
            };
        }
        if is_kde {
            // A normal KDE autostart daemon is not the KWin Virtual Keyboard owner,
            // so it must not try the native Wayland input-method path. KWin launches
            // the virtual-keyboard instance separately with WAYLAND_SOCKET.
            return Self {
                ibus: has_wayland || has_x11,
                xim: has_x11,
                ..Default::default()
            };
        }
        match (has_wayland, has_x11) {
            (true, true) => Self {
                wayland: true,
                xim: true,
                ibus: true,
                ..Default::default()
            },
            (true, false) => Self {
                wayland: true,
                ..Default::default()
            },
            (false, true) => Self {
                xim: true,
                ibus: true,
                ..Default::default()
            },
            (false, false) => Self::default(),
        }
    }

    fn any(self) -> bool {
        self.wayland || self.xim || self.ibus || self.ibus_engine
    }

    fn describe(self) -> String {
        let mut parts = Vec::new();
        if self.ibus_engine {
            parts.push("ibus-engine");
        }
        if self.wayland {
            parts.push("wayland");
        }
        if self.xim {
            parts.push("xim");
        }
        if self.ibus {
            parts.push("ibus");
        }
        parts.join(",")
    }
}

#[cfg(target_os = "linux")]
const LEGACY_LOG_DIR: &str = "/tmp";
#[cfg(target_os = "linux")]
const LOG_PREFIX: &str = "keytao-ime.log";
#[cfg(target_os = "linux")]
const LOG_RETENTION_DAYS: usize = 3;

#[cfg(target_os = "linux")]
fn remove_legacy_kde_env_file() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let env_file = std::path::Path::new(&home)
        .join(".config")
        .join("plasma-workspace")
        .join("env")
        .join("keytao.sh");
    if env_file.exists() {
        match std::fs::remove_file(&env_file) {
            Ok(()) => tracing::info!(
                "Removed legacy KDE IBus env file {} to avoid overriding KWin Virtual Keyboard routing",
                env_file.display()
            ),
            Err(e) => tracing::warn!("Cannot remove KDE env file {}: {e}", env_file.display()),
        }
    }
}

/// Peek/commit view of the reload signal keytao-core's `ReloadStamp` writes.
///
/// `ReloadStampWatcher::has_changed` marks the signal as seen the moment it is
/// asked, which is wrong for a watcher that still has to act on it: a reload
/// that fails right afterwards — the App was halfway through writing the
/// deployment out, say — would never be retried, and the daemon would keep
/// serving the previous dictionaries until the next deployment. The baseline
/// therefore only moves in `commit`, and it moves to exactly the signature that
/// was loaded, so a stamp written while librime was reloading is still picked up
/// on the next tick.
#[cfg(target_os = "linux")]
struct ReloadStampGate {
    path: std::path::PathBuf,
    loaded_signature: Option<String>,
}

#[cfg(target_os = "linux")]
impl ReloadStampGate {
    /// Seeded with what is on disk: a stamp older than this process describes
    /// the deployment the daemon just started up on, not a pending request.
    fn new(user_data_dir: &std::path::Path) -> Self {
        let path = keytao_core::ReloadStamp::path(user_data_dir);
        let loaded_signature = keytao_core::ReloadStamp::signature_at(&path);
        Self {
            path,
            loaded_signature,
        }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The signature a reload is pending for, without consuming the request.
    /// A missing stamp is not a request, so a not yet deployed install never
    /// triggers one.
    fn pending(&self) -> Option<String> {
        let current = keytao_core::ReloadStamp::signature_at(&self.path)?;
        (self.loaded_signature.as_deref() != Some(current.as_str())).then_some(current)
    }

    /// Record the signature a reload actually loaded.
    fn commit(&mut self, signature: String) {
        self.loaded_signature = Some(signature);
    }
}

#[cfg(target_os = "linux")]
fn install_reload_watcher(engine: engine::CoreEngine) {
    let Some(user_data_dir) = keytao_core::default_user_data_dir() else {
        tracing::warn!("cannot determine reload stamp path; dictionary reload watcher disabled");
        return;
    };

    // Seeded here rather than inside the thread: the caller has just finished
    // initialising librime, so this is the last moment at which the stamp on
    // disk still describes the deployment the daemon actually loaded. A stamp
    // written while the thread was waiting to be scheduled would otherwise be
    // adopted as already loaded and never reloaded.
    let mut gate = ReloadStampGate::new(&user_data_dir);

    if let Err(e) = std::thread::Builder::new()
        .name("reload-watcher".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let Some(requested) = gate.pending() else {
                    continue;
                };

                tracing::info!("reload stamp changed: {}", gate.path().display());
                match engine.reload_without_deploy() {
                    Ok(()) => {
                        gate.commit(requested);
                        tracing::info!("librime reloaded after deploy stamp change");
                        // Sessions rebuild lazily, so the backends have to be
                        // told to drop the preedit and candidates the previous
                        // build produced.
                        reload_bus::notify();
                    }
                    // The request stays pending: the next tick retries it.
                    Err(e) => tracing::error!("librime reload failed: {e}"),
                }
            }
        })
    {
        tracing::warn!("failed to spawn reload watcher: {e}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version") {
        println!("keytao-ime {}", env!("CARGO_PKG_VERSION"));
        println!("librime {}", env!("RIME_VERSION"));
        return;
    }

    #[cfg(target_os = "linux")]
    let requested_backends = match BackendSelection::from_args(&args) {
        Ok(selection) => selection,
        Err(err) => {
            eprintln!("keytao-ime: {err}");
            std::process::exit(2);
        }
    };

    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("keytao-ime: this binary only runs on Linux.");
        std::process::exit(1);
    }

    #[cfg(target_os = "linux")]
    {
        init_logging();

        let engine = engine::CoreEngine::new();
        if let Err(e) = engine.init_without_deploy() {
            tracing::error!("librime init failed: {e}");
            std::process::exit(1);
        }
        tracing::info!("librime ready");
        install_reload_watcher(engine.clone());

        // KWin launches the configured Virtual Keyboard with a private
        // WAYLAND_SOCKET. On KDE this socket advertises input-method-v1, not the
        // input-method-v2 manager used by wlroots-style compositors.
        let kwin_socket = std::env::var("WAYLAND_SOCKET")
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .is_some();

        if !kwin_socket {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async {
                if let Err(e) = enforce_single_instance().await {
                    tracing::warn!("Another keytao-ime daemon is already running: {e}");
                    std::process::exit(0);
                }
            });
        }

        let has_wayland = kwin_socket || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let has_x11 = std::env::var_os("DISPLAY").is_some();

        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();
        let is_gnome = desktop
            .split(':')
            .any(|s| matches!(s, "gnome" | "unity" | "budgie" | "pantheon" | "x-cinnamon"));
        // When launched via WAYLAND_SOCKET (KWin Virtual Keyboard), do not treat
        // the process as a normal KDE autostart daemon.
        let is_kde = !kwin_socket && desktop.split(':').any(|s| s == "kde");

        // A standalone KDE daemon plus plasma-workspace IBus env overrides
        // conflicts with KWin Virtual Keyboard mode. Clean up legacy files
        // instead of reasserting them.
        if is_kde && !kwin_socket && !requested_backends.any() {
            remove_legacy_kde_env_file();
        }

        let selected = if kwin_socket {
            // Launched by KWin as its registered Virtual Keyboard. This process
            // owns KDE's private input-method-v1 socket only; the normal session
            // daemon owns XIM/IBus fallback frontends.
            tracing::info!(
                "KWin Virtual Keyboard mode: using WAYLAND_SOCKET for KDE input-method-v1 only"
            );
            BackendSelection {
                wayland: true,
                ..Default::default()
            }
        } else if requested_backends.ibus_engine {
            requested_backends
        } else if requested_backends.any() {
            requested_backends
        } else {
            BackendSelection::for_session(has_wayland, has_x11, is_gnome, is_kde)
        };

        if !selected.any() {
            eprintln!("Neither WAYLAND_DISPLAY nor DISPLAY is set.");
            std::process::exit(1);
        }

        if selected.xim {
            let engine_xim = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("X11/XIM backend started for XWayland apps");
                x11_backend::run(engine_xim);
                tracing::warn!("X11/XIM backend exited");
            });
        }

        if selected.ibus {
            let engine_ibus = engine.clone();
            std::thread::spawn(move || {
                tracing::info!("IBus D-Bus backend started for Chromium/CEF apps");
                tokio::runtime::Runtime::new()
                    .expect("tokio runtime")
                    .block_on(ibus_backend::run(engine_ibus));
                tracing::warn!("IBus D-Bus backend exited");
            });
        }

        tracing::info!(
            "display server: wayland={} x11={} desktop={:?} — selected backends [{}]",
            has_wayland,
            has_x11,
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            selected.describe(),
        );

        if !kwin_socket {
            tray::spawn_tray();
        }

        if selected.ibus_engine {
            // Run as an IBus engine connecting to the existing ibus-daemon.
            // This is the correct path for GNOME Wayland.
            tracing::info!(
                "GNOME/IBus engine mode: connecting to existing ibus-daemon. \
                 Activate via GNOME Settings → Keyboard → Input Sources → Add (Other → Chinese → KeyTao), \
                 or via ibus-setup."
            );
            tokio::runtime::Runtime::new()
                .expect("tokio runtime")
                .block_on(gnome_ibus_engine::run(engine));
            return;
        }

        if selected.wayland {
            let result = if kwin_socket {
                wayland_backend_kde::run(engine)
            } else {
                wayland_backend::run(engine)
            };
            match result {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!("Wayland backend stopped: {e}");
                }
            }
        }
        // Keep the process alive so IBus/XIM threads can serve apps.
        loop {
            std::thread::park();
        }
    }
}

/// Directory holding the rolling daemon log.
///
/// Everything the daemon writes is derived from what the user types, so the log
/// belongs in the per-user state directory (XDG Base Directory) and not in a
/// shared `/tmp`, where any local account could read it or squat the file name.
#[cfg(target_os = "linux")]
fn log_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))?;
    let dir = base.join("keytao").join("log");
    match create_private_dir(&dir) {
        Ok(()) => Some(dir),
        Err(e) => {
            eprintln!("keytao-ime: cannot create {}: {e}", dir.display());
            None
        }
    }
}

/// Create `dir` (and its parents) so that only the owner can list it.
#[cfg(target_os = "linux")]
fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        }
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "linux")]
fn init_logging() {
    use tracing_subscriber::EnvFilter;

    remove_legacy_tmp_logs();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let appender = log_dir().and_then(|dir| {
        tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix(LOG_PREFIX)
            .latest_symlink(LOG_PREFIX)
            .max_log_files(LOG_RETENTION_DAYS)
            .build(&dir)
            .map_err(|e| eprintln!("keytao-ime: cannot open log in {}: {e}", dir.display()))
            .ok()
    });

    // A log the daemon cannot open must never keep the input method from
    // starting; stderr is picked up by the session journal.
    match appender {
        Some(appender) => tracing_subscriber::fmt()
            .with_writer(appender)
            .with_env_filter(filter)
            .init(),
        None => tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .init(),
    }
}

/// Drop the world-readable logs older versions left in `/tmp`.
#[cfg(target_os = "linux")]
fn remove_legacy_tmp_logs() {
    let Ok(entries) = std::fs::read_dir(LEGACY_LOG_DIR) else {
        return;
    };
    let rotated_prefix = format!("{LOG_PREFIX}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == LOG_PREFIX || name.starts_with(&rotated_prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(target_os = "linux")]
async fn enforce_single_instance() -> Result<(), Box<dyn std::error::Error>> {
    use zbus::fdo::DBusProxy;
    use zbus::Connection;
    let conn = Connection::session().await?;
    let dbus_proxy = DBusProxy::new(&conn).await?;
    let name = "org.xkinput.keytao.ime.Daemon";

    let reply = dbus_proxy
        .request_name(
            name.try_into()?,
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        )
        .await?;

    match reply {
        zbus::fdo::RequestNameReply::PrimaryOwner => {
            Box::leak(Box::new(conn));
            tracing::info!("Claimed D-Bus name {name} successfully.");
        }
        other => {
            let owner = dbus_proxy
                .get_name_owner(name.try_into()?)
                .await
                .map(|owner| owner.to_string())
                .unwrap_or_else(|_| "unknown owner".to_owned());
            return Err(format!("D-Bus name {name} is already owned by {owner}: {other:?}").into());
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::ReloadStampGate;
    use keytao_core::ReloadStamp;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("keytao-reload-gate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_failed_reload_keeps_the_request_pending() {
        let dir = temp_dir("retry");
        let mut gate = ReloadStampGate::new(&dir);
        assert_eq!(gate.pending(), None, "a missing stamp is not a request");

        ReloadStamp::write(&dir).expect("write stamp");
        let requested = gate.pending().expect("the first stamp is a request");
        // Peeking must not consume the request: a reload that failed because the
        // deployment was still being written has to be retried on the next tick.
        assert_eq!(gate.pending().as_deref(), Some(requested.as_str()));
        assert_eq!(gate.pending().as_deref(), Some(requested.as_str()));

        gate.commit(requested);
        assert_eq!(gate.pending(), None, "a loaded stamp fires once");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stamp_written_during_the_reload_is_not_swallowed() {
        let dir = temp_dir("overlap");
        ReloadStamp::write(&dir).expect("write stamp");
        // Startup adopts what is on disk: that is the deployment librime was
        // just initialised on.
        let mut gate = ReloadStampGate::new(&dir);
        assert_eq!(gate.pending(), None);

        ReloadStamp::write(&dir).expect("rewrite stamp");
        let requested = gate.pending().expect("a rewritten stamp is a request");

        // The App deploys again while librime is still reloading. Committing the
        // signature the reload started from must leave the newer one pending.
        std::thread::sleep(std::time::Duration::from_millis(5));
        ReloadStamp::write(&dir).expect("rewrite stamp during reload");
        gate.commit(requested);
        assert!(gate.pending().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
