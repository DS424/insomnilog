//! Decoding of binary-encoded log arguments from the SPSC queue.

// Items are unused until later rewrite steps wire them up (see Plan.md).
// This `allow` is removed once `macros.rs` and the backend module use them.
#![allow(dead_code)]

use core::fmt;
use core::mem;
use core::ptr;

use super::encode::{Decoder, TypeTag};
use super::metadata::LogMetadata;
use super::record::RecordHeader;

/// Error returned when decoding a binary log record fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The byte slice was too short to contain a complete field.
    BufferTooShort,
    /// The type-tag byte did not match any known [`TypeTag`].
    UnknownTag(u8),
    /// The string payload was not valid UTF-8.
    InvalidUtf8,
    /// A custom [`Decoder`] function returned a formatting error.
    DecoderFailed,
}

/// A single decoded argument from the binary stream.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedArg {
    /// Decoded `i8`.
    I8(i8),
    /// Decoded `i16`.
    I16(i16),
    /// Decoded `i32`.
    I32(i32),
    /// Decoded `i64`.
    I64(i64),
    /// Decoded `i128`.
    I128(i128),
    /// Decoded `u8`.
    U8(u8),
    /// Decoded `u16`.
    U16(u16),
    /// Decoded `u32`.
    U32(u32),
    /// Decoded `u64`.
    U64(u64),
    /// Decoded `u128`.
    U128(u128),
    /// Decoded `f32`.
    F32(f32),
    /// Decoded `f64`.
    F64(f64),
    /// Decoded `bool`.
    Bool(bool),
    /// Decoded `String` (owned copy of the original `&str`).
    Str(String),
    /// Decoded `usize` (stored as `u64`).
    Usize(u64),
    /// Decoded `isize` (stored as `i64`).
    Isize(i64),
    /// Decoded user-defined type: the output of its [`Decoder`] function,
    /// eagerly formatted into an owned `String` at decode time.
    Custom(String),
}

impl fmt::Display for DecodedArg {
    #[allow(clippy::match_same_arms)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::I8(v) => write!(f, "{v}"),
            Self::I16(v) => write!(f, "{v}"),
            Self::I32(v) => write!(f, "{v}"),
            Self::I64(v) => write!(f, "{v}"),
            Self::I128(v) => write!(f, "{v}"),
            Self::U8(v) => write!(f, "{v}"),
            Self::U16(v) => write!(f, "{v}"),
            Self::U32(v) => write!(f, "{v}"),
            Self::U64(v) => write!(f, "{v}"),
            Self::U128(v) => write!(f, "{v}"),
            Self::F32(v) => write!(f, "{v}"),
            Self::F64(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::Usize(v) => write!(f, "{v}"),
            Self::Isize(v) => write!(f, "{v}"),
            Self::Custom(v) => write!(f, "{v}"),
        }
    }
}

