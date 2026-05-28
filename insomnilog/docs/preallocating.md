# Preallocating thread queues

Every thread that logs through `insomnilog` needs its own SPSC queue.
By default that queue is allocated on the **first log call** from a new thread.
That lazy allocation involves a heap allocation and a mutex lock to register the
queue with the backend — work that does not belong on a real-time or
latency-sensitive hot path.

`preallocate_thread` moves that cost to a controlled point before the hot path
begins:

```rust
{{#include ../examples/preallocating.rs:main_thread}}
```

After this call every subsequent log macro call on the same thread is
allocation-free and lock-free. The function is **idempotent** — calling it more
than once from the same thread is a no-op.

The same applies to any thread that will log. For spawned threads, call
`preallocate_thread` at the start of the thread body before doing any work:

```rust
{{#include ../examples/preallocating.rs:worker_thread}}
```

For thread pools, the right place is the pool's thread-start hook (sometimes
called `on_thread_start` or similar), so every worker pays the cost once at
startup rather than on the first log call during a request.

<details>
<summary>Full example</summary>

```rust
{{#include ../examples/preallocating.rs}}
```

</details>

