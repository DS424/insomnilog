# ADR 0007: Backend robustness — `catch_unwind` per record + re-entrancy guard

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

Sinks are user-implementable, so they can panic in practice (a SQL connection
error, a serialisation bug) and they can misbehave (calling a log macro from
inside `write_record`). Two failure modes threaten the single backend worker
thread:

1. **A panicking sink** that, uncaught, unwinds the worker thread and silently
   kills logging for the rest of the process — queues keep filling, nothing
   drains, no error surfaces.
2. **Re-entrant logging from a sink**: if `write_record` logs, the backend
   thread would try to register itself as a producer and drain its own queue
   while inside dispatch — an infinite loop.

## Considered Options

- **"Sinks must not panic" contract, no guard.** Rejected as unrealistic for
  user-supplied sinks, and a single panic silently breaks all logging.
- **`catch_unwind` per sink.** More granular but more overhead; a panic in one
  sink need not let later sinks see the record anyway.
- **`catch_unwind` per record (chosen)** around the whole sink fan-out, plus an
  explicit re-entrancy guard on producer registration.

## Decision Outcome

Chosen: **one `catch_unwind` per record around the fan-out, with error/panic
counters, plus a backend-thread re-entrancy guard.**

- The worker wraps each record's sink fan-out in
  `panic::catch_unwind(AssertUnwindSafe(...))`. On a caught panic it bumps a
  `panic_count` and writes one line to stderr; the worker stays alive. The cost
  (nanoseconds) is irrelevant because dispatch is not the hot path — the hot
  path is the producer side.
- Behaviour on a sink panic: that record is lost for *later* sinks in the same
  logger (the fan-out loop unwinds), but other records, other loggers, and other
  threads keep flowing.
- **Re-entrancy guard:** the shared producer-registration helper compares
  `thread::current().id()` against the backend worker's `ThreadId` (captured at
  startup) and panics if they match. That panic is then caught by the dispatch
  `catch_unwind`, so a re-entrant sink increments `panic_count` rather than
  hanging. The guard costs a single id load+compare on the registration path
  only; the hot path (TLS slot already populated) pays nothing.

### Consequences

- A buggy or panicking sink degrades to lost records for that record's later
  sinks, never a dead logging subsystem.
- A sink that logs is contained (panic, counted) instead of deadlocking the
  worker.
- The backend additionally tallies `write_errors`/`flush_errors` (from the
  `Result`-returning sink API, see
  [ADR 0002](0002-sinks-own-formatting-decode-once-pass-logrecord.md)) and
  `dropped_records` (full-queue drops, see
  [ADR 0010](0010-logger-passed-explicitly-no-name-lookup-macro.md)).

## Implementation status

Not yet implemented: the counters are **reported at shutdown via stderr**
(`insomnilog/src/backend.rs:261`-`:274`) but are **not programmatically
queryable** — `panic_count` lives on the non-public `BackendRunner` and there is
no public `Backend::panic_count()` getter. Callers that want to detect silent
breakage at runtime cannot do so today; exposing a getter would be an additive
change.
