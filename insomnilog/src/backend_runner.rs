//! Background worker that drains per-thread SPSC queues and fans records out
//! to sinks.
//!
//! [`BackendRunner`] holds all state shared between the backend worker thread
//! and [`crate::backend::Backend`]. It is constructed by `Backend::start`,
//! wrapped in an [`Arc`], and a clone is moved into the worker thread closure.
//! `Backend` retains the other clone so it can read counters and signal
//! shutdown without any additional synchronisation layer — all shared fields
//! are either atomic or already behind a [`Mutex`].

use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crate::decode::{LogRecord, RawDecodedRecord, decode_record};
use crate::logger::Logger;
use crate::per_thread_queue::PerThreadConsumer;
use crate::record::RecordHeader;

/// Maximum records drained from a single consumer per worker pass.
///
/// Bounds per-consumer monopoly so other consumers get a chance each round.
const MAX_BATCH_PER_CONSUMER: usize = 32;

/// Shared state owned by [`crate::backend::Backend`] and driven by the
/// background worker thread.
///
/// All fields that cross the thread boundary are individually synchronised:
/// counters are [`AtomicU64`], the consumer registry is behind a [`Mutex`],
/// and the shutdown signal is an [`AtomicBool`]. No wrapper mutex around
/// `BackendRunner` as a whole is needed.
pub struct BackendRunner {
    /// Set to `true` by `Backend::shutdown`. The worker observes this with an
    /// `Acquire` load and exits once the flag transitions to `true`.
    pub shutdown_flag: AtomicBool,
    /// Per-thread SPSC consumers. `Backend::create_producer` appends one entry
    /// here each time a new producer thread is registered; the worker snapshots
    /// the vec under a brief lock so sink dispatch does not block producers.
    pub consumers: Mutex<Vec<Arc<PerThreadConsumer>>>,
    /// Total [`crate::sink::Sink::write_record`] calls that returned `Err`.
    pub write_errors: AtomicU64,
    /// Total [`crate::sink::Sink::flush`] calls that returned `Err`.
    pub flush_errors: AtomicU64,
    /// Total unwound panics caught by the per-record `catch_unwind`.
    pub panic_count: AtomicU64,
    /// Total log records dropped because the producer queue was full or the
    /// encoded record was larger than the queue capacity.
    pub dropped_records: AtomicU64,
    /// Consecutive empty-poll loops that each call `yield_now` before
    /// escalating to [`Self::idle_sleep`].
    idle_yield_rounds: u32,
    /// Sleep duration once [`Self::idle_yield_rounds`] is exhausted.
    idle_sleep: Duration,
    /// When `true`, [`Self::run`] drains every consumer queue before returning.
    wait_for_drain: bool,
}

impl BackendRunner {
    /// Constructs a new `BackendRunner` with all counters zeroed and the
    /// shutdown flag cleared.
    ///
    /// The runner does not start any thread; call [`Self::run`] from a spawned
    /// thread to begin processing.
    pub const fn new(idle_yield_rounds: u32, idle_sleep: Duration, wait_for_drain: bool) -> Self {
        Self {
            shutdown_flag: AtomicBool::new(false),
            consumers: Mutex::new(Vec::new()),
            write_errors: AtomicU64::new(0),
            flush_errors: AtomicU64::new(0),
            panic_count: AtomicU64::new(0),
            dropped_records: AtomicU64::new(0),
            idle_yield_rounds,
            idle_sleep,
            wait_for_drain,
        }
    }

