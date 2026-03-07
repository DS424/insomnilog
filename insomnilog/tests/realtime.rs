//! Real-time safety contract test.
//!
//! Run via: `just realtime-sanitize`

#![cfg(feature = "rtsan")]

use insomnilog::{LogLevel, Logger, log_debug, log_error, log_info, log_trace, log_warn};

/// All logging macros must be callable from a `#[nonblocking]` (real-time) context.
///
/// `RTSan` will abort the process if any of these calls allocate, lock a mutex,
/// or perform blocking I/O — the core contract of insomnilog's hot path.
#[rtsan_standalone::nonblocking]
fn log_all_levels(logger: &Logger) {
    log_trace!(logger, "trace {} {}", 1i32, true);
    log_debug!(logger, "debug {}", 1.5f64);
    log_info!(logger, "info {}", "hello world");
    log_warn!(logger, "warn {}", 100u64);
    log_error!(logger, "error {} {}", -1i64, 255u8);
}

#[test]
fn hot_path_is_realtime_safe() {
    let logger = Logger::builder().level(LogLevel::Trace).build();

    // Explicit thread initialisation outside any real-time context.
    // Without this call the per-thread queue is created lazily on the first
    // log statement, which RTSan would correctly flag as a violation.
    logger.preallocate();

    // Both calls exercise the pure hot path under RTSan scrutiny.
    log_all_levels(&logger);
    log_all_levels(&logger);

    logger.flush();
}
