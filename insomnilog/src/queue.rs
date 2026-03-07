//! Lock-free bounded SPSC (single-producer, single-consumer) byte queue.
//!
//! Inspired by Quill's `BoundedSPSCQueue`. The queue is a power-of-2 ring
//! buffer backed by a single contiguous allocation. The producer and consumer
//! each cache the other's position to minimize cross-cache-line reads.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Cache-line-padded wrapper to prevent false sharing.
#[repr(align(64))]
struct CachePadded<T> {
    /// The inner value.
    value: T,
}

impl<T> CachePadded<T> {
    /// Creates a new cache-padded value.
    const fn new(value: T) -> Self {
        Self { value }
    }
}

/// Shared state of the SPSC queue.
struct QueueInner {
    /// The underlying ring buffer.
    buf: Box<[u8]>,
    /// Power-of-2 capacity (used as bitmask: `pos & mask == pos % capacity`).
    capacity: usize,
    /// Bitmask: `capacity - 1`.
    mask: usize,
    /// Write position (only written by producer, read by consumer).
    write_pos: CachePadded<AtomicUsize>,
    /// Read position (only written by consumer, read by producer).
    read_pos: CachePadded<AtomicUsize>,
}

/// Producer half of the SPSC queue (used by a logging thread).
///
/// This type is public for macro expansion but is not part of the stable API.
#[doc(hidden)]
pub struct Producer {
    /// Shared queue state.
    inner: Arc<QueueInner>,
    /// Cached read position (avoids frequent atomic loads from consumer).
    cached_read: usize,
    /// Current write position.
    write: usize,
}

/// Consumer half of the SPSC queue (used by the backend worker).
pub struct Consumer {
    /// Shared queue state.
    inner: Arc<QueueInner>,
    /// Cached write position (avoids frequent atomic loads from producer).
    cached_write: usize,
    /// Current read position.
    read: usize,
}

// SAFETY: Consumer is only used by the single backend thread, and the queue's
// atomic positions ensure correct synchronization with the producer.
unsafe impl Send for Consumer {}

/// Creates a bounded SPSC queue split into producer and consumer halves.
///
/// `capacity` is the queue size in **bytes** and will be rounded up to the
/// next power of two.
///
/// # Panics
///
/// Panics if `capacity` is 0 or if rounding up overflows `usize`.
pub fn bounded(capacity: usize) -> (Producer, Consumer) {
    assert!(capacity > 0, "queue capacity must be > 0");
    let capacity = capacity.next_power_of_two();
    let buf = vec![0u8; capacity].into_boxed_slice();
    let inner = Arc::new(QueueInner {
        buf,
        capacity,
        mask: capacity - 1,
        write_pos: CachePadded::new(AtomicUsize::new(0)),
        read_pos: CachePadded::new(AtomicUsize::new(0)),
    });
    let producer = Producer {
        inner: Arc::clone(&inner),
        cached_read: 0,
        write: 0,
    };
    let consumer = Consumer {
        inner,
        cached_write: 0,
        read: 0,
    };
    (producer, consumer)
}

impl Producer {
    /// Tries to reserve `n` contiguous bytes for writing.
    ///
    /// Returns `None` if there is not enough space (silent drop semantics).
    /// The returned slice is only valid until the next call to [`Self::commit`].
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn try_reserve(&mut self, n: usize) -> Option<*mut u8> {
        let capacity = self.inner.capacity;
        if n > capacity {
            return None;
        }

        let mut available = capacity - (self.write - self.cached_read);
        if available < n {
            // Refresh cached read position.
            self.cached_read = self.inner.read_pos.value.load(Ordering::Acquire);
            available = capacity - (self.write - self.cached_read);
            if available < n {
                return None;
            }
        }

        // Check if the write would wrap around the ring boundary.
        // If so, we need contiguous space, so skip to the beginning.
        let start = self.write & self.inner.mask;
        let end_in_buf = start + n;
        if end_in_buf > capacity {
            // Not enough contiguous space at end; check if we can fit at start.
            // We need to "waste" the remaining bytes at the end.
            let waste = capacity - start;
            let total_needed = waste + n;
            if capacity - (self.write - self.cached_read) < total_needed {
                self.cached_read = self.inner.read_pos.value.load(Ordering::Acquire);
                if capacity - (self.write - self.cached_read) < total_needed {
                    return None;
                }
            }
            // Skip to the start of the buffer.
            self.write += waste;
        }

        let offset = self.write & self.inner.mask;
        // SAFETY: offset + n <= capacity (ensured above), and we have exclusive
        // write access to this region.
        let ptr = unsafe { self.inner.buf.as_ptr().add(offset).cast_mut() };
        Some(ptr)
    }

