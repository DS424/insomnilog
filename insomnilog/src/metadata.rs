//! Static per-callsite metadata for log records.

use crate::level::LogLevel;

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

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: LogMetadata = LogMetadata {
        level: LogLevel::Info,
        fmt_str: "user {} logged in",
        file: "src/main.rs",
        line: 42,
        module_path: "my_crate::auth",
        arg_count: 1,
    };

    #[test]
    fn fields_are_accessible() {
        assert_eq!(FIXTURE.level, LogLevel::Info);
        assert_eq!(FIXTURE.fmt_str, "user {} logged in");
        assert_eq!(FIXTURE.file, "src/main.rs");
        assert_eq!(FIXTURE.line, 42);
        assert_eq!(FIXTURE.module_path, "my_crate::auth");
        assert_eq!(FIXTURE.arg_count, 1);
    }

    #[test]
    fn is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<LogMetadata>();
    }

    #[test]
    fn static_metadata_is_valid() {
        static META: LogMetadata = LogMetadata {
            level: LogLevel::Error,
            fmt_str: "fatal: {}",
            file: file!(),
            line: line!(),
            module_path: module_path!(),
            arg_count: 1,
        };
        assert_eq!(META.level, LogLevel::Error);
        assert_eq!(META.arg_count, 1);
    }
}
