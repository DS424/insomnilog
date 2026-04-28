//! Binary encoding of log arguments into the SPSC queue.
//!
//! Each argument is encoded as a 1-byte [`TypeTag`] followed by the value in
//! native-endian byte order. Strings are prefixed with a `u32` length.

// Items are unused until later rewrite steps wire them up (see Plan.md).
// This `allow` is removed once `macros.rs` and the backend module use them.
#![allow(dead_code)]

use core::{fmt, ptr};

/// Discriminant tag written before each encoded argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TypeTag {
    /// `i8` value (1 byte).
    I8 = 0,
    /// `i16` value (2 bytes, native-endian).
    I16 = 1,
    /// `i32` value (4 bytes, native-endian).
    I32 = 2,
    /// `i64` value (8 bytes, native-endian).
    I64 = 3,
    /// `i128` value (16 bytes, native-endian).
    I128 = 4,
    /// `u8` value (1 byte).
    U8 = 5,
    /// `u16` value (2 bytes, native-endian).
    U16 = 6,
    /// `u32` value (4 bytes, native-endian).
    U32 = 7,
    /// `u64` value (8 bytes, native-endian).
    U64 = 8,
    /// `u128` value (16 bytes, native-endian).
    U128 = 9,
    /// `f32` value (4 bytes, native-endian bits).
    F32 = 10,
    /// `f64` value (8 bytes, native-endian bits).
    F64 = 11,
    /// `bool` value (1 byte, 0 or 1).
    Bool = 12,
    /// `&str` value (`u32` length prefix + UTF-8 bytes).
    Str = 13,
    /// `usize` value (cast to `u64`, 8 bytes, native-endian).
    Usize = 14,
    /// `isize` value (cast to `i64`, 8 bytes, native-endian).
    Isize = 15,
    /// User-defined type ([`CustomEncode`]): decoder fn pointer (`usize`,
    /// native-endian) + `u32` payload length + raw payload bytes.
    Custom = 16,
}

impl TryFrom<u8> for TypeTag {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::I8),
            1 => Ok(Self::I16),
            2 => Ok(Self::I32),
            3 => Ok(Self::I64),
            4 => Ok(Self::I128),
            5 => Ok(Self::U8),
            6 => Ok(Self::U16),
            7 => Ok(Self::U32),
            8 => Ok(Self::U64),
            9 => Ok(Self::U128),
            10 => Ok(Self::F32),
            11 => Ok(Self::F64),
            12 => Ok(Self::Bool),
            13 => Ok(Self::Str),
            14 => Ok(Self::Usize),
            15 => Ok(Self::Isize),
            16 => Ok(Self::Custom),
            _ => Err(()),
        }
    }
}

/// Trait for types that can be binary-encoded into the SPSC queue.
///
/// # Safety
///
/// Implementors must ensure that [`Encode::encode_to`] writes exactly
/// [`Encode::encoded_size`] bytes (including the 1-byte tag).
pub trait Encode {
    /// The type tag for this type.
    const TAG: TypeTag;

    /// Returns the total number of bytes needed to encode the value,
    /// including the 1-byte tag.
    fn encoded_size(&self) -> usize;

    /// Writes the tag byte followed by the encoded value to `dst`.
    ///
    /// # Safety
    ///
    /// `dst` must point to at least `encoded_size()` writable bytes.
    unsafe fn encode_to(&self, dst: *mut u8);
}

/// Function-pointer signature used by [`CustomEncode`] decoders.
///
/// The decoder receives the payload slice and writes a human-readable
/// representation into `out`, returning the formatter's `Result` so that
/// errors propagate up to the caller's `Display::fmt`.
///
/// Storage as a single fn pointer (rather than a `Fn` trait object) is what
/// allows the decoder to be embedded in the wire format as
/// `size_of::<usize>()` bytes.
pub type Decoder = fn(&[u8], &mut dyn fmt::Write) -> fmt::Result;

