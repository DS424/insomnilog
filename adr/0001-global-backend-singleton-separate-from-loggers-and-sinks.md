# ADR 0001: Global Backend singleton, separate from loggers and sinks

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

In the legacy design `Logger` and the backend thread were conjoined: building a
`Logger` spawned a backend, and the backend died when the last `Logger` clone
dropped. There was no separation between *who emits records* and *who processes
them*, and no way to share one worker thread across independently-created
loggers.

Two questions had to be answered together:

1. How are the worker thread and its registries owned relative to loggers and
   sinks?
2. How does a caller — including library code deep in the call stack — reach
   the logging system?

## Considered Options

- **Conjoined logger+backend (legacy).** Rejected: couples lifetime of the
  worker to logger clones, no shared worker, no registries.
- **Explicit `Backend` handle threaded through APIs.** A `Backend` value the
  caller passes around. Rejected: "logging is ambient" — library code should be
  able to take `&Logger` and log without plumbing backend state through every
  signature.
- **Global singleton accessed through free functions.** One process-wide
  `Backend` in a `OnceLock`, reached via `start()` and free functions
  (`create_logger`, `register_sink`, …). Loggers and sinks are independent
  named objects owned by registries *inside* the backend.

## Decision Outcome

Chosen: **global singleton, separate from loggers and sinks.**

- A single `Backend` owns the worker thread, the logger/sink/consumer
  registries, and the immutable options. It is created once by `start(opts)`
  and stored in a crate-level `OnceLock`.
- Loggers and sinks are independent objects, registered by name, owned by
  registries living inside the backend. They are decoupled from the worker
  thread's lifetime.
- Public API is a set of free functions over the singleton, matching Quill and
  Python's `logging` "logging is ambient" model.

### Consequences

- Library code logs through an `&Logger`/`Arc<Logger>` without ever naming the
  backend.
- Test isolation cannot rely on tearing down a per-test backend; it is
  delegated to the harness. The project runs `cargo nextest run`, which gives
  each test its own process (hence a fresh `OnceLock`), and each doctest
  compiles to its own binary. Running plain `cargo test` would break isolation
  and is forbidden by the project's tooling.
- The singleton is one-shot per process (see [ADR 0011](0011-no-auto-start-panic-when-not-started.md)
  and [ADR 0012](0012-shutdown-drain-then-stop-raii-guard.md) for start/stop
  semantics).
