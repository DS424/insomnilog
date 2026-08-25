//! Output sinks for log records.
//!
//! Defines the [`Sink`] trait — the contract a sink uses to receive a
//! [`LogRecord`] from the backend worker — plus the [`StreamSink`] engine
//! that composes a [`Formatter`] with a [`Write`] destination, and the
//! ready-made sinks built on it: [`ConsoleSink`], [`ContinuousFileSink`],
//! and [`SessionFileSink`].

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Stdout, Write};
use std::path::{Path, PathBuf};
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

/// Streams formatted records to one file, indefinitely.
///
/// The file is opened once at construction, in append mode, and never
/// rotated.
///
/// The parent directory must already exist — the constructors do not create
/// it.
///
/// The file grows without bounds. There is no size cap and no free-space check.
pub struct ContinuousFileSink<F: Formatter> {
    /// Shared write loop over the buffered log file.
    engine: StreamSink<F, BufWriter<File>>,
}

impl<F: Formatter> ContinuousFileSink<F> {
    /// Opens `path` for appending, creating it if needed, and streams
    /// records to it.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] if the open fails — including
    /// when `path`'s parent directory does not exist.
    pub fn try_new(formatter: F, level: LogLevel, path: impl AsRef<Path>) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        Self::try_from_options(formatter, level, path, options)
    }

    /// Same as [`Self::try_new`], but the caller supplies the
    /// [`OpenOptions`] directly — permission bits, `O_EXCL`, truncation —
    /// instead of the default append-mode open.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] if the open fails.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an owned OpenOptions matches how callers build one, and \
                  keeps room for a rotating sink to store and re-apply it"
    )]
    pub fn try_from_options(
        formatter: F,
        level: LogLevel,
        path: impl AsRef<Path>,
        options: OpenOptions,
    ) -> io::Result<Self> {
        let file = options.open(path)?;
        Ok(Self {
            engine: StreamSink::new(formatter, level, BufWriter::new(file)),
        })
    }
}

delegate_sink_to_engine!(ContinuousFileSink);

/// Streams formatted records to a fresh file per program run.
///
/// Behaves exactly like [`ContinuousFileSink`] once constructed; the
/// difference is that construction first rotates the previous session's
/// files out of the way, keeping at most `max_backups` of them. There is no
/// mid-session trigger — one session, one file.
///
/// For `path = app.log` and `max_backups = N ≥ 1`:
///
/// ```text
/// delete  app.log.N                  (drops the oldest)
/// rename  app.log.{N-1} → app.log.N
///         …
/// rename  app.log       → app.log.1
/// open    app.log                    (fresh, empty — this session)
/// ```
///
/// Every step whose source is missing is skipped rather than failing, so a
/// fresh deployment or a partially-populated backup set rotates cleanly.
/// `max_backups = 0` deletes `app.log` and creates it fresh — the live file
/// is never truncated in place, at any `N`.
///
/// Backups above `max_backups` — left over from a run configured with a
/// larger value — are never touched.
///
/// The parent directory must already exist; see [`ContinuousFileSink`].
pub struct SessionFileSink<F: Formatter> {
    /// Shared write loop over this session's buffered log file.
    engine: StreamSink<F, BufWriter<File>>,
}

