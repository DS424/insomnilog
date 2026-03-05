# insomnilog

An asynchronous Rust logging library that never sleeps.

Log arguments are serialised as raw bytes into a per-thread lock-free SPSC ring
buffer. A dedicated backend thread reads, decodes, formats, and writes to the
console. The logging hot path performs **no allocations**, **never blocks**, and is therefore **realtime-safe**.

## Design Goals

- **Zero allocations on the hot path** — `log_info!` only calls `try_reserve`
  (returns a pointer into the pre-allocated ring buffer) and
  `ptr::copy_nonoverlapping`. No `format!`, no `Box`, no `String`.
- **Never blocks the caller** — if the queue is full the record is silently
  dropped rather than stalling the calling thread.
- **Minimal runtime overhead** — per-callsite metadata is a `static` item
  created once by the macro; the format string is never parsed on the hot path.

## Architecture

```
Logging Thread(s)               Backend Thread
┌─────────────────┐             ┌──────────────────────┐
│ log_info!(...)  │             │ BackendWorker::run() │
│   ↓ level check │             │   ↓ poll all queues  │
│   ↓ encode args │  SPSC Queue │   ↓ decode args      │
│   ↓ write header├────────────→│   ↓ format message   │
│   ↓ commit      │ (per thread)│   ↓ write to sink    │
└─────────────────┘             └──────────────────────┘
```

Each logging thread gets its own SPSC queue (created lazily via `thread_local!`)
and registers the consumer half with the backend on first use.

### Module Structure

| Module        | Purpose                                                      |
|---------------|--------------------------------------------------------------|
| `level`       | `LogLevel` enum (Trace / Debug / Info / Warn / Error)        |
| `metadata`    | `LogMetadata` — static per-callsite data                     |
| `encode`      | `Encode` trait + `TypeTag` enum, impls for all V1 types      |
| `decode`      | `DecodedArg`, `RecordHeader`, `decode_record()`              |
| `queue`       | Lock-free SPSC ring buffer (`Producer` / `Consumer`)         |
| `sink`        | `Sink` trait + `ConsoleSink` (buffered stdout)               |
| `formatter`   | `PatternFormatter` — `[LEVEL ts] file:line message`          |
| `backend`     | `BackendWorker` — dedicated polling thread                   |
| `frontend`    | `Logger` / `LoggerBuilder`, thread-local queue management    |
| `macros`      | `log!`, `log_trace!`, `log_debug!`, `log_info!`, …           |

### Binary Protocol

Each log record written into the ring buffer:

```
[RecordHeader: 24 bytes] [Arg₁: 1 tag + N bytes] [Arg₂: 1 tag + N bytes] …
```

`RecordHeader` (repr(C)) carries:
- `timestamp_ns: u64` — nanoseconds since UNIX epoch
- `metadata_ptr: usize` — pointer to the `&'static LogMetadata`
- `encoded_args_size: u32` — total byte length of the arguments that follow
- `padding: u32`

### Supported Argument Types (V1)

| Type    | Tag | Encoded size    |
|---------|-----|-----------------|
| `i8`    | 0   | 1 byte          |
| `i16`   | 1   | 2 bytes         |
| `i32`   | 2   | 4 bytes         |
| `i64`   | 3   | 8 bytes         |
| `i128`  | 4   | 16 bytes        |
| `u8`    | 5   | 1 byte          |
| `u16`   | 6   | 2 bytes         |
| `u32`   | 7   | 4 bytes         |
| `u64`   | 8   | 8 bytes         |
| `u128`  | 9   | 16 bytes        |
| `f32`   | 10  | 4 bytes         |
| `f64`   | 11  | 8 bytes         |
| `bool`  | 12  | 1 byte (0 or 1) |
| `&str`  | 13  | 4 + len bytes   |
| `usize` | 14  | 8 bytes (→ u64) |
| `isize` | 15  | 8 bytes (→ i64) |

All multi-byte values use native-endian byte order.

## Quick Start

```rust
use insomnilog::{Logger, LogLevel, log_info, log_warn};

let logger = Logger::builder()
    .level(LogLevel::Info)
    .queue_capacity(128 * 1024)  // 128 KiB per-thread queue
    .build();

log_info!(logger, "application started");
log_info!(logger, "user {} logged in with id {}", "alice", 42_u64);
log_warn!(logger, "disk usage at {}%", 87.5_f64);

logger.flush();  // drain before exit
```

## Development

This project uses [`just`](https://github.com/casey/just) as a command runner.

**Install:**

```sh
cargo install just
```

**Usage:**

```sh
just                      # lint + test (default)
just build                # build the project
just test                 # run tests
just fmt                  # format code
just lint                 # run clippy
just doc                  # build and open documentation
just generate-changelog   # append new entries to CHANGELOG.md
```

### Changelog Generation

Changelog entries are appended to `CHANGELOG.md` using
[JReleaser](https://jreleaser.org). It parses commits since the last tag and
categorises them using the [Conventional Commits](https://www.conventionalcommits.org)
preset.
