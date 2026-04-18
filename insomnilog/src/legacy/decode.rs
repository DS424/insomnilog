//! Decoding of binary-encoded log arguments from the SPSC queue.

use core::fmt;
use core::ptr;

use super::encode::TypeTag;
use super::metadata::LogMetadata;

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
        }
    }
}

/// Decodes one argument from `data`, returning the decoded arg and the number
/// of bytes consumed (including the 1-byte tag).
///
/// Returns `None` if the data is too short or the tag is unknown.
pub fn decode_one(data: &[u8]) -> Option<(DecodedArg, usize)> {
    let (&tag_byte, rest) = data.split_first()?;
    match TypeTag::try_from(tag_byte).ok()? {
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
    }
}

/// Decodes a fixed-size value from the buffer.
fn decode_fixed<const N: usize>(
    data: &[u8],
    make: impl FnOnce([u8; N]) -> DecodedArg,
) -> Option<(DecodedArg, usize)> {
    if data.len() < N {
        return None;
    }
    let mut buf = [0u8; N];
    // SAFETY: we checked data.len() >= N.
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), buf.as_mut_ptr(), N);
    }
    Some((make(buf), 1 + N))
}

/// Decodes a length-prefixed string from the buffer.
fn decode_str(data: &[u8]) -> Option<(DecodedArg, usize)> {
    if data.len() < 4 {
        return None;
    }
    let mut len_buf = [0u8; 4];
    len_buf.copy_from_slice(&data[..4]);
    let len = u32::from_ne_bytes(len_buf) as usize;
    if data.len() < 4 + len {
        return None;
    }
    let s = core::str::from_utf8(&data[4..4 + len]).ok()?;
    // 1 (tag) + 4 (len prefix) + len (string bytes)
    Some((DecodedArg::Str(s.to_owned()), 1 + 4 + len))
}

/// The binary record header written at the start of each log entry in the queue.
#[repr(C)]
pub struct RecordHeader {
    /// Nanoseconds since UNIX epoch.
    pub timestamp_ns: u64,
    /// Pointer to the static `LogMetadata` for this callsite.
    pub metadata_ptr: usize,
    /// Total size in bytes of the encoded arguments that follow.
    pub encoded_args_size: u32,
    /// Padding for alignment.
    pub padding: u32,
}

impl RecordHeader {
    /// The fixed size of a record header in bytes.
    pub const SIZE: usize = size_of::<Self>();
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
/// Returns `None` if the data is malformed.
///
/// # Safety
///
/// The `metadata_ptr` field in the header must be a valid pointer to a
/// `&'static LogMetadata`.
#[cfg_attr(feature = "rtsan", rtsan_standalone::blocking)]
pub unsafe fn decode_record(data: &[u8]) -> Option<DecodedRecord> {
    if data.len() < RecordHeader::SIZE {
        return None;
    }

    // SAFETY: RecordHeader is repr(C) and data has enough bytes.
    // We use read_unaligned because the byte buffer may not be aligned.
    let header = unsafe { ptr::read_unaligned(data.as_ptr().cast::<RecordHeader>()) };
    let args_data = &data[RecordHeader::SIZE..];

    if args_data.len() < header.encoded_args_size as usize {
        return None;
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

    Some(DecodedRecord {
        timestamp_ns: header.timestamp_ns,
        metadata,
        args,
    })
}

#[cfg(test)]
mod tests {
    use super::super::encode::{Encode, TypeTag};
    use super::*;

    #[test]
    fn decode_i32_roundtrip() {
        let val: i32 = -999;
        let mut buf = [0u8; 5];
        buf[0] = TypeTag::I32 as u8;
        unsafe { val.encode_to(buf[1..].as_mut_ptr()) };
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, 5);
        assert_eq!(arg, DecodedArg::I32(-999));
    }

    #[test]
    fn decode_str_roundtrip() {
        let val: &str = "world";
        let size = 1 + val.encoded_size();
        let mut buf = vec![0u8; size];
        buf[0] = TypeTag::Str as u8;
        unsafe { val.encode_to(buf[1..].as_mut_ptr()) };
        let (arg, consumed) = decode_one(&buf).unwrap();
        assert_eq!(consumed, size);
        assert_eq!(arg, DecodedArg::Str("world".to_owned()));
    }

    #[test]
    fn decode_bool_roundtrip() {
        let mut buf = [0u8; 2];
        buf[0] = TypeTag::Bool as u8;
        unsafe { true.encode_to(buf[1..].as_mut_ptr()) };
        let (arg, _) = decode_one(&buf).unwrap();
        assert_eq!(arg, DecodedArg::Bool(true));
    }

    #[test]
    fn decode_all_int_types() {
        // Test u64
        let val: u64 = 123_456_789;
        let mut buf = [0u8; 9];
        buf[0] = TypeTag::U64 as u8;
        unsafe { val.encode_to(buf[1..].as_mut_ptr()) };
        let (arg, _) = decode_one(&buf).unwrap();
        assert_eq!(arg, DecodedArg::U64(123_456_789));

        // Test f64
        let val: f64 = 2.72;
        let mut buf = [0u8; 9];
        buf[0] = TypeTag::F64 as u8;
        unsafe { val.encode_to(buf[1..].as_mut_ptr()) };
        let (arg, _) = decode_one(&buf).unwrap();
        assert_eq!(arg, DecodedArg::F64(2.72));
    }
}
