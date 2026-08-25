# ADR 0015: File sinks open append-only via a fallible `try_new`, with a raw `OpenOptions` escape hatch

- Status: Accepted
- Date: 2026-08-25

## Context and Problem Statement

Opening a log file involves several small policy choices that are hard to
change once they are public API: what the constructor is called, whether the
file is appended to or truncated, who creates the parent directory, and how
durable a written record is. Existing loggers answer these differently, and
each answer tends to arrive as another constructor parameter or another
public enum.

## Considered Options

- **Constructor named `open`.** Reads naturally for a one-shot call, but
  sinks open — and, for session and future rotating sinks, close and reopen —
  files more often than that name suggests.
- **A `FileMode { Append, Truncate }` parameter.** The conventional knob, but
  it is a public enum plus a parameter most callers never touch, and it
  covers only two of the many things `OpenOptions` can express.
- **A `create_dirs` flag.** Convenient on first run, but pulls
  `create_dir_all`'s own failure surface (permissions, a path component that
  is a file, races) into a small constructor.
- **Per-record `fsync` or an opt-in durability knob.**
- **Append-only `try_new` plus `try_from_options` (chosen).**

## Decision Outcome

Chosen: **fallible `try_new` constructors that always open in append mode,
with `try_from_options` as the escape hatch; buffered writes; parent
directory must already exist.**

- **`try_new`, not `open`.** `try_new` is the standard Rust spelling for a
  fallible constructor (mirroring `TryFrom::try_from`) and keeps the
  construction API uniform across sinks — `ConsoleSink::new` (infallible),
  `ContinuousFileSink::try_new`, `SessionFileSink::try_new` — regardless of
  what each does internally. Console constructors stay infallible; anything
  that touches the filesystem returns `io::Result`.
- **Always append; no `FileMode`.** Matches Python's `logging.FileHandler`
  and C++ Quill's `FileSink`. "Start from an empty file" is a session-start
  concept, not a per-open one, and is served by `SessionFileSink` (ADR 0016)
  — including at `max_backups = 0`, which deletes and recreates rather than
  truncating.
- **`try_from_options` escape hatch.** For what `FileMode` never covered
  anyway — permission bits (`OpenOptionsExt::mode`), `O_EXCL`, truncation —
  the caller supplies a `std::fs::OpenOptions` directly instead of the
  library growing a leaky enum. `try_new` is implemented in terms of it.
  Deliberately *not* extended to future rotating sinks: those reopen their
  file on their own schedule, so caller-supplied options would have to be
  re-applied on every rotation, which fights the "a rotating sink owns its
  own reopen logic" design of ADR 0014.
- **Parent directories must exist.** No `create_dirs` knob. Matches Python's
  `FileHandler`. Callers with a genuine fresh-deploy need call
  `std::fs::create_dir_all` themselves first — that is caller-side setup, not
  a sink concern.
- **Buffered, no per-record `fsync`.** File sinks use `BufWriter<File>`,
  mirroring the console sink. The backend worker already flushes after each
  batch and at shutdown.

### Consequences

- A missing parent directory surfaces as the raw `io::Error` from the open,
  at construction time, rather than as a silent directory creation.
- The exposure of buffered writes is losing unflushed records on a hard crash
  (`SIGKILL`, power loss). A per-record `fsync` would trade that for a
  blocking disk round-trip on every log call, on every sink, for a case most
  callers do not have. Revisit only if a concrete audit/compliance use case
  asks for it.
- `OpenOptions` is `std` API, now part of this library's public surface. That
  is the intended trade: one stable, well-documented builder instead of an
  enum that grows a variant per request.
