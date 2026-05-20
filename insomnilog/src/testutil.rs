//! Fixture types used by unit test suites across the crate.

use core::{fmt, ptr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{thread, time::Duration};

use crate::decode::LogRecord;
use crate::encode::{CustomEncode, Decoder};
use crate::level::LogLevel;
use crate::sink::{Sink, SinkError};

/// RGB colour — exercises a three-byte custom payload.
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

unsafe impl CustomEncode for Color {
    fn payload_size(&self) -> usize {
        3
    }

    unsafe fn encode_payload(&self, dst: *mut u8) {
        // SAFETY: caller guarantees 3 bytes available.
        unsafe {
            *dst = self.r;
            *dst.add(1) = self.g;
            *dst.add(2) = self.b;
        }
    }

    fn decoder() -> Decoder {
        |bytes, out: &mut dyn fmt::Write| {
            write!(out, "rgb({}, {}, {})", bytes[0], bytes[1], bytes[2])
        }
    }
}

/// 2-D point — exercises an eight-byte custom payload.
pub struct Point2D {
    /// X coordinate.
    pub x: f32,
    /// Y coordinate.
    pub y: f32,
}

unsafe impl CustomEncode for Point2D {
    fn payload_size(&self) -> usize {
        8
    }

    unsafe fn encode_payload(&self, dst: *mut u8) {
        let xb = self.x.to_ne_bytes();
        let yb = self.y.to_ne_bytes();
        // SAFETY: caller guarantees 8 bytes available.
        unsafe {
            ptr::copy_nonoverlapping(xb.as_ptr(), dst, 4);
            ptr::copy_nonoverlapping(yb.as_ptr(), dst.add(4), 4);
        }
    }

    fn decoder() -> Decoder {
        |bytes, out: &mut dyn fmt::Write| {
            let x = f32::from_ne_bytes(bytes[..4].try_into().unwrap());
            let y = f32::from_ne_bytes(bytes[4..].try_into().unwrap());
            write!(out, "({x}, {y})")
        }
    }
}

/// Zero-payload custom type — exercises the degenerate
/// `payload_size() == 0` path through the blanket `Encode` impl.
pub struct Marker;

unsafe impl CustomEncode for Marker {
    fn payload_size(&self) -> usize {
        0
    }

    unsafe fn encode_payload(&self, _dst: *mut u8) {}

    fn decoder() -> Decoder {
        |_bytes, out: &mut dyn fmt::Write| write!(out, "marker")
    }
}

/// Spins calling `pred` until it returns `true` or `timeout` elapses.
pub fn spin_until(pred: impl Fn() -> bool, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if pred() {
            return true;
        }
        thread::yield_now();
    }
    false
}

/// A [`Sink`] that counts every record it receives, for use in tests.
pub struct RecordingSink {
    level: LogLevel,
    count: AtomicUsize,
}

impl RecordingSink {
    /// Creates a new sink that accepts records at or above `level`.
    #[must_use]
    pub const fn new(level: LogLevel) -> Self {
        Self {
            level,
            count: AtomicUsize::new(0),
        }
    }

    /// Returns the number of records received so far.
    pub fn record_count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

impl Sink for RecordingSink {
    fn write_record(&self, _record: &LogRecord) -> Result<(), SinkError> {
        self.count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn flush(&self) -> Result<(), SinkError> {
        Ok(())
    }

    fn level(&self) -> LogLevel {
        self.level
    }
}
