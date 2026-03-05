//! Log level definitions.

use core::fmt;

/// Severity level for a log record.
///
/// Levels are ordered from least to most severe:
/// `Trace` < `Debug` < `Info` < `Warn` < `Error`.
///
/// Discriminants are spaced by 10 so that additional levels (e.g. a finer
/// trace granularity or a `Critical` level above `Error`) can be inserted
/// later without breaking the ordering or requiring a protocol version bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum LogLevel {
    /// Very fine-grained diagnostic information.
    Trace = 10,
    /// Diagnostic information useful during development.
    Debug = 20,
    /// General informational messages.
    Info = 30,
    /// Potentially harmful situations.
    Warn = 40,
    /// Error events that might still allow the application to continue.
    Error = 50,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trace => f.write_str("TRACE"),
            Self::Debug => f.write_str("DEBUG"),
            Self::Info => f.write_str("INFO"),
            Self::Warn => f.write_str("WARN"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}
