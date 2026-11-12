use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    time::{Duration, Instant},
};

pub struct Frontend {
    name: &'static str,
    events: Sender<(&'static str, bool)>,
}

impl Frontend {
    pub fn new(name: &'static str, events: &Sender<(&'static str, bool)>) -> Self {
        Self {
            name,
            events: events.clone(),
        }
    }

    pub fn connected(&self) {
        let _ = self.events.send((self.name, true));
    }
}

impl Drop for Frontend {
    fn drop(&mut self) {
        let _ = self.events.send((self.name, false));
    }
}

struct ReadyFile {
    path: PathBuf,
    pid: u32,
    active: BTreeSet<&'static str>,
}

impl ReadyFile {
    fn publish(&self) -> std::io::Result<()> {
        if self.active.is_empty() {
            return self.remove();
        }
        let pending = self.path.with_extension(format!("ready.{}", self.pid));
        let record = format!(
            "{} {}\n",
            self.pid,
            self.active.iter().copied().collect::<Vec<_>>().join(",")
        );
        fs::write(&pending, record)?;
        let result = fs::rename(&pending, &self.path);
        if result.is_err() {
            let _ = fs::remove_file(pending);
        }
        result
    }

    fn remove(&self) -> std::io::Result<()> {
        match fs::remove_file(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

impl Drop for ReadyFile {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

pub fn wait_for_frontends(
    path: PathBuf,
    events: Receiver<(&'static str, bool)>,
    mut remaining: usize,
    stopping: impl Fn() -> bool,
) -> Result<(), String> {
    let mut ready = ReadyFile {
        path,
        pid: std::process::id(),
        active: BTreeSet::new(),
    };
    ready
        .remove()
        .map_err(|e| format!("cannot clear readiness file: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if stopping() {
            return Ok(());
        }
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                let mut last_stopped = None;
                for (name, connected) in std::iter::once(event).chain(events.try_iter()) {
                    if connected {
                        ready.active.insert(name);
                    } else {
                        ready.active.remove(name);
                        remaining -= 1;
                        last_stopped = Some(name);
                    }
                }
                ready
                    .publish()
                    .map_err(|e| format!("cannot publish readiness: {e}"))?;
                if ready.active.is_empty() && (remaining == 0 || Instant::now() >= deadline) {
                    return Err(format!(
                        "no frontend backend is active (last stopped: {})",
                        last_stopped.unwrap_or("none")
                    ));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err("no frontend backend is active (all backend threads exited)".into());
            }
            Err(RecvTimeoutError::Timeout) => {
                if ready.active.is_empty() && Instant::now() >= deadline {
                    return Err("no frontend backend connected within 3 seconds".into());
                }
            }
        }
    }
}

pub fn channel() -> (Sender<(&'static str, bool)>, Receiver<(&'static str, bool)>) {
    mpsc::channel()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("keytao-frontend-{}-{label}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("keytao-ime.ready")
    }

    #[test]
    fn an_unavailable_backend_cannot_leave_the_daemon_running() {
        let path = path("unavailable");
        fs::write(&path, "123 wayland\n").unwrap();
        let (tx, rx) = channel();
        drop(Frontend::new("wayland", &tx));
        drop(tx);
        let result = wait_for_frontends(path.clone(), rx, 1, || false);
        assert!(result.unwrap_err().contains("last stopped: wayland"));
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_connected_backend_is_published_and_removed_on_exit() {
        let path = path("connected");
        let ready = ReadyFile {
            path: path.clone(),
            pid: 42,
            active: BTreeSet::from(["wayland", "xim"]),
        };
        ready.publish().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "42 wayland,xim\n");
        drop(ready);
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_failed_backend_does_not_stop_an_active_fallback() {
        let path = path("fallback");
        let (tx, rx) = channel();
        let xim = Frontend::new("xim", &tx);
        xim.connected();
        drop(Frontend::new("wayland", &tx));
        drop(xim);
        drop(tx);
        let result = wait_for_frontends(path.clone(), rx, 2, || false);
        assert!(result.unwrap_err().contains("last stopped: xim"));
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_failed_backend_allows_a_pending_fallback_to_connect() {
        let path = path("pending");
        let (tx, rx) = channel();
        let wayland = Frontend::new("wayland", &tx);
        let ibus = Frontend::new("ibus", &tx);
        wayland.connected();
        drop(wayland);
        ibus.connected();
        drop(ibus);
        drop(tx);
        let result = wait_for_frontends(path.clone(), rx, 2, || false);
        assert!(result.unwrap_err().contains("last stopped: ibus"));
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn shutdown_removes_published_readiness() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let path = path("shutdown");
        let (tx, rx) = channel();
        let frontend = Frontend::new("wayland", &tx);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let ready_path = path.clone();
        let monitor = std::thread::spawn(move || {
            wait_for_frontends(ready_path, rx, 1, || stop.load(Ordering::Relaxed))
        });
        frontend.connected();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let record = fs::read_to_string(&path);
        stopping.store(true, Ordering::Relaxed);
        assert!(monitor.join().unwrap().is_ok());
        assert_eq!(record.unwrap(), format!("{} wayland\n", std::process::id()));
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_disconnected_channel_cannot_leave_the_daemon_running() {
        let path = path("disconnected");
        let (tx, rx) = channel();
        drop(tx);
        assert!(wait_for_frontends(path.clone(), rx, 0, || false).is_err());
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
