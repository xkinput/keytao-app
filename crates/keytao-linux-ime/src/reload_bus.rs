//! Fan-out when the input engine reloads or changes enabled state.
//!
//! `ImeRuntimeSession` rebuilds its engine lazily, so a backend that is idle
//! when the App finishes a deployment keeps showing whatever the old
//! dictionaries produced — a client-side preedit that will never be updated
//! again and a candidate list from the previous build.  Every backend
//! subscribes to this bus and tears that UI down as soon as a reload lands.
//!
//! The signal is an `eventfd` so a backend can poll it next to its protocol
//! socket instead of waking up on a timer.

use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard, OnceLock,
    },
};
use tokio::sync::watch;

static SUBSCRIBERS: Mutex<Vec<RawFd>> = Mutex::new(Vec::new());
static ASYNC_SUBSCRIBERS: OnceLock<watch::Sender<()>> = OnceLock::new();
static ENGINE_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn engine_enabled() -> bool {
    ENGINE_ENABLED.load(Ordering::Acquire)
}

pub fn set_engine_enabled(enabled: bool) {
    ENGINE_ENABLED.store(enabled, Ordering::Release);
}

/// IBus already runs inside Tokio and can await reloads without one fd per context.
pub fn subscribe_async() -> watch::Receiver<()> {
    ASYNC_SUBSCRIBERS
        .get_or_init(|| watch::channel(()).0)
        .subscribe()
}

fn subscribers() -> MutexGuard<'static, Vec<RawFd>> {
    SUBSCRIBERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A pollable handle that becomes readable once librime has been reloaded.
pub struct ReloadSignal {
    fd: OwnedFd,
}

impl ReloadSignal {
    /// Poll this alongside the backend's protocol socket.
    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Consume every pending notification; `true` when at least one reload
    /// happened since the last call.
    pub fn take(&self) -> bool {
        let mut counter = 0u64;
        let read = unsafe {
            libc::read(
                self.fd.as_raw_fd(),
                std::ptr::addr_of_mut!(counter).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        read == std::mem::size_of::<u64>() as isize && counter > 0
    }
}

impl Drop for ReloadSignal {
    fn drop(&mut self) {
        // Deregister before the descriptor closes, otherwise a later `notify`
        // could write into whatever reused the number.
        let fd = self.fd.as_raw_fd();
        subscribers().retain(|&registered| registered != fd);
    }
}

/// Register for reload notifications.  `None` when the eventfd cannot be
/// created, in which case the backend simply keeps its lazy-refresh behaviour.
pub fn subscribe() -> Option<ReloadSignal> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        tracing::warn!(
            "reload bus: eventfd failed: {}",
            std::io::Error::last_os_error()
        );
        return None;
    }
    subscribers().push(fd);
    // SAFETY: `eventfd` just handed us an owned descriptor.
    Some(ReloadSignal {
        fd: unsafe { OwnedFd::from_raw_fd(fd) },
    })
}

/// Wake every subscriber.  Called from the reload watcher thread.
pub fn notify() {
    if let Some(sender) = ASYNC_SUBSCRIBERS.get() {
        sender.send_replace(());
    }
    let value: u64 = 1;
    for fd in subscribers().iter() {
        let written = unsafe {
            libc::write(
                *fd,
                std::ptr::addr_of!(value).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if written != std::mem::size_of::<u64>() as isize {
            tracing::warn!(
                "reload bus: failed to notify subscriber: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{notify, subscribe, subscribe_async};
    use std::time::Duration;

    #[tokio::test]
    async fn reload_wakes_polling_and_dbus_frontends() {
        let polling = subscribe().expect("eventfd");
        let mut dbus = subscribe_async();
        assert!(!polling.take());
        assert!(!dbus.has_changed().unwrap());

        notify();
        assert!(polling.take());
        assert!(!polling.take());
        tokio::time::timeout(Duration::from_secs(1), dbus.changed())
            .await
            .expect("reload notification")
            .expect("open reload bus");
        assert!(!dbus.has_changed().unwrap());
    }
}
