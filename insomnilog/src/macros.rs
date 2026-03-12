//! Logging macros.
//!
//! These macros provide the primary user-facing API for emitting log records.
//! Each macro invocation creates a `static` `LogMetadata` at the callsite
//! and encodes the arguments directly into the per-thread SPSC queue.

/// Core logging macro. Prefer the level-specific convenience macros.
///
/// # Examples
///
/// ```
/// use insomnilog::{Logger, LogLevel, log};
///
/// let logger = Logger::builder().level(LogLevel::Info).build();
/// log!(logger, LogLevel::Info, "hello {}", "world");
/// logger.flush();
/// ```
#[macro_export]
macro_rules! log {
    ($logger:expr, $level:expr, $fmt:expr $(, $arg:expr)*) => {{
        let logger: &$crate::Logger = &$logger;
        if $level >= logger.level_filter() {
            static METADATA: $crate::LogMetadata = $crate::LogMetadata {
                level: $level,
                fmt_str: $fmt,
                file: file!(),
                line: line!(),
                module_path: module_path!(),
                arg_count: $crate::_count_args!($($arg),*),
            };
            // Calculate total size: header + (1 tag byte + encoded value) per arg.
            let arg_size: usize = 0 $(+ 1 + $crate::Encode::encoded_size(&$arg))*;
            let total = $crate::_record_header_size() + arg_size;
            $crate::with_producer(logger, |producer| {
                if let Some(ptr) = producer.try_reserve(total) {
                    unsafe {
                        // Write the record header.
                        let timestamp_ns = $crate::_timestamp_ns();
                        let header = $crate::_RecordHeader {
                            timestamp_ns,
                            metadata_ptr: &METADATA as *const $crate::LogMetadata as usize,
                            encoded_args_size: arg_size as u32,
                            padding: 0,
                        };
                        core::ptr::copy_nonoverlapping(
                            (&header as *const $crate::_RecordHeader).cast::<u8>(),
                            ptr,
                            $crate::_record_header_size(),
                        );

                        // Encode each argument.
                        let mut _offset = $crate::_record_header_size();
                        $(
                            // Write tag byte.
                            *ptr.add(_offset) = $crate::Encode::tag(&$arg);
                            _offset += 1;
                            // Write encoded value.
                            _offset += $crate::Encode::encode_to(&$arg, ptr.add(_offset));
                        )*

                        producer.commit(total);
                    }
                }
                // else: silently drop
            });
        }
    }};
}

/// Logs a message at the `Trace` level.
#[macro_export]
macro_rules! log_trace {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::log!($logger, $crate::LogLevel::Trace, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Debug` level.
#[macro_export]
macro_rules! log_debug {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::log!($logger, $crate::LogLevel::Debug, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Info` level.
#[macro_export]
macro_rules! log_info {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::log!($logger, $crate::LogLevel::Info, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Warn` level.
#[macro_export]
macro_rules! log_warn {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::log!($logger, $crate::LogLevel::Warn, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Error` level.
#[macro_export]
macro_rules! log_error {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::log!($logger, $crate::LogLevel::Error, $fmt $(, $arg)*)
    };
}

/// Counts the number of arguments (internal helper macro).
#[doc(hidden)]
#[macro_export]
macro_rules! _count_args {
    () => { 0u8 };
    ($head:expr $(, $tail:expr)*) => { 1u8 + $crate::_count_args!($($tail),*) };
}
