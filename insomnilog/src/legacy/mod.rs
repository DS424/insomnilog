//! Legacy (original) insomnilog implementation.
//!
//! This module preserves the pre-rewrite architecture during the transition to
//! the new backend/sink/logger separation described in `Plan.md`. It compiles
//! and runs unchanged so that existing tests and examples keep working as the
//! new modules are built alongside it.
//!
//! New code should not depend on anything in this module — it will be removed
//! once the rewrite is complete.

mod backend;
mod decode;
mod encode;
mod formatter;
mod frontend;
mod level;
pub mod macros;
mod metadata;
mod queue;
mod sink;

// Public re-exports.
pub use encode::Encode;
pub use frontend::{Logger, LoggerBuilder, with_producer};
pub use level::LogLevel;
pub use metadata::LogMetadata;

// Re-exports used by the legacy macros (hidden from docs).
#[doc(hidden)]
pub use decode::RecordHeader as _RecordHeader;
#[doc(hidden)]
pub use queue::Producer as _Producer;

// Re-exports used by compile-fail doctests (hidden from docs).
#[doc(hidden)]
pub use queue::Consumer as _Consumer;
#[doc(hidden)]
pub use queue::new as _queue_new;

/// Returns the size of a [`RecordHeader`](decode::RecordHeader) in bytes.
///
/// Used by the legacy logging macros to calculate buffer sizes.
#[doc(hidden)]
#[inline]
#[must_use]
pub const fn _record_header_size() -> usize {
    decode::RecordHeader::SIZE
}

/// Returns the current timestamp in nanoseconds since the UNIX epoch.
///
/// Used by the legacy logging macros to stamp each record.
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