    /// Commits `n` bytes, making them visible to the consumer.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `n` would advance past the consumer.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn commit(&mut self, n: usize) {
        self.write += n;
        self.inner
            .write_pos
            .value
            .store(self.write, Ordering::Release);
    }
}

impl Consumer {
    /// Returns the number of bytes available for reading.
    pub fn available(&mut self) -> usize {
        let avail = self.cached_write - self.read;
        if avail > 0 {
            return avail;
        }
        self.cached_write = self.inner.write_pos.value.load(Ordering::Acquire);
        self.cached_write - self.read
    }

    /// Reads `len` bytes starting at the current read position into the
    /// provided staging buffer if the data wraps around the ring boundary,
    /// or returns a direct slice into the ring buffer.
    ///
    /// # Panics
    ///
    /// Panics if `len` exceeds the available bytes.
    pub fn read_bytes<'a>(&self, len: usize, staging: &'a mut Vec<u8>) -> &'a [u8] {
        let start = self.read & self.inner.mask;
        let end = start + len;

        if end <= self.inner.capacity {
            // No wrap — return a direct slice.
            // SAFETY: start..start+len is within bounds and the producer has
            // committed these bytes.
            unsafe {
                let ptr = self.inner.buf.as_ptr().add(start);
                core::slice::from_raw_parts(ptr, len)
            }
        } else {
            // Wrap around — copy into staging buffer.
            staging.clear();
            staging.reserve(len);
            let first = self.inner.capacity - start;
            staging.extend_from_slice(&self.inner.buf[start..self.inner.capacity]);
            staging.extend_from_slice(&self.inner.buf[..len - first]);
            staging
        }
    }

    /// Advances the read position by `n` bytes.
    pub fn advance(&mut self, n: usize) {
        self.read += n;
        self.inner
            .read_pos
            .value
            .store(self.read, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_produce_consume() {
        let (mut prod, mut cons) = bounded(64);

        // Write some data.
        let ptr = prod.try_reserve(4).unwrap();
        unsafe {
            core::ptr::copy_nonoverlapping([1u8, 2, 3, 4].as_ptr(), ptr, 4);
        }
        prod.commit(4);

        // Read it back.
        assert_eq!(cons.available(), 4);
        let mut staging = Vec::new();
        let data = cons.read_bytes(4, &mut staging);
        assert_eq!(data, &[1, 2, 3, 4]);
        cons.advance(4);

        assert_eq!(cons.available(), 0);
    }

    #[test]
    fn queue_full_returns_none() {
        let (mut prod, _cons) = bounded(64);
        // Fill the queue.
        assert!(prod.try_reserve(64).is_some());
        prod.commit(64);
        // Now it should be full.
        assert!(prod.try_reserve(1).is_none());
    }

    #[test]
    fn oversized_reserve_returns_none() {
        let (mut prod, _cons) = bounded(64);
        assert!(prod.try_reserve(65).is_none());
    }

    #[test]
    fn wrap_around() {
        let (mut prod, mut cons) = bounded(64);
        let mut staging = Vec::new();

        // Fill most of the buffer.
        let ptr = prod.try_reserve(60).unwrap();
        unsafe {
            core::ptr::write_bytes(ptr, 0xAA, 60);
        }
        prod.commit(60);

        // Consume it.
        assert_eq!(cons.available(), 60);
        cons.read_bytes(60, &mut staging);
        cons.advance(60);

        // Write again — this should wrap around.
        let ptr = prod.try_reserve(16).unwrap();
        unsafe {
            core::ptr::write_bytes(ptr, 0xBB, 16);
        }
        prod.commit(16);

        // available() includes the 4 wasted bytes at the end of the buffer
        // that were skipped to maintain contiguity, plus the 16 actual bytes.
        let avail = cons.available();
        assert_eq!(avail, 20);
        // Skip the 4 wasted bytes, then read the 16 data bytes.
        cons.advance(4);
        let data = cons.read_bytes(16, &mut staging);
        assert_eq!(data, &[0xBB; 16]);
    }
}
