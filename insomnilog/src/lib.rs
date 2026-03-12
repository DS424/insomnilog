#![doc = include_str!("../README.md")]
#![doc = include_str!("../CHANGELOG.md")]

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