/// Decodes one argument from `data`, returning the decoded arg and the number
/// of bytes consumed (including the 1-byte tag).
///
/// # Errors
///
/// Returns [`DecodeError`] if the data is too short or the tag is unknown.
fn decode_one(data: &[u8]) -> Result<(DecodedArg, usize), DecodeError> {
    let (&tag_byte, rest) = data.split_first().ok_or(DecodeError::BufferTooShort)?;
    match TypeTag::try_from(tag_byte).map_err(|()| DecodeError::UnknownTag(tag_byte))? {
        TypeTag::I8 => decode_fixed::<1>(rest, |b| DecodedArg::I8(i8::from_ne_bytes(b))),
        TypeTag::I16 => decode_fixed::<2>(rest, |b| DecodedArg::I16(i16::from_ne_bytes(b))),
        TypeTag::I32 => decode_fixed::<4>(rest, |b| DecodedArg::I32(i32::from_ne_bytes(b))),
        TypeTag::I64 => decode_fixed::<8>(rest, |b| DecodedArg::I64(i64::from_ne_bytes(b))),
        TypeTag::I128 => decode_fixed::<16>(rest, |b| DecodedArg::I128(i128::from_ne_bytes(b))),
        TypeTag::U8 => decode_fixed::<1>(rest, |b| DecodedArg::U8(u8::from_ne_bytes(b))),
        TypeTag::U16 => decode_fixed::<2>(rest, |b| DecodedArg::U16(u16::from_ne_bytes(b))),
        TypeTag::U32 => decode_fixed::<4>(rest, |b| DecodedArg::U32(u32::from_ne_bytes(b))),
        TypeTag::U64 => decode_fixed::<8>(rest, |b| DecodedArg::U64(u64::from_ne_bytes(b))),
        TypeTag::U128 => decode_fixed::<16>(rest, |b| DecodedArg::U128(u128::from_ne_bytes(b))),
        TypeTag::F32 => decode_fixed::<4>(rest, |b| DecodedArg::F32(f32::from_ne_bytes(b))),
        TypeTag::F64 => decode_fixed::<8>(rest, |b| DecodedArg::F64(f64::from_ne_bytes(b))),
        TypeTag::Bool => decode_fixed::<1>(rest, |b| DecodedArg::Bool(b[0] != 0)),
        TypeTag::Str => decode_str(rest),
        TypeTag::Usize => decode_fixed::<8>(rest, |b| DecodedArg::Usize(u64::from_ne_bytes(b))),
        TypeTag::Isize => decode_fixed::<8>(rest, |b| DecodedArg::Isize(i64::from_ne_bytes(b))),
        TypeTag::Custom => decode_custom(rest),
    }
}

/// Decodes a fixed-size value from the buffer.
fn decode_fixed<const N: usize>(
    data: &[u8],
    make: impl FnOnce([u8; N]) -> DecodedArg,
) -> Result<(DecodedArg, usize), DecodeError> {
    let buf = data
        .get(..N)
        .ok_or(DecodeError::BufferTooShort)?
        .try_into()
        .map_err(|_| DecodeError::BufferTooShort)?;
    Ok((make(buf), 1 + N))
}

/// Decodes a length-prefixed string from the buffer.
fn decode_str(data: &[u8]) -> Result<(DecodedArg, usize), DecodeError> {
    const LEN_SIZE: usize = size_of::<u32>();
    if data.len() < LEN_SIZE {
        return Err(DecodeError::BufferTooShort);
    }
    let len = u32::from_ne_bytes(data[..LEN_SIZE].try_into().unwrap()) as usize;
    if data.len() < LEN_SIZE + len {
        return Err(DecodeError::BufferTooShort);
    }
    let s = core::str::from_utf8(&data[LEN_SIZE..LEN_SIZE + len])
        .map_err(|_| DecodeError::InvalidUtf8)?;
    // 1 (tag) + LEN_SIZE (len prefix) + len (string bytes)
    Ok((DecodedArg::Str(s.to_owned()), 1 + LEN_SIZE + len))
}

/// Decodes a custom-encoded argument: reads the fn pointer, payload length, and
/// payload bytes, calls the decoder, and returns the formatted string.
fn decode_custom(data: &[u8]) -> Result<(DecodedArg, usize), DecodeError> {
    let ptr_size = size_of::<Decoder>();
    if data.len() < ptr_size + 4 {
        return Err(DecodeError::BufferTooShort);
    }
    // Read the length prefix and verify the full payload is present before
    // calling assume_init(). This avoids UB when the fn-pointer bytes are
    // invalid (e.g. all-zero in a truncated-buffer test): assume_init() is
    // only reached when the record was written by a real encode operation.
    let mut len_buf = [0u8; 4];
    len_buf.copy_from_slice(&data[ptr_size..ptr_size + 4]);
    let payload_len = u32::from_ne_bytes(len_buf) as usize;
    let payload_start = ptr_size + 4;
    if data.len() < payload_start + payload_len {
        return Err(DecodeError::BufferTooShort);
    }
    // SAFETY: we verified the entire record (fn ptr + length + payload) is
    // present, so the bytes were written by the blanket Encode impl for
    // CustomEncode and contain a valid, non-null fn pointer.
    let decoder: Decoder = unsafe {
        let mut slot = mem::MaybeUninit::<Decoder>::uninit();
        ptr::copy_nonoverlapping(data.as_ptr(), slot.as_mut_ptr().cast::<u8>(), ptr_size);
        slot.assume_init()
    };
    let payload = &data[payload_start..payload_start + payload_len];
    let mut out = String::new();
    decoder(payload, &mut out).map_err(|_| DecodeError::DecoderFailed)?;
    Ok((DecodedArg::Custom(out), 1 + ptr_size + 4 + payload_len))
}

