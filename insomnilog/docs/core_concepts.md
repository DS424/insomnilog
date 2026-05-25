# Core concepts

There are three objects you work with directly in `insomnilog`: a **Backend**,
one or more **Sinks**, and one or more **Loggers**. Understanding what each one
owns and what it controls makes the rest of the API straightforward.

## Backend

The backend is the engine that drives everything. It owns the worker thread,
the per-thread queues, and the registries that keep loggers and sinks alive. You
start it once at the beginning of your program and shut it down at the end:

```rust
{{#include ../../examples/examples/core_concepts.rs:backend}}
```

The returned `ShutdownGuard` keeps the backend alive. When it drops — at the
end of `main`, or when you call `insomnilog::shutdown()` explicitly — the
worker thread drains every pending log record and exits cleanly.

Everything else (sinks, loggers, log macros) requires the backend to be running.
Calling any of them before `start()` panics immediately.

## Sink

A `Sink` is the destination for log records. It receives a decoded `LogRecord`
and decides what to do with it — write formatted text to stdout, serialise to
JSON, forward to a metrics system, or anything else.

Every sink has a **level filter** set at construction time. Records below that
level are skipped by the backend before `write_record` is ever called.

`insomnilog` ships with two built-in sinks:

- **`ConsoleSink`** — formats records using a [`Formatter`] and writes them to
  stdout. The default formatter produces human-readable timestamped lines.
- **`NullSink`** — silently discards every record. Useful in tests.

Sinks are heap-allocated and reference-counted. You wrap one in an `Arc` and
register it with a name before attaching it to any logger:

```rust
{{#include ../../examples/examples/core_concepts.rs:sink}}
```

Registration is optional if you only use the sink with a single logger — you
can pass the `Arc` directly to `create_logger` without registering it. The
registry exists so that multiple loggers can share a named sink looked up at
configuration time. A registered sink can be retrieved at any point with
`insomnilog::get_sink("name")`.

## Logger

A `Logger` is the handle you pass to every log macro at the call site. It holds:

- A **name** — unique within the process.
- A **level filter** — the producer-side gate that runs on the calling thread
  before anything is written to the queue.
- A **list of sinks** — fixed at creation; every record that passes the level
  filter is delivered to all of them.

You create a logger once, typically during application startup, and store the
`Arc<Logger>` wherever your code needs it:

```rust
{{#include ../../examples/examples/core_concepts.rs:logger}}
```

The logger's level and the sink's level work **in series**: the logger's level
is checked first on the hot path (the calling thread), and the sink's level is
checked second on the backend thread. A record must pass both filters to reach
a sink's `write_record`. This means the effective level for any sink is
`max(logger.level, sink.level)` — setting a sink to `Trace` while the logger
is at `Info` will never produce `Debug` or `Trace` output; lower the logger's
level instead.

The logger's level is the only property that can be changed after creation,
via [`Logger::set_level`]. The sink list is immutable. A registered logger can
be retrieved at any point with `insomnilog::get_logger("name")`.

## Putting it together

```text
Backend (started once, lives until ShutdownGuard drops)
  │
  ├── Sink "console"  ← level: Trace
  ├── Sink "errors"   ← level: Error
  │
  ├── Logger "app"    ← level: Info, sinks: ["console"]
  └── Logger "audit"  ← level: Trace, sinks: ["console", "errors"]
```

A `log_debug!` call on logger `"app"` is dropped immediately on the calling
thread — the logger's `Info` filter rejects it before the queue is touched.

A `log_error!` call on logger `"audit"` passes the `Trace` filter, enters the
queue, and is delivered to both sinks on the backend thread.
