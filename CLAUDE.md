# insomnilog — Claude Project Instructions

## Dev Commands

```sh
just          # lint (fmt check + clippy) + test
just fmt      # auto-format (run if fmt check fails)
cargo run -p insomnilog-examples   # run the example binary
```

Toolchain: Rust **1.93** (pinned in `rust-toolchain.toml`).

## Architecture

```
log_info!(logger, "…", args)
  │
  ├─ level check (atomic load, relaxed)
  ├─ encode args → ptr::copy_nonoverlapping into ring buffer (no alloc)
  └─ commit (Release store on write_pos)
          ↓ SPSC queue (one per thread, lazily created)
  BackendWorker (dedicated thread)
  ├─ poll all Consumer queues round-robin
  ├─ decode_record() → DecodedRecord
  ├─ PatternFormatter::format() → &str
  └─ ConsoleSink::write_line() → BufWriter<Stdout>
```

## Module Dependency Order

`level` → `metadata` → `encode` → `decode` → `queue` → `sink` → `formatter`
→ `backend` → `frontend` → `macros` → `lib`

Each module only imports from modules earlier in this chain (plus `std`).
`lib.rs` is the only place that ties them together.

## Key Invariants — Do Not Break

- **Hot path: zero allocations, zero locks.** The macro only calls
  `try_reserve` + `ptr::copy_nonoverlapping` + `commit`. No `format!`,
  no `Mutex`, no `Arc` clone on the hot path.
- **Silent drop on full queue.** `try_reserve` returning `None` discards the
  record. Never block or propagate an error to the caller.
- **`metadata_ptr` is a valid `&'static LogMetadata` pointer.** The macro
  stores `&METADATA as *const _ as usize`; `decode_record` casts it back.
  The static lifetime is guaranteed by the macro `static METADATA` expansion.
- **All records are contiguous in the ring buffer.** The producer skips to
  offset 0 when a write would straddle the boundary; the consumer advances
  over the wasted bytes. Do not change this without updating both sides.

## Lint Conventions

Lints are strict: `pedantic`, `nursery`, `cargo`, `missing_docs`,
`missing_docs_in_private_items`.

- Use `#[expect(clippy::lint_name, reason = "…")]` rather than `#[allow]` —
  the compiler warns if the suppression becomes unnecessary.
- The `_`-prefixed items in `lib.rs` (`_RecordHeader`, `_Producer`, etc.) are
  macro helpers; they are `#[doc(hidden)]` intentionally.

## Adding a New Loggable Type

1. Add a variant to `TypeTag` in `encode.rs` (next free integer).
2. Implement `Encode` for the type in `encode.rs`.
3. Add a variant to `DecodedArg` in `decode.rs` and a `Display` arm.
4. Add a match arm in `decode_one()` mapping the new tag byte to the variant.
5. Add encode + decode round-trip tests in both files.
