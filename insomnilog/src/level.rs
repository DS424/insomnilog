//! Log level definitions.

use core::fmt;

/// Severity level for a log record.
///
/// Levels are ordered from least to most severe:
/// `Trace` < `Debug` < `Info` < `Warning` < `Error`.
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
    Warning = 40,
    /// Error events that might still allow the application to continue.
    Error = 50,
}

impl TryFrom<u8> for LogLevel {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, u8> {
        match value {
            10 => Ok(Self::Trace),
            20 => Ok(Self::Debug),
            30 => Ok(Self::Info),
            40 => Ok(Self::Warning),
            50 => Ok(Self::Error),
            other => Err(other),
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trace => f.write_str("TRACE"),
            Self::Debug => f.write_str("DEBUG"),
            Self::Info => f.write_str("INFO"),
            Self::Warning => f.write_str("WARNING"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_u8_roundtrip() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
        ] {
            assert_eq!(LogLevel::try_from(level as u8), Ok(level));
        }
    }

    #[test]
    fn try_from_u8_rejects_invalid_discriminants() {
        for bad in [0u8, 5, 15, 25, 35, 45, 51, 255] {
            assert_eq!(LogLevel::try_from(bad), Err(bad));
        }
    }

    #[test]
    fn ordering_least_to_most_severe() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Error);
    }

    #[test]
    fn display_labels() {
        assert_eq!(LogLevel::Trace.to_string(), "TRACE");
        assert_eq!(LogLevel::Debug.to_string(), "DEBUG");
        assert_eq!(LogLevel::Info.to_string(), "INFO");
        assert_eq!(LogLevel::Warning.to_string(), "WARNING");
        assert_eq!(LogLevel::Error.to_string(), "ERROR");
    }
}
