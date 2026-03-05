//! Binary encoding of log arguments into the SPSC queue.
//!
//! Each argument is encoded as a 1-byte [`TypeTag`] followed by the value in
//! native-endian byte order. Strings are prefixed with a `u32` length.

use core::ptr;

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
            _ => Err(()),
        }
    }
}

/// Trait for types that can be binary-encoded into the SPSC queue.
///
/// # Safety
///
/// Implementors must ensure that [`Encode::encode_to`] writes exactly
/// [`Encode::encoded_size`] bytes (not counting the tag byte, which is
/// written by the caller).
pub trait Encode {
    /// The type tag for this type.
    const TAG: TypeTag;

    /// Returns the number of bytes needed to encode the value
    /// (excluding the 1-byte tag).
    fn encoded_size(&self) -> usize;

    /// Returns the tag byte for this value (helper for macro expansion).
    fn tag(&self) -> u8 {
        Self::TAG as u8
    }

    /// Writes the encoded value to `dst`.
    ///
    /// Returns the number of bytes written (must equal `encoded_size()`).
    ///
    /// # Safety
    ///
    /// `dst` must point to at least `encoded_size()` writable bytes.
    unsafe fn encode_to(&self, dst: *mut u8) -> usize;
}

/// Implements the [`Encode`] trait for an integer type.
macro_rules! impl_encode_int {
    ($ty:ty, $tag:expr) => {
        impl Encode for $ty {
            const TAG: TypeTag = $tag;

            #[inline]
            fn encoded_size(&self) -> usize {
                size_of::<$ty>()
            }

            #[inline]
            unsafe fn encode_to(&self, dst: *mut u8) -> usize {
                let bytes = self.to_ne_bytes();
                // SAFETY: caller guarantees dst has encoded_size() bytes available.
                unsafe {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
                }
                bytes.len()
            }
        }
    };
}

impl_encode_int!(i8, TypeTag::I8);
impl_encode_int!(i16, TypeTag::I16);
impl_encode_int!(i32, TypeTag::I32);
impl_encode_int!(i64, TypeTag::I64);
impl_encode_int!(i128, TypeTag::I128);
impl_encode_int!(u8, TypeTag::U8);
impl_encode_int!(u16, TypeTag::U16);
impl_encode_int!(u32, TypeTag::U32);
impl_encode_int!(u64, TypeTag::U64);
impl_encode_int!(u128, TypeTag::U128);

impl Encode for f32 {
    const TAG: TypeTag = TypeTag::F32;

    #[inline]
    fn encoded_size(&self) -> usize {
        size_of::<Self>()
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) -> usize {
        let bytes = self.to_ne_bytes();
        // SAFETY: caller guarantees dst has encoded_size() bytes available.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        }
        bytes.len()
    }
}

impl Encode for f64 {
    const TAG: TypeTag = TypeTag::F64;

    #[inline]
    fn encoded_size(&self) -> usize {
        size_of::<Self>()
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) -> usize {
        let bytes = self.to_ne_bytes();
        // SAFETY: caller guarantees dst has encoded_size() bytes available.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        }
        bytes.len()
    }
}

impl Encode for bool {
    const TAG: TypeTag = TypeTag::Bool;

    #[inline]
    fn encoded_size(&self) -> usize {
        1
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) -> usize {
        // SAFETY: caller guarantees dst has 1 byte available.
        unsafe {
            *dst = u8::from(*self);
        }
        1
    }
}

impl Encode for &str {
    const TAG: TypeTag = TypeTag::Str;

    #[inline]
    fn encoded_size(&self) -> usize {
        size_of::<u32>() + self.len()
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) -> usize {
        let len = self.len();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "strings > 4 GiB are not supported"
        )]
        let len_u32 = len as u32;
        let len_bytes = len_u32.to_ne_bytes();
        // SAFETY: caller guarantees dst has encoded_size() bytes available.
        unsafe {
            ptr::copy_nonoverlapping(len_bytes.as_ptr(), dst, 4);
            if len > 0 {
                ptr::copy_nonoverlapping(self.as_ptr(), dst.add(4), len);
            }
        }
        4 + len
    }
}

impl Encode for usize {
    const TAG: TypeTag = TypeTag::Usize;

    #[inline]
    fn encoded_size(&self) -> usize {
        size_of::<u64>()
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) -> usize {
        let val = *self as u64;
        let bytes = val.to_ne_bytes();
        // SAFETY: caller guarantees dst has encoded_size() bytes available.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        }
        bytes.len()
    }
}

impl Encode for isize {
    const TAG: TypeTag = TypeTag::Isize;

    #[inline]
    fn encoded_size(&self) -> usize {
        size_of::<i64>()
    }

    #[inline]
    unsafe fn encode_to(&self, dst: *mut u8) -> usize {
        let val = *self as i64;
        let bytes = val.to_ne_bytes();
        // SAFETY: caller guarantees dst has encoded_size() bytes available.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        }
        bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_i32_roundtrip() {
        let val: i32 = -42;
        let mut buf = [0u8; 4];
        let n = unsafe { val.encode_to(buf.as_mut_ptr()) };
        assert_eq!(n, 4);
        assert_eq!(i32::from_ne_bytes(buf), -42);
    }

    #[test]
    fn encode_str_roundtrip() {
        let val: &str = "hello";
        let mut buf = [0u8; 64];
        let n = unsafe { val.encode_to(buf.as_mut_ptr()) };
        assert_eq!(n, 4 + 5);
        let len = u32::from_ne_bytes(buf[..4].try_into().unwrap()) as usize;
        assert_eq!(len, 5);
        assert_eq!(&buf[4..4 + len], b"hello");
    }

    #[test]
    fn encode_bool() {
        let mut buf = [0u8; 1];
        let n = unsafe { true.encode_to(buf.as_mut_ptr()) };
        assert_eq!(n, 1);
        assert_eq!(buf[0], 1);
    }
}
