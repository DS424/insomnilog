//! Basic usage of `insomnilog`: build a logger and emit records at every level.

use insomnilog::{LogLevel, Logger, log_debug, log_error, log_info, log_trace, log_warn};

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

    log_trace!(logger, "trace message");
    log_debug!(logger, "debug: checking value {}", 123_i32);
    log_info!(logger, "application started");
    log_info!(logger, "user {} logged in with id {}", "alice", 42_u64);
    log_warn!(logger, "disk usage at {}%", 87.5_f64);
    log_error!(logger, "connection lost to host {}", "db-primary");
    log_info!(logger, "bool test: {}", true);
    log_info!(logger, "i128 test: {}", 999_999_999_999_999_i128);

    logger.flush();
}
