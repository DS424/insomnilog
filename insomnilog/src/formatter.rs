//! Log record formatting.

use std::fmt::Write;

use crate::decode::{DecodedArg, DecodedRecord};

/// Formats decoded records into human-readable log lines.
///
/// Output format: `[LEVEL timestamp] file:line message`
pub struct PatternFormatter {
    /// Reusable string buffer to avoid per-record allocations.
    buf: String,
}

impl PatternFormatter {
    /// Creates a new formatter.
    pub fn new() -> Self {
        Self {
            buf: String::with_capacity(256),
        }
    }

    /// Formats a decoded record into a log line.
    ///
    /// Returns a reference to an internal buffer that is valid until the next
    /// call to `format`.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
    pub fn format(&mut self, record: &DecodedRecord) -> &str {
        self.buf.clear();

        // Timestamp: seconds.millis since UNIX epoch.
        let secs = record.timestamp_ns / 1_000_000_000;
        let millis = (record.timestamp_ns % 1_000_000_000) / 1_000_000;

        let _ = write!(
            self.buf,
            "[{} {secs}.{millis:03}] {}:{} ",
            record.metadata.level, record.metadata.file, record.metadata.line,
        );

        // Replace {} placeholders in the format string with decoded args.
        format_message(&mut self.buf, record.metadata.fmt_str, &record.args);

        &self.buf
    }
}

/// Scans `fmt_str` for `{}` placeholders and replaces them with successive
/// decoded arguments.
fn format_message(buf: &mut String, fmt_str: &str, args: &[DecodedArg]) {
    let mut arg_iter = args.iter();
    let mut chars = fmt_str.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next(); // consume '}'
            if let Some(arg) = arg_iter.next() {
                let _ = write!(buf, "{arg}");
            } else {
                buf.push_str("{}");
            }
        } else {
            buf.push(ch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::DecodedRecord;
    use crate::level::LogLevel;
    use crate::metadata::LogMetadata;

    #[test]
    fn format_basic_message() {
        static META: LogMetadata = LogMetadata {
            level: LogLevel::Info,
            fmt_str: "hello {} world {}",
            file: "test.rs",
            line: 42,
            module_path: "test",
            arg_count: 2,
        };

        let record = DecodedRecord {
            timestamp_ns: 1_700_000_000_123_000_000,
            metadata: &META,
            args: vec![DecodedArg::Str("alice".to_owned()), DecodedArg::U64(99)],
        };

        let mut fmt = PatternFormatter::new();
        let line = fmt.format(&record);
        assert!(line.contains("INFO"));
        assert!(line.contains("hello alice world 99"));
        assert!(line.contains("test.rs:42"));
    }

    #[test]
    fn format_no_args() {
        static META: LogMetadata = LogMetadata {
            level: LogLevel::Warn,
            fmt_str: "simple message",
            file: "lib.rs",
            line: 1,
            module_path: "test",
            arg_count: 0,
        };

        let record = DecodedRecord {
            timestamp_ns: 0,
            metadata: &META,
            args: vec![],
        };

        let mut fmt = PatternFormatter::new();
        let line = fmt.format(&record);
        assert!(line.contains("simple message"));
    }
}