/// A fully decoded log record ready for formatting.
pub struct DecodedRecord {
    /// Timestamp in nanoseconds since UNIX epoch.
    pub timestamp_ns: u64,
    /// Reference to the static callsite metadata.
    pub metadata: &'static LogMetadata,
    /// The decoded argument values.
    pub args: Vec<DecodedArg>,
}

/// Decodes a complete record (header + arguments) from a contiguous byte slice.
///
/// # Errors
///
/// Returns [`DecodeError`] if the data is malformed.
///
/// # Safety
///
/// The `metadata_ptr` field in the header must be a valid pointer to a
/// `&'static LogMetadata`.
#[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
pub unsafe fn decode_record(data: &[u8]) -> Result<DecodedRecord, DecodeError> {
    if data.len() < RecordHeader::SIZE {
        return Err(DecodeError::BufferTooShort);
    }

    // SAFETY: RecordHeader is repr(C) and data has enough bytes.
    // We use read_unaligned because the byte buffer may not be aligned.
    let header = unsafe { ptr::read_unaligned(data.as_ptr().cast::<RecordHeader>()) };
    let args_data = &data[RecordHeader::SIZE..];

    if args_data.len() < header.encoded_args_size as usize {
        return Err(DecodeError::BufferTooShort);
    }

    // SAFETY: caller guarantees metadata_ptr is valid.
    let metadata: &'static LogMetadata = unsafe { &*(header.metadata_ptr as *const LogMetadata) };

    let mut args = Vec::with_capacity(metadata.arg_count as usize);
    let mut offset = 0;
    let args_end = header.encoded_args_size as usize;

    while offset < args_end {
        let (arg, consumed) = decode_one(&args_data[offset..args_end])?;
        args.push(arg);
        offset += consumed;
    }

    Ok(DecodedRecord {
        timestamp_ns: header.timestamp_ns,
        metadata,
        args,
    })
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use super::super::encode::{CustomEncode, Decoder, Encode, TypeTag};
    use super::*;

    /// Encode+decode roundtrip test — compares `to_ne_bytes` to avoid
    /// `clippy::float_cmp` while keeping bitwise-exact semantics for all types.
    macro_rules! test_decode_roundtrip {
        ($name:ident, bool, $tag:ident, Bool, $val:expr) => {
            #[test]
            fn $name() {
                let val: bool = $val;
                let mut buf = vec![0u8; val.encoded_size()];
                unsafe { val.encode_to(buf.as_mut_ptr()) };
                let (arg, consumed) = decode_one(&buf).unwrap();
                assert_eq!(consumed, 1 + size_of::<bool>());
                assert_eq!(arg, DecodedArg::Bool(val));
            }
        };
        ($name:ident, $ty:ty, $tag:ident, $variant:ident, $val:expr, cast $wire:ty) => {
            test_decode_roundtrip!($name, $ty, $wire, $tag, $variant, $val);
        };
        ($name:ident, $ty:ty, $tag:ident, $variant:ident, $val:expr) => {
            test_decode_roundtrip!($name, $ty, $ty, $tag, $variant, $val);
        };
        ($name:ident, $ty:ty, $wire:ty, $tag:ident, $variant:ident, $val:expr) => {
            #[test]
            fn $name() {
                let val: $ty = $val;
                let mut buf = vec![0u8; val.encoded_size()];
                unsafe { val.encode_to(buf.as_mut_ptr()) };
                let (arg, consumed) = decode_one(&buf).unwrap();
                assert_eq!(consumed, 1 + size_of::<$wire>());
                let DecodedArg::$variant(got) = arg else {
                    panic!("expected {}", stringify!($variant))
                };
                assert_eq!(got.to_ne_bytes(), (val as $wire).to_ne_bytes());
            }
        };
    }

    test_decode_roundtrip!(decode_i8_roundtrip, i8, I8, I8, i8::MIN);
    test_decode_roundtrip!(decode_i16_roundtrip, i16, I16, I16, -1_000_i16);
    test_decode_roundtrip!(decode_i32_roundtrip, i32, I32, I32, -999_i32);
    test_decode_roundtrip!(decode_i64_roundtrip, i64, I64, I64, i64::MIN);
    test_decode_roundtrip!(decode_i128_roundtrip, i128, I128, I128, i128::MIN);
    test_decode_roundtrip!(decode_u8_roundtrip, u8, U8, U8, u8::MAX);
    test_decode_roundtrip!(decode_u16_roundtrip, u16, U16, U16, u16::MAX);
    test_decode_roundtrip!(decode_u32_roundtrip, u32, U32, U32, u32::MAX);
    test_decode_roundtrip!(decode_u64_roundtrip, u64, U64, U64, u64::MAX);
    test_decode_roundtrip!(decode_u128_roundtrip, u128, U128, U128, u128::MAX);
    test_decode_roundtrip!(decode_bool_true, bool, Bool, Bool, true);
    test_decode_roundtrip!(decode_bool_false, bool, Bool, Bool, false);
    test_decode_roundtrip!(decode_usize_roundtrip, usize, Usize, Usize, usize::MAX, cast u64);
    test_decode_roundtrip!(decode_isize_roundtrip, isize, Isize, Isize, isize::MIN, cast i64);
    test_decode_roundtrip!(decode_f32_roundtrip, f32, F32, F32, core::f32::consts::PI);
    test_decode_roundtrip!(decode_f64_roundtrip, f64, F64, F64, core::f64::consts::E);

    #[test]
    fn decode_str_roundtrip() {
        let val: &str = "world";
        let mut buf = vec![0u8; val.encoded_size()];
        unsafe { val.encode_to(buf.as_mut_ptr()) };
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, val.encoded_size());
        assert_eq!(arg, DecodedArg::Str("world".to_owned()));
    }

    #[test]
    fn decode_one_empty_slice_returns_err() {
        assert!(decode_one(&[]).is_err());
    }

    #[test]
    fn decode_one_unknown_tag_returns_err() {
        assert!(decode_one(&[17]).is_err());
        assert!(decode_one(&[u8::MAX]).is_err());
    }

    #[test]
    fn decode_one_truncated_fixed_returns_err() {
        // Tag only, no payload bytes.
        assert!(decode_one(&[TypeTag::I32 as u8]).is_err());
        // One byte short.
        assert!(decode_one(&[TypeTag::I32 as u8, 0, 0, 0]).is_err());
    }

    #[test]
    fn decode_one_truncated_str_returns_err() {
        // Length prefix present but no string bytes.
        let mut buf = [0u8; 5];
        buf[0] = TypeTag::Str as u8;
        let len: u32 = 10;
        buf[1..5].copy_from_slice(&len.to_ne_bytes());
        assert!(decode_one(&buf).is_err());
    }

    #[test]
    fn decode_one_truncated_custom_returns_err() {
        // Tag only — fn pointer field is incomplete.
        assert!(decode_one(&[TypeTag::Custom as u8]).is_err());

        // Tag + fn pointer, but no length prefix.
        let partial = vec![TypeTag::Custom as u8; 1 + size_of::<usize>()];
        assert!(decode_one(&partial).is_err());

        // Tag + fn pointer + length prefix claiming 4 payload bytes, but 0 provided.
        let mut buf = vec![0u8; 1 + size_of::<usize>() + 4];
        buf[0] = TypeTag::Custom as u8;
        let len: u32 = 4;
        buf[1 + size_of::<usize>()..].copy_from_slice(&len.to_ne_bytes());
        assert!(decode_one(&buf).is_err());
    }

    use crate::testutil::{Color, Marker, Point2D};

    /// Builds the full wire bytes for a custom argument (tag + fn ptr + len + payload).
    fn pack_custom(decoder: Decoder, payload: &[u8]) -> Vec<u8> {
        let ptr_size = size_of::<Decoder>();
        let mut buf = vec![0u8; 1 + ptr_size + 4 + payload.len()];
        buf[0] = TypeTag::Custom as u8;
        unsafe {
            ptr::copy_nonoverlapping(
                ptr::addr_of!(decoder).cast::<u8>(),
                buf[1..].as_mut_ptr(),
                ptr_size,
            );
        }
        let len_bytes = u32::try_from(payload.len()).unwrap().to_ne_bytes();
        buf[1 + ptr_size..1 + ptr_size + 4].copy_from_slice(&len_bytes);
        buf[1 + ptr_size + 4..].copy_from_slice(payload);
        buf
    }

    /// Encodes a value (tag byte + payload) into a fresh Vec.
    fn encode_tagged<T: Encode>(val: &T) -> Vec<u8> {
        let mut buf = vec![0u8; val.encoded_size()];
        unsafe { val.encode_to(buf.as_mut_ptr()) };
        buf
    }

    #[test]
    fn decode_custom_manual_color() {
        let decoder: Decoder =
            |bytes, out| write!(out, "rgb({}, {}, {})", bytes[0], bytes[1], bytes[2]);
        let buf = pack_custom(decoder, &[255, 128, 0]);
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(arg, DecodedArg::Custom("rgb(255, 128, 0)".to_owned()));
    }

    #[test]
    fn decode_custom_manual_zero_payload() {
        let decoder: Decoder = |_bytes, out| write!(out, "empty");
        let buf = pack_custom(decoder, &[]);
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(arg, DecodedArg::Custom("empty".to_owned()));
    }

    #[test]
    fn decode_custom_color_roundtrip() {
        let val = Color {
            r: 255,
            g: 128,
            b: 0,
        };
        let buf = encode_tagged(&val);
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(arg, DecodedArg::Custom("rgb(255, 128, 0)".to_owned()));
    }

    #[test]
    fn decode_custom_point2d_roundtrip() {
        let val = Point2D { x: 1.5, y: -3.25 };
        let buf = encode_tagged(&val);
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        let DecodedArg::Custom(ref s) = arg else {
            panic!("expected Custom")
        };
        assert!(s.contains("1.5"), "expected x=1.5 in output, got: {s:?}");
        assert!(
            s.contains("-3.25"),
            "expected y=-3.25 in output, got: {s:?}"
        );
    }

    #[test]
    fn decode_custom_marker_roundtrip() {
        let val = Marker;
        let buf = encode_tagged(&val);
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(arg, DecodedArg::Custom("marker".to_owned()));
    }

    #[test]
    fn decode_custom_consumed_bytes() {
        // consumed must equal 1 (tag) + size_of::<usize>() (fn ptr) + 4 (len) + payload_size().
        let val = Color { r: 0, g: 0, b: 0 };
        let buf = encode_tagged(&val);
        let (_, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, 1 + size_of::<usize>() + 4 + val.payload_size());
    }

    #[test]
    fn decoded_arg_custom_display() {
        let arg = DecodedArg::Custom("rgb(1, 2, 3)".to_owned());
        assert_eq!(format!("{arg}"), "rgb(1, 2, 3)");
    }
}
