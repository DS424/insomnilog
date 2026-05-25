# ADR 0010: Logger passed explicitly at the call site; no name-lookup macro

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

A logging macro needs to know which logger a call belongs to. A common
convenience is a name-lookup form, `log_info!("name", "msg", args)`, that
resolves the logger from the registry at each call site. We had to decide
whether to support it.

## Considered Options

- **Name-lookup macro form.** Rejected — permanently, not just for v1. A
  registry lookup on every call site would add a `RwLock::read` +
  `HashMap::get` + `Arc::clone` to the hot path, which defeats the library's
  entire value proposition (zero locks, zero allocations on the hot path).
- **Explicit logger handle (chosen).** The only supported form is
  `log_info!(logger, "msg {}", arg)` where `logger` is an `&Logger` /
  `Arc<Logger>` the caller already holds. Callers fetch the logger once at
  startup and store it.

## Decision Outcome

Chosen: **logger is always passed explicitly; no name-lookup macro form, ever.**

- The macro takes an expression that derefs to `&Logger` (so both `&Logger` and
  `Arc<Logger>` work). The hot path is: atomic `Relaxed` level load → bail if
  below threshold → static per-call-site `METADATA` → reserve bytes in the
  thread-local producer → write header (`logger as *const Logger as usize` into
  `logger_ptr`) + encoded args via `ptr::copy_nonoverlapping`.
- On a full queue, `Producer::write` returns `Err(QueueFull)`, which the macro
  **silently discards** — the caller never sees an error (a hard invariant). The
  backend's `dropped_records` counter is bumped so the loss is reported at
  shutdown.

### Consequences

- Zero registry traffic on the hot path; the only per-call cost before the queue
  write is the atomic level load and per-call-site static metadata.
- Callers must hold and pass a logger handle. There is no ambient
  "current logger" lookup by name.
- Distinct call sites produce distinct `&'static LogMetadata` (one static per
  expansion), so the backend can recover file/line/module per record.
