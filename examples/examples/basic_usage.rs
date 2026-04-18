//! Basic usage of `insomnilog`: build a logger and emit records at every level.

use insomnilog::legacy::{LogLevel, Logger};
use insomnilog::{
    legacy_log_debug, legacy_log_error, legacy_log_info, legacy_log_trace, legacy_log_warn,
};

fn main() {
    let logger = Logger::builder()
        .level(LogLevel::Trace)
        .queue_capacity(128 * 1024)
        .build();

    // Pre-allocate the per-thread queue before entering the logging hot path.
    // This ensures all subsequent log calls are real-time safe (zero allocation,
    // zero locks). Without this call the allocation happens lazily on the first
    // log statement instead.
    logger.preallocate();

    legacy_log_trace!(logger, "trace message");
    legacy_log_debug!(logger, "debug: checking value {}", 123_i32);
    legacy_log_info!(logger, "application started");
    legacy_log_info!(logger, "user {} logged in with id {}", "alice", 42_u64);
    legacy_log_warn!(logger, "disk usage at {}%", 87.5_f64);
    legacy_log_error!(logger, "connection lost to host {}", "db-primary");
    legacy_log_info!(logger, "bool test: {}", true);
    legacy_log_info!(logger, "i128 test: {}", 999_999_999_999_999_i128);

    logger.flush();
}
