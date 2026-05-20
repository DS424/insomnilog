//! [`PerThreadProducer`] + [`PerThreadConsumer`]: the two halves of a per-thread SPSC queue.
//!
//! Each logging thread owns a [`PerThreadProducer`] (the write half). The backend
//! owns the matching [`PerThreadConsumer`] (the read half). The `alive` flag lets
//! the backend detect when a thread has exited and its queue can be drained
//! and retired.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::queue::{Consumer, Producer};

/// Backend-owned read half of a per-thread SPSC queue.
///
/// Holds the consumer half of the per-thread SPSC queue and the shared
/// `alive` flag. The backend creates one `PerThreadConsumer` per registered
/// logging thread via [`new`].
pub struct PerThreadConsumer {
    /// Consumer half of the per-thread SPSC queue, behind a `Mutex` so that
    /// `Arc<PerThreadConsumer>` can be shared with the worker loop. The Mutex is
    /// always uncontended in practice: only the worker thread reads from it.
    pub(crate) consumer: Mutex<Consumer>,
    /// Shared liveness flag. `true` while the producer thread is alive;
    /// flipped to `false` by [`PerThreadProducer`]'s `Drop` impl.
    pub(crate) alive: Arc<AtomicBool>,
}

/// Thread-local write half of a per-thread SPSC queue.
///
/// Wraps the producer half of the queue and the shared `alive` flag.
/// When dropped — either because the thread exits or because its TLS slot is
/// cleared — `alive` is set to `false` with a `Release` store so the backend
/// can retire the paired [`PerThreadConsumer`].
///
/// # Not `Send`
///
/// `PerThreadProducer` is deliberately `!Send`. The SPSC producer must only be
/// written by the thread that created it; moving it to another thread would
/// violate that contract. The `PhantomData<*mut ()>` field opts the type out
/// of `Send` (raw pointers are `!Send` by definition).
pub struct PerThreadProducer {
    /// Producer half of the per-thread SPSC queue.
    pub(crate) producer: Producer,
    /// Shared liveness flag. Cleared to `false` in [`Drop::drop`].
    pub(crate) alive: Arc<AtomicBool>,
    /// Makes `PerThreadProducer` `!Send` + `!Sync`. The producer half of the
    /// SPSC queue must stay on its creating thread.
    _not_send: PhantomData<*mut ()>,
}

impl Drop for PerThreadProducer {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

/// Creates a matched [`PerThreadProducer`] + [`PerThreadConsumer`] pair backed
/// by a fresh SPSC queue of `capacity` bytes.
///
/// Both halves share the same `Arc<AtomicBool>` liveness flag, which starts
/// as `true` and is cleared to `false` when the [`PerThreadProducer`] is dropped.
pub fn new(capacity: usize) -> (PerThreadProducer, PerThreadConsumer) {
    let (producer, consumer) = crate::queue::new(capacity);
    let alive = Arc::new(AtomicBool::new(true));
    let pt_producer = PerThreadProducer {
        producer,
        alive: Arc::clone(&alive),
        _not_send: PhantomData,
    };
    let pt_consumer = PerThreadConsumer {
        consumer: Mutex::new(consumer),
        alive,
    };
    (pt_producer, pt_consumer)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    #[test]
    fn alive_starts_true() {
        let (producer, consumer) = new(64);
        assert!(
            producer.alive.load(Ordering::Acquire),
            "producer.alive must be true after construction",
        );
        assert!(
            consumer.alive.load(Ordering::Acquire),
            "consumer.alive must be true after construction",
        );
    }

    #[test]
    fn both_halves_share_same_arc() {
        let (producer, consumer) = new(64);
        assert!(
            Arc::ptr_eq(&producer.alive, &consumer.alive),
            "producer and consumer must share the same Arc<AtomicBool>",
        );
    }

    #[test]
    fn drop_handle_sets_alive_false() {
        let (producer, consumer) = new(64);
        drop(producer);
        assert!(
            !consumer.alive.load(Ordering::Acquire),
            "consumer.alive must be false after PerThreadProducer is dropped",
        );
    }

    #[test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard is needed for all operations through the end of the test"
    )]
    fn queue_roundtrip_through_wrappers() {
        let (mut producer, consumer) = new(64);

        producer
            .producer
            .write(4, |buf| buf.copy_from_slice(&[1u8, 2, 3, 4]))
            .expect("write must succeed on a fresh queue");

        let mut consumer = consumer.consumer.lock().unwrap();

        assert_eq!(
            consumer.available(),
            4,
            "consumer must see 4 bytes after the write",
        );

        // peek does not advance the read position.
        assert_eq!(
            consumer.peek(4),
            &[1u8, 2, 3, 4],
            "peek must return the written bytes without consuming them",
        );
        assert_eq!(
            consumer.available(),
            4,
            "available must still be 4 after peek",
        );

        // read advances the read position.
        let got = consumer.read(4, <[u8]>::to_vec);
        assert_eq!(got, &[1u8, 2, 3, 4], "read must return the written bytes");
        assert_eq!(
            consumer.available(),
            0,
            "queue must be empty after consuming all bytes",
        );
    }
}
