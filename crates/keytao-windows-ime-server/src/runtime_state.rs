use std::{sync::Arc, time::Duration};

use keytao_ime_ipc::EngineError;
use parking_lot::{Mutex, MutexGuard};

pub(crate) const KEY_WAIT_BUDGET: Duration = Duration::from_millis(150);

pub(crate) struct Runtime<E> {
    pub(crate) engine: Option<Arc<E>>,
    pub(crate) generation: u64,
    pub(crate) ready: bool,
    pub(crate) deploying: bool,
    pub(crate) refreshing: bool,
}

impl<E> Default for Runtime<E> {
    fn default() -> Self {
        Self {
            engine: None,
            generation: 0,
            ready: false,
            deploying: false,
            refreshing: false,
        }
    }
}

impl<E> Runtime<E> {
    pub(crate) fn key_error(&self, session_generation: Option<u64>) -> Option<EngineError> {
        if self.refreshing {
            Some(EngineError::Busy)
        } else if !self.ready || session_generation != Some(self.generation) {
            Some(EngineError::NotReady)
        } else {
            None
        }
    }
}

pub(crate) fn reload_candidate<E>(
    runtime: &Mutex<Runtime<E>>,
    changed: impl FnOnce(&E) -> bool,
) -> Option<Arc<E>> {
    let (engine, ready) = {
        let runtime = runtime.lock();
        if runtime.deploying {
            return None;
        }
        (Arc::clone(runtime.engine.as_ref()?), runtime.ready)
    };
    (changed(engine.as_ref()) || !ready).then_some(engine)
}

pub(crate) fn lock_for_key<T>(runtime: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    runtime.try_lock_for(KEY_WAIT_BUDGET)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc::{self, RecvTimeoutError},
        },
        thread,
        time::Instant,
    };

    use super::*;

    #[test]
    fn slow_refresh_checks_do_not_starve_key_processing() {
        struct FakeEngine(AtomicUsize);

        let engine = Arc::new(FakeEngine(AtomicUsize::new(0)));
        let runtime = Arc::new(Mutex::new(Runtime {
            engine: Some(Arc::clone(&engine)),
            ready: true,
            ..Runtime::default()
        }));
        let (checking_tx, checking_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let refresh_runtime = Arc::clone(&runtime);
        let refresh = thread::spawn(move || {
            let candidate = reload_candidate(&refresh_runtime, |_| {
                checking_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                true
            });
            finished_tx.send(candidate).unwrap();
        });
        checking_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let started = Instant::now();
        let locked = lock_for_key(&runtime).expect("refresh checks must not hold the runtime lock");
        locked
            .engine
            .as_ref()
            .unwrap()
            .0
            .fetch_add(1, Ordering::SeqCst);
        assert!(started.elapsed() < KEY_WAIT_BUDGET);
        drop(locked);

        release_tx.send(()).unwrap();
        let candidate = finished_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        refresh.join().unwrap();
        assert!(Arc::ptr_eq(&candidate, &engine));
        assert_eq!(engine.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn key_lock_waits_for_contention_to_clear() {
        let runtime = Arc::new(Mutex::new(0));
        let held = runtime.lock();
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let key_runtime = Arc::clone(&runtime);
        let key = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let processed = lock_for_key(&key_runtime).map(|mut count| *count += 1);
            finished_tx.send(processed.is_some()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_millis(20)),
            Err(RecvTimeoutError::Timeout)
        );
        drop(held);
        assert!(finished_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        key.join().unwrap();
        assert_eq!(*runtime.lock(), 1);
    }

    #[test]
    fn key_lock_returns_none_after_its_wait_budget() {
        let runtime = Mutex::new(());
        let _held = runtime.lock();
        let started = Instant::now();
        assert!(lock_for_key(&runtime).is_none());
        assert!(started.elapsed() >= KEY_WAIT_BUDGET);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn deployment_skips_reload_checks() {
        let runtime = Mutex::new(Runtime {
            engine: Some(Arc::new(())),
            deploying: true,
            ..Runtime::default()
        });
        assert!(reload_candidate(&runtime, |_| panic!("deployment must skip checks")).is_none());
    }

    #[test]
    fn reload_requires_changed_inputs_or_an_unready_engine() {
        for (ready, changed, expected) in [
            (true, false, false),
            (true, true, true),
            (false, false, true),
            (false, true, true),
        ] {
            let runtime = Mutex::new(Runtime {
                engine: Some(Arc::new(())),
                ready,
                ..Runtime::default()
            });
            assert_eq!(reload_candidate(&runtime, |_| changed).is_some(), expected);
        }
        let runtime = Mutex::new(Runtime::<()>::default());
        assert_eq!(runtime.lock().generation, 0);
        assert!(reload_candidate(&runtime, |_| panic!("no engine to check")).is_none());
    }

    #[test]
    fn refreshing_is_busy_before_readiness_or_generation_checks() {
        for ready in [false, true] {
            let runtime = Runtime::<()> {
                generation: 7,
                ready,
                refreshing: true,
                ..Runtime::default()
            };
            for session_generation in [None, Some(6), Some(7)] {
                assert_eq!(
                    runtime.key_error(session_generation),
                    Some(EngineError::Busy)
                );
            }
        }
    }

    #[test]
    fn concurrent_keys_retry_refresh_and_preserve_schema_only_sessions() {
        struct FakeEngine(AtomicUsize);

        for (ready_after, generation_after, expected) in [
            (true, 7, None),
            (true, 8, Some(EngineError::NotReady)),
            (false, 7, Some(EngineError::NotReady)),
        ] {
            let engine = Arc::new(FakeEngine(AtomicUsize::new(0)));
            let runtime = Arc::new(Mutex::new(Runtime {
                engine: Some(Arc::clone(&engine)),
                generation: 7,
                ready: true,
                ..Runtime::default()
            }));
            let (refreshing_tx, refreshing_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let (finished_tx, finished_rx) = mpsc::channel();
            let refresh_runtime = Arc::clone(&runtime);
            let refresh = thread::spawn(move || {
                {
                    let mut runtime = refresh_runtime.lock();
                    runtime.refreshing = true;
                    runtime.ready = false;
                }
                refreshing_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                {
                    let mut runtime = refresh_runtime.lock();
                    runtime.refreshing = false;
                    runtime.ready = ready_after;
                    runtime.generation = generation_after;
                }
                finished_tx.send(()).unwrap();
            });
            refreshing_rx.recv_timeout(Duration::from_secs(1)).unwrap();

            let started = Instant::now();
            let locked =
                lock_for_key(&runtime).expect("reload work must not hold the runtime lock");
            assert_eq!(locked.key_error(Some(7)), Some(EngineError::Busy));
            assert!(started.elapsed() < KEY_WAIT_BUDGET);
            drop(locked);

            release_tx.send(()).unwrap();
            finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            refresh.join().unwrap();
            let locked = lock_for_key(&runtime).unwrap();
            assert_eq!(locked.key_error(Some(7)), expected);
            if expected.is_none() {
                locked
                    .engine
                    .as_ref()
                    .unwrap()
                    .0
                    .fetch_add(1, Ordering::SeqCst);
            }
            assert_eq!(
                engine.0.load(Ordering::SeqCst),
                usize::from(expected.is_none())
            );
        }
    }
}
