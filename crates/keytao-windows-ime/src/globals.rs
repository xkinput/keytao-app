//! Global DLL state: HMODULE handle + reference counters.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
pub static DLL_INSTANCE: OnceLock<isize> = OnceLock::new();

/// Counts live COM objects and worker activities executing code from this DLL.
static DLL_OBJ_COUNT: AtomicI32 = AtomicI32::new(0);

/// Counts LockServer(TRUE) calls.
static DLL_LOCK_COUNT: AtomicI32 = AtomicI32::new(0);

pub fn obj_add() {
    DLL_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub fn obj_release() {
    let previous = DLL_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
    debug_assert!(previous > 0, "KeyTao DLL object count underflow");
}

/// Keeps the DLL loaded for the lifetime of a COM object or background task.
pub struct DllActivityGuard;

impl DllActivityGuard {
    pub fn new() -> Self {
        obj_add();
        Self
    }
}

impl Drop for DllActivityGuard {
    fn drop(&mut self) {
        obj_release();
    }
}

pub fn lock_server(lock: bool) {
    if lock {
        DLL_LOCK_COUNT.fetch_add(1, Ordering::SeqCst);
    } else {
        let previous = DLL_LOCK_COUNT.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0, "KeyTao DLL lock count underflow");
    }
}

/// Returns true when the DLL can be safely unloaded.
pub fn can_unload() -> bool {
    DLL_OBJ_COUNT.load(Ordering::SeqCst) == 0 && DLL_LOCK_COUNT.load(Ordering::SeqCst) == 0
}

#[cfg(test)]
mod tests {
    use super::{can_unload, DllActivityGuard};

    #[test]
    fn activity_guard_prevents_unload() {
        assert!(can_unload());
        let guard = DllActivityGuard::new();
        assert!(!can_unload());
        drop(guard);
        assert!(can_unload());
    }
}
