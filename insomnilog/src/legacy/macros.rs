//! Legacy logging macros.
//!
//! These macros are the pre-rewrite user-facing API. They are kept so the
//! legacy implementation keeps compiling alongside the new architecture. All
//! of them are prefixed with `legacy_` so they do not collide with the new
//! macros (which, like these, land at the crate root via `#[macro_export]`).

/// Core logging macro. Prefer the level-specific convenience macros.
///
/// # Examples
///
/// ```
/// use insomnilog::legacy::{Logger, LogLevel};
/// use insomnilog::legacy_log;
///
/// let logger = Logger::builder().level(LogLevel::Info).build();
/// legacy_log!(logger, LogLevel::Info, "hello {}", "world");
/// logger.flush();
/// ```
#[macro_export]
macro_rules! legacy_log {
    ($logger:expr, $level:expr, $fmt:expr $(, $arg:expr)*) => {{
        let logger: &$crate::legacy::Logger = &$logger;
        if $level >= logger.level_filter() {
            static METADATA: $crate::legacy::LogMetadata = $crate::legacy::LogMetadata {
                level: $level,
                fmt_str: $fmt,
                file: file!(),
                line: line!(),
                module_path: module_path!(),
                arg_count: $crate::_legacy_count_args!($($arg),*),
            };
            // Calculate total size: header + (1 tag byte + encoded value) per arg.
            let arg_size: usize = 0 $(+ 1 + $crate::legacy::Encode::encoded_size(&$arg))*;
            let total = $crate::legacy::_record_header_size() + arg_size;
            $crate::legacy::with_producer(logger, |producer| {
                // Silently drop if the queue is full or the record is oversized.
                let _ = producer.write(total, |buf| {
                    let ptr = buf.as_mut_ptr();
                    unsafe {
                        // Write the record header.
                        let timestamp_ns = $crate::legacy::_timestamp_ns();
                        let header = $crate::legacy::_RecordHeader {
                            timestamp_ns,
                            metadata_ptr: &METADATA as *const $crate::legacy::LogMetadata as usize,
                            encoded_args_size: arg_size as u32,
                            padding: 0,
                        };
                        core::ptr::copy_nonoverlapping(
                            (&header as *const $crate::legacy::_RecordHeader).cast::<u8>(),
                            ptr,
                            $crate::legacy::_record_header_size(),
                        );

                        // Encode each argument.
                        let mut _offset = $crate::legacy::_record_header_size();
                        $(
                            // Write tag byte.
                            *ptr.add(_offset) = $crate::legacy::Encode::tag(&$arg);
                            _offset += 1;
                            // Write encoded value.
                            _offset += $crate::legacy::Encode::encode_to(&$arg, ptr.add(_offset));
                        )*
                    }
                });
            });
        }
    }};
}

/// Logs a message at the `Trace` level.
#[macro_export]
macro_rules! legacy_log_trace {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::legacy_log!($logger, $crate::legacy::LogLevel::Trace, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Debug` level.
#[macro_export]
macro_rules! legacy_log_debug {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::legacy_log!($logger, $crate::legacy::LogLevel::Debug, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Info` level.
#[macro_export]
macro_rules! legacy_log_info {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::legacy_log!($logger, $crate::legacy::LogLevel::Info, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Warn` level.
#[macro_export]
macro_rules! legacy_log_warn {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::legacy_log!($logger, $crate::legacy::LogLevel::Warn, $fmt $(, $arg)*)
    };
}

/// Logs a message at the `Error` level.
#[macro_export]
macro_rules! legacy_log_error {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::legacy_log!($logger, $crate::legacy::LogLevel::Error, $fmt $(, $arg)*)
    };
}

/// Counts the number of arguments (internal helper macro).
#[doc(hidden)]
#[macro_export]
macro_rules! _legacy_count_args {
    () => { 0u8 };
    ($head:expr $(, $tail:expr)*) => { 1u8 + $crate::_legacy_count_args!($($tail),*) };
}
