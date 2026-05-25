# ADR 0003: Expose loggers and sinks as `Arc<T>`, not `&'static`

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

Registered loggers and sinks live for the process lifetime (see
[ADR 0004](0004-raw-logger-pointer-dispatch.md)). Since they are effectively
permanent, an arena that leaks them and hands out `&'static T` references was a
plausible way to avoid atomic reference-count traffic. We had to choose the
user-facing handle type.

## Considered Options

- **`&'static T` via a leaking arena.** Zero refcount traffic. Rejected: the
  `'static` lifetime leaks into every user-facing type — struct fields and
  function signatures that hold a logger must all carry `'static`, which is
  viral and un-ergonomic. The only runtime win (no `Arc` refcount bumps) is
  negligible because the hot dispatch path does **not** go through the `Arc` at
  all — it dereferences a raw `*const Logger` from the record header
  (see [ADR 0004](0004-raw-logger-pointer-dispatch.md)).
- **`Arc<T>`.** Standard Rust ergonomics; the registry holds the authoritative
  strong `Arc`, callers get cheap cloned `Arc`s.

## Decision Outcome

Chosen: **`Arc<T>`.**

- `create_logger` returns `Arc<Logger>`; `get_logger`/`get_sink` return
  `Option<Arc<…>>`; `register_sink` takes an `Arc<dyn Sink>`.
- The registry holds the authoritative strong `Arc`; dropping every user-side
  clone leaves the registry's `Arc` as the owner.

### Consequences

- Logger/sink handles compose naturally into user types with no lifetime
  annotations.
- A cheap `Arc::clone` (one atomic increment) per `get_*` lookup — paid off the
  hot path, at logger-acquisition time, not per log call.
- No measurable cost on the dispatch path, which uses the raw pointer rather
  than the `Arc`.
