//! An asynchronous Rust logging library that never blocks.
//!
//! `insomnilog` serializes log arguments as raw bytes into per-thread lock-free
//! SPSC ring buffers. A dedicated backend thread reads, decodes, formats, and
//! writes to the console. The logging hot path performs **no allocations** and
//! **never blocks**.

mod decode;
mod encode;
pub mod level;
pub mod metadata;

pub use encode::Encode;
pub use level::LogLevel;
pub use metadata::LogMetadata;
