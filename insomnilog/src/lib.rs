#![doc = include_str!("../README.md")]
#![doc = include_str!("../CHANGELOG.md")]

mod encode;
pub mod legacy;
mod level;
mod metadata;
mod queue;
mod record;

#[cfg(test)]
pub(crate) mod testutil;
