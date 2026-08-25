//! Output sinks for log records.
//!
//! Defines the [`Sink`] trait — the contract a sink uses to receive a
//! [`LogRecord`] from the backend worker — and provides a default
//! [`ConsoleSink`] that composes a [`Formatter`] with a buffered stdout
//! writer.

// Items are unused until later rewrite steps wire them up (see Plan.md).
// This `allow` is removed once `macros.rs` and the backend module use them.
#![allow(dead_code)]

use std::error::Error;
use std::fmt;
use std::io::{self, BufWriter, Stdout, Write};
use std::sync::{Mutex, PoisonError};

use crate::decode::LogRecord;
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

/// Receives `LogRecord`s from the backend worker and decides their
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
    /// Processes a log record. Called by the backend worker once per
    /// record, after the worker confirms `self.level() <= record.level`.
    ///
    /// # Errors
    ///
    /// Returns a `SinkError` if the record could not be written. The backend
    /// counts these errors and reports them at shutdown; it never propagates
    /// them to the caller.
    fn write_record(&self, record: &LogRecord) -> Result<(), SinkError>;

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

/// Formats `record` into `scratch` and writes it, plus a trailing newline,
/// to `writer`.
///
/// The single place the on-the-wire line shape is decided, so every sink emits
/// byte-identical output.
///
/// # Errors
///
/// Forwards the writer's I/O error.
fn format_line(
    formatter: &impl Formatter,
    scratch: &mut String,
    record: &LogRecord,
    writer: &mut dyn Write,
) -> io::Result<()> {
    scratch.clear();
    formatter.format(record, scratch);
    writer.write_all(scratch.as_bytes())?;
    writer.write_all(b"\n")
}

/// State held under the [`StreamSink`] mutex: the writer plus a scratch
/// `String` reused across `write_record` calls so the sink doesn't
/// reallocate on every line.
struct StreamState<W: Write> {
    /// Writer receiving formatted records.
    writer: W,
    /// Scratch buffer for the formatted record. Cleared, not freed, between
    /// records so the allocation is reused.
    scratch: String,
}

/// Streams formatted records to a [`Write`] destination.
///
/// Composes a [`Formatter`] with a writer. The writer plus a reusable
/// scratch `String` live behind a [`Mutex`] because [`Sink::write_record`]
/// takes `&self`; in practice the lock is uncontended — sinks are usually
/// invoked only from the backend worker thread.
///
/// This is the shared engine behind more specific sinks types:
/// each of them holds a `StreamSink` and forwards
/// the three [`Sink`] methods to it, so there is exactly one write loop.
/// Use it directly to stream to a destination none of those cover — an
/// in-memory `Vec<u8>` in tests, a pipe, a socket.
pub struct StreamSink<F: Formatter, W: Write> {
    /// Renders [`LogRecord`]s into the scratch buffer.
    formatter: F,
    /// Filter level, fixed at construction (no atomic, no `set_level`).
    level: LogLevel,
    /// Writer + scratch buffer behind a single lock so each formatted line
    /// reaches the OS as one atomic `write_all` pair.
    state: Mutex<StreamState<W>>,
}

impl<F: Formatter, W: Write> StreamSink<F, W> {
    /// Constructs a [`StreamSink`] writing to the given `writer`.
    pub const fn new(formatter: F, level: LogLevel, writer: W) -> Self {
        Self {
            formatter,
            level,
            state: Mutex::new(StreamState {
                writer,
                scratch: String::new(),
            }),
        }
    }
}

impl<F: Formatter> StreamSink<F, Vec<u8>> {
    /// Returns a copy of the bytes written to the sink so far.
    pub fn captured_output(&self) -> Vec<u8> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .writer
            .clone()
    }
}

