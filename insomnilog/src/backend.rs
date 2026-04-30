//! Backend worker thread and its configuration.
//!
//! [`Backend`] is a process-wide singleton created by [`crate::start`] and
//! stored in a `OnceLock`. It owns a dedicated thread that idles until
//! [`Backend::shutdown`] is called.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Configuration for the backend worker thread.
#[derive(Debug, Clone)]
pub struct BackendOptions {
    /// Name of the worker thread. Visible in `thread::current().name()` and
    /// in panic-handler output.
    pub thread_name: String,
    /// Per-thread queue size in bytes. Power-of-2 enforced by `queue::new`.
    pub queue_capacity: usize,
    /// When the worker finds no work, it `yield_now()`s for this many rounds
    /// before falling through to [`Self::idle_sleep`]. Two-stage backoff
    /// trades a little CPU for lower wakeup latency under steady load.
    pub idle_yield_rounds: u32,
    /// Sleep duration once [`Self::idle_yield_rounds`] is exhausted.
    pub idle_sleep: Duration,
    /// If `true`, [`crate::shutdown`] drains every queue before joining the
    /// worker.
    pub wait_for_queues_to_empty_before_exit: bool,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            thread_name: "insomnilog-backend".into(),
            queue_capacity: 128 * 1024,
            idle_yield_rounds: 32,
            idle_sleep: Duration::from_micros(100),
            wait_for_queues_to_empty_before_exit: true,
        }
    }
}

/// Process-wide backend. Created once by [`crate::start`] and stored in a
/// crate-level `OnceLock`.
pub struct Backend {
    /// Configuration this backend was started with.
    #[expect(
        dead_code,
        reason = "fields read by the worker loop once it is wired up"
    )]
    options: BackendOptions,
    /// Set to `true` by [`Self::shutdown`]. The worker thread observes this
    /// with `Acquire` and exits when it transitions to `true`.
    shutdown_flag: Arc<AtomicBool>,
    /// Worker thread join handle. `take`n by [`Self::shutdown`] so a second
    /// call is a no-op.
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Backend {
    /// Spawns the worker thread and returns the constructed backend.
    ///
    /// # Panics
    ///
    /// Panics if `thread::Builder::spawn` fails (e.g. the OS refuses to
    /// create a new thread). The backend is unusable without its worker, so
    /// failing fast is the only sensible response.
    pub fn start(options: BackendOptions) -> Self {
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let worker_flag = Arc::clone(&shutdown_flag);
        let idle_sleep = options.idle_sleep;
        let handle = thread::Builder::new()
            .name(options.thread_name.clone())
            .spawn(move || worker_loop(&worker_flag, idle_sleep))
            .expect("OS should be able to spawn a thread");
        Self {
            options,
            shutdown_flag,
            worker: Mutex::new(Some(handle)),
        }
    }

    /// Signals the worker to stop and joins it. Idempotent: a second call
    /// finds the join handle already taken and returns immediately.
    pub fn shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::Release);
        let handle = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            // Best-effort join. A panicking worker is contained here so the
            // shutdown path itself stays panic-free.
            let _ = handle.join();
        }
    }
}

/// Worker thread body. Sleeps in a loop until `shutdown` is set.
fn worker_loop(shutdown: &AtomicBool, idle_sleep: Duration) {
    while !shutdown.load(Ordering::Acquire) {
        thread::sleep(idle_sleep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_match_legacy_constants() {
        let opts = BackendOptions::default();
        assert_eq!(opts.thread_name, "insomnilog-backend");
        assert_eq!(opts.queue_capacity, 128 * 1024);
        assert_eq!(opts.idle_yield_rounds, 32);
        assert_eq!(opts.idle_sleep, Duration::from_micros(100));
        assert!(opts.wait_for_queues_to_empty_before_exit);
    }

    #[test]
    fn backend_start_spawns_worker_and_shutdown_joins_it() {
        let backend = Backend::start(BackendOptions::default());
        backend.shutdown();
        // After shutdown, the join handle slot must be empty.
        assert!(
            backend
                .worker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none(),
        );
    }

    #[test]
    fn backend_shutdown_is_idempotent() {
        let backend = Backend::start(BackendOptions::default());
        backend.shutdown();
        backend.shutdown(); // must not panic, must not block forever
    }

    #[test]
    fn worker_thread_name_matches_configured_name() {
        let backend = Backend::start(BackendOptions {
            thread_name: "my-custom-thread".into(),
            ..BackendOptions::default()
        });
        let name = backend
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|h| h.thread().name().map(str::to_owned));
        backend.shutdown();
        assert_eq!(name.as_deref(), Some("my-custom-thread"));
    }
}
