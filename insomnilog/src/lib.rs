//! An asynchronous Rust logging library that never blocks.
//!
//! `insomnilog` serializes log arguments as raw bytes into per-thread lock-free
//! SPSC ring buffers. A dedicated backend thread reads, decodes, formats, and
//! writes to the console. The logging hot path performs **no allocations** and
//! **never blocks**.

pub mod level;
pub mod metadata;

pub use level::LogLevel;
pub use metadata::LogMetadata;
