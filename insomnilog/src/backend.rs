//! Backend worker thread that drains log queues and writes to the sink.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::decode::{self, RecordHeader};
use crate::formatter::PatternFormatter;
use crate::queue::Consumer;
use crate::sink::{ConsoleSink, Sink};

/// Maximum number of idle poll rounds before sleeping.
const MAX_IDLE_ROUNDS: u32 = 32;

/// Sleep duration when the backend is idle.
const IDLE_SLEEP: Duration = Duration::from_micros(100);

/// Shared state between the frontend logger(s) and the backend worker.
pub struct SharedState {
    /// Registry of consumer halves (one per logging thread).
    pub registry: Mutex<Vec<Consumer>>,
    /// Flag set to `true` when the logger is shutting down.
    pub shutdown: AtomicBool,
    /// Per-thread queue capacity in bytes.
    pub queue_capacity: usize,
}

/// The backend worker that runs on a dedicated thread.
pub struct BackendWorker {
    /// Shared state with the frontend.
    shared: Arc<SharedState>,
    /// Formatter for producing human-readable log lines.
    formatter: PatternFormatter,
    /// Output sink (console).
    sink: ConsoleSink,
}

impl BackendWorker {
    /// Creates a new backend worker.
    pub fn new(shared: Arc<SharedState>) -> Self {
        Self {
            shared,
            formatter: PatternFormatter::new(),
            sink: ConsoleSink::new(),
        }
    }

    /// Runs the backend loop. Returns when shutdown is complete and all queues
    /// are drained.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
    pub fn run(&mut self) {
        let mut idle_rounds: u32 = 0;

        loop {
            let did_work = self.poll_all();

            if did_work {
                idle_rounds = 0;
            } else if self.shared.shutdown.load(Ordering::Acquire) {
                // Final drain — poll one more time to catch stragglers.
                self.poll_all();
                let _ = self.sink.flush();
                return;
            } else {
                idle_rounds = idle_rounds.saturating_add(1);
                if idle_rounds < MAX_IDLE_ROUNDS {
                    thread::yield_now();
                } else {
                    thread::sleep(IDLE_SLEEP);
                }
            }
        }
    }

    /// Polls all registered consumers once. Returns `true` if any work was done.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
    fn poll_all(&mut self) -> bool {
        // Take the consumers out of the mutex briefly to avoid holding the lock
        // while doing I/O. Other threads may register new consumers while we
        // are polling; they will be picked up on the next round.
        let mut consumers = {
            let mut registry = self
                .shared
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            core::mem::take(&mut *registry)
        };

        let mut did_work = false;

        for consumer in &mut consumers {
            while self.drain_one(consumer) {
                did_work = true;
            }
        }

        // Put consumers back (and merge with any newly registered ones).
        {
            let mut registry = self
                .shared
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            consumers.append(&mut *registry);
            *registry = consumers;
        }

        if did_work {
            let _ = self.sink.flush();
        }

        did_work
    }

    /// Tries to read and process one record from `consumer`.
    /// Returns `true` if a record was processed.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
    fn drain_one(&mut self, consumer: &mut Consumer) -> bool {
        let avail = consumer.available();
        if avail < RecordHeader::SIZE {
            return false;
        }

        let total = record_total(consumer);
        if avail < total {
            // Incomplete record — wait for the producer to commit more bytes.
            return false;
        }

        // SAFETY: metadata_ptr was set by the log macro to a valid static ref.
        let decoded = consumer.read(total, |record_bytes| unsafe {
            decode::decode_record(record_bytes)
        });

        if let Some(record) = decoded {
            let line = self.formatter.format(&record);
            let _ = self.sink.write_line(line);
        }

        true
    }
}

/// Peeks at the record header to determine the total byte length of the
/// next record (header + encoded args). Does not advance the read position.
fn record_total(consumer: &mut Consumer) -> usize {
    let header_bytes = consumer.peek(RecordHeader::SIZE);
    // SAFETY: `peek` returns exactly `RecordHeader::SIZE` bytes committed by
    // the producer. `read_unaligned` handles any alignment offset.
    let header = unsafe { core::ptr::read_unaligned(header_bytes.as_ptr().cast::<RecordHeader>()) };
    RecordHeader::SIZE + header.encoded_args_size as usize
}