    /// Snapshots the consumer list under a brief lock so sink dispatch does
    /// not block threads calling `Backend::create_producer`.
    fn snapshot_consumers(&self) -> Vec<Arc<PerThreadConsumer>> {
        self.consumers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Reads one record from `consumer` without blocking.
    ///
    /// Returns `None` when the queue holds fewer bytes than a complete record
    /// or when the queue reports a size inconsistency (corrupted record).
    fn try_read_one(consumer: &PerThreadConsumer) -> Option<RawDecodedRecord> {
        let mut guard = consumer
            .consumer
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let avail = guard.available();
        if avail < RecordHeader::SIZE {
            return None;
        }

        let total_len = {
            let header_bytes = guard.peek(RecordHeader::SIZE);
            // SAFETY: the producer wrote these bytes via `RecordHeader::write_to`;
            // the struct is repr(C) and the layout is stable within a process.
            let header =
                unsafe { ptr::read_unaligned(header_bytes.as_ptr().cast::<RecordHeader>()) };
            RecordHeader::SIZE + header.encoded_args_size as usize
        };

        if avail < total_len {
            return None;
        }

        guard
            .read(total_len, |bytes| unsafe { decode_record(bytes) })
            .ok()
    }

    /// Drains up to `MAX_BATCH_PER_CTX` records from one consumer.
    ///
    /// Returns `true` if at least one record was processed.
    fn drain_consumer(&self, consumer: &PerThreadConsumer) -> bool {
        let mut did_work = false;
        for _ in 0..MAX_BATCH_PER_CONSUMER {
            let Some(raw) = Self::try_read_one(consumer) else {
                break;
            };
            self.process_record(raw);
            did_work = true;
        }
        did_work
    }

    /// Resolves the logger pointer in `raw`, builds a `LogRecord`, and fans it
    /// out to every sink attached to that logger.
    fn process_record(&self, raw: RawDecodedRecord) {
        // SAFETY: The backend's logger registry holds a strong `Arc<Logger>` for the
        // entire process lifetime; no `Arc<Logger>` is ever removed from the registry.
        let logger = unsafe { &*(raw.logger_ptr as *const Logger) };
        let log_record = LogRecord {
            timestamp_ns: raw.timestamp_ns,
            logger_name: logger.name().to_owned(),
            metadata: raw.metadata,
            args: raw.args,
        };
        self.fanout_to_sinks(logger, &log_record);
    }

    /// Dispatches `log_record` to every sink on `logger` whose level accepts it.
    ///
    /// A panicking sink is caught via `catch_unwind` so the worker thread
    /// survives and subsequent records keep flowing.
    fn fanout_to_sinks(&self, logger: &Logger, log_record: &LogRecord) {
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            for sink in logger.sinks() {
                if log_record.metadata.level < sink.level() {
                    continue;
                }
                match sink.write_record(log_record) {
                    Ok(()) => {
                        if sink.flush().is_err() {
                            self.flush_errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        self.write_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));

        if result.is_err() {
            self.panic_count.fetch_add(1, Ordering::Relaxed);
            eprintln!("insomnilog: a sink panicked during write_record");
        }
    }

    /// Removes consumers whose thread has exited and whose queue is fully
    /// drained. Bounds registry size and lets the drain-on-shutdown check
    /// converge.
    fn retire_dead_consumers(&self) {
        self.consumers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|consumer| {
                consumer.alive.load(Ordering::Acquire)
                    || consumer
                        .consumer
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .available()
                        > 0
            });
    }

    /// Returns `true` if any registered consumer still has unread bytes.
    fn any_queues_pending(&self) -> bool {
        self.consumers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|consumer| {
                consumer
                    .consumer
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .available()
                    > 0
            })
    }

    /// Worker thread body: polls all registered consumers, decodes records,
    /// and fans them out to sinks.
    ///
    /// Returns once `shutdown_flag` is `true` and — when `wait_for_drain` is
    /// also `true` — every consumer queue is fully empty.
    pub fn run(&self) {
        let mut idle_streak: u32 = 0;
        loop {
            let shutting_down = self.shutdown_flag.load(Ordering::Acquire);

            if shutting_down && !self.wait_for_drain {
                break;
            }

            let consumers = self.snapshot_consumers();
            let mut did_work = false;
            for consumer in &consumers {
                did_work |= self.drain_consumer(consumer);
            }

            self.retire_dead_consumers();

            if shutting_down {
                if !self.any_queues_pending() {
                    break;
                }
                continue; // records still in flight — keep draining without idling
            }

            if did_work {
                idle_streak = 0;
            } else {
                idle_streak = idle_streak.saturating_add(1);
                if idle_streak <= self.idle_yield_rounds {
                    thread::yield_now();
                } else {
                    thread::sleep(self.idle_sleep);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::Arc;
    use std::sync::PoisonError;
    use std::sync::atomic::Ordering;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use super::*;
    use crate::level::LogLevel;
    use crate::logger::Logger;
    use crate::metadata::LogMetadata;
    use crate::per_thread_queue;
    use crate::queue::Producer;
    use crate::record::RecordHeader;
    use crate::sink::{Sink, SinkError};
    use crate::testutil::RecordingSink;
    #[cfg(not(miri))]
    use crate::testutil::spin_until;

    fn fast_runner(wait_for_drain: bool) -> Arc<BackendRunner> {
        Arc::new(BackendRunner::new(
            0,
            Duration::from_micros(10),
            wait_for_drain,
        ))
    }

    fn spawn_runner(runner: Arc<BackendRunner>) -> JoinHandle<()> {
        thread::spawn(move || runner.run())
    }

    fn shutdown_runner(runner: &BackendRunner, handle: JoinHandle<()>) {
        runner.shutdown_flag.store(true, Ordering::Release);
        handle.join().expect("runner thread must not panic");
    }

    fn add_producer(runner: &BackendRunner) -> per_thread_queue::PerThreadProducer {
        let (producer, consumer) = per_thread_queue::new(128 * 1024);
        runner
            .consumers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(consumer));
        producer
    }

    fn consumer_count(runner: &BackendRunner) -> usize {
        runner
            .consumers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn logger_with_sinks(sinks: Vec<Arc<dyn Sink>>) -> Arc<Logger> {
        Arc::new(Logger::new("test".to_owned(), sinks, LogLevel::Trace))
    }

    static DISPATCH_META: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "",
        file: "backend_runner_test.rs",
        line: 1,
        module_path: "tests",
        arg_count: 0,
    };

    fn write_raw_record(
        producer: &mut Producer,
        metadata: &'static LogMetadata,
        logger: &Arc<Logger>,
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

    struct PanickingSink {
        level: LogLevel,
    }

    impl Sink for PanickingSink {
        fn write_record(&self, _record: &LogRecord) -> Result<(), SinkError> {
            panic!("PanickingSink: intentional test panic");
        }

        fn flush(&self) -> Result<(), SinkError> {
            Ok(())
        }

        fn level(&self) -> LogLevel {
            self.level
        }
    }

    struct WriteErrorSink {
        level: LogLevel,
    }

    impl Sink for WriteErrorSink {
        fn write_record(&self, _record: &LogRecord) -> Result<(), SinkError> {
            Err(SinkError::other(std::io::Error::other("test write error")))
        }

        fn flush(&self) -> Result<(), SinkError> {
            Ok(())
        }

        fn level(&self) -> LogLevel {
            self.level
        }
    }

    struct FlushErrorSink {
        level: LogLevel,
    }

    impl Sink for FlushErrorSink {
        fn write_record(&self, _record: &LogRecord) -> Result<(), SinkError> {
            Ok(())
        }

        fn flush(&self) -> Result<(), SinkError> {
            Err(SinkError::other(std::io::Error::other("test flush error")))
        }

        fn level(&self) -> LogLevel {
            self.level
        }
    }

    #[test]
    fn new_write_errors_starts_at_zero() {
        let runner = BackendRunner::new(0, Duration::from_millis(1), false);
        assert_eq!(runner.write_errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn new_flush_errors_starts_at_zero() {
        let runner = BackendRunner::new(0, Duration::from_millis(1), false);
        assert_eq!(runner.flush_errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn new_panic_count_starts_at_zero() {
        let runner = BackendRunner::new(0, Duration::from_millis(1), false);
        assert_eq!(runner.panic_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn new_shutdown_flag_starts_false() {
        let runner = BackendRunner::new(0, Duration::from_millis(1), false);
        assert!(!runner.shutdown_flag.load(Ordering::Relaxed));
    }

    #[test]
    fn new_consumers_starts_empty() {
        let runner = BackendRunner::new(0, Duration::from_millis(1), false);
        assert_eq!(consumer_count(&runner), 0);
    }

    #[test]
    fn run_exits_immediately_when_shutdown_flag_preset_no_drain() {
        let runner = fast_runner(false);
        runner.shutdown_flag.store(true, Ordering::Release);
        let handle = spawn_runner(Arc::clone(&runner));
        handle.join().expect("runner must not panic");
    }

    #[test]
    fn run_exits_immediately_when_shutdown_flag_preset_with_drain_and_empty_queues() {
        let runner = fast_runner(true);
        runner.shutdown_flag.store(true, Ordering::Release);
        let handle = spawn_runner(Arc::clone(&runner));
        handle.join().expect("runner must not panic");
    }

    #[test]
    fn run_exits_after_async_shutdown_signal() {
        let runner = fast_runner(false);
        let handle = spawn_runner(Arc::clone(&runner));
        runner.shutdown_flag.store(true, Ordering::Release);
        handle.join().expect("runner must not panic");
    }

    #[cfg(not(miri))]
    #[test]
    fn single_record_reaches_sink() {
        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = fast_runner(true);
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        let reached = spin_until(|| recording.record_count() >= 1, Duration::from_secs(5));
        shutdown_runner(&runner, runner_handle);

        assert!(reached, "record must reach the sink within 5 s");
        assert_eq!(
            recording.record_count(),
            1,
            "exactly one record must be delivered"
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn many_records_on_one_consumer_all_drain() {
        const N: usize = 500;

        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = fast_runner(true);
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        for _ in 0..N {
            write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        }
        drop(handle);

        let reached = spin_until(|| recording.record_count() >= N, Duration::from_secs(5));
        shutdown_runner(&runner, runner_handle);

        assert!(reached, "all {N} records must reach the sink");
        assert_eq!(recording.record_count(), N);
    }

    #[cfg(not(miri))]
    #[test]
    fn many_consumers_concurrent_total_count_matches() {
        use std::sync::Barrier;

        const THREADS: usize = 4;
        const RECORDS_PER_THREAD: usize = 100;
        const TOTAL: usize = THREADS * RECORDS_PER_THREAD;

        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = fast_runner(true);
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);

        let barrier = Arc::new(Barrier::new(THREADS));
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let runner = Arc::clone(&runner);
                let logger = Arc::clone(&logger);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let mut handle = add_producer(&runner);
                    for _ in 0..RECORDS_PER_THREAD {
                        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
                    }
                    // PerThreadProducer is !Send — dropped here on its creating thread.
                })
            })
            .collect();

        for h in handles {
            h.join().expect("producer thread must not panic");
        }

        let reached = spin_until(|| recording.record_count() >= TOTAL, Duration::from_secs(5));
        shutdown_runner(&runner, runner_handle);

        assert!(reached, "all {TOTAL} records must reach the sink");
        assert_eq!(recording.record_count(), TOTAL);
    }

    #[cfg(not(miri))]
    #[test]
    fn sink_panic_increments_panic_count_and_later_sinks_in_fanout_not_called() {
        let recording_a = Arc::new(RecordingSink::new(LogLevel::Trace));

        // Logger A: [PanickingSink, RecordingSink].
        // When PanickingSink panics, RecordingSink must NOT receive the same record.
        let logger_a = logger_with_sinks(vec![
            Arc::new(PanickingSink {
                level: LogLevel::Trace,
            }) as Arc<dyn Sink>,
            Arc::clone(&recording_a) as Arc<dyn Sink>,
        ]);

        // Logger B: RecordingSink only. Its record must survive the panic.
        let recording_b = Arc::new(RecordingSink::new(LogLevel::Trace));
        let logger_b = logger_with_sinks(vec![Arc::clone(&recording_b) as Arc<dyn Sink>]);

        let runner = fast_runner(true);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger_a);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger_b);
        drop(handle);
        let reached = spin_until(
            || runner.panic_count.load(Ordering::Acquire) >= 1 && recording_b.record_count() >= 1,
            Duration::from_secs(5),
        );
        shutdown_runner(&runner, runner_handle);

        assert!(
            reached,
            "panic must be observed and logger_b record must arrive"
        );
        assert_eq!(
            runner.panic_count.load(Ordering::Relaxed),
            1,
            "one panic must be counted",
        );
        assert_eq!(
            recording_a.record_count(),
            0,
            "sink after PanickingSink must not receive the panicked record",
        );
        assert_eq!(
            recording_b.record_count(),
            1,
            "logger_b record must be delivered"
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn sink_level_filter_skips_records_below_sink_threshold() {
        let debug_sink = Arc::new(RecordingSink::new(LogLevel::Debug));
        let warn_sink = Arc::new(RecordingSink::new(LogLevel::Warning));

        // DISPATCH_META.level == Info.
        // debug_sink (Debug ≤ Info) accepts it; warn_sink (Warning > Info) filters it.
        let logger = logger_with_sinks(vec![
            Arc::clone(&debug_sink) as Arc<dyn Sink>,
            Arc::clone(&warn_sink) as Arc<dyn Sink>,
        ]);

        let runner = fast_runner(true);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        let reached = spin_until(|| debug_sink.record_count() >= 1, Duration::from_secs(5));
        shutdown_runner(&runner, runner_handle);

        assert!(reached, "Info record must reach the Debug-level sink");
        assert_eq!(
            debug_sink.record_count(),
            1,
            "debug_sink must receive the record"
        );
        assert_eq!(
            warn_sink.record_count(),
            0,
            "warn_sink must filter out the Info record",
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn write_error_increments_write_error_counter() {
        let logger = logger_with_sinks(vec![Arc::new(WriteErrorSink {
            level: LogLevel::Trace,
        }) as Arc<dyn Sink>]);

        let runner = fast_runner(true);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        let reached = spin_until(
            || runner.write_errors.load(Ordering::Acquire) >= 1,
            Duration::from_secs(5),
        );
        shutdown_runner(&runner, runner_handle);

        assert!(reached, "write error must be counted within 5 s");
        assert_eq!(runner.write_errors.load(Ordering::Relaxed), 1);
    }

    #[cfg(not(miri))]
    #[test]
    fn flush_error_increments_flush_error_counter() {
        let logger = logger_with_sinks(vec![Arc::new(FlushErrorSink {
            level: LogLevel::Trace,
        }) as Arc<dyn Sink>]);

        let runner = fast_runner(true);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        let reached = spin_until(
            || runner.flush_errors.load(Ordering::Acquire) >= 1,
            Duration::from_secs(5),
        );
        shutdown_runner(&runner, runner_handle);

        assert!(reached, "flush error must be counted within 5 s");
        assert_eq!(runner.flush_errors.load(Ordering::Relaxed), 1);
    }

    #[cfg(not(miri))]
    #[test]
    fn drain_on_shutdown_delivers_all_queued_records() {
        const N: usize = 2000;

        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = fast_runner(true);
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        for _ in 0..N {
            write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        }
        drop(handle);

        // Signal shutdown immediately after writing; drain semantics mean
        // run() only returns after every queued record is processed.
        shutdown_runner(&runner, runner_handle);

        assert_eq!(
            recording.record_count(),
            N,
            "all {N} records must reach the sink before shutdown completes",
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn no_panic_when_shutdown_without_drain() {
        const N: usize = 200;

        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = fast_runner(false); // wait_for_drain = false
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);
        let runner_handle = spawn_runner(Arc::clone(&runner));

        let mut handle = add_producer(&runner);
        for _ in 0..N {
            write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        }
        drop(handle);

        shutdown_runner(&runner, runner_handle); // must not panic

        // Count is in [0, N]; this is a sanity check, not a delivery guarantee.
        assert!(
            recording.record_count() <= N,
            "count must not exceed the number of records written",
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn dead_thread_context_removed_after_queue_drains() {
        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = fast_runner(true);
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);

        assert_eq!(
            consumer_count(&runner),
            0,
            "fresh runner must have no consumers"
        );

        let runner_handle = spawn_runner(Arc::clone(&runner));

        let runner2 = Arc::clone(&runner);
        let logger2 = Arc::clone(&logger);
        let t = thread::spawn(move || {
            let mut h = add_producer(&runner2);
            write_raw_record(&mut h.producer, &DISPATCH_META, &logger2);
            // PerThreadProducer dropped here → alive flips to false.
        });
        t.join().expect("producer thread must not panic");

        // Poll until the worker drains the queue and prunes the dead consumer.
        let removed = spin_until(|| consumer_count(&runner) == 0, Duration::from_secs(5));

        shutdown_runner(&runner, runner_handle);

        assert!(
            removed,
            "dead consumer must disappear from the registry within 5 s",
        );
        assert_eq!(
            recording.record_count(),
            1,
            "the logged record must have been delivered"
        );
    }

    // Miri-only synchronous tests
    //
    // These drive `run()` directly in the test thread: write records, set
    // the shutdown flag, then call `runner.run()` which drains and returns.
    // No wall-clock spin loops — compatible with Miri's execution model.
    fn miri_runner() -> BackendRunner {
        BackendRunner::new(0, Duration::ZERO, true)
    }

    #[test]
    fn miri_single_record_reaches_sink() {
        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = miri_runner();
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);

        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        runner.shutdown_flag.store(true, Ordering::Release);
        runner.run();

        assert_eq!(recording.record_count(), 1);
    }

    #[test]
    fn miri_level_filter_accepted_and_rejected() {
        let debug_sink = Arc::new(RecordingSink::new(LogLevel::Debug));
        let warn_sink = Arc::new(RecordingSink::new(LogLevel::Warning));
        // DISPATCH_META.level == Info; debug_sink accepts it, warn_sink does not.
        let logger = logger_with_sinks(vec![
            Arc::clone(&debug_sink) as Arc<dyn Sink>,
            Arc::clone(&warn_sink) as Arc<dyn Sink>,
        ]);

        let runner = miri_runner();
        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        runner.shutdown_flag.store(true, Ordering::Release);
        runner.run();

        assert_eq!(debug_sink.record_count(), 1, "debug_sink must accept Info");
        assert_eq!(warn_sink.record_count(), 0, "warn_sink must reject Info");
    }

    #[test]
    fn miri_sink_panic_counted_and_later_sink_not_called() {
        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let logger = logger_with_sinks(vec![
            Arc::new(PanickingSink {
                level: LogLevel::Trace,
            }) as Arc<dyn Sink>,
            Arc::clone(&recording) as Arc<dyn Sink>,
        ]);

        let runner = miri_runner();
        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        runner.shutdown_flag.store(true, Ordering::Release);
        runner.run();

        assert_eq!(runner.panic_count.load(Ordering::Relaxed), 1);
        assert_eq!(
            recording.record_count(),
            0,
            "sink after panic must not be called"
        );
    }

    #[test]
    fn miri_write_error_counted() {
        let logger = logger_with_sinks(vec![Arc::new(WriteErrorSink {
            level: LogLevel::Trace,
        }) as Arc<dyn Sink>]);

        let runner = miri_runner();
        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        runner.shutdown_flag.store(true, Ordering::Release);
        runner.run();

        assert_eq!(runner.write_errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn miri_flush_error_counted() {
        let logger = logger_with_sinks(vec![Arc::new(FlushErrorSink {
            level: LogLevel::Trace,
        }) as Arc<dyn Sink>]);

        let runner = miri_runner();
        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle);

        runner.shutdown_flag.store(true, Ordering::Release);
        runner.run();

        assert_eq!(runner.flush_errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn miri_drain_delivers_all_records() {
        const N: usize = 5;

        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = miri_runner();
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);

        let mut handle = add_producer(&runner);
        for _ in 0..N {
            write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        }
        drop(handle);

        runner.shutdown_flag.store(true, Ordering::Release);
        runner.run();

        assert_eq!(recording.record_count(), N);
    }

    #[test]
    fn miri_dead_consumer_pruned() {
        let recording = Arc::new(RecordingSink::new(LogLevel::Trace));
        let runner = miri_runner();
        let logger = logger_with_sinks(vec![Arc::clone(&recording) as Arc<dyn Sink>]);

        assert_eq!(consumer_count(&runner), 0);

        let mut handle = add_producer(&runner);
        write_raw_record(&mut handle.producer, &DISPATCH_META, &logger);
        drop(handle); // alive flips to false

        runner.shutdown_flag.store(true, Ordering::Release);
        runner.run(); // drains record, retires dead consumer, returns

        assert_eq!(consumer_count(&runner), 0, "dead consumer must be pruned");
        assert_eq!(recording.record_count(), 1);
    }
}
