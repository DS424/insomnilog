#![doc = include_str!("../README.md")]
#![doc = include_str!("../CHANGELOG.md")]

mod backend;
mod decode;
mod encode;
pub mod legacy;
mod level;
mod lifecycle;
mod metadata;
mod queue;
mod record;

#[cfg(test)]
pub(crate) mod testutil;

pub use backend::BackendOptions;
pub use lifecycle::{AlreadyStarted, ShutdownGuard, shutdown, start};
