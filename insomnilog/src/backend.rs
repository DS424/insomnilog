//! Backend worker thread and its configuration.
//!
//! [`Backend`] is a process-wide singleton created by [`crate::start`] and
//! stored in a `OnceLock`. It owns a dedicated thread that idles until
//! [`Backend::shutdown`] is called.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::sink::Sink;

/// Error returned by `create_sink` when a sink is already registered under
/// the given name.
pub struct SinkAlreadyRegistered {
    /// The sink that was registered under the name before this call.
    pub existing: Arc<dyn Sink>,
}

impl std::fmt::Debug for SinkAlreadyRegistered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinkAlreadyRegistered")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for SinkAlreadyRegistered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a sink with this name is already registered")
    }
}

impl std::error::Error for SinkAlreadyRegistered {}

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
    /// Named registry of sinks. Holds strong `Arc<dyn Sink>` for the process
    /// lifetime. Lookups go through `RwLock::read`; insertions go through
    /// `RwLock::write` so concurrent `register_sink` calls with the same name
    /// are serialized and exactly one wins.
    sinks: RwLock<HashMap<String, Arc<dyn Sink>>>,
    /// Set to `true` by [`Self::shutdown`]. The worker thread observes this
    /// with `Acquire` and exits when it transitions to `true`.
    shutdown_flag: Arc<AtomicBool>,
    /// Worker thread join handle. `take`n by [`Self::shutdown`] so a second
    /// call is a no-op.
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Total number of [`Sink::write_record`] calls that returned `Err` across
    /// all sinks. Incremented by the worker loop; read at shutdown for the
    /// error summary.
    write_errors: AtomicU64,
    /// Total number of [`Sink::flush`] calls that returned `Err` across all
    /// sinks. Incremented by the worker loop; read at shutdown for the error
    /// summary.
    flush_errors: AtomicU64,
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
            sinks: RwLock::new(HashMap::new()),
            shutdown_flag,
            worker: Mutex::new(Some(handle)),
            write_errors: AtomicU64::new(0),
            flush_errors: AtomicU64::new(0),
        }
    }

    /// Returns the sink registered under `name`, if any.
    ///
    /// One `Arc::clone` per call (atomic refcount bump); no `Weak::upgrade`,
    /// no branch for dead entries — registered sinks live for the process
    /// lifetime.
    pub fn get_sink(&self, name: &str) -> Option<Arc<dyn Sink>> {
        self.sinks
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// Registers `sink` under `name`.
    ///
    /// Returns <code>Err([SinkAlreadyRegistered])</code> if a sink is already
    /// registered under `name`. The error carries the existing `Arc` so the
    /// caller can inspect or compare it.
    ///
    /// The write lock is held across the lookup-and-insert, so concurrent
    /// callers with the same `name` are serialized: exactly one wins and the
    /// rest receive `Err`.
    pub fn register_sink(
        &self,
        name: &str,
        sink: Arc<dyn Sink>,
    ) -> Result<(), SinkAlreadyRegistered> {
        let mut map = self.sinks.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = map.get(name) {
            return Err(SinkAlreadyRegistered {
                existing: Arc::clone(existing),
            });
        }
        map.insert(name.to_owned(), sink);
        drop(map);
        Ok(())
    }

    /// Signals the worker to stop and joins it. Idempotent: a second call
    /// finds the join handle already taken and returns immediately.
    ///
    /// Prints a summary to stderr if any sink errors were recorded during
    /// this session.
    pub fn shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::Release);
        let handle = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            // Best-effort join. A panicking worker is contained here so the
            // shutdown path itself stays panic-free.
            let _ = handle.join();
        }
        let write_errors = self.write_errors.load(Ordering::Relaxed);
        let flush_errors = self.flush_errors.load(Ordering::Relaxed);
        if write_errors > 0 || flush_errors > 0 {
            eprintln!(
                "insomnilog: sink errors at shutdown — write_record: {write_errors}, flush: {flush_errors}"
            );
        }
    }

    /// Increments the write-error counter. Called by the worker loop when a
    /// sink's [`crate::sink::Sink::write_record`] returns `Err`.
    #[expect(
        dead_code,
        reason = "called by the worker loop once sink dispatch is wired"
    )]
    pub(crate) fn record_write_error(&self) {
        self.write_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the flush-error counter. Called by the worker loop when a
    /// sink's [`crate::sink::Sink::flush`] returns `Err`.
    #[expect(
        dead_code,
        reason = "called by the worker loop once sink dispatch is wired"
    )]
    pub(crate) fn record_flush_error(&self) {
        self.flush_errors.fetch_add(1, Ordering::Relaxed);
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
    use std::sync::Barrier;

    use super::*;
    use crate::level::LogLevel;
    use crate::sink::NullSink;

    fn shutdown_after<F>(opts: BackendOptions, body: F)
    where
        F: FnOnce(&Backend),
    {
        let backend = Backend::start(opts);
        body(&backend);
        backend.shutdown();
    }

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
                .unwrap_or_else(PoisonError::into_inner)
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
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|h| h.thread().name().map(str::to_owned));
        backend.shutdown();
        assert_eq!(name.as_deref(), Some("my-custom-thread"));
    }

    #[test]
    fn sink_registry_is_initially_empty() {
        shutdown_after(BackendOptions::default(), |backend| {
            assert!(backend.get_sink("anything").is_none());
        });
    }

    #[test]
    fn register_sink_succeeds_on_first_call() {
        shutdown_after(BackendOptions::default(), |backend| {
            backend
                .register_sink("console", Arc::new(NullSink::new(LogLevel::Info)))
                .expect("first registration must succeed");
            assert_eq!(
                backend
                    .get_sink("console")
                    .expect("must be retrievable")
                    .level(),
                LogLevel::Info,
            );
        });
    }

    #[test]
    fn register_sink_returns_error_on_duplicate_name() {
        shutdown_after(BackendOptions::default(), |backend| {
            let first: Arc<dyn Sink> = Arc::new(NullSink::new(LogLevel::Info));
            backend
                .register_sink("console", Arc::clone(&first))
                .expect("first registration must succeed");

            let err = backend
                .register_sink("console", Arc::new(NullSink::new(LogLevel::Error)))
                .expect_err("second registration under same name must fail");

            assert!(
                Arc::ptr_eq(&first, &err.existing),
                "error must carry the originally registered Arc",
            );
            assert_eq!(
                err.existing.level(),
                LogLevel::Info,
                "existing sink must be unchanged"
            );
        });
    }

    #[test]
    fn get_sink_returns_registered_sink() {
        shutdown_after(BackendOptions::default(), |backend| {
            let sink: Arc<dyn Sink> = Arc::new(NullSink::new(LogLevel::Warning));
            backend
                .register_sink("console", Arc::clone(&sink))
                .expect("registration must succeed");
            let fetched = backend
                .get_sink("console")
                .expect("registered sink must be retrievable");
            assert!(Arc::ptr_eq(&sink, &fetched));
        });
    }

    #[test]
    fn distinct_names_produce_distinct_sinks() {
        shutdown_after(BackendOptions::default(), |backend| {
            let a: Arc<dyn Sink> = Arc::new(NullSink::new(LogLevel::Info));
            let b: Arc<dyn Sink> = Arc::new(NullSink::new(LogLevel::Error));
            backend
                .register_sink("a", Arc::clone(&a))
                .expect("registration must succeed");
            backend
                .register_sink("b", Arc::clone(&b))
                .expect("registration must succeed");
            assert!(!Arc::ptr_eq(&a, &b));
            assert_eq!(a.level(), LogLevel::Info);
            assert_eq!(b.level(), LogLevel::Error);
        });
    }

    #[test]
    fn registry_pins_sink_when_caller_drops_arc() {
        shutdown_after(BackendOptions::default(), |backend| {
            let initial: Arc<dyn Sink> = Arc::new(NullSink::new(LogLevel::Debug));
            backend
                .register_sink("pinned", Arc::clone(&initial))
                .expect("registration must succeed");
            assert!(
                Arc::strong_count(&initial) >= 2,
                "registry + caller must each hold a strong Arc",
            );
            drop(initial);

            let after_drop = backend
                .get_sink("pinned")
                .expect("registry must still hold the sink");
            assert_eq!(after_drop.level(), LogLevel::Debug);
        });
    }

    #[test]
    fn concurrent_register_sink_exactly_one_wins() {
        const N: usize = 8;

        let backend = Arc::new(Backend::start(BackendOptions::default()));
        let barrier = Arc::new(Barrier::new(N));

        #[expect(
            clippy::needless_collect,
            reason = "all threads must spawn before any joins; \
                      a lazy chain would deadlock on the Barrier"
        )]
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let backend = Arc::clone(&backend);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    backend.register_sink("contested", Arc::new(NullSink::new(LogLevel::Info)))
                })
            })
            .collect();

        let results: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("worker thread must not panic"))
            .collect();

        let wins = results.iter().filter(|r| r.is_ok()).count();

        let registered = backend
            .get_sink("contested")
            .expect("winning registration must be retrievable");
        backend.shutdown();

        assert_eq!(wins, 1, "exactly one thread must win the race");
        for result in results.iter().filter_map(|r| r.as_ref().err()) {
            assert!(
                Arc::ptr_eq(&registered, &result.existing),
                "all losers must report the winning Arc as existing",
            );
        }
    }
}
