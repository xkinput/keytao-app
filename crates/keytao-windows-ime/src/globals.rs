//! Global DLL state: HMODULE handle + reference counters.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
pub static DLL_INSTANCE: OnceLock<isize> = OnceLock::new();

/// Counts active COM object instances (AddRef-tracked by ClassFactory).
static DLL_OBJ_COUNT: AtomicI32 = AtomicI32::new(0);

/// Counts LockServer(TRUE) calls.
static DLL_LOCK_COUNT: AtomicI32 = AtomicI32::new(0);

pub fn obj_add() {
    DLL_OBJ_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub fn obj_release() {
    DLL_OBJ_COUNT.fetch_sub(1, Ordering::SeqCst);
}

pub fn lock_server(lock: bool) {
    if lock {
        DLL_LOCK_COUNT.fetch_add(1, Ordering::SeqCst);
    } else {
        DLL_LOCK_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Returns true when the DLL can be safely unloaded.
pub fn can_unload() -> bool {
    DLL_OBJ_COUNT.load(Ordering::SeqCst) <= 0 && DLL_LOCK_COUNT.load(Ordering::SeqCst) <= 0
}
