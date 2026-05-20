#![doc = include_str!("../README.md")]
#![doc = include_str!("../CHANGELOG.md")]

mod backend;
mod backend_runner;
mod decode;
mod encode;
mod formatter;
mod frontend;
pub mod legacy;
mod level;
mod lifecycle;
mod logger;
mod macros;
mod metadata;
mod per_thread_queue;
mod queue;
mod record;
mod sink;

#[cfg(test)]
pub(crate) mod testutil;

pub use backend::{BackendOptions, LoggerAlreadyRegistered, SinkAlreadyRegistered};
pub use encode::{CustomEncode, Decoder};
pub use formatter::{Formatter, InvalidPatternError, PatternFormatter};
pub use level::LogLevel;
pub use lifecycle::{
    AlreadyStarted, ShutdownGuard, create_logger, get_logger, get_sink, preallocate_thread,
    register_sink, shutdown, start,
};
pub use logger::Logger;
pub use sink::{ConsoleSink, NullSink, Sink};

// `#[macro_export]` macros reference `$crate::macros_internal`; re-export it
// here so the path resolves at the crate root.
#[doc(hidden)]
pub use macros::macros_internal;
