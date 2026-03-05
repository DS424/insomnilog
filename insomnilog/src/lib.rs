//! An asynchronous Rust logging library that never blocks.
//!
//! `insomnilog` serializes log arguments as raw bytes into per-thread lock-free
//! SPSC ring buffers. A dedicated backend thread reads, decodes, formats, and
//! writes to the console. The logging hot path performs **no allocations** and
//! **never blocks**.

mod backend;
mod decode;
mod encode;
mod formatter;
pub mod level;
pub mod metadata;
mod queue;
mod sink;

pub use encode::Encode;
pub use level::LogLevel;
pub use metadata::LogMetadata;
