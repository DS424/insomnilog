# insomnilog

An asynchronous Rust logging library that never sleeps.

Log arguments are serialised as raw bytes into a per-thread lock-free SPSC ring
buffer. A dedicated backend thread reads, decodes, formats, and writes to the
console. The logging hot path performs **no allocations**, **never blocks**, and is therefore **realtime-safe**.

insomnilog is highly inspired by [Quill](https://github.com/odygrd/quill).

## Quick Start

```rust
use insomnilog::legacy::{Logger, LogLevel};
use insomnilog::{legacy_log_info, legacy_log_warn};

let logger = Logger::builder()
    .level(LogLevel::Info)
    .queue_capacity(128 * 1024)  // 128 KiB per-thread queue
    .build();

legacy_log_info!(logger, "application started");
legacy_log_info!(logger, "user {} logged in with id {}", "alice", 42_u64);
legacy_log_warn!(logger, "disk usage at {}%", 87.5_f64);

logger.flush();  // drain before exit
```

## Design Goals

- **Zero allocations on the hot path** — `log_info!` only calls `try_reserve`
  (returns a pointer into the pre-allocated ring buffer) and
  `ptr::copy_nonoverlapping`. No `format!`, no `Box`, no `String`.
- **Never blocks the caller** — if the queue is full the record is silently
  dropped rather than stalling the calling thread.
- **Minimal runtime overhead** — per-callsite metadata is a `static` item
  created once by the macro; the format string is never parsed on the hot path.

## Architecture

```txt
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

```txt
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

## Real-Time Safety Validation

insomnilog's hot path is designed to be real-time safe: no allocations, no
locks, no blocking I/O on the logging thread. This contract can be verified
mechanically with the [LLVM RealtimeSanitizer (RTSan)](https://clang.llvm.org/docs/RealtimeSanitizer.html)
via the optional `rtsan` Cargo feature.

### Validating insomnilog itself

```sh
just realtime-sanitize
# expands to: RTSAN_ENABLE=1 cargo nextest run -p insomnilog --features rtsan
```

`RTSan` aborts the process with a precise diagnostic if any hot-path code
allocates, locks a mutex, or performs blocking I/O.

### Validating your application

If you want to verify that your own real-time threads stay clean when using
insomnilog, follow these two steps:

**1. Preallocate the per-thread queue before entering the real-time context.**

The first log call from a new thread allocates the SPSC queue. Move that
allocation out of the hot path by calling `preallocate()` during thread
initialisation (e.g. in thread setup, before the audio callback starts):

```rust
# use insomnilog::legacy::Logger;
# let logger = Logger::builder().build();
// Thread setup — outside the real-time callback:
logger.preallocate();
```

**2. Annotate your real-time entry point and enable `RTSan`.**

```rust,ignore
#[rtsan_standalone::nonblocking]   // tells RTSan this is a real-time context
fn audio_callback(logger: &Logger, /* … */) {
    log_info!(logger, "processing block {}", block_id);
    // … your DSP code …
}
```

Add `rtsan-standalone` as a dev-dependency and enable the `rtsan` feature on
insomnilog, then run your test suite with `RTSAN_ENABLE=1`:

```sh
RTSAN_ENABLE=1 cargo nextest run --features rtsan
```

The `rtsan-standalone` crate is a no-op when `RTSAN_ENABLE` is unset, so
you can ship `--features rtsan` in production builds without any overhead.
`RTSan` is supported on Linux and macOS.

## Development

This project uses [`just`](https://github.com/casey/just) as a command runner.

**Install:**

```sh
cargo install just
cargo install --locked cargo-nextest
```

**Usage:**

```sh
just                      # lint + test (default)
just build                # build the project
just test                 # run tests (via cargo-nextest)
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