/// Trait for user-defined types that can be encoded into the SPSC queue.
///
/// Implementors provide a raw byte payload and a static decoder function.
/// The decoder is stored as a `usize`-wide function pointer in the encoded
/// record so the backend can recover and call it without a type-tag lookup.
///
/// Wire format (written by the blanket [`Encode`] impl):
/// ```text
/// [type tag:       1 byte (TypeTag::Custom)]
/// [decoder fn ptr: usize, native-endian]
/// [payload size: u32,   native-endian]
/// [payload bytes:  payload_size() bytes]
/// ```
///
/// # Safety
///
/// 1. `encode_payload` must write exactly `payload_size()` bytes into `dst`.
/// 2. The bytes written must be valid input for [`decoder`](Self::decoder).
///    The decoder must not index past `payload_size()` bytes — the slice it
///    receives is sized to exactly that many bytes, and indexing past the
///    end will panic in debug builds and read garbage from neighbouring
///    records in release.
pub unsafe trait CustomEncode {
    /// Number of raw payload bytes (excluding the decoder pointer and length prefix).
    fn payload_size(&self) -> usize;

    /// Writes the raw payload bytes to `dst`.
    ///
    /// # Safety
    ///
    /// `dst` must point to at least `payload_size()` writable bytes.
    unsafe fn encode_payload(&self, dst: *mut u8);

    /// Returns the static decoder function for this type.
    ///
    /// The same pointer must be returned for every instance of the same
    /// type — the backend uses it as a type identity token as well as a
    /// callable. Non-capturing closures and named `fn`s coerce
    /// deterministically; capturing closures cannot be returned here, since
    /// the return type is a bare `fn`, not `impl Fn`.
    ///
    /// The `Self: Sized` bound is intentional. The wire format identifies
    /// the type via the monomorphized fn pointer, which has no meaning
    /// behind `dyn CustomEncode`, so dynamic dispatch over this trait is
    /// not supported.
    fn decoder() -> Decoder
    where
        Self: Sized;
}

// SAFETY: CustomEncode::encode_payload writes payload_size() bytes, so the
// total written by encode_to is 1 + size_of::<usize>() + 4 + payload_size(),
// which equals encoded_size(). The fn pointer stored as a usize is recovered
// by the decoder via mem::transmute — sound because it points to a 'static fn.
impl<T: CustomEncode> Encode for T {
    const TAG: TypeTag = TypeTag::Custom;

    fn encoded_size(&self) -> usize {
        1 + size_of::<usize>() + size_of::<u32>() + self.payload_size()
    }

    unsafe fn encode_to(&self, dst: *mut u8) {
        let fn_ptr = T::decoder();
        let payload_size = self.payload_size();
        debug_assert!(
            u32::try_from(payload_size).is_ok(),
            "CustomEncode payload exceeds u32::MAX",
        );
        #[expect(
            clippy::cast_possible_truncation,
            reason = "payloads > 4 GiB are not supported"
        )]
        let payload_size_bytes = (payload_size as u32).to_ne_bytes();
        // SAFETY: caller guarantees encoded_size() bytes available.
        unsafe {
            *dst = Self::TAG as u8;
            let dst_fn = dst.add(1);
            ptr::copy_nonoverlapping(
                ptr::addr_of!(fn_ptr).cast::<u8>(),
                dst_fn,
                size_of::<Decoder>(),
            );
            let dst_len = dst_fn.add(size_of::<Decoder>());
            ptr::copy_nonoverlapping(payload_size_bytes.as_ptr(), dst_len, size_of::<u32>());
            self.encode_payload(dst_len.add(size_of::<u32>()));
        }
    }
}

/// Implements [`Encode`] for any type with `.to_ne_bytes()` and a fixed `size_of`.
///
/// Two forms:
/// - `($ty, $tag)` — encodes the value directly; on-wire size is `size_of::<$ty>()`.
/// - `($ty, $tag, cast $as)` — casts to `$as` first; on-wire size is
///   `size_of::<$as>()`. Used for `usize` / `isize` so the wire format is
///   independent of pointer width.
///
/// Both forms forward to the private `@build` arm, which owns the impl body.
macro_rules! impl_encode_ne_bytes {
    ($ty:ty, $tag:expr) => {
        impl_encode_ne_bytes!(@build $ty, $tag, $ty,);
    };
    ($ty:ty, $tag:expr, cast $as:ty) => {
        impl_encode_ne_bytes!(@build $ty, $tag, $as, as $as);
    };
    (@build $ty:ty, $tag:expr, $size_ty:ty, $($cast:tt)*) => {
        impl Encode for $ty {
            const TAG: TypeTag = $tag;

            #[inline]
            fn encoded_size(&self) -> usize {
                1 + size_of::<$size_ty>()
            }

            #[inline]
            unsafe fn encode_to(&self, dst: *mut u8) {
                let bytes = (*self $($cast)*).to_ne_bytes();
                // SAFETY: caller guarantees encoded_size() bytes available.
                unsafe {
                    *dst = Self::TAG as u8;
                    ptr::copy_nonoverlapping(bytes.as_ptr(), dst.add(1), bytes.len());
                }
            }
        }
    };
}

