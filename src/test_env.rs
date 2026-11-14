//! Environment-variable access for tests.
//!
//! `std::env::set_var` / `remove_var` are `unsafe` since the 2024 edition:
//! they race with any other thread reading the environment. Tests run in
//! parallel, so every test that changes a variable goes through one global
//! lock here, and the guard removes what it set when it is dropped (also when
//! the test panics).

use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    touched: Vec<OsString>,
}

/// Takes the process-wide environment lock. Hold it while reading variables
/// another test may change, too.
pub(crate) fn lock() -> EnvGuard {
    EnvGuard {
        _lock: LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
        touched: Vec::new(),
    }
}

impl EnvGuard {
    pub(crate) fn set(&mut self, key: &str, value: impl AsRef<OsStr>) {
        self.touched.push(key.into());
        // SAFETY: all environment mutation in tests is serialized by `LOCK`,
        // which this guard holds.
        unsafe { std::env::set_var(key, value) };
    }

    pub(crate) fn remove(&mut self, key: &str) {
        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(key) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in self.touched.drain(..) {
            // SAFETY: as in `set`; the lock is still held here.
            unsafe { std::env::remove_var(key) };
        }
    }
}