#[allow(
    clippy::missing_docs_in_private_items,
    reason = "private helpers are self-explanatory"
)]
impl<F: Formatter> SessionFileSink<F> {
    /// Rotates existing logs, keeping at most `max_backups`, then opens a
    /// fresh session file at `path`.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] if a rename or delete in the
    /// rotation cascade fails, or if the fresh file cannot be opened. A
    /// failure aborts construction rather than continuing best-effort.
    pub fn try_new(
        formatter: F,
        level: LogLevel,
        path: impl AsRef<Path>,
        max_backups: usize,
    ) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        Self::try_from_options(formatter, level, path, max_backups, options)
    }

    /// Same as [`Self::try_new`], but the caller supplies the
    /// [`OpenOptions`] for *this session's* fresh file.
    ///
    /// The rotation cascade runs first either way; `options` only replaces
    /// the default append-mode open. Renamed backups keep the permissions
    /// they were created with, since `rename` moves a directory entry
    /// rather than reopening the file.
    ///
    /// # Errors
    ///
    /// Same as [`Self::try_new`].
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an owned OpenOptions matches how callers build one, and \
                  keeps room for a rotating sink to store and re-apply it"
    )]
    pub fn try_from_options(
        formatter: F,
        level: LogLevel,
        path: impl AsRef<Path>,
        max_backups: usize,
        options: OpenOptions,
    ) -> io::Result<Self> {
        let path = path.as_ref();
        Self::rotate(path, max_backups)?;
        let file = options.open(path)?;
        Ok(Self {
            engine: StreamSink::new(formatter, level, BufWriter::new(file)),
        })
    }

    fn get_backup_path_for_index(path: &Path, index: usize) -> PathBuf {
        if index == 0 {
            return path.to_path_buf();
        }
        let mut name = OsString::from(path.as_os_str());
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }

    fn remove_if_present(path: &Path) -> io::Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn rename_if_present(from: &Path, to: &Path) -> io::Result<()> {
        match std::fs::rename(from, to) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn rotate(path: &Path, max_backups: usize) -> io::Result<()> {
        Self::remove_if_present(&Self::get_backup_path_for_index(path, max_backups))?;
        for index in (1..=max_backups).rev() {
            Self::rename_if_present(
                &Self::get_backup_path_for_index(path, index - 1),
                &Self::get_backup_path_for_index(path, index),
            )?;
        }
        Ok(())
    }
}

