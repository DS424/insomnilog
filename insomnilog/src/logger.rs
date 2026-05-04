//! Named logger holding a pinned sink list and an atomic level.
//!
//! A [`Logger`] is constructed via
//! `crate::create_logger` and returned to callers
//! as `Arc<Logger>`. The backend's `LoggerRegistry` holds the authoritative
//! strong reference for the process lifetime, so any `*const Logger` written
//! into a record header remains valid for the duration the record
//! may be read by the worker.
//!
//! ## Mutability
//!
//! `name` and `sinks` are fixed at construction. Only `level` can change at
//! runtime, and it is stored in an [`AtomicU8`] so the macro hot path
//! can load it with a `Relaxed` read.

// Items are unused until later rewrite steps wire them up (see Plan.md).
// This `allow` is removed once `macros.rs` and the backend module use them.
#![allow(dead_code)]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::level::LogLevel;
use crate::sink::Sink;

/// Named logger: a registered, identity-bearing object holding a pinned list
/// of sinks plus an atomic level filter.
///
/// Construct one via `crate::create_logger`; callers receive an
/// `Arc<Logger>`. The backend's logger registry holds a strong `Arc<Logger>`
/// for the process lifetime, so dropping every caller-side clone leaves the
/// logger alive and reachable via `crate::get_logger`.
///
/// Two filters run in series at log time:
///
/// 1. The macro drops records below `logger.level` (hot path, [`AtomicU8`]
///    `Relaxed` load).
/// 2. The backend then skips sinks below `sink.level` per surviving record.
///
/// So a sink's effective level is `max(logger.level, sink.level)` — a sink
/// configured *more permissive* than its logger never receives the
/// difference. To get more output through a sink, lower this `level`, not
/// the sink's.
pub struct Logger {
    /// Registry name. Fixed at construction; used purely for identification
    /// in lookups and printing logger name in sinks.
    name: String,
    /// Sinks that receive each record this logger emits, in declaration
    /// order. Fixed at construction.
    sinks: Vec<Arc<dyn Sink>>,
    /// Filter level. Stored as a `LogLevel` discriminant (see
    /// [`LogLevel`]).
    level: AtomicU8,
}

impl Logger {
    /// Constructs a new [`Logger`].
    ///
    /// Visible only inside the crate: the only construction path exposed to
    /// users is [`crate::create_logger`], which routes through the
    /// `LoggerRegistry`. Going around the registry would defeat the
    /// raw-pointer-in-record-header dispatch model.
    pub(crate) fn new(name: String, sinks: Vec<Arc<dyn Sink>>, level: LogLevel) -> Self {
        Self {
            name,
            sinks,
            level: AtomicU8::new(level as u8),
        }
    }

    /// Returns the name this logger was registered under.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the sink list, in declaration order. The slice is fixed at
    /// construction; no mutation API exists.
    ///
    /// The backend's per-record dispatch loop iterates this slice
    /// directly via the `*const Logger` in the record header — no locks, no
    /// lookups.
    #[must_use]
    pub fn sinks(&self) -> &[Arc<dyn Sink>] {
        &self.sinks
    }

    /// Returns the current filter level (`Relaxed` atomic load).
    ///
    /// # Panics
    ///
    /// Panics if the stored byte does not round-trip back to a [`LogLevel`].
    /// This indicates either memory corruption or a discriminant added to
    /// [`LogLevel`] without updating its [`TryFrom<u8>`] impl — both
    /// programmer errors, not runtime conditions.
    #[must_use]
    pub fn level(&self) -> LogLevel {
        let raw = self.level.load(Ordering::Relaxed);
        LogLevel::try_from(raw).expect(
            "Logger.level only ever holds a value written from a valid LogLevel; \
             a stored byte that does not round-trip indicates memory corruption \
             or an unhandled LogLevel variant",
        )
    }

    /// Updates the filter level (`Relaxed` atomic store).
    ///
    /// `Relaxed` matches the load in [`Self::level`]: this field gates
    /// emission, it does not synchronise with anything else, so paying for
    /// `Acquire`/`Release` would buy nothing.
    pub fn set_level(&self, level: LogLevel) {
        self.level.store(level as u8, Ordering::Relaxed);
    }
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("name", &self.name)
            .field("level", &self.level())
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;
    use crate::sink::NullSink;

    fn stub(level: LogLevel) -> Arc<dyn Sink> {
        Arc::new(NullSink::new(level))
    }

    #[test]
    fn name_round_trips_through_construction() {
        let logger = Logger::new("app".to_owned(), Vec::new(), LogLevel::Info);
        assert_eq!(logger.name(), "app");
    }

    #[test]
    fn level_round_trips_each_variant() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warning,
            LogLevel::Error,
        ] {
            let logger = Logger::new("x".to_owned(), Vec::new(), level);
            assert_eq!(logger.level(), level);
        }
    }

    #[test]
    fn set_level_updates_level() {
        let logger = Logger::new("x".to_owned(), Vec::new(), LogLevel::Info);
        assert_eq!(logger.level(), LogLevel::Info);
        logger.set_level(LogLevel::Error);
        assert_eq!(logger.level(), LogLevel::Error);
        logger.set_level(LogLevel::Trace);
        assert_eq!(logger.level(), LogLevel::Trace);
    }

    #[test]
    fn sinks_round_trip_through_construction() {
        let a = stub(LogLevel::Info);
        let b = stub(LogLevel::Error);
        let logger = Logger::new(
            "two".to_owned(),
            vec![Arc::clone(&a), Arc::clone(&b)],
            LogLevel::Info,
        );
        let stored = logger.sinks();
        assert_eq!(stored.len(), 2);
        assert!(Arc::ptr_eq(&stored[0], &a));
        assert!(Arc::ptr_eq(&stored[1], &b));
    }

    #[test]
    fn logger_is_send_and_sync() {
        // Required because the
        // registry hands `Arc<Logger>` across threads and the backend
        // dispatches from its worker thread
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Logger>();
    }

    #[test]
    fn set_level_is_visible_across_threads() {
        let logger = Arc::new(Logger::new("shared".to_owned(), Vec::new(), LogLevel::Info));
        let setter = Arc::clone(&logger);
        thread::spawn(move || {
            setter.set_level(LogLevel::Error);
        })
        .join()
        .expect("spawned thread must not panic");
        assert_eq!(logger.level(), LogLevel::Error);
    }

    #[test]
    fn logger_pins_sinks_until_dropped() {
        let sink = stub(LogLevel::Info);
        let weak = Arc::downgrade(&sink);
        let logger = Logger::new("pin".to_owned(), vec![sink], LogLevel::Info);
        assert!(
            weak.upgrade().is_some(),
            "logger must hold a strong ref to its sinks while alive"
        );
        drop(logger);
        assert!(
            weak.upgrade().is_none(),
            "dropping the logger must release its sinks"
        );
    }
}
