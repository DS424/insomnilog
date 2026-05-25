# About insomnilog
`insomnilog` is an asynchronous Rust logging library designed for real-time and
latency-sensitive applications. All logging on the hot path is **zero-allocation**
and **lock-free** — log records are serialised as raw bytes into a per-thread
ring buffer and decoded by a background worker thread.

## Design goals

- **Real-time safe hot path.** The logging macros never allocate heap memory,
  never acquire a lock, and never block. They perform a level check, encode
  arguments directly into a pre-allocated ring buffer, and return.

- **Asynchronous by default.** Formatting, sink dispatch, and I/O happen on a
  dedicated backend worker thread. The calling thread pays only the cost of the
  ring-buffer write.

- **Explicit configuration, no global magic.** You start the backend once with
  `insomnilog::start(opts)`, create named loggers and sinks explicitly, and pass
  a logger reference to each call site. There is no implicit global logger, no
  auto-start, and no hidden process-exit hook.

- **Composable sinks.** A `Sink` receives a decoded `LogRecord` and decides its
  own output format. One sink can write human-readable text, another can write
  structured JSON, a third can forward to a metrics system — all from the same
  logger.

## Inspiration

`insomnilog` is heavily inspired by [Quill](https://github.com/odygrd/quill),
a C++ low-latency logging library with similar goals: asynchronous background
processing, per-thread lock-free queues, and a hot path that does as little
as possible.
