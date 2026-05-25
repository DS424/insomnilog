# ADR 0004: Per-record logger identity is a raw `*const Logger`

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

Queues are per-thread, not per-logger (see
[ADR 0005](0005-per-thread-spsc-queues-split-ownership.md)), so a record must
carry **which logger emitted it** for the backend to dispatch it to the right
sinks after decoding. Backend dispatch runs once per record, so the encoding of
that identity is on a per-record path and its cost matters. Three things had to
be decided together because they are interdependent:

1. How is logger identity encoded in the record header?
2. What keeps that identity valid by the time the backend reads it?
3. What may mutate on a `Logger` at runtime?

## Considered Options

For **identity encoding**:

| Option | Per-record cost | Safety | Verdict |
| --- | --- | --- | --- |
| Logger name (string) | string bytes + `HashMap` lookup | safe via lookup | too expensive |
| Integer ID | 4 bytes + `Vec` index | safe via lookup | avoidable indirection |
| Raw `*const Logger` | 8 bytes + pointer deref | needs lifetime guarantee | cheapest; viable |

For **lifetime safety**: `Weak`-in-registry (cleanup when last user `Arc`
drops) vs **strong `Arc` in registry** (process-lifetime pinning).

For **mutability**: fully mutable `sinks` (needs `ArcSwap`/`RwLock` read on
every dispatch) vs **immutable `sinks`/`name`, atomic `level` only**.

## Decision Outcome

Chosen: **raw `*const Logger` in the record header, made sound by a strong-`Arc`
registry with no runtime removal in v1, and an otherwise-immutable `Logger`.**

- The record header carries `logger_ptr: usize`. The producer writes
  `logger as *const Logger as usize`; the backend casts it back to `&Logger`
  and iterates `logger.sinks()` directly — no registry lookup, no integer-ID
  indirection, no `Weak::upgrade` on the dispatch path.
- **Lifetime safety:** the logger registry holds a strong `Arc<Logger>` for the
  process lifetime. There is no `remove_logger` in v1. Once registered, a logger
  outlives every record that can reference it, so the raw pointer is always
  valid when dereferenced.
- **Immutability:** a `Logger`'s `name` and `sinks` are fixed at construction;
  only `level` (an `AtomicU8`) changes at runtime. This lets the backend read
  `logger.sinks()` per record with no lock and makes `Logger` trivially `Sync`
  with no interior-mutability gymnastics.

### Consequences

- The per-record dispatch path is pointer-deref + slice iteration: no locks, no
  map lookups, no atomic upgrades.
- Registering many uniquely-named loggers/sinks grows memory monotonically
  (no cleanup). Acceptable for realistic use (dozens of named loggers).
- No `Logger::add_sink`/`remove_sink` and no `set_sinks`. To reconfigure sinks,
  construct a new logger. The documented upgrade path, if live reconfiguration
  is ever needed, is to swap `sinks` for an `ArcSwap<[Arc<dyn Sink>]>` behind
  the same `&[Arc<dyn Sink>]` read API.
- Future runtime removal is purely additive (add `valid: AtomicBool`, a
  `remove_*` that pushes to a backend "pending cleanup" list drained once no
  queue references it). It does not change the header format, macros, traits, or
  hot path — so this v1 choice does not constrain a later move.
- Every `unsafe { &*logger_ptr }` deref must cite the strong-`Arc`
  process-lifetime guarantee in a `SAFETY:` comment.
