# ADR 0005: Per-thread SPSC queues with split producer/consumer ownership

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

The hot path must be lock-free and allocation-free. That requires
single-producer/single-consumer (SPSC) queues, which in turn forces a decision
about what a queue belongs to and how its two halves are owned across the
thread boundary.

## Considered Options

For **what owns a queue**:

- **Per-logger queue / global MPSC.** Rejected: threads that share a logger
  contend on the same queue, reintroducing the locking SPSC was meant to avoid.
  A global MPSC contends across all threads.
- **Per-thread SPSC.** Each thread owns one queue shared across every logger it
  uses. Threads are the actual source of contention, so per-thread queues are
  the only shape that keeps every write lock-free.

For **how the two halves are owned** ("Shape B"):

- Producer half lives in thread-local storage; consumer half lives in the
  backend; a shared `Arc<AtomicBool>` (`alive`) lets the backend detect thread
  death. Each side holds only what it uses — one fewer indirection than a shared
  handle holding both halves.

## Decision Outcome

Chosen: **per-thread SPSC queues, split ownership (Shape B).**

- A queue belongs to a *thread*, not a logger. An `Arc<Logger>` cloned across N
  threads produces records flowing through N different queues. This is *why* the
  record must carry logger identity (see
  [ADR 0004](0004-raw-logger-pointer-dispatch.md)).
- The producer half lives in a `thread_local!` slot; the consumer half is held
  by the backend. A shared `alive: Arc<AtomicBool>` (init `true`) is flipped to
  `false` by the producer's `Drop`, signalling thread death.
- Registration is lazy and one-shot per thread: the first log call from a thread
  allocates its queue, builds the shared flag, installs the producer in TLS, and
  pushes the consumer into the backend. Every later call reuses the TLS handle
  and touches no shared state.
- That one-shot cost is also exposed as `preallocate_thread()`, a free function
  (not a method on `Logger`, since the queue has no per-logger identity) for
  latency-sensitive threads to pay registration up front, e.g. from a thread
  pool's `on_thread_start`.

### Consequences

- Library code logs through an ambient `&Logger` without plumbing per-thread
  state through its API — TLS holds the per-thread piece.
- The backend detects an exited thread via `alive == false`, drains that
  queue to empty, then retires the consumer (see
  [ADR 0012](0012-shutdown-drain-then-stop-raii-guard.md)).
- Records carry logger identity precisely because queues are not per-logger.

## Implementation status

Implementation note: because each consumer is shared as
`Arc<PerThreadConsumer>` for the snapshot-and-iterate pattern, the inner
`Consumer` sits behind an **uncontended `Mutex<Consumer>`** — only the worker
thread ever reads it (`insomnilog/src/per_thread_queue.rs:24`).
