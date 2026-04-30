//! Binary record header shared between the write path (macros) and the read path (decode).

// Not yet used by macros or backend — removed once those modules are wired up.
#![allow(dead_code)]

use core::ptr;

/// The binary record header written at the start of each log entry in the queue.
#[repr(C)]
pub struct RecordHeader {
    /// Nanoseconds since UNIX epoch.
    pub timestamp_ns: u64,
    /// Pointer to the static `LogMetadata` for this callsite.
    pub metadata_ptr: usize,
    /// Total size in bytes of the encoded arguments that follow.
    pub encoded_args_size: u32,
    /// Padding to bring the struct to a multiple of 8 bytes.
    pub(crate) padding: u32,
}

impl RecordHeader {
    /// The fixed size of a record header in bytes.
    pub const SIZE: usize = size_of::<Self>();

    /// Constructs a new [`RecordHeader`], always zeroing the padding field.
    pub const fn new(timestamp_ns: u64, metadata_ptr: usize, encoded_args_size: u32) -> Self {
        Self {
            timestamp_ns,
            metadata_ptr,
            encoded_args_size,
            padding: 0,
        }
    }

    /// Copies this header into `dst`.
    ///
    /// # Safety
    ///
    /// `dst` must point to at least [`Self::SIZE`] writable bytes.
    pub const unsafe fn write_to(&self, dst: *mut u8) {
        // SAFETY: caller guarantees SIZE bytes available; repr(C) layout is stable.
        unsafe {
            ptr::copy_nonoverlapping(ptr::from_ref(self).cast::<u8>(), dst, Self::SIZE);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::mem;
    use core::ptr;

    use super::*;

    #[test]
    fn new_sets_timestamp_ns() {
        let h = RecordHeader::new(123_456_789, 0, 0);
        assert_eq!(h.timestamp_ns, 123_456_789);
    }

    #[test]
    fn new_sets_metadata_ptr() {
        let h = RecordHeader::new(0, 0xDEAD_BEEF, 0);
        assert_eq!(h.metadata_ptr, 0xDEAD_BEEF);
    }

    #[test]
    fn new_sets_encoded_args_size() {
        let h = RecordHeader::new(0, 0, 42);
        assert_eq!(h.encoded_args_size, 42);
    }

    #[test]
    fn new_padding_is_always_zero() {
        // Even when every other field is at its maximum value.
        let h = RecordHeader::new(u64::MAX, usize::MAX, u32::MAX);
        assert_eq!(h.padding, 0);
    }

    #[test]
    fn size_matches_struct_layout() {
        assert_eq!(RecordHeader::SIZE, mem::size_of::<RecordHeader>());
    }

    #[test]
    fn write_to_roundtrip() {
        let original = RecordHeader::new(987_654_321, 0x1234_5678, 64);
        let mut buf = [0u8; RecordHeader::SIZE];
        // SAFETY: buf is exactly SIZE bytes.
        unsafe { original.write_to(buf.as_mut_ptr()) };
        // SAFETY: buf holds SIZE bytes written by write_to.
        let recovered = unsafe { ptr::read_unaligned(buf.as_ptr().cast::<RecordHeader>()) };
        assert_eq!(recovered.timestamp_ns, original.timestamp_ns);
        assert_eq!(recovered.metadata_ptr, original.metadata_ptr);
        assert_eq!(recovered.encoded_args_size, original.encoded_args_size);
        assert_eq!(recovered.padding, 0);
    }

    #[test]
    fn write_to_writes_exactly_size_bytes() {
        // Sentinel bytes placed immediately after SIZE must not be touched.
        let header = RecordHeader::new(1, 2, 3);
        let mut buf = [0xFF_u8; RecordHeader::SIZE + 2];
        // SAFETY: buf has SIZE + 2 bytes, so SIZE bytes from the start are valid.
        unsafe { header.write_to(buf.as_mut_ptr()) };
        assert_eq!(
            buf[RecordHeader::SIZE],
            0xFF,
            "byte past SIZE was overwritten"
        );
        assert_eq!(
            buf[RecordHeader::SIZE + 1],
            0xFF,
            "byte past SIZE was overwritten"
        );
    }
}
