# ADR 0011: No implicit auto-start; using the logger before `start()` panics

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

The backend is a process-wide singleton initialised by `start(options)` (see
[ADR 0001](0001-global-backend-singleton-separate-from-loggers-and-sinks.md)).
What should happen if `create_logger`, `register_sink`, `get_*`, or a log macro
runs before `start()` has been called?

## Considered Options

- **Implicit auto-start with default options.** Whichever call runs first
  silently spawns the backend with `BackendOptions::default()`. Rejected: a
  later explicit `start(custom_opts)` would then either fail with
  `AlreadyStarted` or silently lose its options (queue capacity, thread name,
  drain policy) — both worse than failing loudly.
- **Panic with a clear message (chosen).** No backend is created implicitly;
  every entry point requires `start()` to have run first.

## Decision Outcome

Chosen: **panic if the backend has not been started.**

- `create_logger`, `get_logger`, `register_sink`, `get_sink`, and the log macros
  (via the producer-registration path) all panic when `start()` has not been
  called, with the message:
  `"insomnilog: call insomnilog::start() before using the logger"`.
- There is no implicit auto-start and no default-options fallback.

### Consequences

- Misconfiguration surfaces immediately and unambiguously rather than silently
  committing to default options.
- Tests and examples must call `start(BackendOptions::default())` in setup. A
  `start_for_tests()` convenience could be added later if the boilerplate
  becomes painful.
- Pairs with the one-shot `start` semantics in
  [ADR 0012](0012-shutdown-drain-then-stop-raii-guard.md): `start` returns
  `AlreadyStarted` on a second call rather than auto-restarting.
