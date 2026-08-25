# Choosing a sink

A **sink** is the output endpoint of the library. The backend hands every
record that passes the level filters to each of a logger's sinks, and the sink
decides where it ends up e.g. your terminal, a file, or anywhere else you can
write to.

Sinks are independent of one another. A logger can hold as many as you like,
and the same record reaches all of them. It is possible to "print to the console *and* keep a
file" as this is configured just as a list of the desired sinks:

```rust,no_run
{{#include ../examples/sinks.rs:logger}}
```

The rest of this chapter walks through the sinks that ship with `insomnilog`
and the situation each one is meant for. All snippets come from the runnable
`sinks` example. Run it a few times in a row to watch the difference between
the file sinks first-hand:

```text
cargo run --example sinks
```

<details>
<summary>Full example</summary>

```rust,no_run
{{#include ../examples/sinks.rs}}
```

</details>

## Logging to the console

`ConsoleSink` writes formatted lines to stdout. It is typically used while
developing or while watching a program run. Output appears where you already
are, and nothing is left behind afterwards.

```rust,no_run
{{#include ../examples/sinks.rs:console}}
```

The formatter controls what each line looks like. See
[Configuring the formatter](#configuring-the-formatter) for more details.

## Logging to one continuous file

`ContinuousFileSink` appends to a single file and never replaces it. Restart
the program and new records continue in the same file, below the previous
run's. Nothing the library does removes or truncates it.

```rust,no_run
{{#include ../examples/sinks.rs:continuous}}
```

Use it when the full history across runs is needed. Examples include a long-lived service you
occasionally grep through, a bug that only shows up after several restarts, or
a small tool whose logs stay small.

The trade-off is that the file grows without bound. There is no size cap and no
free-space check, so **you** are responsible for watching disk usage.

## Logging to one file, fresh every run

If you only ever care about the run you are looking at right now, use
`SessionFileSink` with `max_backups = 0`. Each start of your program deletes
the previous file and creates an empty one in its place:

```rust,no_run
{{#include ../examples/sinks.rs:overwrite}}
```

The result is a file that never grows across runs and never needs cleaning up.
It always holds exactly the current session. On the flip side, the moment
you restart, the previous run's log is gone. If you might want to compare two
runs, keep a backup or two instead, as described next.

## Keeping each run in its own file

`SessionFileSink` with `max_backups ≥ 1` gives every run its own file while
holding on to a fixed number of earlier runs. This is the usual choice for a
program people restart regularly and whose last few runs are worth having when
someone reports a problem.

```rust,no_run
{{#include ../examples/sinks.rs:history}}
```

### How the rotation works

Rotation happens **once, at construction**. There is no
mid-run trigger, so a line is never split across two files.

For `session_history.log` with `max_backups = 3`, starting the program:

```text
delete  session_history.log.3     (the oldest run drops off)
rename  session_history.log.2  →  session_history.log.3
rename  session_history.log.1  →  session_history.log.2
rename  session_history.log    →  session_history.log.1
create  session_history.log       (empty — this run writes here)
```

So the live file is always the plain name, and higher numbers are further in
the past.

### Configuration options

| Option | Effect |
| --- | --- |
| `path` | The live file for the current run. Backups are this name plus `.1`, `.2`, … The parent directory must already exist — the sink does not create it. |
| `max_backups = 0` | No history: the previous file is deleted and recreated. |
| `max_backups = N` | Keeps the `N` most recent previous runs, deleting the `N+1`-th. |
| `level` | Records below this level never reach the file. Remember the effective filter is `max(logger.level, sink.level)`. |
| `formatter` | Line layout, exactly as for the console. |

Two details worth knowing:

- Any step whose source file is missing is skipped rather than failing, so a
  first run on a fresh machine, or a half-populated set of backups, rotates
  cleanly.
- Lowering `max_backups` between runs does not clean up the leftovers. Files
  numbered above the current `max_backups` are never touched. Delete them
  yourself if you want them gone.

Like `ContinuousFileSink`, this sink caps neither the size of one file nor the
total disk usage. `max_backups` bounds the *number* of files, not their bytes.
A single very chatty run still produces a single very large file.

## Advanced options

These exist mostly for tests and for destinations the ready-made sinks do not
cover. Reach for them only when you have a concrete reason to.

```rust,no_run
{{#include ../examples/sinks.rs:advanced}}
```

- **`StreamSink`** is the shared engine behind all the sinks above. It pairs a
  formatter with any `std::io::Write` destination. Use it directly to stream
  somewhere else e.g. an in-memory `Vec<u8>` you assert on in a test, a pipe, a
  socket. Note that it just writes; anything the destination needs beyond that
  (reconnecting, rotating, backpressure) is yours to handle.
- **`NullSink`** accepts every record and discards it. Handy in tests and
  benchmarks where the output is noise, or as a placeholder while the real sink
  is not wired up yet.
- **`try_from_options`** on both file sinks takes a `std::fs::OpenOptions` you
  built yourself instead of the default append-mode open, which is how you set
  permission bits or opt into flags like `O_EXCL`. `SessionFileSink` still
  runs its rotation first. The options only apply to the fresh file this run
  opens.
