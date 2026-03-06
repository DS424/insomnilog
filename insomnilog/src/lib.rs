//! An asynchronous Rust logging library that never blocks.
//!
//! `insomnilog` serializes log arguments as raw bytes into per-thread lock-free
//! SPSC ring buffers. A dedicated backend thread reads, decodes, formats, and
//! writes to the console. The logging hot path performs **no allocations** and
//! **never blocks**.
//!
//! # Quick Start
//!
//! ```
//! use insomnilog::{Logger, LogLevel, log_info, log_warn};
//!
//! let logger = Logger::builder()
//!     .level(LogLevel::Info)
//!     .queue_capacity(128 * 1024)
//!     .build();
//!
//! log_info!(logger, "application started");
//! log_info!(logger, "user {} logged in with id {}", "alice", 42_u64);
//! log_warn!(logger, "disk usage at {}%", 87.5_f64);
//!
//! logger.flush();
//! ```

mod backend;
mod decode;
mod encode;
mod formatter;
mod frontend;
pub mod level;
#[macro_use]
pub mod macros;
pub mod metadata;
mod queue;
mod sink;

// Public re-exports.
pub use encode::Encode;
pub use frontend::{Logger, LoggerBuilder, with_producer};
pub use level::LogLevel;
pub use metadata::LogMetadata;

// Re-exports used by macros (hidden from docs).
#[doc(hidden)]
pub use decode::RecordHeader as _RecordHeader;
#[doc(hidden)]
pub use queue::Producer as _Producer;

/// Returns the size of a [`RecordHeader`](decode::RecordHeader) in bytes.
///
/// Used by the logging macros to calculate buffer sizes.
#[doc(hidden)]
#[inline]
#[must_use]
pub const fn _record_header_size() -> usize {
    decode::RecordHeader::SIZE
}

/// Returns the current timestamp in nanoseconds since the UNIX epoch.
///
/// Used by the logging macros to stamp each record.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn _timestamp_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "u64 nanos won't overflow until year 2554"
        )]
        let ns = d.as_nanos() as u64;
        ns
    })
}