delegate_sink_to_engine!(SessionFileSink);

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
    use std::fs;
    use std::path::PathBuf;
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

    /// Scratch directory unique to one test, removed on drop.
    ///
    /// Avoids a `tempfile` dev-dependency; the crate ships with none.
    struct TempDir {
        /// Absolute path of the created directory.
        path: PathBuf,
    }

    impl TempDir {
        /// Creates a fresh directory under the system temp dir.
        fn new(tag: &str) -> Self {
            /// Disambiguates directories created within one process.
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "insomnilog-{tag}-{pid}-{seq}",
                pid = std::process::id()
            ));
            // A leftover directory from a crashed run would poison the test.
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp dir is creatable");
            Self { path }
        }

        /// Returns `name` resolved inside this directory.
        fn join(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Reads `path` as UTF-8, failing the test if it is missing.
    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// Writes `contents` to `path`, failing the test on error.
    fn seed(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap_or_else(|e| panic!("seeding {}: {e}", path.display()));
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
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn continuous_file_sink_try_new_creates_and_writes_the_file() {
        let dir = TempDir::new("continuous-create");
        let path = dir.join("app.log");
        let sink = ContinuousFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path)
            .unwrap();

        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();

        assert_eq!(read(&path), LINE);
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn continuous_file_sink_try_new_appends_to_existing_content() {
        let dir = TempDir::new("continuous-append");
        let path = dir.join("app.log");
        seed(&path, "pre-existing\n");

        let sink = ContinuousFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path)
            .unwrap();
        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();

        assert_eq!(read(&path), format!("pre-existing\n{LINE}"));
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn continuous_file_sink_reopening_keeps_earlier_records() {
        let dir = TempDir::new("continuous-reopen");
        let path = dir.join("app.log");

        for _ in 0..2 {
            let sink =
                ContinuousFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path)
                    .unwrap();
            sink.write_record(&make_record()).unwrap();
            sink.flush().unwrap();
        }

        assert_eq!(read(&path), format!("{LINE}{LINE}"));
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn continuous_file_sink_try_new_errors_when_parent_dir_is_missing() {
        let dir = TempDir::new("continuous-no-parent");
        let path = dir.join("absent").join("app.log");

        let Err(err) =
            ContinuousFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path)
        else {
            panic!("a missing parent directory must not be created");
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn continuous_file_sink_try_from_options_honors_caller_options() {
        let dir = TempDir::new("continuous-options");
        let path = dir.join("app.log");
        seed(&path, "stale\n");

        let mut options = OpenOptions::new();
        options.write(true).truncate(true).create(true);
        let sink = ContinuousFileSink::try_from_options(
            PatternFormatter::default(),
            LogLevel::Info,
            &path,
            options,
        )
        .unwrap();
        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();

        assert_eq!(read(&path), LINE);
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn continuous_file_sink_level_round_trips_each_variant() {
        let dir = TempDir::new("continuous-level");
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
        ] {
            let path = dir.join("app.log");
            let sink =
                ContinuousFileSink::try_new(PatternFormatter::default(), level, &path).unwrap();
            assert_eq!(sink.level(), level);
        }
    }

    #[test]
    fn continuous_file_sink_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ContinuousFileSink<PatternFormatter>>();
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn continuous_file_sink_arc_coerces_to_arc_dyn_sink() {
        let dir = TempDir::new("continuous-dyn");
        let path = dir.join("app.log");
        let concrete: std::sync::Arc<ContinuousFileSink<PatternFormatter>> = std::sync::Arc::new(
            ContinuousFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path)
                .unwrap(),
        );
        let erased: std::sync::Arc<dyn Sink> = concrete;
        assert_eq!(erased.level(), LogLevel::Info);
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_try_new_on_fresh_deployment_creates_empty_live_file() {
        let dir = TempDir::new("session-fresh");
        let path = dir.join("app.log");

        let sink = SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 3)
            .unwrap();
        drop(sink);

        assert_eq!(read(&path), "");
        assert!(!dir.join("app.log.1").exists(), "no backup to create yet");
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_try_new_moves_live_file_to_first_backup() {
        let dir = TempDir::new("session-first-backup");
        let path = dir.join("app.log");
        seed(&path, "previous session\n");

        let sink = SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 2)
            .unwrap();
        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();

        assert_eq!(read(&dir.join("app.log.1")), "previous session\n");
        assert_eq!(read(&path), LINE);
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_cascade_shifts_backups_and_drops_the_oldest() {
        let dir = TempDir::new("session-cascade");
        let path = dir.join("app.log");
        seed(&path, "live\n");
        seed(&dir.join("app.log.1"), "one\n");
        seed(&dir.join("app.log.2"), "two\n");

        let sink = SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 2)
            .unwrap();
        drop(sink);

        assert_eq!(read(&dir.join("app.log.1")), "live\n");
        assert_eq!(read(&dir.join("app.log.2")), "one\n");
        assert_eq!(read(&path), "", "the live file starts empty");
        assert!(
            !dir.join("app.log.3").exists(),
            "max_backups = 2 must not create a third slot"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_skips_rename_steps_with_a_missing_source() {
        let dir = TempDir::new("session-partial");
        let path = dir.join("app.log");
        seed(&path, "live\n");
        // Deliberately no `app.log.1`: the `.1 -> .2` step has nothing to do.
        seed(&dir.join("app.log.2"), "two\n");

        let sink = SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 3)
            .unwrap();
        drop(sink);

        assert_eq!(read(&dir.join("app.log.3")), "two\n");
        assert!(!dir.join("app.log.2").exists(), "the gap shifts up with it");
        assert_eq!(read(&dir.join("app.log.1")), "live\n");
        assert_eq!(read(&path), "");
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_max_backups_zero_deletes_without_creating_a_backup() {
        let dir = TempDir::new("session-zero");
        let path = dir.join("app.log");
        seed(&path, "live\n");

        let sink = SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 0)
            .unwrap();
        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();

        assert_eq!(read(&path), LINE, "the live file is recreated, not kept");
        assert!(
            !dir.join("app.log.1").exists(),
            "max_backups = 0 keeps no backups"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_leaves_backups_above_max_untouched() {
        let dir = TempDir::new("session-shrink");
        let path = dir.join("app.log");
        seed(&path, "live\n");
        seed(&dir.join("app.log.1"), "one\n");
        seed(&dir.join("app.log.2"), "two\n");
        seed(&dir.join("app.log.3"), "three\n");

        let sink = SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 1)
            .unwrap();
        drop(sink);

        assert_eq!(read(&dir.join("app.log.1")), "live\n");
        assert_eq!(read(&dir.join("app.log.2")), "two\n", "orphan, untouched");
        assert_eq!(read(&dir.join("app.log.3")), "three\n", "orphan, untouched");
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_rotates_once_per_construction() {
        let dir = TempDir::new("session-per-run");
        let path = dir.join("app.log");

        for tag in ["first", "second"] {
            let sink =
                SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 2)
                    .unwrap();
            sink.write_record(&make_record()).unwrap();
            sink.flush().unwrap();
            drop(sink);
            assert_eq!(read(&path), LINE, "{tag} session writes one line");
        }

        assert_eq!(read(&dir.join("app.log.1")), LINE, "the first session");
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_try_from_options_still_rotates() {
        let dir = TempDir::new("session-options");
        let path = dir.join("app.log");
        seed(&path, "live\n");

        let mut options = OpenOptions::new();
        options.append(true).create(true);
        let sink = SessionFileSink::try_from_options(
            PatternFormatter::default(),
            LogLevel::Info,
            &path,
            2,
            options,
        )
        .unwrap();
        sink.write_record(&make_record()).unwrap();
        sink.flush().unwrap();

        assert_eq!(
            read(&dir.join("app.log.1")),
            "live\n",
            "the cascade runs before the fresh open, options or not"
        );
        assert_eq!(read(&path), LINE);
    }

    #[test]
    #[cfg(unix)]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_try_from_options_applies_permission_bits() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let dir = TempDir::new("session-mode");
        let path = dir.join("app.log");

        let mut options = OpenOptions::new();
        options.append(true).create(true).mode(0o400);
        let sink = SessionFileSink::try_from_options(
            PatternFormatter::default(),
            LogLevel::Info,
            &path,
            1,
            options,
        )
        .unwrap();
        drop(sink);

        // the kernel applies `mode & !umask`, and only a pathological
        // umask carrying 0o400 could yield this from a default open.
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o400, "got {mode:o}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_try_new_errors_when_parent_dir_is_missing() {
        let dir = TempDir::new("session-no-parent");
        let path = dir.join("absent").join("app.log");

        let Err(err) =
            SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 2)
        else {
            panic!("a missing parent directory must not be created");
        };
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_level_round_trips_each_variant() {
        let dir = TempDir::new("session-level");
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
        ] {
            let path = dir.join("app.log");
            let sink =
                SessionFileSink::try_new(PatternFormatter::default(), level, &path, 1).unwrap();
            assert_eq!(sink.level(), level);
        }
    }

    #[test]
    fn session_file_sink_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SessionFileSink<PatternFormatter>>();
    }

    #[test]
    #[cfg_attr(miri, ignore = "touches the real filesystem")]
    fn session_file_sink_arc_coerces_to_arc_dyn_sink() {
        let dir = TempDir::new("session-dyn");
        let path = dir.join("app.log");
        let concrete: std::sync::Arc<SessionFileSink<PatternFormatter>> = std::sync::Arc::new(
            SessionFileSink::try_new(PatternFormatter::default(), LogLevel::Info, &path, 1)
                .unwrap(),
        );
        let erased: std::sync::Arc<dyn Sink> = concrete;
        assert_eq!(erased.level(), LogLevel::Info);
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
