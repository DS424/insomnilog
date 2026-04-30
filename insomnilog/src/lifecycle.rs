//! Process-wide backend lifecycle: [`start`], [`shutdown`], and their guards.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backend;

/// Error returned by [`start`] when the backend has already been initialised
/// in this process.
///
/// `start` is conceptually one-shot per process: once called, subsequent calls
/// — including those after [`shutdown`] — return this error rather than
/// silently spawning a fresh backend with possibly different options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyStarted;

impl core::fmt::Display for AlreadyStarted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("insomnilog backend has already been started")
    }
}

impl std::error::Error for AlreadyStarted {}

/// RAII guard returned from [`start`]. Tears the backend down on drop.
///
/// Bind it for the lifetime you want logging to be alive — typically
/// `let _guard = insomnilog::start(opts)?;` near the top of `main`. The
/// `#[must_use]` attribute makes the compiler warn on `let _ = start(...)`,
/// which would drop the guard immediately.
#[must_use = "ShutdownGuard drops the backend immediately if not bound; \
              bind it with `let _guard = ...` for the desired lifetime"]
pub struct ShutdownGuard {
    /// Private field so callers cannot construct this type directly.
    _private: (),
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        shutdown();
    }
}

/// Latch flipped by [`start`] to detect repeat initialisation.
///
/// Kept separate from [`BACKEND`] because `OnceLock::set` consumes its
/// argument: relying on the lock alone would force us to spawn a worker
/// thread before learning that initialisation is illegal. The latch keeps
/// the spawn out of the failing branch.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Process-wide backend, initialised exactly once by [`start`]. Subsequent
/// reads via [`shutdown`] go through `OnceLock::get`.
static BACKEND: OnceLock<backend::Backend> = OnceLock::new();

/// Initialises the process-wide backend.
///
/// On success returns a [`ShutdownGuard`] whose `Drop` tears the backend
/// down (drain → join in later migration steps; just join for now). Bind
/// the guard in `main` for the desired lifetime of the logging system.
///
/// # Errors
///
/// Returns [`AlreadyStarted`] if `start` has already been called in this
/// process. There is no automatic restart.
///
/// # Panics
///
/// Panics if the backend worker thread cannot be spawned (the OS refused
/// to create a new thread).
pub fn start(options: backend::BackendOptions) -> Result<ShutdownGuard, AlreadyStarted> {
    if STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(AlreadyStarted);
    }
    let backend = backend::Backend::start(options);
    if BACKEND.set(backend).is_err() {
        unreachable!("BACKEND set after STARTED CAS won — no other thread can have set it");
    }
    Ok(ShutdownGuard { _private: () })
}

/// Drains and tears the backend down.
///
/// Idempotent: safe to call multiple times, before or after a
/// [`ShutdownGuard`] drops, and even if [`start`] has not been called (in
/// which case it is a no-op).
pub fn shutdown() {
    if let Some(backend) = BACKEND.get() {
        backend.shutdown();
    }
}
