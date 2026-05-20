//! Thread-local producer slot.
//!
//! [`with_producer`] is the single entry point for macro-generated code to
//! obtain a mutable reference to the calling thread's [`Producer`] without
//! any allocation or lock on the hot path.

use std::cell::RefCell;

use crate::lifecycle;
use crate::per_thread_queue::PerThreadProducer;
use crate::queue::Producer;

thread_local! {
    /// Per-thread producer handle, lazily populated on the first call to
    /// [`with_producer`] from a given thread.
    static TL_PRODUCER: RefCell<Option<PerThreadProducer>> = const { RefCell::new(None) };
}

/// Ensures the calling thread's [`TL_PRODUCER`] slot is populated.
///
/// If the slot is empty, registers the thread with the backend (allocating
/// the SPSC queue and pushing the context). Subsequent calls from the same
/// thread are no-ops.
///
/// This is the shared lazy-init helper used by both [`with_producer`] and
/// [`crate::lifecycle::preallocate_thread`].
///
/// # Panics
///
/// - Panics if [`crate::start`] has not been called.
/// - Panics if called from the backend worker thread (re-entrancy guard).
pub fn ensure_producer_registered() {
    TL_PRODUCER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            *borrow = Some(lifecycle::get_backend().create_producer());
        }
    });
}

/// Calls `f` with the calling thread's [`Producer`].
///
/// On the first invocation from a given thread, the thread is lazily
/// registered with the backend by calling
/// [`Backend::register_producer`][crate::backend::Backend::register_producer]
/// and the resulting [`ProducerHandle`] is stored in [`TL_PRODUCER`].
/// Subsequent calls from the same thread reuse the cached handle with no
/// synchronisation.
///
/// # Panics
///
/// - Panics with `"insomnilog: call insomnilog::start() before using the
///   logger"` if [`crate::start`] has not been called.
/// - Panics with the re-entrancy message from
///   [`Backend::register_producer`][crate::backend::Backend::register_producer]
///   if called from the backend worker thread itself.
pub fn with_producer<R>(f: impl FnOnce(&mut Producer) -> R) -> R {
    ensure_producer_registered();
    TL_PRODUCER.with(|cell| {
        // `unwrap` is infallible: `ensure_producer_registered` guarantees `Some`.
        f(&mut cell.borrow_mut().as_mut().unwrap().producer)
    })
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use super::with_producer;
    use crate::backend::BackendOptions;
    use crate::decode::LogRecord;
    use crate::level::LogLevel;
    use crate::lifecycle;
    use crate::metadata::LogMetadata;
    use crate::record::RecordHeader;
    use crate::sink::{Sink, SinkError};
    use crate::testutil::spin_until;

    fn fast_options() -> BackendOptions {
        BackendOptions {
            idle_sleep: Duration::from_micros(10),
            idle_yield_rounds: 0,
            ..BackendOptions::default()
        }
    }

    fn write_raw_record(
        producer: &mut crate::queue::Producer,
        metadata: &'static LogMetadata,
        logger: &Arc<crate::logger::Logger>,
    ) {
        let header = RecordHeader::new(
            0,
            ptr::from_ref(metadata) as usize,
            Arc::as_ptr(logger) as usize,
            0,
        );
        producer
            .write(RecordHeader::SIZE, |buf| {
                // SAFETY: buf has exactly RecordHeader::SIZE bytes.
                unsafe { header.write_to(buf.as_mut_ptr()) };
            })
            .expect("queue write must succeed on a fresh producer");
    }

    #[test]
    fn first_with_producer_registers_one_context_second_reuses() {
        let _guard = lifecycle::start(fast_options()).expect("start must succeed");
        let backend = lifecycle::get_backend();

        assert_eq!(
            backend.consumer_count(),
            0,
            "fresh backend must have no contexts"
        );

        with_producer(|_| {});
        assert_eq!(
            backend.consumer_count(),
            1,
            "first call must register exactly one context"
        );

        with_producer(|_| {});
        assert_eq!(
            backend.consumer_count(),
            1,
            "second call on the same thread must reuse the TLS slot"
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn n_distinct_threads_produce_n_contexts() {
        const N: usize = 4;

        let _guard = lifecycle::start(fast_options()).expect("start must succeed");
        let backend = lifecycle::get_backend();

        // N+1-party barrier used in two rounds:
        //   round 1 — all N threads have their TL_PRODUCER set; main can check.
        //   round 2 — main has finished checking; threads exit (TLS drops handle).
        let barrier = Arc::new(Barrier::new(N + 1));

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    with_producer(|_| {});
                    barrier.wait(); // round 1: TLS handle alive
                    barrier.wait(); // round 2: wait for main, then exit
                })
            })
            .collect();

        barrier.wait(); // round 1: all N TLS handles are live
        assert_eq!(
            backend.consumer_count(),
            N,
            "N distinct threads must produce N contexts"
        );
        barrier.wait(); // round 2: release threads

        for h in handles {
            h.join().expect("thread must not panic");
        }
    }

    #[test]
    #[should_panic(expected = "insomnilog: call insomnilog::start() before using the logger")]
    fn with_producer_before_start_panics_with_not_started_message() {
        with_producer(|_| {});
    }

    #[cfg(not(miri))]
    #[test]
    fn with_producer_from_worker_thread_hits_reentrancy_guard() {
        use std::sync::atomic::AtomicBool;

        struct ReentrantSink {
            did_panic: Arc<AtomicBool>,
        }

        impl Sink for ReentrantSink {
            fn write_record(&self, _record: &LogRecord) -> Result<(), SinkError> {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    with_producer(|_| {});
                }));
                if let Err(payload) = result {
                    self.did_panic.store(true, Ordering::Release);
                    std::panic::resume_unwind(payload);
                }
                Ok(())
            }

            fn flush(&self) -> Result<(), SinkError> {
                Ok(())
            }

            fn level(&self) -> LogLevel {
                LogLevel::Trace
            }
        }

        static META: LogMetadata = LogMetadata {
            level: LogLevel::Info,
            fmt_str: "",
            file: "frontend_test.rs",
            line: 1,
            module_path: "tests",
            arg_count: 0,
        };

        let did_panic = Arc::new(AtomicBool::new(false));

        let _guard = lifecycle::start(fast_options()).expect("start must succeed");
        let backend = lifecycle::get_backend();

        let logger = backend
            .create_logger(
                "frontend_reentrant",
                vec![Arc::new(ReentrantSink {
                    did_panic: Arc::clone(&did_panic),
                }) as Arc<dyn Sink>],
                LogLevel::Trace,
            )
            .expect("logger must be created");

        let mut handle = backend.create_producer();
        write_raw_record(&mut handle.producer, &META, &logger);
        drop(handle);

        let reached = spin_until(|| did_panic.load(Ordering::Acquire), Duration::from_secs(5));
        assert!(
            reached,
            "re-entrancy panic must be observed by the worker within 5 s"
        );
    }
}