impl<F: Formatter, W: Write + Send> Sink for StreamSink<F, W> {
    #[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the lock must cover format + write_all so concurrent \
                  StreamSinks don't interleave bytes mid-line"
    )]
    fn write_record(&self, record: &LogRecord) -> Result<(), SinkError> {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // Destructure so the formatter's `&mut scratch` and the writer's
        // `&mut self` borrows don't collide through MutexGuard's Deref.
        let StreamState { writer, scratch } = &mut *guard;
        format_line(&self.formatter, scratch, record, writer)?;
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

/// Implements [`Sink`] for a sink that holds its write loop in an
/// `engine: StreamSink<F, _>` field and adds no per-record behavior of its
/// own.
///
/// Generating the three forwarders leaves one code path to test rather than
/// one hand-written impl per sink — see `delegated_sink_*` in the tests,
/// which drive the same expansion through an observable writer.
///
/// A sink that must act *inside* `write_record` — a rotating sink, which
/// owns its own lock over writer + rotation state — writes its own impl and
/// calls [`format_line`] directly instead of using this.
macro_rules! delegate_sink_to_engine {
    ($sink:ident) => {
        impl<F: Formatter> Sink for $sink<F> {
            fn write_record(&self, record: &LogRecord) -> Result<(), SinkError> {
                self.engine.write_record(record)
            }

            fn flush(&self) -> Result<(), SinkError> {
                self.engine.flush()
            }

            fn level(&self) -> LogLevel {
                self.engine.level()
            }
        }
    };
}

/// Writes formatted records to standard output.
///
/// Wraps a [`StreamSink`] over a [`BufWriter<Stdout>`]; the backend worker
/// flushes it after each batch and at shutdown.
pub struct ConsoleSink<F: Formatter> {
    /// Shared write loop over buffered stdout.
    engine: StreamSink<F, BufWriter<Stdout>>,
}

impl<F: Formatter> ConsoleSink<F> {
    /// Constructs a [`ConsoleSink`] writing to a fresh [`BufWriter<Stdout>`].
    pub fn new(formatter: F, level: LogLevel) -> Self {
        Self {
            engine: StreamSink::new(formatter, level, BufWriter::new(io::stdout())),
        }
    }
}

delegate_sink_to_engine!(ConsoleSink);

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
    fn write_record(&self, _record: &LogRecord) -> Result<(), SinkError> {
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
    use crate::decode::{DecodedArg, LogRecord};
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
        fn write_record(&self, record: &LogRecord) -> Result<(), SinkError> {
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

    fn make_record() -> LogRecord {
        LogRecord {
            timestamp_ns: 0,
            logger_name: "test".to_owned(),
            metadata: &META,
            args: vec![DecodedArg::U32(7)],
        }
    }

    /// The single line the default pattern produces for [`make_record`].
    const LINE: &str = "[INFO 0.000] f.rs:1 x=7\n";

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

    /// Holds the engine over an observable writer and delegates through the
    /// same macro as the shipping sinks, so the generated forwarders can be
    /// asserted on.
    struct DelegatingProbe<F: Formatter> {
        /// Shared write loop over an in-memory buffer.
        engine: StreamSink<F, Vec<u8>>,
    }

    delegate_sink_to_engine!(DelegatingProbe);

    fn make_probe(level: LogLevel) -> DelegatingProbe<PatternFormatter> {
        DelegatingProbe {
            engine: StreamSink::new(PatternFormatter::default(), level, Vec::new()),
        }
    }

    #[test]
    fn delegated_sink_write_record_reaches_the_engine_writer() {
        let sink = make_probe(LogLevel::Info);
        sink.write_record(&make_record()).unwrap();
        assert_eq!(
            String::from_utf8(sink.engine.captured_output()).unwrap(),
            LINE
        );
    }

    #[test]
    fn delegated_sink_flush_emits_no_output_of_its_own() {
        let sink = make_probe(LogLevel::Info);
        sink.flush().unwrap();

        assert!(sink.engine.captured_output().is_empty());
    }

    #[test]
    fn delegated_sink_level_reports_the_engine_level() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
        ] {
            let sink = make_probe(level);
            assert_eq!(sink.level(), level);
        }
    }

    fn make_vec_sink() -> StreamSink<PatternFormatter, Vec<u8>> {
        StreamSink::new(PatternFormatter::default(), LogLevel::Info, Vec::new())
    }

    fn captured(sink: StreamSink<PatternFormatter, Vec<u8>>) -> String {
        let bytes = sink
            .state
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .writer;
        String::from_utf8(bytes).expect("sink output is valid UTF-8")
    }

    #[test]
    fn stream_sink_write_record_appends_newline() {
        let sink = make_vec_sink();
        sink.write_record(&make_record()).unwrap();
        let out = captured(sink);
        assert!(
            out.ends_with('\n'),
            "expected trailing newline, got: {out:?}"
        );
    }

    #[test]
    fn stream_sink_write_record_contains_formatted_arg() {
        let sink = make_vec_sink();
        sink.write_record(&make_record()).unwrap();
        let out = captured(sink);
        assert!(
            out.contains("x=7"),
            "expected 'x=7' in output, got: {out:?}"
        );
    }

    #[test]
    fn stream_sink_write_record_accumulates_lines() {
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
    fn stream_sink_flush_succeeds_on_vec_writer() {
        let sink = make_vec_sink();
        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();
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

    #[test]
    fn console_sink_new_writes_one_record_without_panicking() {
        // Smoke test only: the real stdout wiring can't be observed from
        // in-process, so this just proves `new` builds a usable sink and a
        // record can be pushed through it.
        let sink = ConsoleSink::new(PatternFormatter::default(), LogLevel::Info);
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

    #[test]
    fn log_record_logger_name_is_accessible_from_sink() {
        // Sinks must be able to read logger_name without any unsafe code.
        let record = LogRecord {
            timestamp_ns: 0,
            logger_name: "payments".to_owned(),
            metadata: &META,
            args: vec![],
        };
        // A sink can branch on or include the logger name in its output.
        assert_eq!(record.logger_name, "payments");
    }
}
