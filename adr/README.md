# Architecture Decision Records

Developer-facing records of the significant architecture decisions.

Format: [MADR](https://adr.github.io/madr/) (Markdown Any Decision Records).

## Index

| # | Title | Status |
|---|-------|--------|
| [0001](0001-global-backend-singleton-separate-from-loggers-and-sinks.md) | Global Backend singleton, separate from loggers and sinks | Accepted |
| [0002](0002-sinks-own-formatting-decode-once-pass-logrecord.md) | Sinks own formatting; decode once and pass a `LogRecord` | Accepted |
| [0003](0003-loggers-and-sinks-exposed-as-arc.md) | Expose loggers and sinks as `Arc<T>`, not `&'static` | Accepted |
| [0004](0004-raw-logger-pointer-dispatch.md) | Per-record logger identity is a raw `*const Logger` | Accepted |
| [0005](0005-per-thread-spsc-queues-split-ownership.md) | Per-thread SPSC queues with split producer/consumer ownership | Accepted |
| [0006](0006-explicit-sink-registration.md) | Explicit `register_sink`; a duplicate name is an error | Accepted |
| [0007](0007-backend-robustness-catch-unwind-and-reentrancy-guard.md) | Backend robustness: `catch_unwind` per record + re-entrancy guard | Accepted |
| [0008](0008-bounded-per-consumer-batch-fairness.md) | Bounded per-consumer work per pass for fairness | Accepted |
| [0009](0009-sink-level-filtering.md) | Sink filtering is `LogLevel`-only, fixed at construction | Accepted |
| [0010](0010-logger-passed-explicitly-no-name-lookup-macro.md) | Logger passed explicitly at the call site; no name-lookup macro | Accepted |
| [0011](0011-no-auto-start-panic-when-not-started.md) | No implicit auto-start; using the logger before `start()` panics | Accepted |
| [0012](0012-shutdown-drain-then-stop-raii-guard.md) | Shutdown drains then stops, driven by an RAII guard | Accepted |
| [0013](0013-sinks-compose-a-shared-streamsink-engine.md) | Sinks compose a shared `StreamSink` engine and delegate to it | Accepted |
| [0014](0014-format-line-helper-not-a-hookable-engine.md) | Share a `format_line` helper, not a hookable engine | Accepted |
| [0015](0015-file-sink-construction-append-only-fallible-try-new.md) | File sinks open append-only via a fallible `try_new` | Accepted |
| [0016](0016-session-rotation-numbered-backups-at-construction.md) | Session rotation uses numbered backups at construction only | Accepted |
