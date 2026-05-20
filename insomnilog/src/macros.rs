//! Log macros: `log_trace!`, `log_debug!`, `log_info!`, `log_warn!`, `log_error!`.
//!
//! All five wrappers delegate to the private `__log!` macro, which assembles the
//! level-check → static metadata → SPSC queue write sequence on the hot path.

/// Internal helpers used exclusively by `__log!` macro expansions.
///
/// This module is `#[doc(hidden)]` and semver-exempt; its API may change at
/// any time without notice.
#[doc(hidden)]
pub mod macros_internal {
    pub use crate::encode::Encode;
    pub use crate::metadata::LogMetadata;
    pub use crate::record::RecordHeader;

    /// Returns the current time as nanoseconds since the UNIX epoch.
    #[must_use]
    pub fn timestamp_ns() -> u64 {
        use std::time::SystemTime;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "timestamps past year 2554 are not a supported use case"
        )]
        let ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        ns
    }

    /// Calls `f` with a raw pointer to `total_size` bytes reserved in the
    /// calling thread's SPSC producer queue.
    ///
    /// Silently discards the write if the queue is full or oversized, but
    /// increments the backend's dropped-records counter so the loss is
    /// reported at shutdown.
    pub fn log_write(total_size: usize, f: impl FnOnce(*mut u8)) {
        crate::frontend::with_producer(|p| {
            if p.write(total_size, |buf| f(buf.as_mut_ptr())).is_err() {
                crate::lifecycle::get_backend().increment_dropped_records();
            }
        });
    }
}

/// Counts macro arguments at compile time.  Internal helper for `__log!`.
///
/// Uses recursive `macro_rules!` expansion so the result is a `const u8`
/// suitable for [`crate::macros_internal::LogMetadata::arg_count`].
#[doc(hidden)]
#[macro_export]
macro_rules! __count_args {
    () => { 0u8 };
    ($head:expr $(, $tail:expr)*) => { 1u8 + $crate::__count_args!($($tail),*) };
}

/// Core log macro.  All five level-named wrappers delegate here.
///
/// # Hot-path contract
///
/// 1. Atomic `Relaxed` load of the logger level — branch-predict-friendly.
/// 2. Static `METADATA` defined once per call-site (zero runtime cost after
///    the first branch).
/// 3. `macros_internal::log_write` → `Producer::write` →
///    `ptr::copy_nonoverlapping` into the reserved buffer.  No allocation,
///    no lock, no `format!`.
/// 4. `QueueFull` is silently discarded; the caller never sees an error.
#[doc(hidden)]
#[macro_export]
macro_rules! __log {
    ($logger:expr, $level:expr, $fmt:expr $(, $arg:expr)*) => {{
        // Coerce Arc<Logger> / &Logger → &Logger via Deref chain.
        let logger_ref: &$crate::Logger = &*$logger;
        if $level >= logger_ref.level() {
            static METADATA: $crate::macros_internal::LogMetadata =
                $crate::macros_internal::LogMetadata {
                    level: $level,
                    fmt_str: $fmt,
                    file: file!(),
                    line: line!(),
                    module_path: module_path!(),
                    arg_count: $crate::__count_args!($($arg),*),
                };
            // Total bytes = fixed header + encoded payload (tag included in encoded_size).
            #[expect(
                clippy::cast_possible_truncation,
                reason = "log records larger than 4 GiB are not supported"
            )]
            let encoded_args_size: u32 =
                (0usize $(+ $crate::macros_internal::Encode::encoded_size(&$arg))*)
                    as u32;
            let total_size =
                $crate::macros_internal::RecordHeader::SIZE + encoded_args_size as usize;
            // log_write silently swallows QueueFull — caller never sees an error.
            $crate::macros_internal::log_write(total_size, |buf_ptr| {
                // SAFETY: `buf_ptr` points to `total_size` writable bytes
                // reserved by the SPSC producer.  Each `encode_to` call writes
                // exactly `encoded_size()` bytes past the current offset.
                unsafe {
                    let header = $crate::macros_internal::RecordHeader::new(
                        $crate::macros_internal::timestamp_ns(),
                        core::ptr::from_ref(&METADATA) as usize,
                        core::ptr::from_ref(logger_ref) as usize,
                        encoded_args_size,
                    );
                    header.write_to(buf_ptr);
                    // `#[allow(unused_mut)]` is needed because in the zero-arg
                    // case the repetition body never executes so `offset` is
                    // never mutated.  `let _ = offset` at the end prevents both
                    // `unused_variables` (zero-arg) and `unused_assignments`
                    // (dead store after the last iteration).
                    #[allow(unused_mut)]
                    let mut offset = $crate::macros_internal::RecordHeader::SIZE;
                    $(
                        // encode_to writes the tag byte + encoded value.
                        let arg_size =
                            $crate::macros_internal::Encode::encoded_size(&$arg);
                        $crate::macros_internal::Encode::encode_to(
                            &$arg,
                            buf_ptr.add(offset),
                        );
                        offset += arg_size;
                    )*
                    let _ = offset;
                }
            });
        }
    }};
}

