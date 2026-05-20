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
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

use crate::backend_runner::BackendRunner;
use crate::level::LogLevel;
use crate::logger::Logger;
use crate::sink::Sink;

/// Error returned by [`crate::create_logger`] when a logger is already
/// registered under the given name.
pub struct LoggerAlreadyRegistered {
    /// The logger that was registered under the name before this call.
    pub existing: Arc<Logger>,
}

impl std::fmt::Debug for LoggerAlreadyRegistered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoggerAlreadyRegistered")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for LoggerAlreadyRegistered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a logger with this name is already registered")
    }
}

impl std::error::Error for LoggerAlreadyRegistered {}

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
    options: BackendOptions,
    /// Named registry of sinks. Holds strong `Arc<dyn Sink>` for the process
    /// lifetime. Lookups go through `RwLock::read`; insertions go through
    /// `RwLock::write` so concurrent `register_sink` calls with the same name
    /// are serialized and exactly one wins.
    sinks: RwLock<HashMap<String, Arc<dyn Sink>>>,
    /// Named registry of loggers.
    ///
    /// The registry is the authoritative owner. Once registered, a logger
    /// outlives every record that references it via `*const Logger` in the
    /// record header — that lifetime guarantee is what makes the
    /// raw-pointer dispatch model safe.
    loggers: RwLock<HashMap<String, Arc<Logger>>>,
    /// Shared worker state. `Backend` holds one `Arc` clone; the worker
    /// thread holds another. All cross-thread state (counters, consumers,
    /// shutdown flag) lives here so each piece of data exists in exactly one
    /// place.
    runner: Arc<BackendRunner>,
    /// Worker thread join handle. `take`n by [`Self::shutdown`] so a second
    /// call is a no-op.
    worker: Mutex<Option<JoinHandle<()>>>,
    /// `ThreadId` of the spawned worker thread. Captured inside the worker
    /// closure and shipped back through a one-shot channel so
    /// `create_producer` can detect re-entrant registration from the worker
    /// itself.
    worker_thread_id: ThreadId,
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
        let runner = Arc::new(BackendRunner::new(
            options.idle_yield_rounds,
            options.idle_sleep,
            options.wait_for_queues_to_empty_before_exit,
        ));
        let worker_runner = Arc::clone(&runner);
        // One-shot handshake: the worker reports its `ThreadId` to
        // `start` before entering the dispatch loop. `sync_channel(1)`
        // gives us the cheapest fixed-capacity mpsc available in std.
        let (tid_tx, tid_rx) = std::sync::mpsc::sync_channel::<ThreadId>(1);
        let handle = thread::Builder::new()
            .name(options.thread_name.clone())
            .spawn(move || {
                tid_tx
                    .send(thread::current().id())
                    .expect("Backend::start must still be holding the receiver");
                drop(tid_tx);
                worker_runner.run();
            })
            .expect("OS should be able to spawn a thread");
        let worker_thread_id = tid_rx
            .recv()
            .expect("worker must report its ThreadId before running");
        Self {
            options,
            sinks: RwLock::new(HashMap::new()),
            loggers: RwLock::new(HashMap::new()),
            runner,
            worker: Mutex::new(Some(handle)),
            worker_thread_id,
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

    /// Returns the logger registered under `name`, if any.
    pub fn get_logger(&self, name: &str) -> Option<Arc<Logger>> {
        self.loggers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// Creates a new logger under `name` with the given `sinks` and `level`.
    ///
    /// Returns <code>Err([LoggerAlreadyRegistered])</code> if a logger is
    /// already registered under `name`. The error carries the existing `Arc`
    /// so the caller can inspect or compare it.
    ///
    /// Concurrent callers with the same `name` are serialized: exactly one wins and the
    /// rest receive `Err`.
    pub fn create_logger(
        &self,
        name: &str,
        sinks: Vec<Arc<dyn Sink>>,
        level: LogLevel,
    ) -> Result<Arc<Logger>, LoggerAlreadyRegistered> {
        let mut map = self.loggers.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = map.get(name) {
            return Err(LoggerAlreadyRegistered {
                existing: Arc::clone(existing),
            });
        }
        let arc = Arc::new(Logger::new(name.to_owned(), sinks, level));
        map.insert(name.to_owned(), Arc::clone(&arc));
        drop(map);
        Ok(arc)
    }

    /// Signals the worker to stop and joins it. Idempotent: a second call
    /// finds the join handle already taken and returns immediately.
    ///
    /// Prints a summary to stderr if any sink errors were recorded during
    /// this session.
    pub fn shutdown(&self) {
        self.runner.shutdown_flag.store(true, Ordering::Release);
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
        let write_errors = self.runner.write_errors.load(Ordering::Relaxed);
        let flush_errors = self.runner.flush_errors.load(Ordering::Relaxed);
        let panic_count = self.runner.panic_count.load(Ordering::Relaxed);
        if write_errors > 0 || flush_errors > 0 || panic_count > 0 {
            eprintln!(
                "insomnilog: sink errors at shutdown — write_record: {write_errors}, flush: {flush_errors}, panics: {panic_count}"
            );
        }
        let dropped_records = self.runner.dropped_records.load(Ordering::Relaxed);
        if dropped_records > 0 {
            eprintln!(
                "insomnilog: {dropped_records} log record(s) dropped at shutdown (queue full or oversized)"
            );
        }
    }

    /// Increments the dropped-records counter by one.
    ///
    /// Called from the hot path (macro expansion) when `Producer::write`
    /// returns an error, so it uses a `Relaxed` store — ordering relative to
    /// other counters is not required.
    pub(crate) fn increment_dropped_records(&self) {
        self.runner.dropped_records.fetch_add(1, Ordering::Relaxed);
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

    fn null_sink(level: LogLevel) -> Arc<dyn Sink> {
        Arc::new(NullSink::new(level))
    }

    #[test]
    fn logger_registry_is_initially_empty() {
        shutdown_after(BackendOptions::default(), |backend| {
            assert!(backend.get_logger("anything").is_none());
        });
    }

    #[test]
    fn create_logger_registers_on_first_call() {
        shutdown_after(BackendOptions::default(), |backend| {
            let logger = backend
                .create_logger("app", vec![null_sink(LogLevel::Info)], LogLevel::Warning)
                .expect("first registration must succeed");
            assert_eq!(logger.name(), "app");
            assert_eq!(logger.level(), LogLevel::Warning);
            assert_eq!(logger.sinks().len(), 1);
        });
    }

    #[test]
    fn create_logger_returns_error_on_collision() {
        shutdown_after(BackendOptions::default(), |backend| {
            let first = backend
                .create_logger("app", vec![null_sink(LogLevel::Info)], LogLevel::Info)
                .expect("first registration must succeed");

            let second_sink = null_sink(LogLevel::Error);
            let weak = Arc::downgrade(&second_sink);
            let err = backend
                .create_logger("app", vec![Arc::clone(&second_sink)], LogLevel::Error)
                .expect_err("duplicate name must fail");
            drop(second_sink);

            assert!(
                Arc::ptr_eq(&first, &err.existing),
                "error must carry the originally registered Arc",
            );
            assert_eq!(
                err.existing.level(),
                LogLevel::Info,
                "existing logger must be unchanged",
            );
            assert_eq!(
                err.existing.sinks().len(),
                1,
                "existing sink list must be unchanged",
            );
            assert!(
                weak.upgrade().is_none(),
                "rejected second-call sink must not be retained anywhere",
            );
        });
    }

    #[test]
    fn get_logger_returns_registered_logger() {
        shutdown_after(BackendOptions::default(), |backend| {
            let registered = backend
                .create_logger("app", Vec::new(), LogLevel::Warning)
                .expect("first registration must succeed");
            let fetched = backend
                .get_logger("app")
                .expect("registered logger must be retrievable");
            assert!(Arc::ptr_eq(&registered, &fetched));
            assert_eq!(fetched.level(), LogLevel::Warning);
        });
    }

    #[test]
    fn distinct_names_produce_distinct_loggers() {
        shutdown_after(BackendOptions::default(), |backend| {
            let a = backend
                .create_logger("a", Vec::new(), LogLevel::Info)
                .expect("registration must succeed");
            let b = backend
                .create_logger("b", Vec::new(), LogLevel::Error)
                .expect("registration must succeed");
            assert!(!Arc::ptr_eq(&a, &b));
            assert_eq!(a.name(), "a");
            assert_eq!(b.name(), "b");
            assert_eq!(a.level(), LogLevel::Info);
            assert_eq!(b.level(), LogLevel::Error);
        });
    }

    #[test]
    fn registry_pins_logger_when_caller_drops_arc() {
        shutdown_after(BackendOptions::default(), |backend| {
            let initial = backend
                .create_logger("pinned", Vec::new(), LogLevel::Debug)
                .expect("first registration must succeed");
            assert!(
                Arc::strong_count(&initial) >= 2,
                "registry + caller must each hold a strong Arc",
            );
            drop(initial);

            let after_drop = backend
                .get_logger("pinned")
                .expect("registry must still hold the logger");
            assert_eq!(after_drop.level(), LogLevel::Debug);
        });
    }

    #[test]
    fn worker_thread_id_matches_spawned_worker_handle() {
        let backend = Backend::start(BackendOptions::default());
        let test_thread_id = thread::current().id();
        let handle_thread_id = backend
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|h| h.thread().id())
            .expect("worker handle is present before shutdown");
        assert_ne!(
            backend.worker_thread_id, test_thread_id,
            "worker must be a different thread than the test",
        );
        assert_eq!(
            backend.worker_thread_id, handle_thread_id,
            "stored ThreadId must match the JoinHandle's thread",
        );
        backend.shutdown();
    }

    #[test]
    fn concurrent_create_logger_exactly_one_wins() {
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
                    backend.create_logger("contested", Vec::new(), LogLevel::Info)
                })
            })
            .collect();

        let results: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("worker thread must not panic"))
            .collect();

        let wins = results.iter().filter(|r| r.is_ok()).count();
        let registered = backend
            .get_logger("contested")
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
