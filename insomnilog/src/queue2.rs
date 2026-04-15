//! Lock-free bounded SPSC (single-producer, single-consumer) byte queue.
//!
//! Inspired by Quill's `BoundedSPSCQueue`. The queue is a power-of-2 ring
//! buffer backed by a 2× capacity allocation. The double allocation ensures
//! that every [`Producer::write`] is physically contiguous — no wrap-around
//! bookkeeping or waste bytes needed.
//!
//! # Read granularity invariant
//!
//! **Each [`Consumer::read`] call must request exactly as many bytes as the
//! corresponding [`Producer::write`] call wrote.**
//!
//! The 2× allocation makes a single write of `n` bytes starting at any ring
//! offset contiguous in memory. The consumer reads that same region by using
//! the same masked offset. Because each `read` advances the position, a `read`
//! of the wrong size shifts subsequent reads to the wrong offset, silently
//! corrupting all following records. The queue has no record boundary tracking
//! and cannot detect this misuse at runtime.
//!
//! [`Consumer::peek`] does **not** advance the position, so it may safely
//! read a prefix of the next written chunk (e.g., just the record header to
//! determine the full record length). The subsequent `read` must still consume
//! the full chunk written by the producer.

#![allow(dead_code, reason = "WIP replacement for queue, not yet wired in")]

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Errors returned by [`Producer::write`].
#[derive(Debug)]
pub enum WriteError {
    /// The requested size exceeds the queue capacity.
    OversizedWrite,
    /// The queue is currently full.
    QueueFull,
}

/// Cache-line-padded wrapper to prevent false sharing.
#[repr(align(64))]
struct CachePadded<T> {
    /// The inner value, padded to a 64-byte cache line.
    value: T,
}

impl<T> CachePadded<T> {
    /// Wraps `value` in a cache-line-padded container.
    const fn new(value: T) -> Self {
        Self { value }
    }
}

/// Shared state of the SPSC queue.
struct QueueInner {
    /// The underlying ring buffer, allocated at 2× capacity.
    ///
    /// The double allocation means a write starting at any offset in
    /// `[0, capacity)` will never exceed the physical buffer bounds, even if
    /// it extends past `capacity - 1`. Both producer and consumer index with
    /// `pos & mask` into the first half; the second half acts as overflow
    /// space that makes every write contiguous without any wrap-around logic.
    buf: Box<[UnsafeCell<u8>]>,
    /// Power-of-2 capacity (logical half of the physical allocation).
    capacity: usize,
    /// Bitmask: `capacity - 1`.
    mask: usize,
    /// Write position (only written by producer, read by consumer).
    write_pos: CachePadded<AtomicUsize>,
    /// Read position (only written by consumer, read by producer).
    read_pos: CachePadded<AtomicUsize>,
}

// SAFETY: The ring buffer is accessed only through the atomic read/write
// positions, which guarantee that producer and consumer never touch the same
// bytes concurrently. `Box<[UnsafeCell<u8>]>` is `!Sync` by default; this
// impl asserts that the invariant above makes concurrent access safe.
unsafe impl Sync for QueueInner {}

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

// SAFETY: `Consumer` is `!Send` by default because `Arc<QueueInner>` requires
// `QueueInner: Sync`, which fails due to the `UnsafeCell` buffer. However,
// sending a `Consumer` to another thread is sound because:
//   1. `Consumer` is not `Clone`, so exclusive ownership is enforced by the
//      type system — no two threads can hold a `Consumer` simultaneously.
//   2. Access to the shared buffer is partitioned by the atomic positions:
//      the consumer only reads bytes the producer has committed via a Release
//      store, and the consumer reads them via an Acquire load. This ordering
//      is correct regardless of which thread owns the `Consumer`.
unsafe impl Send for Consumer {}