/// Logs a message at the [`Trace`][crate::LogLevel::Trace] level.
#[macro_export]
macro_rules! log_trace {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::__log!($logger, $crate::LogLevel::Trace, $fmt $(, $arg)*)
    };
}

/// Logs a message at the [`Debug`][crate::LogLevel::Debug] level.
#[macro_export]
macro_rules! log_debug {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::__log!($logger, $crate::LogLevel::Debug, $fmt $(, $arg)*)
    };
}

/// Logs a message at the [`Info`][crate::LogLevel::Info] level.
#[macro_export]
macro_rules! log_info {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::__log!($logger, $crate::LogLevel::Info, $fmt $(, $arg)*)
    };
}

/// Logs a message at the [`Warning`][crate::LogLevel::Warning] level.
#[macro_export]
macro_rules! log_warn {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::__log!($logger, $crate::LogLevel::Warning, $fmt $(, $arg)*)
    };
}

/// Logs a message at the [`Error`][crate::LogLevel::Error] level.
#[macro_export]
macro_rules! log_error {
    ($logger:expr, $fmt:expr $(, $arg:expr)*) => {
        $crate::__log!($logger, $crate::LogLevel::Error, $fmt $(, $arg)*)
    };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::backend::BackendOptions;
    use crate::level::LogLevel;
    use crate::lifecycle;
    use crate::sink::Sink;
    use crate::testutil::RecordingSink;

    fn fast_options() -> BackendOptions {
        BackendOptions {
            idle_sleep: Duration::from_micros(10),
            idle_yield_rounds: 0,
            ..BackendOptions::default()
        }
    }

    #[test]
    fn record_below_logger_level_is_not_emitted_and_skips_with_producer() {
        let sink = Arc::new(RecordingSink::new(LogLevel::Trace));
        let _guard = lifecycle::start(fast_options()).expect("start");
        let backend = lifecycle::get_backend();
        let logger = lifecycle::create_logger(
            "macro_level_filter",
            vec![Arc::clone(&sink) as Arc<dyn Sink>],
            LogLevel::Warning,
        )
        .expect("create_logger");

        // Spawn so the test thread's TLS slot stays clean.
        let logger_arc = Arc::clone(&logger);
        std::thread::spawn(move || {
            // Info < Warning → level check must suppress before with_producer.
            log_info!(logger_arc, "below level, must be dropped");
        })
        .join()
        .expect("spawned thread must not panic");

        // The spawned thread must not have registered a producer context.
        assert_eq!(
            backend.consumer_count(),
            0,
            "level check must prevent with_producer from being called"
        );
        assert_eq!(
            sink.record_count(),
            0,
            "no records must be emitted below the logger level"
        );
    }
}
