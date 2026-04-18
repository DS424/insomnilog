//! Static per-callsite metadata for log records.

use super::level::LogLevel;

/// Static metadata associated with a single log callsite.
///
/// Created once per `log!` macro invocation as a `static` item.
/// The backend reads this via a raw pointer stored in the record header.
pub struct LogMetadata {
    /// Severity level of this callsite.
    pub level: LogLevel,
    /// Format string with `{}` placeholders (e.g. `"user {} logged in"`).
    pub fmt_str: &'static str,
    /// Source file where the log macro was invoked.
    pub file: &'static str,
    /// Line number in the source file.
    pub line: u32,
    /// Module path of the callsite.
    pub module_path: &'static str,
    /// Number of arguments expected by the format string.
    pub arg_count: u8,
}

/// # Safety
///
/// `LogMetadata` contains only `'static` references and `Copy` types.
/// It is always created as a `static` item and never mutated after creation.
unsafe impl Sync for LogMetadata {}