impl_encode_ne_bytes!(i8, TypeTag::I8);
impl_encode_ne_bytes!(i16, TypeTag::I16);
impl_encode_ne_bytes!(i32, TypeTag::I32);
impl_encode_ne_bytes!(i64, TypeTag::I64);
impl_encode_ne_bytes!(i128, TypeTag::I128);
impl_encode_ne_bytes!(u8, TypeTag::U8);
impl_encode_ne_bytes!(u16, TypeTag::U16);
impl_encode_ne_bytes!(u32, TypeTag::U32);
impl_encode_ne_bytes!(u64, TypeTag::U64);
impl_encode_ne_bytes!(u128, TypeTag::U128);
impl_encode_ne_bytes!(f32, TypeTag::F32);
impl_encode_ne_bytes!(f64, TypeTag::F64);
impl_encode_ne_bytes!(usize, TypeTag::Usize, cast u64);
impl_encode_ne_bytes!(isize, TypeTag::Isize, cast i64);

impl Encode for bool {
    const TAG: TypeTag = TypeTag::Bool;

    #[inline]
    fn encoded_size(&self) -> usize {
        2
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) {
        // SAFETY: caller guarantees encoded_size() bytes available.
        unsafe {
            *dst = Self::TAG as u8;
            *dst.add(1) = u8::from(*self);
        }
    }
}

impl Encode for &str {
    const TAG: TypeTag = TypeTag::Str;

