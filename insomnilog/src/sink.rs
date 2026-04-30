//! Output sinks for log records.
//!
//! Defines the [`Sink`] trait — the contract a sink uses to receive a
//! [`DecodedRecord`] from the backend worker — and provides a default
//! [`ConsoleSink`] that composes a [`Formatter`] with a buffered stdout
//! writer.

// Items are unused until later rewrite steps wire them up (see Plan.md).
// This `allow` is removed once `macros.rs` and the backend module use them.
#![allow(dead_code)]

use std::error::Error;
use std::fmt;
use std::io::{self, BufWriter, Stdout, Write};
use std::sync::{Mutex, PoisonError};

use crate::decode::DecodedRecord;
use crate::formatter::Formatter;
use crate::level::LogLevel;

/// Error returned by [`Sink::write_record`] and [`Sink::flush`].
///
/// The enum is `#[non_exhaustive]` so that new variants (e.g. `Network`,
/// `Database`) can be added without breaking existing match arms in downstream
/// code.
#[non_exhaustive]
pub enum SinkError {
    /// An I/O failure — covers console, file, and pipe sinks.
    Io(io::Error),
    /// Any error not yet covered by a named variant.
    Other(Box<dyn Error + Send + Sync + 'static>),
}

impl SinkError {
    /// Wraps any error that does not fit a named variant.
    pub fn other(e: impl Error + Send + Sync + 'static) -> Self {
        Self::Other(Box::new(e))
    }
}

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Other(e) => fmt::Display::fmt(e, f),
        }
    }
}

impl fmt::Debug for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => f.debug_tuple("Io").field(e).finish(),
            Self::Other(e) => f.debug_tuple("Other").field(e).finish(),
        }
    }
}

impl Error for SinkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Other(e) => Some(e.as_ref()),
        }
    }
}

impl From<io::Error> for SinkError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Receives `DecodedRecord`s from the backend worker and decides their
/// output shape.
///
/// Implementations must be [`Send`] and [`Sync`] because a single sink is
/// stored in the backend's registry as `Arc<dyn Sink>` and dispatched
/// from the worker thread.
///
/// The level returned by [`Self::level`] is fixed at construction; there is
/// no `set_level`. Implementations should typically store the level in a
/// plain field and return it directly.
///
/// **Effective filtering is `max(logger.level, sink.level)`.** The
/// producer-side filter (`logger.level`) runs first on the hot path, so a
/// sink configured *more permissive* than its logger never sees the
/// difference. To get more output through a sink, lower the logger's level,
/// not the sink's.
pub trait Sink: Send + Sync {
    /// Processes a decoded record. Called by the backend worker once per
    /// record, after the worker confirms `self.level() <= record.level`.
    ///
    /// # Errors
    ///
    /// Returns a `SinkError` if the record could not be written. The backend
    /// counts these errors and reports them at shutdown; it never propagates
    /// them to the caller.
    fn write_record(&self, record: &DecodedRecord) -> Result<(), SinkError>;

    /// Flushes any buffered output. Called by the worker after each batch
    /// of records and at shutdown.
    ///
    /// # Errors
    ///
    /// Returns a `SinkError` if the flush failed. Counted alongside
    /// write errors in the backend's shutdown report.
    fn flush(&self) -> Result<(), SinkError>;

    /// Returns the sink's filter level. Fixed at construction.
    fn level(&self) -> LogLevel;
}

/// State held under the [`ConsoleSink`] mutex: the writer plus a scratch
/// `String` reused across `write_record` calls so the sink doesn't
/// reallocate on every line.
struct ConsoleState<W: Write> {
    /// Writer receiving formatted records.
    writer: W,
    /// Scratch buffer for the formatted record. Cleared, not freed, between
    /// records so the allocation is reused.
    scratch: String,
}

/// Writes formatted records to a [`Write`] destination.
///
/// Composes a [`Formatter`] with a buffered writer. The writer plus a
/// reusable scratch `String` live behind a [`Mutex`] because
/// [`Sink::write_record`] takes `&self`; in practice the lock is
/// uncontended — sinks are usually invoked only from the backend worker
/// thread.
///
/// The writer type `W` defaults to [`BufWriter<Stdout>`], which is what
/// [`ConsoleSink::new`] produces. Use [`ConsoleSink::with_writer`] to supply
/// an alternative destination (e.g. a `Vec<u8>` in tests).
pub struct ConsoleSink<F: Formatter, W: Write = BufWriter<Stdout>> {
    /// Renders [`DecodedRecord`]s into the scratch buffer.
    formatter: F,
    /// Filter level, fixed at construction (no atomic, no `set_level`).
    level: LogLevel,
    /// Writer + scratch buffer behind a single lock so each formatted line
    /// reaches the OS as one atomic `write_all` pair.
    state: Mutex<ConsoleState<W>>,
}