/// Creates a bounded SPSC queue split into producer and consumer halves.
///
/// `capacity` is the queue size in **bytes** and will be rounded up to the
/// next power of two. The physical allocation is `2 × capacity` to guarantee
/// contiguous reads and writes without wrap-around bookkeeping.
///
/// # Panics
///
/// Panics if `capacity` is 0 or if rounding up overflows `usize`.
pub fn new(capacity: usize) -> (Producer, Consumer) {
    assert!(capacity > 0, "queue capacity must be > 0");
    let capacity = capacity.next_power_of_two();
    // Allocate 2× capacity so every write is physically contiguous.
    let buf: Box<[UnsafeCell<u8>]> = core::iter::repeat_with(|| UnsafeCell::new(0u8))
        .take(2 * capacity)
        .collect::<Vec<_>>()
        .into_boxed_slice();
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
    /// Writes `n` bytes into the queue by calling `f` with a mutable slice.
    ///
    /// The closure receives a `&mut [u8]` of exactly `n` bytes and may write
    /// any data into it. The bytes become visible to the consumer atomically
    /// after `f` returns.
    ///
    /// Returns [`WriteError::OversizedWrite`] if `n > capacity`, or
    /// [`WriteError::QueueFull`] if there is not enough space right now.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn write(&mut self, n: usize, f: impl FnOnce(&mut [u8])) -> Result<(), WriteError> {
        let Some(ptr) = self.try_reserve(n) else {
            return if n > self.inner.capacity {
                Err(WriteError::OversizedWrite)
            } else {
                Err(WriteError::QueueFull)
            };
        };
        // SAFETY: `try_reserve` guarantees `n` contiguous writable bytes at
        // `ptr`. We have exclusive write access to this region.
        let buf = unsafe { core::slice::from_raw_parts_mut(ptr, n) };
        f(buf);
        self.commit(n);
        Ok(())
    }

    /// Tries to reserve `n` contiguous bytes for writing.
    ///
    /// Returns `None` when the queue is full or `n` exceeds capacity.
    /// Because the buffer is 2× capacity, the reserved region is always
    /// physically contiguous — no boundary check is needed.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    fn try_reserve(&mut self, n: usize) -> Option<*mut u8> {
        let capacity = self.inner.capacity;
        if n > capacity {
            return None;
        }

        let mut available = capacity - (self.write - self.cached_read);
        if available < n {
            self.cached_read = self.inner.read_pos.value.load(Ordering::Acquire);
            available = capacity - (self.write - self.cached_read);
            if available < n {
                return None;
            }
        }

        let offset = self.write & self.inner.mask;
        // SAFETY: `offset` is in `[0, capacity)` and the physical buffer has
        // `2 * capacity` bytes, so `offset + n <= 2 * capacity` always holds.
        // `raw_get` derives a *mut u8 without creating an intermediate reference.
        Some(unsafe { UnsafeCell::raw_get(self.inner.buf.as_ptr().add(offset)) })
    }

    /// Commits `n` bytes, making them visible to the consumer.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    fn commit(&mut self, n: usize) {
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
        if self.cached_write > self.read {
            return self.cached_write - self.read;
        }
        self.cached_write = self.inner.write_pos.value.load(Ordering::Acquire);
        self.cached_write - self.read
    }

    /// Returns a slice of `len` bytes at the current read position **without**
    /// advancing. The returned slice is a direct view into the ring buffer.
    ///
    /// `len` may be less than the size of the next written chunk — for example,
    /// peeking at a record header to determine the full record length. Because
    /// the position is not advanced, the subsequent [`Self::read`] still lands
    /// at the correct offset. `len` must not exceed `available()`.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `len` exceeds the number of available bytes.
    pub fn peek(&mut self, len: usize) -> &[u8] {
        let avail = self.available();
        debug_assert!(
            len <= avail,
            "peek: requested {len} bytes but only {avail} available",
        );
        let offset = self.read & self.inner.mask;
        // SAFETY: The 2× allocation guarantees `offset + len` never exceeds
        // the physical buffer size. The caller must ensure `len <= available()`.
        // `raw_get` derives the pointer without creating a reference over
        // producer-owned bytes.
        unsafe {
            let ptr = UnsafeCell::raw_get(self.inner.buf.as_ptr().add(offset)).cast_const();
            core::slice::from_raw_parts(ptr, len)
        }
    }

    /// Returns a slice of `len` bytes at the current read position **and**
    /// advances the read position by `len`.
    ///
    /// `len` must match exactly the `n` passed to the corresponding
    /// [`Producer::write`] call. See the [module-level docs](self) for why
    /// splitting or merging reads across write boundaries corrupts data silently.
    ///
    /// The closure receives a `&[u8]` of exactly `len` bytes. The read position
    /// advances only after the closure returns, so the producer cannot reclaim
    /// those bytes while the closure holds the slice.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `len` exceeds the number of available bytes.
    pub fn read<R>(&mut self, len: usize, f: impl FnOnce(&[u8]) -> R) -> R {
        let avail = self.available();
        debug_assert!(
            len <= avail,
            "read: requested {len} bytes but only {avail} available",
        );
        let offset = self.read & self.inner.mask;
        // SAFETY: The 2× allocation ensures `offset + len` stays within the
        // physical buffer. The debug_assert above has confirmed `len` committed
        // bytes are present.
        let result = unsafe {
            let ptr = UnsafeCell::raw_get(self.inner.buf.as_ptr().add(offset)).cast_const();
            f(core::slice::from_raw_parts(ptr, len))
        };
        self.advance(len);
        result
    }

    /// Advances the read position by `n` bytes.
    fn advance(&mut self, n: usize) {
        self.read = self.read.wrapping_add(n);
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
    fn oversized_write_error() {
        let (mut prod, _cons) = new(64);
        let result = prod.write(65, |_| {});
        assert!(matches!(result, Err(WriteError::OversizedWrite)));
    }

    #[test]
    fn queue_full_error() {
        let (mut prod, _cons) = new(64);
        // Fill the queue completely.
        prod.write(64, |_| {}).unwrap();
        // Any further write must return QueueFull.
        let result = prod.write(1, |_| {});
        assert!(matches!(result, Err(WriteError::QueueFull)));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "peek: requested 5 bytes but only 4 available")]
    fn peek_panics_on_insufficient_data() {
        let (mut prod, mut cons) = new(64);
        prod.write(4, |buf| buf.fill(0xAA)).unwrap();
        cons.peek(5);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "read: requested 5 bytes but only 4 available")]
    fn read_panics_on_insufficient_data() {
        let (mut prod, mut cons) = new(64);
        prod.write(4, |buf| buf.fill(0xAA)).unwrap();
        cons.read(5, |_| {});
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "peek: requested 1 bytes but only 0 available")]
    fn peek_panics_on_empty_queue() {
        let (_prod, mut cons) = new(64);
        cons.peek(1);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "read: requested 1 bytes but only 0 available")]
    fn read_panics_on_empty_queue() {
        let (_prod, mut cons) = new(64);
        cons.read(1, |_| {});
    }

    #[test]
    fn basic_write_peek_read() {
        let (mut prod, mut cons) = new(64);

        prod.write(4, |buf| buf.copy_from_slice(&[1u8, 2, 3, 4]))
            .unwrap();

        assert_eq!(cons.available(), 4);

        // peek does not advance.
        assert_eq!(cons.peek(4), &[1u8, 2, 3, 4]);
        assert_eq!(cons.available(), 4);

        // read advances.
        assert_eq!(cons.read(4, <[u8]>::to_vec), &[1u8, 2, 3, 4]);
        assert_eq!(cons.available(), 0);
    }

    #[test]
    fn wrap_around_contiguous() {
        let (mut prod, mut cons) = new(64);

        // Fill most of the buffer, then consume it to advance the read head.
        prod.write(60, |buf| buf.fill(0xAA)).unwrap();
        assert_eq!(cons.available(), 60);
        cons.read(60, |_| {});

        // The write head is now at offset 60; the next 16-byte write logically
        // crosses the ring boundary (60 + 16 = 76 > 64). With 2× allocation
        // the bytes are physically contiguous — no waste bytes, no skip needed.
        prod.write(16, |buf| buf.fill(0xBB)).unwrap();

        assert_eq!(cons.available(), 16);
        assert_eq!(cons.read(16, <[u8]>::to_vec), &[0xBBu8; 16]);
        assert_eq!(cons.available(), 0);
    }

    #[test]
    fn queue_reuse_after_cycle() {
        let (mut prod, mut cons) = new(16);

        for i in 0..8_u8 {
            prod.write(4, |buf| buf.copy_from_slice(&[i, i + 1, i + 2, i + 3]))
                .unwrap();
            assert_eq!(cons.read(4, <[u8]>::to_vec), &[i, i + 1, i + 2, i + 3]);
        }
        assert_eq!(cons.available(), 0);
    }

    #[test]
    fn write_closure_buffer_length() {
        let (mut prod, _cons) = new(64);
        prod.write(13, |buf| {
            assert_eq!(buf.len(), 13);
        })
        .unwrap();
    }

    #[test]
    fn successive_reads() {
        let (mut prod, mut cons) = new(64);

        prod.write(4, |buf| buf.copy_from_slice(&[1u8, 2, 3, 4]))
            .unwrap();
        prod.write(4, |buf| buf.copy_from_slice(&[5u8, 6, 7, 8]))
            .unwrap();

        assert_eq!(cons.read(4, <[u8]>::to_vec), &[1u8, 2, 3, 4]);
        assert_eq!(cons.read(4, <[u8]>::to_vec), &[5u8, 6, 7, 8]);
        assert_eq!(cons.available(), 0);
    }

    #[test]
    fn successive_reads_across_capacity_boundary() {
        let (mut prod, mut cons) = new(16);

        // Advance heads to offset 12.
        prod.write(12, |buf| buf.fill(0x00)).unwrap();
        cons.read(12, |_| {});

        prod.write(8, |buf| buf.fill(0xAA)).unwrap();
        prod.write(4, |buf| buf.fill(0xBB)).unwrap();

        assert_eq!(cons.available(), 12);
        assert_eq!(cons.peek(4), &[0xAAu8; 4]);
        assert_eq!(cons.peek(8), &[0xAAu8; 8]);
        assert_eq!(cons.available(), 12);
        assert_eq!(cons.read(8, <[u8]>::to_vec), &[0xAAu8; 8]);
        assert_eq!(cons.available(), 4);

        assert_eq!(cons.read(4, <[u8]>::to_vec), &[0xBBu8; 4]);
        assert_eq!(cons.available(), 0);
    }

    #[test]
    #[should_panic(expected = "queue capacity must be > 0")]
    fn new_panics_on_zero_capacity() {
        new(0);
    }

    #[test]
    fn capacity_rounds_to_next_power_of_two() {
        // new(3) must round up to 4. Verify indirectly: writing 4 bytes
        // (the rounded capacity) succeeds, writing 5 bytes is oversized.
        let (mut prod, _cons) = new(3);
        assert!(prod.write(4, |_| {}).is_ok());

        let (mut prod, _cons) = new(3);
        assert!(matches!(
            prod.write(5, |_| {}),
            Err(WriteError::OversizedWrite)
        ));
    }

    #[test]
    fn full_capacity_write_after_partial_drain() {
        let (mut prod, mut cons) = new(16);

        prod.write(8, |buf| buf.fill(0x00)).unwrap();
        cons.read(8, |_| {});

        prod.write(16, |buf| buf.fill(0xCC)).unwrap();
        assert_eq!(cons.available(), 16);
        assert_eq!(cons.read(16, <[u8]>::to_vec), &[0xCCu8; 16]);
        assert_eq!(cons.available(), 0);
    }

    #[test]
    fn read_race_advance_before_data_used() {
        let (mut prod, mut cons) = new(4);

        // Fill the queue with known values.
        prod.write(4, |buf| buf.copy_from_slice(&[1u8, 2, 3, 4]))
            .unwrap();

        // Producer thread: blocked because the queue is full. Once read_pos
        // advances (inside Consumer::read), it immediately overwrites the same
        // bytes with 0xFF.
        let handle = std::thread::spawn(move || {
            loop {
                if prod.write(4, |buf| buf.fill(0xFF)).is_ok() {
                    break;
                }
                std::hint::spin_loop();
            }
        });

        // advance() fires after the closure returns. The producer is still
        // blocked while we inspect the slice inside the closure.
        let got = cons.read(4, |slice| {
            let copy: [u8; 4] = slice.try_into().unwrap();
            copy
        });

        // Only now does advance unblock the producer; it may overwrite those
        // bytes with 0xFF, but we already have an owned copy.
        handle.join().unwrap();

        assert_eq!(got, [1u8, 2, 3, 4]);
    }

    #[test]
    fn threaded_producer_consumer() {
        let (mut prod, mut cons) = new(64);

        let consumer_thread = std::thread::spawn(move || {
            // Spin until 4 bytes are available, then read and return a copy.
            loop {
                if cons.available() >= 4 {
                    return cons.read(4, <[u8]>::to_vec);
                }
                std::hint::spin_loop();
            }
        });

        prod.write(4, |buf| buf.copy_from_slice(&[1u8, 2, 3, 4]))
            .unwrap();

        let received = consumer_thread.join().unwrap();
        assert_eq!(received, &[1u8, 2, 3, 4]);
    }
}
