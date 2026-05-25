# ADR 0006: Explicit `register_sink`; a duplicate name is an error

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

Sinks are registered by name. We had to decide the registration API shape and
what happens when two registration sites use the same name.

## Considered Options

- **`get_or_create_sink(name, build: FnOnce) -> Arc<dyn Sink>`.** Atomic lazy
  construction: build only if the slot is free, first-call-wins on collision.
  Rejected: it silently hides the "which definition wins" ambiguity — two
  independent sites with different configs produce whichever runs first, with no
  signal to the loser. The `FnOnce` existed only to make construction lazy, a
  concern that evaporates once registration is explicit.
- **Explicit `register_sink(name, Arc<dyn Sink>) -> Result<(), SinkAlreadyRegistered>`.**
  Caller constructs the `Arc` and passes it in; a duplicate name is an error
  carrying the existing `Arc`.

## Decision Outcome

Chosen: **explicit registration; duplicate name is an error.**

- `register_sink(name, sink)` inserts on success. On a name collision it returns
  `Err(SinkAlreadyRegistered { existing })`, carrying the previously-registered
  `Arc` so the caller can inspect or compare it.
- `get_sink(name)` is a separate lookup returning `Option<Arc<dyn Sink>>`.
- The same explicit-with-collision-error rule applies to loggers via
  `create_logger` / `LoggerAlreadyRegistered`.

### Consequences

- No hidden first-call-wins: conflicting registrations are surfaced, not
  silently dropped.
- Sink construction is paid unconditionally by the caller; this is fine because
  sink construction is cheap and off the hot path.
- The lookup-and-insert is performed under the registry write lock, so
  concurrent registrations of the same name are serialised: exactly one wins,
  the rest receive `Err`.

