# ADR 0016: Session rotation uses numbered backups and runs at construction only

- Status: Accepted
- Date: 2026-08-25

## Context and Problem Statement

`SessionFileSink` gives each program run its own log file, so runs can be
told apart and correlated after the fact. That requires a rotation scheme:
how backups are named, when rotation runs, what happens when files are
missing or a step fails, and whether growth within a single run is also
bounded.

## Considered Options

- **Timestamped backups** (`app.log.2026-08-25T10:04:00`). Rejected: adds a
  clock dependency, and the name asserts "rotated at this time", which stops
  being true once size- or schedule-triggered rotations exist.
- **A mid-session `max_size` trigger on `SessionFileSink`.** Rejected — see
  below.
- **Best-effort rotation** that continues past a failed rename.
- **Truncate the live file in place at `max_backups = 0`.**
- **Numbered backups, rotated once at construction, hard-failing (chosen).**

## Decision Outcome

Chosen: **numbered backups `app.log.1 … app.log.N`, rotated once at
construction, never truncating in place, hard-failing on real I/O errors.**

For `path = app.log` and `max_backups = N ≥ 1`:

```text
delete  app.log.N          (drops the oldest)
rename  app.log.{N-1} → app.log.N
        …
rename  app.log       → app.log.1
open    app.log            (fresh, empty — this session)
```

- **Numbered, not timestamped.** No clock dependency, matches the common
  session-backup convention, and stays trigger-agnostic if size- or
  time-triggered rotation is added later. Operators care about recency and
  ordering, not about why a rotation happened.
- **A missing source skips the step, it does not fail.** This holds for every
  rename, not just the delete — it is what makes rotation correct on a fresh
  deployment (no `app.log` yet) and on a partially-populated backup set.
- **`max_backups = 0` deletes and recreates**, running the same shape with no
  rename step. The live file is never truncated in place, at any `N`. One
  uniform delete-then-create shape means one rotation code path to test, and
  keeps the "never truncate" policy of ADR 0015 intact rather than
  reintroducing it as a special case.
- **Hard-fail on genuine errors.** A failed rename or delete — permissions,
  disk full — propagates its `io::Error` and aborts construction. A
  half-completed rotation must surface to the caller rather than silently
  leaving backups inconsistent or quietly dropping one.
- **Construction-time only; no mid-session size trigger.** `SessionFileSink`
  exists for run-to-run correlation, where growth is already bounded by
  process lifetime. Its `max_backups` means "N sessions"; layering a size
  trigger on top would silently redefine that to "N files" whenever one
  session overflowed twice — a semantic regression. Unbounded growth is a
  long-running-process problem, and belongs to a future scheduled/sized
  rotating sink (ADR 0014), which has no session identity to protect and for
  which "rotate on schedule or on size, whichever comes first" is
  well-precedented.

### Consequences

- Delete-then-create is not atomic: a crash between the two leaves *no*
  `app.log` rather than a truncated one. Accepted for the same reason ADR
  0015 accepts losing unflushed records on a hard crash.
- **Lowering `max_backups` between runs leaves orphans.** The cascade only
  touches indices `1..=N` for the current call, so files above that survive
  untouched. This matches Python's `RotatingFileHandler` and Quill's rotating
  sink; Python's `TimedRotatingFileHandler` is the outlier that globs the
  directory and prunes, which this design deliberately does not take on.
  Changing the value downward is an operator action with an operator-visible
  consequence, not a bug for the sink to hide.
- Because rotation happens before opening, `SessionFileSink`'s runtime
  behavior is identical to `ContinuousFileSink`'s — it wraps the same engine
  (ADR 0013) with no architecture fork.
- Backups renamed during rotation keep whatever permissions they were
  originally created with; `rename` moves a directory entry rather than
  reopening the file, so custom `OpenOptions` (ADR 0015) affect only the new
  live file and propagate to backups only as each is itself replaced.