impl<F: Formatter> ConsoleSink<F> {
    /// Constructs a [`ConsoleSink`] writing to a fresh [`BufWriter<Stdout>`].
    #[expect(
        clippy::use_self,
        reason = "Self here is ConsoleSink<F> but the return type is \
                  ConsoleSink<F, BufWriter<Stdout>>; they differ in W"
    )]
    pub fn new(formatter: F, level: LogLevel) -> ConsoleSink<F, BufWriter<Stdout>> {
        ConsoleSink::with_writer(formatter, level, BufWriter::new(io::stdout()))
    }
}

impl<F: Formatter, W: Write> ConsoleSink<F, W> {
    /// Constructs a [`ConsoleSink`] writing to the given `writer`.
    ///
    /// Prefer [`ConsoleSink::new`] for production use. This constructor
    /// exists mainly to allow tests to capture output without touching stdout.
    pub const fn with_writer(formatter: F, level: LogLevel, writer: W) -> Self {
        Self {
            formatter,
            level,
            state: Mutex::new(ConsoleState {
                writer,
                scratch: String::new(),
            }),
        }
    }
}

impl<F: Formatter, W: Write + Send> Sink for ConsoleSink<F, W> {
    #[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the lock must cover format + write_all so concurrent \
                  ConsoleSinks don't interleave bytes mid-line"
    )]
    fn write_record(&self, record: &DecodedRecord) -> Result<(), SinkError> {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // Destructure so the formatter's `&mut scratch` and the writer's
        // `&mut self` borrows don't collide through MutexGuard's Deref.
        let ConsoleState { writer, scratch } = &mut *guard;
        scratch.clear();
        self.formatter.format(record, scratch);
        writer.write_all(scratch.as_bytes())?;
        writer.write_all(b"\n")?;
        Ok(())
    }

    #[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
    fn flush(&self) -> Result<(), SinkError> {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        guard.writer.flush().map_err(SinkError::Io)
    }

    fn level(&self) -> LogLevel {
        self.level
    }
}

/// A no-op [`Sink`] that silently discards every record.
///
/// Useful in tests and benchmarks where output is not needed, and as a
/// placeholder when wiring up the backend before a real sink is configured.
pub struct NullSink {
    /// Filter level reported by [`Sink::level`].
    level: LogLevel,
}

impl NullSink {
    /// Creates a [`NullSink`] that accepts records at or above `level`.
    #[must_use]
    pub const fn new(level: LogLevel) -> Self {
        Self { level }
    }
}

impl Sink for NullSink {
    fn write_record(&self, _record: &DecodedRecord) -> Result<(), SinkError> {
        Ok(())
    }

    fn flush(&self) -> Result<(), SinkError> {
        Ok(())
    }

    fn level(&self) -> LogLevel {
        self.level
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::decode::{DecodedArg, DecodedRecord};
    use crate::formatter::PatternFormatter;
    use crate::metadata::LogMetadata;

    static META: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "x={}",
        file: "f.rs",
        line: 1,
        module_path: "test",
        arg_count: 1,
    };

    /// In-memory sink used to drive the trait surface in tests without
    /// touching stdout. Records each `write_record` / `flush` call so the
    /// trait API can be exercised end-to-end.
    struct CountingSink {
        /// Filter level reported by [`Sink::level`].
        level: LogLevel,
        /// Number of `write_record` calls observed.
        records: AtomicUsize,
        /// Number of `flush` calls observed.
        flushes: AtomicUsize,
        /// Levels seen by `write_record`, in order — used to assert the
        /// worker hands records to the sink in their record-level form.
        seen_levels: Mutex<Vec<LogLevel>>,
    }

    impl CountingSink {
        fn new(level: LogLevel) -> Self {
            Self {
                level,
                records: AtomicUsize::new(0),
                flushes: AtomicUsize::new(0),
                seen_levels: Mutex::new(Vec::new()),
            }
        }
    }

