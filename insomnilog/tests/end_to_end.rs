//! End-to-end integration tests
//!
//! Each test calls `start()` at the top; `cargo nextest run` gives every test
//! a fresh process so the global `OnceLock<Backend>` starts clean.

use std::sync::Arc;
use std::thread;

use std::time::Duration;

use std::fmt;

use insomnilog::{
    BackendOptions, ConsoleSink, CustomEncode, Decoder, LogLevel, Logger, NullSink,
    PatternFormatter, Sink, create_logger, log_debug, log_error, log_info, log_trace, log_warn,
    preallocate_thread, register_sink, start,
};

fn fast_options() -> BackendOptions {
    BackendOptions {
        idle_sleep: Duration::from_micros(10),
        idle_yield_rounds: 0,
        // 512 KiB holds ~1 440 iterations (364 bytes/iter × 1 000 = 364 000 bytes)
        // so the sequential_logging test can never overflow before shutdown drains.
        queue_capacity: 512 * 1024,
        ..BackendOptions::default()
    }
}

/// Returns the bytes captured by a `ConsoleSink<_, Vec<u8>>` as a `String`.
fn sink_output(sink: &ConsoleSink<PatternFormatter, Vec<u8>>) -> String {
    String::from_utf8(sink.captured_output()).expect("sink output is valid UTF-8")
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
fn log_info_no_args_emits_one_info_record_with_empty_args() {
    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::default(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let guard = start(fast_options()).expect("start");
    let logger = create_logger(
        "macro_no_args",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger");

    log_info!(logger, "hi");
    drop(guard);

    let output = sink_output(&sink);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one record must be emitted");
    assert!(
        lines[0].contains("[INFO "),
        "line must carry the INFO level marker"
    );
    assert!(
        lines[0].ends_with("hi"),
        "message must be 'hi' with no substituted args"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
fn log_info_with_args_encodes_args_in_order() {
    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::default(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let guard = start(fast_options()).expect("start");
    let logger = create_logger(
        "macro_with_args",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger");

    log_info!(logger, "x={} y={}", 1u32, "z");
    drop(guard);

    let output = sink_output(&sink);
    assert!(
        output.lines().next().unwrap().ends_with("x=1 y=z"),
        "args must be decoded and substituted in declaration order"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
fn macro_accepts_arc_logger_and_ref_logger() {
    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::default(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let guard = start(fast_options()).expect("start");
    let logger = create_logger(
        "macro_deref",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger");

    log_info!(logger, "via Arc");
    log_info!(&*logger, "via ref");
    drop(guard);

    assert_eq!(
        sink_output(&sink).lines().count(),
        2,
        "both invocations must emit a record"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
fn silent_drop_on_queue_full_causes_no_panic() {
    let opts = BackendOptions {
        // 64 bytes ≈ 2 zero-arg records (RecordHeader::SIZE = 32).
        queue_capacity: 64,
        idle_sleep: Duration::from_micros(10),
        idle_yield_rounds: 0,
        ..BackendOptions::default()
    };
    let sink = Arc::new(NullSink::new(LogLevel::Trace));
    let guard = start(opts).expect("start");
    let logger = create_logger(
        "macro_queue_full",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger");

    for _ in 0..1_000 {
        insomnilog::log_info!(logger, "fill the queue");
    }
    drop(guard);
    // Reaching this point without panicking is the success criterion.
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "{logger} and {message} are PatternFormatter field placeholders, not Rust format args"
)]
fn logger_identity_reaches_sink_as_correct_name() {
    // Use a pattern that includes {logger} so the name is visible in the output.
    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::new("{logger} {message}").unwrap(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let guard = start(fast_options()).expect("start");
    let logger = create_logger(
        "macro_ptr_roundtrip",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger");

    log_info!(logger, "logger name test");
    drop(guard);

    let output = sink_output(&sink);
    assert_eq!(
        output.lines().next().unwrap(),
        "macro_ptr_roundtrip logger name test",
        "backend must resolve the macro-written logger pointer to the registered name"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
fn all_five_level_macros_emit_correct_levels() {
    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::default(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let guard = start(fast_options()).expect("start");
    let logger = create_logger(
        "macro_all_levels",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Trace,
    )
    .expect("create_logger");

    log_trace!(logger, "t");
    log_debug!(logger, "d");
    log_info!(logger, "i");
    log_warn!(logger, "w");
    log_error!(logger, "e");
    drop(guard);

    let output = sink_output(&sink);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 5, "all five level macros must emit a record");
    assert!(lines[0].contains("[TRACE "), "first record must be TRACE");
    assert!(lines[1].contains("[DEBUG "), "second record must be DEBUG");
    assert!(lines[2].contains("[INFO "), "third record must be INFO");
    assert!(
        lines[3].contains("[WARNING "),
        "fourth record must be WARNING"
    );
    assert!(lines[4].contains("[ERROR "), "fifth record must be ERROR");
}

/// 3-D position with double-precision coordinates.
pub struct Pose3d {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Z coordinate.
    pub z: f64,
}

unsafe impl CustomEncode for Pose3d {
    fn payload_size(&self) -> usize {
        24 // 3 × 8 bytes (f64 little-endian)
    }

    unsafe fn encode_payload(&self, dst: *mut u8) {
        // SAFETY: caller guarantees 24 bytes available.
        unsafe {
            std::ptr::copy_nonoverlapping(self.x.to_le_bytes().as_ptr(), dst, 8);
            std::ptr::copy_nonoverlapping(self.y.to_le_bytes().as_ptr(), dst.add(8), 8);
            std::ptr::copy_nonoverlapping(self.z.to_le_bytes().as_ptr(), dst.add(16), 8);
        }
    }

    fn decoder() -> Decoder {
        |bytes, out: &mut dyn fmt::Write| {
            let x = f64::from_le_bytes(bytes[0..8].try_into().unwrap());
            let y = f64::from_le_bytes(bytes[8..16].try_into().unwrap());
            let z = f64::from_le_bytes(bytes[16..24].try_into().unwrap());
            write!(out, "Pose3d({x}, {y}, {z})")
        }
    }
}

#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
#[expect(
    clippy::cast_precision_loss,
    reason = "test values are small enough that f64 cast is exact"
)]
fn log_all_levels(logger: &Logger, iteration: u64) {
    log_trace!(logger, "trace message");
    log_debug!(logger, "debug: checking value {}", 123_i32);
    log_info!(logger, "application started");
    log_info!(logger, "user {} logged in with id {}", "alice", 42_u64);
    log_warn!(logger, "disk usage at {}%", 87.5_f64);
    log_error!(logger, "connection lost to host {}", "db-primary");
    log_info!(logger, "bool test: {}", true);
    log_info!(logger, "i128 test: {}", 999_999_999_999_999_i128);
    log_info!(logger, "iteration {}", iteration);
    let pose = Pose3d {
        x: iteration as f64,
        y: (iteration * 2) as f64,
        z: (iteration * 3) as f64,
    };
    log_info!(logger, "pose: {}", pose);
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "{logger} and {message} are PatternFormatter field placeholders, not Rust format args"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "test values are small enough that f64 cast is exact"
)]
fn sequential_logging() {
    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::new("{level} {logger} {message}").unwrap(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));

    let guard = start(fast_options()).expect("start must succeed");
    register_sink("e2e_main", Arc::clone(&sink) as Arc<dyn Sink>)
        .expect("register_sink must succeed");
    let logger = create_logger(
        "app",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger must succeed");

    preallocate_thread();

    for i in 0..1000_u64 {
        log_all_levels(&logger, i);
        thread::sleep(Duration::from_micros(10)); //Simulate some calculations and allow backend to work
    }

    // Drop the guard to call shutdown(), which drains all queues before returning.
    drop(guard);

    let output = sink_output(&sink);
    let lines: Vec<&str> = output.lines().collect();

    // trace and debug are filtered by the logger's LogLevel::Info threshold;
    // the remaining 8 records per iteration must arrive in emission order.
    let expected: Vec<String> = (0..1000_u64)
        .flat_map(|i| {
            vec![
                format!("INFO app application started"),
                format!("INFO app user alice logged in with id 42"),
                format!("WARNING app disk usage at 87.5%"),
                format!("ERROR app connection lost to host db-primary"),
                format!("INFO app bool test: true"),
                format!("INFO app i128 test: 999999999999999"),
                format!("INFO app iteration {i}"),
                format!(
                    "INFO app pose: Pose3d({}, {}, {})",
                    i as f64,
                    (i * 2) as f64,
                    (i * 3) as f64
                ),
            ]
        })
        .collect();

    assert_eq!(
        lines.len(),
        expected.len(),
        "all {} records must reach the sink after shutdown drain",
        expected.len(),
    );
    for (idx, (actual, exp)) in lines.iter().zip(expected.iter()).enumerate() {
        assert_eq!(*actual, exp, "record {idx} must match exactly",);
    }
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "{logger} and {message} are PatternFormatter field placeholders, not Rust format args"
)]
fn sequential_logging_full_speed() {
    // Small queue forces drops; the point is that the hot path remains
    // realtime-safe (no allocations, no locks) even when records are lost.
    let opts = BackendOptions {
        queue_capacity: 2048,
        idle_sleep: Duration::from_micros(10),
        idle_yield_rounds: 0,
        ..BackendOptions::default()
    };

    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::new("{level} {logger} {message}").unwrap(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));

    let guard = start(opts).expect("start must succeed");
    register_sink("e2e_full_speed", Arc::clone(&sink) as Arc<dyn Sink>)
        .expect("register_sink must succeed");
    let logger = create_logger(
        "app",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger must succeed");

    preallocate_thread();

    for i in 0..1000_u64 {
        log_all_levels(&logger, i);
    }

    drop(guard);

    // 8 records per iteration × 1 000 iterations = 8 000 if none were dropped.
    // With a 2 KiB queue and no sleep, the queue must overflow, so we assert
    // that fewer records arrived — confirming drops occurred without a crash.
    let lines = sink_output(&sink);
    let line_count = lines.lines().count();
    assert!(
        line_count < 8000,
        "some records must have been dropped with a 2 KiB queue at full speed, but got {line_count}"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "{logger} and {message} are PatternFormatter field placeholders, not Rust format args"
)]
fn two_threads_parallel_logging() {
    const RECORDS_PER_THREAD: u32 = 100;

    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::new("{logger} {message}").unwrap(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));

    let guard = start(fast_options()).expect("start must succeed");
    register_sink("e2e_main", Arc::clone(&sink) as Arc<dyn Sink>)
        .expect("register_sink must succeed");
    let logger = create_logger(
        "app",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger must succeed");

    let handles: Vec<_> = (0..2)
        .map(|_| {
            let logger = Arc::clone(&logger);
            thread::spawn(move || {
                for n in 0..RECORDS_PER_THREAD {
                    log_info!(logger, "hello {}", n);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("worker thread must not panic");
    }

    // Drop the guard to call shutdown(), which drains all queues before returning.
    drop(guard);

    let output = sink_output(&sink);
    let lines: Vec<&str> = output.lines().collect();
    let expected = 2 * RECORDS_PER_THREAD as usize;
    assert_eq!(
        lines.len(),
        expected,
        "all {expected} records must reach the sink after shutdown drain",
    );
    for line in &lines {
        assert!(
            line.starts_with("app "),
            "every record must carry the correct logger name (logger_ptr round-trip)",
        );
    }
}

#[test]
#[cfg_attr(miri, ignore = "Low Miri ROI")]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "{logger} and {message} are PatternFormatter field placeholders, not Rust format args"
)]
fn two_loggers_fan_records_to_respective_sinks_only() {
    const RECORDS_PER_LOGGER: u32 = 50;

    let sink_a = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::new("{logger} {message}").unwrap(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let sink_b = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::new("{logger} {message}").unwrap(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));

    let guard = start(fast_options()).expect("start must succeed");
    let logger_a = create_logger(
        "app_a",
        vec![Arc::clone(&sink_a) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create logger_a must succeed");
    let logger_b = create_logger(
        "app_b",
        vec![Arc::clone(&sink_b) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create logger_b must succeed");

    let handle_a = {
        let logger = Arc::clone(&logger_a);
        thread::spawn(move || {
            for n in 0..RECORDS_PER_LOGGER {
                insomnilog::log_info!(logger, "from_a {}", n);
            }
        })
    };
    let handle_b = {
        let logger = Arc::clone(&logger_b);
        thread::spawn(move || {
            for n in 0..RECORDS_PER_LOGGER {
                insomnilog::log_info!(logger, "from_b {}", n);
            }
        })
    };

    handle_a.join().expect("thread A must not panic");
    handle_b.join().expect("thread B must not panic");

    // Drop the guard to call shutdown(), which drains before returning.
    drop(guard);

    let output_a = sink_output(&sink_a);
    let output_b = sink_output(&sink_b);
    let lines_a: Vec<&str> = output_a.lines().collect();
    let lines_b: Vec<&str> = output_b.lines().collect();

    assert_eq!(
        lines_a.len(),
        RECORDS_PER_LOGGER as usize,
        "sink_a must receive exactly {RECORDS_PER_LOGGER} records",
    );
    assert_eq!(
        lines_b.len(),
        RECORDS_PER_LOGGER as usize,
        "sink_b must receive exactly {RECORDS_PER_LOGGER} records",
    );
    for line in &lines_a {
        assert!(
            line.starts_with("app_a "),
            "sink_a must only receive records from logger app_a",
        );
    }
    for line in &lines_b {
        assert!(
            line.starts_with("app_b "),
            "sink_b must only receive records from logger app_b",
        );
    }
}

/// Thin Miri-targeted test: verifies that the `&'static LogMetadata` pointer
/// written by the macro as a `usize` and cast back by the backend is sound,
/// and that the encode→queue→decode pipeline contains no UB.
///
/// All other integration tests are ignored under Miri — they duplicate this
/// coverage at much higher cost.
#[test]
fn miri_e2e_metadata_pointer_roundtrip() {
    let sink = Arc::new(ConsoleSink::with_writer(
        PatternFormatter::default(),
        LogLevel::Trace,
        Vec::<u8>::new(),
    ));
    let guard = start(fast_options()).expect("start");
    let logger = create_logger(
        "miri_probe",
        vec![Arc::clone(&sink) as Arc<dyn Sink>],
        LogLevel::Info,
    )
    .expect("create_logger");

    log_info!(logger, "probe {} {}", 42_u32, "hello");
    drop(guard);

    let output = sink_output(&sink);
    assert!(
        output.lines().next().unwrap().ends_with("probe 42 hello"),
        "metadata pointer round-trip and encode/decode must be sound"
    );
}
