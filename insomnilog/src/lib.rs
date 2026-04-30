#![doc = include_str!("../README.md")]
#![doc = include_str!("../CHANGELOG.md")]

mod backend;
mod decode;
mod encode;
mod formatter;
pub mod legacy;
mod level;
mod lifecycle;
mod metadata;
mod queue;
mod record;
mod sink;

#[cfg(test)]
pub(crate) mod testutil;

pub use backend::BackendOptions;
pub use formatter::{Formatter, InvalidPatternError, PatternFormatter};
pub use lifecycle::{AlreadyStarted, ShutdownGuard, shutdown, start};
pub use sink::{ConsoleSink, NullSink, Sink};
