//! Default usage test

use insomnilog::legacy::{LogLevel, Logger};
use insomnilog::{
    legacy_log_debug, legacy_log_error, legacy_log_info, legacy_log_trace, legacy_log_warn,
};

/// Exercise all log-level macros.
///
/// When built with the `rtsan` feature, the `#[nonblocking]` attribute causes
/// `RTSan` to abort the process if any of these calls allocate, lock a mutex,
/// or perform blocking I/O — the core contract of insomnilog's hot path.
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
fn log_all_levels(logger: &Logger) {
    legacy_log_trace!(logger, "trace {} {}", 1i32, true);
    legacy_log_debug!(logger, "debug {}", 1.5f64);
    legacy_log_info!(logger, "info {}", "hello world");
    legacy_log_warn!(logger, "warn {}", 100u64);
    legacy_log_error!(logger, "error {} {}", -1i64, 255u8);
}

#[test]
fn default_usage() {
    let logger = Logger::builder().level(LogLevel::Trace).build();

    // Explicit thread initialisation outside any real-time context.
    // Without this call the per-thread queue is created lazily on the first
    // log statement, which RTSan would correctly flag as a violation.
    logger.preallocate();

    // Both calls exercise the pure hot path (under RTSan scrutiny when enabled).
    log_all_levels(&logger);
    log_all_levels(&logger);

    logger.flush();
}