    #[inline]
    fn encoded_size(&self) -> usize {
        1 + size_of::<u32>() + self.len()
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) {
        let len = self.len();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "strings > 4 GiB are not supported"
        )]
        let len_u32 = len as u32;
        let len_bytes = len_u32.to_ne_bytes();
        // SAFETY: caller guarantees encoded_size() bytes available.
        unsafe {
            *dst = Self::TAG as u8;
            ptr::copy_nonoverlapping(len_bytes.as_ptr(), dst.add(1), 4);
            if len > 0 {
                ptr::copy_nonoverlapping(self.as_ptr(), dst.add(5), len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem;

    /// Encodes `val` into a fresh `Vec<u8>` of size `encoded_size()` and
    /// asserts that the first byte matches the expected tag. Returns the buffer.
    macro_rules! assert_encode {
        ($val:expr, $expected_tag:expr) => {{
            let val = $val;
            let size = val.encoded_size();
            let mut buf = vec![0u8; size];
            unsafe { val.encode_to(buf.as_mut_ptr()) };
            assert_eq!(buf[0], $expected_tag as u8);
            buf
        }};
    }

    /// Roundtrip test for numeric types — compares `to_ne_bytes` to avoid
    /// `clippy::float_cmp` while keeping bitwise-exact semantics for all types.
    macro_rules! test_roundtrip {
        ($name:ident, $ty:ty, $tag:expr, $val:expr) => {
            #[test]
            fn $name() {
                let val: $ty = $val;
                let buf = assert_encode!(val, $tag);
                let recovered = <$ty>::from_ne_bytes(buf[1..].try_into().unwrap());
                assert_eq!(recovered.to_ne_bytes(), val.to_ne_bytes());
            }
        };
    }

    #[test]
    fn typetag_tryfrom_roundtrip() {
        let tags = [
            TypeTag::I8,
            TypeTag::I16,
            TypeTag::I32,
            TypeTag::I64,
            TypeTag::I128,
            TypeTag::U8,
            TypeTag::U16,
            TypeTag::U32,
            TypeTag::U64,
            TypeTag::U128,
            TypeTag::F32,
            TypeTag::F64,
            TypeTag::Bool,
            TypeTag::Str,
            TypeTag::Usize,
            TypeTag::Isize,
            TypeTag::Custom,
        ];
        for tag in tags {
            let byte = tag as u8;
            let recovered = TypeTag::try_from(byte).expect("known tag must roundtrip");
            assert_eq!(recovered, tag);
        }
        assert!(TypeTag::try_from(17).is_err());
        assert!(TypeTag::try_from(u8::MAX).is_err());
    }

    test_roundtrip!(encode_i8, i8, TypeTag::I8, i8::MIN);
    test_roundtrip!(encode_i16, i16, TypeTag::I16, -1_000_i16);
    test_roundtrip!(encode_i32, i32, TypeTag::I32, -42_i32);
    test_roundtrip!(encode_i64, i64, TypeTag::I64, i64::MIN);
    test_roundtrip!(encode_i128, i128, TypeTag::I128, i128::MIN);
    test_roundtrip!(encode_u8, u8, TypeTag::U8, u8::MAX);
    test_roundtrip!(encode_u16, u16, TypeTag::U16, u16::MAX);
    test_roundtrip!(encode_u32, u32, TypeTag::U32, u32::MAX);
    test_roundtrip!(encode_u64, u64, TypeTag::U64, u64::MAX);
    test_roundtrip!(encode_u128, u128, TypeTag::U128, u128::MAX);
    test_roundtrip!(encode_f32, f32, TypeTag::F32, core::f32::consts::PI);
    test_roundtrip!(encode_f64, f64, TypeTag::F64, core::f64::consts::E);

    #[test]
    fn encode_usize_roundtrip() {
        let val: usize = usize::MAX;
        let buf = assert_encode!(val, TypeTag::Usize);
        assert_eq!(buf.len(), 1 + size_of::<u64>());
        assert_eq!(u64::from_ne_bytes(buf[1..].try_into().unwrap()), val as u64);
    }

    #[test]
    fn encode_isize_roundtrip() {
        let val: isize = isize::MIN;
        let buf = assert_encode!(val, TypeTag::Isize);
        assert_eq!(buf.len(), 1 + size_of::<i64>());
        assert_eq!(i64::from_ne_bytes(buf[1..].try_into().unwrap()), val as i64);
    }

    #[test]
    fn encode_bool_true() {
        let buf = assert_encode!(true, TypeTag::Bool);
        assert_eq!(buf[1], 1);
    }

    #[test]
    fn encode_bool_false() {
        let buf = assert_encode!(false, TypeTag::Bool);
        assert_eq!(buf[1], 0);
    }

    #[test]
    fn encode_str_ascii() {
        let buf = assert_encode!("hello", TypeTag::Str);
        let len = u32::from_ne_bytes(buf[1..5].try_into().unwrap()) as usize;
        assert_eq!(len, 5);
        assert_eq!(&buf[5..5 + len], b"hello");
    }

    #[test]
    fn encode_str_empty() {
        let buf = assert_encode!("", TypeTag::Str);
        let len = u32::from_ne_bytes(buf[1..5].try_into().unwrap()) as usize;
        assert_eq!(len, 0);
        assert_eq!(buf.len(), 5);
    }

    #[test]
    fn encode_str_unicode() {
        let val = "héllo"; // é = 2 bytes in UTF-8, so len = 6
        let buf = assert_encode!(val, TypeTag::Str);
        let len = u32::from_ne_bytes(buf[1..5].try_into().unwrap()) as usize;
        assert_eq!(len, val.len());
        assert_eq!(&buf[5..5 + len], val.as_bytes());
    }

    use crate::testutil::{Color, Marker, Point2D};

    /// Recovers the decoder fn pointer and payload slice from a custom-encoded buffer.
    fn unpack_custom(buf: &[u8]) -> (Decoder, &[u8]) {
        let ptr_size = size_of::<Decoder>();
        let decoder: Decoder = unsafe {
            let mut slot = mem::MaybeUninit::<Decoder>::uninit();
            ptr::copy_nonoverlapping(buf.as_ptr(), slot.as_mut_ptr().cast::<u8>(), ptr_size);
            slot.assume_init()
        };
        let len = u32::from_ne_bytes(buf[ptr_size..ptr_size + 4].try_into().unwrap()) as usize;
        (decoder, &buf[ptr_size + 4..ptr_size + 4 + len])
    }

    #[test]
    fn custom_color_size_and_tag() {
        let val = Color { r: 0, g: 0, b: 0 };
        assert_eq!(
            val.encoded_size(),
            1 + size_of::<usize>() + size_of::<u32>() + 3
        );
    }

    #[test]
    fn custom_color_payload_bytes() {
        let val = Color {
            r: 255,
            g: 128,
            b: 0,
        };
        let buf = assert_encode!(val, TypeTag::Custom);
        let (_, payload) = unpack_custom(&buf[1..]);
        assert_eq!(payload, &[255, 128, 0]);
    }

    #[test]
    fn custom_color_decoder_output() {
        let val = Color {
            r: 255,
            g: 128,
            b: 0,
        };
        let buf = assert_encode!(val, TypeTag::Custom);
        let (decoder, payload) = unpack_custom(&buf[1..]);
        let mut out = String::new();
        decoder(payload, &mut out).unwrap();
        assert_eq!(out, "rgb(255, 128, 0)");
    }

    #[test]
    fn custom_point2d_payload_bytes() {
        let val = Point2D { x: 1.5, y: -3.25 };
        let buf = assert_encode!(val, TypeTag::Custom);
        let (_, payload) = unpack_custom(&buf[1..]);
        assert_eq!(
            f32::from_ne_bytes(payload[..4].try_into().unwrap()).to_bits(),
            1.5_f32.to_bits(),
        );
        assert_eq!(
            f32::from_ne_bytes(payload[4..].try_into().unwrap()).to_bits(),
            (-3.25_f32).to_bits(),
        );
    }

    #[test]
    fn custom_point2d_decoder_output() {
        let val = Point2D { x: 1.5, y: -3.25 };
        let buf = assert_encode!(val, TypeTag::Custom);
        let (decoder, payload) = unpack_custom(&buf[1..]);
        let mut out = String::new();
        decoder(payload, &mut out).unwrap();
        assert!(
            out.starts_with('('),
            "expected formatted point, got: {out:?}"
        );
        assert!(
            out.contains("1.5"),
            "expected x=1.5 in output, got: {out:?}"
        );
        assert!(
            out.contains("-3.25"),
            "expected y=-3.25 in output, got: {out:?}"
        );
    }

    /// The unpacked payload slice must have exactly `payload_size()` bytes —
    /// guards against off-by-one errors in the wire-format length prefix.
    #[test]
    fn custom_payload_slice_matches_payload_size() {
        let val = Color { r: 1, g: 2, b: 3 };
        let expected_payload_size = val.payload_size();
        let buf = assert_encode!(val, TypeTag::Custom);
        let (_, payload) = unpack_custom(&buf[1..]);
        assert_eq!(payload.len(), expected_payload_size);
    }

    #[test]
    fn custom_zero_payload_size() {
        let val = Marker;
        assert_eq!(
            val.encoded_size(),
            1 + size_of::<usize>() + size_of::<u32>()
        );
        assert_eq!(val.payload_size(), 0);
    }

    #[test]
    fn custom_zero_payload_decoder_output() {
        let val = Marker;
        let buf = assert_encode!(val, TypeTag::Custom);
        let (decoder, payload) = unpack_custom(&buf[1..]);
        assert!(payload.is_empty());
        let mut out = String::new();
        decoder(payload, &mut out).unwrap();
        assert_eq!(out, "marker");
    }

    // Miri does not model stable fn-pointer coercion addresses — each call to
    // a closure-returning fn gets a fresh identity — so this test is native-only.
    #[cfg(not(miri))]
    #[test]
    fn custom_same_type_same_decoder_ptr() {
        let c1 = Color { r: 255, g: 0, b: 0 };
        let c2 = Color { r: 0, g: 255, b: 0 };
        let mut b1 = vec![0u8; c1.encoded_size()];
        let mut b2 = vec![0u8; c2.encoded_size()];
        unsafe {
            c1.encode_to(b1.as_mut_ptr());
            c2.encode_to(b2.as_mut_ptr());
        }
        let ptr_size = size_of::<Decoder>();
        assert_eq!(
            &b1[1..=ptr_size],
            &b2[1..=ptr_size],
            "same type must always produce the same decoder fn pointer bytes"
        );
    }

    #[test]
    fn custom_different_types_different_decoder_ptrs() {
        let color = Color { r: 0, g: 0, b: 0 };
        let point = Point2D { x: 0.0, y: 0.0 };
        let mut bc = vec![0u8; color.encoded_size()];
        let mut bp = vec![0u8; point.encoded_size()];
        unsafe {
            color.encode_to(bc.as_mut_ptr());
            point.encode_to(bp.as_mut_ptr());
        }
        let ptr_size = size_of::<usize>();
        let fnc = usize::from_ne_bytes(bc[1..=ptr_size].try_into().unwrap());
        let fnp = usize::from_ne_bytes(bp[1..=ptr_size].try_into().unwrap());
        assert_ne!(
            fnc, fnp,
            "different types must produce different decoder fn pointers"
        );
    }

    // --- Realtime-safety check ---
    //
    // The helper below exercises every in-tree `Encode` and `CustomEncode`
    // impl. When built with the `rtsan` feature, `#[nonblocking]` causes
    // `RTSan` to abort the process if any of these calls allocate, lock,
    // or perform blocking I/O. Without the feature it is a plain function
    // and the wrapping `#[test]` simply verifies the impls do not panic.
    //
    // All buffers are stack-allocated — heap allocation inside the
    // nonblocking region would itself be a violation.

    /// Compile-time encoded sizes for the custom-encoded test types.
    const CUSTOM_HEADER_SIZE: usize = size_of::<usize>() + size_of::<u32>();

    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    fn exercise_all_encodes() {
        let mut b1 = [0u8; 2];
        let mut b2 = [0u8; 3];
        let mut b4 = [0u8; 5];
        let mut b8 = [0u8; 9];
        let mut b16 = [0u8; 17];
        let mut b_str = [0u8; 1 + size_of::<u32>() + 5];
        let mut b_color = [0u8; 1 + CUSTOM_HEADER_SIZE + 3];
        let mut b_point = [0u8; 1 + CUSTOM_HEADER_SIZE + 8];
        let mut b_marker = [0u8; 1 + CUSTOM_HEADER_SIZE];

        // SAFETY: every buffer is sized to the encoded value's
        // `encoded_size()` and lives for the duration of the call.
        unsafe {
            i8::MIN.encode_to(b1.as_mut_ptr());
            (-1000_i16).encode_to(b2.as_mut_ptr());
            (-42_i32).encode_to(b4.as_mut_ptr());
            i64::MIN.encode_to(b8.as_mut_ptr());
            i128::MIN.encode_to(b16.as_mut_ptr());
            u8::MAX.encode_to(b1.as_mut_ptr());
            u16::MAX.encode_to(b2.as_mut_ptr());
            u32::MAX.encode_to(b4.as_mut_ptr());
            u64::MAX.encode_to(b8.as_mut_ptr());
            u128::MAX.encode_to(b16.as_mut_ptr());
            core::f32::consts::PI.encode_to(b4.as_mut_ptr());
            core::f64::consts::E.encode_to(b8.as_mut_ptr());
            true.encode_to(b1.as_mut_ptr());
            usize::MAX.encode_to(b8.as_mut_ptr());
            isize::MIN.encode_to(b8.as_mut_ptr());
            "hello".encode_to(b_str.as_mut_ptr());

            (Color { r: 1, g: 2, b: 3 }).encode_to(b_color.as_mut_ptr());
            (Point2D { x: 1.5, y: -3.25 }).encode_to(b_point.as_mut_ptr());
            Marker.encode_to(b_marker.as_mut_ptr());
        }

        // Cover the non-`encode_to` trait surface too: `encoded_size` and
        // `payload_size` are also reachable from the hot path.
        let _ = i32::MIN.encoded_size();
        let _ = "hello".encoded_size();
        let _ = (Color { r: 0, g: 0, b: 0 }).encoded_size();
        let _ = (Color { r: 0, g: 0, b: 0 }).payload_size();
    }

    #[test]
    fn encode_methods_are_nonblocking() {
        exercise_all_encodes();
    }
}
