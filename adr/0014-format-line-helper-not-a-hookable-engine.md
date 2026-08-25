# ADR 0014: Share a `format_line` helper, not a hookable engine

- Status: Accepted
- Date: 2026-08-25

## Context and Problem Statement

Sinks that rotate *during* a run (rotate on a schedule, or once the file
passes a size threshold) must swap the open file at the moment of writing,
atomically with the write, so no record lands in a half-rotated state.

The shared `StreamSink` engine (ADR 0013) owns the lock and the write loop
for the non-rotating sinks. The question was whether that engine should grow
an extension point — a `before_write` hook, a rotation policy object — so
rotating sinks could reuse it too, or whether rotating sinks should stand
outside it.

## Considered Options

- **A hookable engine.** Give `StreamSink` a `before_write` hook (or a
  policy trait) invoked inside its critical section, so a rotating sink can
  reopen the file there. Rejected: it is speculative weight carried by the
  three sinks that never rotate mid-run, and it makes the engine's locking
  contract — the thing most worth keeping simple — the extension surface.
- **A free `format_line` helper (chosen).** Share only the format-and-write
  inner step; let each sink own its own lock.

## Decision Outcome

Chosen: **the only thing shared across all sinks is a free `format_line`
helper; rotating sinks are standalone structs owning their own lock.**

```rust
fn format_line(
    formatter: &impl Formatter,
    scratch: &mut String,
    record: &LogRecord,
    writer: &mut dyn Write,
) -> io::Result<()>;
```

- `format_line` is the single place the on-the-wire line shape is decided
  (scratch reuse, trailing newline), so every sink emits byte-identical
  output.
- `StreamSink` locks its state and calls `format_line`. The sinks that wrap
  it inherit that path.
- A rotating sink is a standalone struct with its own
  `Mutex<(writer, rotation state, scratch)>`. Its `write_record` reads
  `lock → if rotation due { reopen } → format_line`. It therefore already
  has the atomic critical section it needs, in *its own* code, and never
  reaches into the engine's lock — which is exactly why the engine needs no
  hook.

### Consequences

- A rotating sink re-writes the ~3-line lock wrapper around `format_line`.
  That duplication is small and contained; the formatting and line shape are
  not duplicated.
- The engine's locking contract stays closed and easy to reason about.
- Rotating sinks can pick a different locking granularity, or hold extra
  state, without renegotiating anything with the engine.
- `format_line` is the compatibility point to preserve: changing it changes
  every sink's output at once, which is the intent.
