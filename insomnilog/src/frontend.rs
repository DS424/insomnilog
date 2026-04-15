//! Frontend logger and thread-local queue management.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::backend::{BackendWorker, SharedState};
use crate::level::LogLevel;
use crate::queue::{self, Consumer, Producer};

/// Default per-thread queue capacity (128 KiB).
const DEFAULT_QUEUE_CAPACITY: usize = 128 * 1024;

/// The main logger handle.
///
/// Create one via [`LoggerBuilder`] and pass it (or a clone) to the logging
/// macros. When all clones are dropped the backend thread is shut down after
/// draining remaining records.
///
/// # Examples
///
/// ```
/// use insomnilog::{Logger, LogLevel, log_info};
///
/// let logger = Logger::builder().level(LogLevel::Info).build();
/// log_info!(logger, "hello {}", "world");
/// logger.flush();
/// ```
pub struct Logger {
    /// Shared state with the backend.
    shared: Arc<SharedState>,
    /// Current log level filter (atomic for fast reads on the hot path).
    level: Arc<AtomicU8>,
    /// Handle to the backend thread (wrapped in `Option` for drop).
    backend_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Clone for Logger {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            level: Arc::clone(&self.level),
            backend_handle: Arc::clone(&self.backend_handle),
        }
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        // Only shut down when this is the last Logger handle (besides the Arc
        // in backend_handle which keeps it alive until join).
        // shared: one in Logger, backend_handle's Mutex also holds a ref.
        // We check if we are the last Logger clone by checking Arc strong count.
        // Arc::strong_count for shared: 1 for this Logger + 1 for BackendWorker.
        if Arc::strong_count(&self.shared) == 2 {
            self.shared.shutdown.store(true, Ordering::Release);
            if let Ok(mut guard) = self.backend_handle.lock()
                && let Some(handle) = guard.take()
            {
                let _ = handle.join();
            }
        }
    }
}

impl Logger {
    /// Returns a new [`LoggerBuilder`] with default settings.
    #[must_use]
    pub const fn builder() -> LoggerBuilder {
        LoggerBuilder {
            level: LogLevel::Trace,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
        }
    }

    /// Returns the current log level filter.
    #[inline]
    #[must_use]
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn level_filter(&self) -> LogLevel {
        LogLevel::from(self.level.load(Ordering::Relaxed))
    }

    /// Pre-allocates the per-thread SPSC queue for the calling thread.
    ///
    /// Call this once per thread during non-real-time initialisation, before
    /// entering any real-time context. After this call, all logging macros on
    /// this thread are real-time safe: the hot path performs no allocation and
    /// acquires no locks.
    ///
    /// Logging still works without this call — the queue is created lazily on
    /// the first log statement — but that first call will allocate, which
    /// violates real-time constraints. Calling `preallocate` moves that
    /// allocation out of the hot path and into the initialisation phase where
    /// it belongs.
    ///
    /// # Examples
    ///
    /// ```
    /// use insomnilog::{Logger, LogLevel, log_info};
    ///
    /// let logger = Logger::builder().level(LogLevel::Trace).build();
    ///
    /// // Initialise before the real-time section.
    /// logger.preallocate();
    ///
    /// // From here the hot path is allocation-free.
    /// log_info!(logger, "hello {}", "world");
    /// logger.flush();
    /// ```
    pub fn preallocate(&self) {
        TL_PRODUCER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let (producer, consumer) = queue::new(self.shared.queue_capacity);
                register_consumer(&self.shared, consumer);
                *borrow = Some(ThreadLocalProducer { producer });
            }
        });
    }

    /// Blocks until all pending log records have been written to the sink.
    pub fn flush(&self) {
        // Simple busy-wait: we check that all registered consumers are drained.
        // This is only intended for use before shutdown, not on the hot path.
        for _ in 0..1000 {
            let all_empty = {
                let registry = self
                    .shared
                    .registry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                registry.is_empty()
            };
            if all_empty {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// Builder for configuring and creating a [`Logger`].
pub struct LoggerBuilder {
    /// The log level filter.
    level: LogLevel,
    /// Per-thread queue capacity.
    queue_capacity: usize,
}

impl LoggerBuilder {
    /// Sets the minimum log level. Messages below this level are discarded.
    #[must_use]
    pub const fn level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }

    /// Sets the per-thread queue capacity in bytes.
    ///
    /// Will be rounded up to the next power of two. Defaults to 128 KiB.
    #[must_use]
    pub const fn queue_capacity(mut self, capacity: usize) -> Self {
        self.queue_capacity = capacity;
        self
    }

    /// Builds the logger and spawns the backend worker thread.
    ///
    /// # Panics
    ///
    /// Panics if the backend thread cannot be spawned.
    #[must_use]
    pub fn build(self) -> Logger {
        let shared = Arc::new(SharedState {
            registry: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
            queue_capacity: self.queue_capacity,
        });

        let level = Arc::new(AtomicU8::new(self.level as u8));

        let backend_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("insomnilog-backend".into())
            .spawn(move || {
                let mut worker = BackendWorker::new(backend_shared);
                worker.run();
            })
            .expect("failed to spawn insomnilog backend thread");

        Logger {
            shared,
            level,
            backend_handle: Arc::new(Mutex::new(Some(handle))),
        }
    }
}

/// Thread-local state holding the per-thread producer.
struct ThreadLocalProducer {
    /// The producer half of this thread's SPSC queue.
    producer: Producer,
}

thread_local! {
    /// Each logging thread lazily gets its own SPSC queue producer.
    static TL_PRODUCER: RefCell<Option<ThreadLocalProducer>> = const { RefCell::new(None) };
}

/// Calls `f` with the thread-local producer for the given logger.
///
/// On first call from a new thread, creates a new SPSC queue and registers
/// the consumer half with the backend. That first call is **not** real-time
/// safe; call [`Logger::preallocate`] during thread initialisation to move the
/// allocation out of the hot path.
#[inline]
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
pub fn with_producer<F>(logger: &Logger, f: F)
where
    F: FnOnce(&mut Producer),
{
    TL_PRODUCER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            let (producer, consumer) = queue::new(logger.shared.queue_capacity);
            register_consumer(&logger.shared, consumer);
            *borrow = Some(ThreadLocalProducer { producer });
        }
        if let Some(tl) = borrow.as_mut() {
            f(&mut tl.producer);
        }
    });
}

/// Registers a consumer with the backend's registry.
fn register_consumer(shared: &Arc<SharedState>, consumer: Consumer) {
    let mut registry = shared
        .registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registry.push(consumer);
}
