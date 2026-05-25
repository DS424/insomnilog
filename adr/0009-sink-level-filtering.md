# ADR 0009: Sink filtering is `LogLevel`-only, fixed at construction

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

A logger already filters on the hot path by its own atomic `level`. We had to
decide whether sinks have independent filtering, how expressive it is, and
whether it can change at runtime — and to surface the interaction between the
two filters, which is easy to get wrong.

## Considered Options

- **Predicate filters per sink** (`fn accepts(&self, record) -> bool`).
  Rejected for v1: every realistic case (per-module routing, tag-based routing)
  is covered by combining per-logger levels with multiple named loggers. Kept as
  an additive future option.
- **Mutable per-sink level** (`set_level`, atomic storage). Rejected: the
  runtime-tunable knob is the *logger's* level (already atomic); a sink's level
  is a construction-time property.
- **Fixed `LogLevel` at construction (chosen).** `Sink::level()` returns the
  level the sink was built with; the backend skips a sink when
  `record.level < sink.level()`.

## Decision Outcome

Chosen: **`LogLevel`-only sink filter, fixed at construction; no `set_level`,
no atomic.**

- Two filters run in series:
  1. The macro drops records below `logger.level` on the hot path
     (`AtomicU8` `Relaxed` load).
  2. The backend then skips sinks below `sink.level()` per surviving record.
- A sink's **effective level is `max(logger.level, sink.level)`** — a sink
  configured *more permissive* than its logger never receives the difference,
  because the producer-side filter already discarded those records. To get more
  output through a sink, lower the *logger's* level, not the sink's.

### Consequences

- This asymmetry is a documented footgun: a `Debug` sink under an `Info` logger
  silently sees no `Debug` output. Both the `Sink::level()` doc and the
  `create_logger` doc must call it out.
- Runtime level changes go through `Logger::set_level`; sink levels are
  immutable, keeping the sink free of interior mutability.
- If predicate filtering is ever needed, the additive upgrade is a default
  `fn accepts(&self, record) -> bool { self.level() <= record.level }`.
