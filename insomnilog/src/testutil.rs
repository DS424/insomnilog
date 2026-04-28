//! Fixture types used by the `encode` and `decode` test suites.

use core::{fmt, ptr};

use crate::encode::{CustomEncode, Decoder};

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