    impl Sink for CountingSink {
        fn write_record(&self, record: &DecodedRecord) -> Result<(), SinkError> {
            self.records.fetch_add(1, Ordering::Relaxed);
            self.seen_levels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(record.metadata.level);
            Ok(())
        }

        fn flush(&self) -> Result<(), SinkError> {
            self.flushes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn level(&self) -> LogLevel {
            self.level
        }
    }

    fn make_record() -> DecodedRecord {
        DecodedRecord {
            timestamp_ns: 0,
            metadata: &META,
            args: vec![DecodedArg::U32(7)],
        }
    }

    #[test]
    fn sink_trait_is_dyn_compatible() {
        let arc: std::sync::Arc<dyn Sink> = std::sync::Arc::new(CountingSink::new(LogLevel::Info));
        // Use the dyn reference so the coercion isn't optimised away.
        assert_eq!(arc.level(), LogLevel::Info);
    }

    #[test]
    fn sink_trait_bounds_are_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Sink>();
    }

    #[test]
    fn console_sink_level_round_trips_each_variant() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
        ] {
            let sink = ConsoleSink::new(PatternFormatter::default(), level);
            assert_eq!(sink.level(), level);
        }
    }

    #[test]
    fn console_sink_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ConsoleSink<PatternFormatter>>();
    }

    #[test]
    fn console_sink_arc_coerces_to_arc_dyn_sink() {
        let concrete: std::sync::Arc<ConsoleSink<PatternFormatter>> = std::sync::Arc::new(
            ConsoleSink::new(PatternFormatter::default(), LogLevel::Info),
        );
        let erased: std::sync::Arc<dyn Sink> = concrete;
        assert_eq!(erased.level(), LogLevel::Info);
    }

    fn make_vec_sink() -> ConsoleSink<PatternFormatter, Vec<u8>> {
        ConsoleSink::with_writer(PatternFormatter::default(), LogLevel::Info, Vec::new())
    }

    fn captured(sink: ConsoleSink<PatternFormatter, Vec<u8>>) -> String {
        let bytes = sink
            .state
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .writer;
        String::from_utf8(bytes).expect("sink output is valid UTF-8")
    }

    #[test]
    fn console_sink_write_record_appends_newline() {
        let sink = make_vec_sink();
        sink.write_record(&make_record()).unwrap();
        let out = captured(sink);
        assert!(
            out.ends_with('\n'),
            "expected trailing newline, got: {out:?}"
        );
    }

    #[test]
    fn console_sink_write_record_contains_formatted_arg() {
        let sink = make_vec_sink();
        sink.write_record(&make_record()).unwrap();
        let out = captured(sink);
        assert!(
            out.contains("x=7"),
            "expected 'x=7' in output, got: {out:?}"
        );
    }

    #[test]
    fn console_sink_write_record_accumulates_lines() {
        let sink = make_vec_sink();
        sink.write_record(&make_record()).unwrap();
        sink.write_record(&make_record()).unwrap();
        let out = captured(sink);
        // Verbatim: two identical lines from the default pattern
        // "[{level} {secs}.{millis:03}] {file}:{line} {message}"
        // with timestamp_ns=0, INFO, file="f.rs", line=1, message="x=7".
        assert_eq!(
            out,
            "[INFO 0.000] f.rs:1 x=7\n\
             [INFO 0.000] f.rs:1 x=7\n",
        );
    }

    #[test]
    fn console_sink_flush_succeeds_on_vec_writer() {
        let sink = make_vec_sink();
        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();
    }

    #[test]
    fn sink_trait_dispatch_drives_implementation() {
        let sink = CountingSink::new(LogLevel::Warning);
        let dynamic: &dyn Sink = &sink;
        assert_eq!(dynamic.level(), LogLevel::Warning);

        let record = make_record();
        dynamic.write_record(&record).unwrap();
        dynamic.write_record(&record).unwrap();
        dynamic.flush().unwrap();

        assert_eq!(sink.records.load(Ordering::Relaxed), 2);
        assert_eq!(sink.flushes.load(Ordering::Relaxed), 1);
        assert_eq!(
            sink.seen_levels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[LogLevel::Info, LogLevel::Info],
        );
    }
}
