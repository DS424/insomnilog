# insomnilog — Claude Project Instructions

## Dev commands

`just` is the command runner. Run it without arguments as the default pre-edit
check; it lints, tests, and verifies docs.

```sh
just          # lint + test + doc-check (default)
just fmt      # auto-format (run if fmt check fails)
just test     # run tests only
just doc      # build and open API docs
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full command reference, CI
pipeline details, and Miri test conventions.

## Architecture

The
[ADRs](adr/) record the reasoning behind the key decisions.

High-level flow:

```text
log_info!(logger, "…", args)
  │
  ├─ level check (atomic load, relaxed)
  └─ Producer::write(total_len, |buf| {
       encode args → ptr::copy_nonoverlapping into buf (no alloc)
     })   // commits (Release store on write_pos) on success
          ↓ SPSC queue (one per thread, lazily created)
  BackendWorker (dedicated thread)
  ├─ poll all Consumer queues (round-robin, bounded per-consumer batch)
  ├─ decode_record() → DecodedRecord
  ├─ dispatch to logger.sinks (raw *const Logger in record header)
  └─ each Sink::write_record() → formats and writes output
```

## Module dependency order

`level` → `metadata` → `encode` → `record` → `decode` → `queue` → `formatter` → `sink`
→ `backend` → `frontend` → `macros` → `lib`

Each module only imports from modules earlier in this chain (plus `std`).
`lib.rs` is the only place that ties them together.

## Key invariants — do not break

- **Hot path: zero allocations, zero locks.** The macro only calls
  `Producer::write` with a closure that does `ptr::copy_nonoverlapping`
  into the reserved buffer. No `format!`, no `Mutex`, no `Arc` clone on
  the hot path.
- **Silent drop on full queue.** `Producer::write` returning
  `Err(QueueFull)` discards the record. Never block or propagate an
  error to the caller.
- **`metadata_ptr` is a valid `&'static LogMetadata` pointer.** The macro
  stores `&METADATA as *const _ as usize`; `decode_record` casts it back.
  The static lifetime is guaranteed by the macro `static METADATA` expansion.
- **`logger_ptr` is a valid `*const Logger` for the process lifetime.** The
  `LoggerRegistry` holds a strong `Arc<Logger>`; the backend dereferences the
  raw pointer without a lookup. Do not remove a logger from the registry.
- **All records are contiguous in the ring buffer.** The queue is backed
  by a 2× capacity allocation, so any reservation of size `n ≤ capacity`
  is physically contiguous regardless of where it lands in the ring.
  Each `Consumer::read` must request exactly the bytes the matching
  `Producer::write` wrote — see `queue.rs` for the full invariant.

## Lint conventions

Lints are strict: `pedantic`, `nursery`, `cargo`, `missing_docs`,
`missing_docs_in_private_items`.

- Use `#[expect(clippy::lint_name, reason = "…")]` rather than `#[allow]` —
  the compiler warns if the suppression becomes unnecessary.
- The `_`-prefixed items in `lib.rs` (`_RecordHeader`, `_Producer`, etc.) are
  macro helpers; they are `#[doc(hidden)]` intentionally.

## Adding a new loggable type

1. Add a variant to `TypeTag` in `encode.rs` (next free integer).
2. Implement `Encode` for the type in `encode.rs`.
3. Add a variant to `DecodedArg` in `decode.rs` and a `Display` arm.
4. Add a match arm in `decode_one()` mapping the new tag byte to the variant.
5. Add encode + decode round-trip tests in both files.
