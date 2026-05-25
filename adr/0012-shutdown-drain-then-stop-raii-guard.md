# ADR 0012: Shutdown drains then stops, driven by an RAII guard

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

Two related questions: (1) what does shutdown do with records still queued, and
(2) how is shutdown triggered at process exit, given that Rust statics are never
dropped so there is no native "static destructor drains at exit" hook.

## Considered Options

For **shutdown behaviour**:

- **Drain-then-stop by default**, with an opt-out flag, and **no timeout** in
  v1 — the contract is that the user stops producing before calling shutdown.
  A timeout-abort knob can be added later if a pathological "producer won't
  stop" case appears.

For **exit triggering**:

- **`libc::atexit` / a `#[dtor]` crate.** Rejected: callbacks can't capture, run
  after significant process teardown, have fragile ordering vs other libraries'
  atexit handlers, and differ on Windows MSVCRT.
- **RAII `ShutdownGuard` returned from `start()` (chosen).** Pure Rust,
  cross-platform, FFI-free, well-defined drop order; matches
  `tracing_subscriber::WorkerGuard`. Trade-off: the user must bind the guard for
  the lifetime they want logging alive.

## Decision Outcome

Chosen: **drain-then-stop by default; shutdown driven by an RAII guard.**

- `start()` returns `Result<ShutdownGuard, AlreadyStarted>`. Dropping the guard
  calls `shutdown()`. Users bind it in `main`:
  `let _guard = insomnilog::start(opts)?;`. The type is `#[must_use]` so the
  compiler warns on `let _ = start(...)`, which would drop the guard
  immediately and tear the backend straight back down.
- `BackendOptions::wait_for_queues_to_empty_before_exit` (default `true`): when
  `true`, the worker drains every consumer queue once shutdown is observed, then
  exits; when `false`, it exits as soon as the flag is observed (records may be
  dropped). No timeout in v1.
- `shutdown()` is also exposed directly and is **idempotent** — calling it
  before the guard drops, then letting the guard drop, must not panic or
  double-drain. It is even a no-op if `start()` was never called.
- During drain the logger registry is kept alive so every `logger_ptr` in flight
  stays valid (ties to [ADR 0004](0004-raw-logger-pointer-dispatch.md));
  registries drop last. A dead thread's consumer (`alive == false`, queue empty)
  is retired mid-run, not only at shutdown.

### Consequences

- Drain-on-exit is achieved through the guard, not "for free" — the user is
  responsible for binding it for the desired lifetime.
- No FFI, no atexit-ordering hazards, deterministic teardown.
- A producer that never stops while `wait_for_queues_to_empty_before_exit` is
  `true` could block shutdown indefinitely; accepted for v1 (timeout knob is a
  documented future addition).
