# Contributing to insomnilog

## Required tools

```sh
cargo install just
cargo install --locked cargo-nextest
```

[JReleaser](https://jreleaser.org) is needed only for changelog generation (see below).
The Rust toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it automatically.

## Dev workflow

[`just`](https://github.com/casey/just) is the command runner for this project.
Running `just` without arguments is the standard pre-PR check: it lints, runs
tests, and verifies that docs build without warnings.

```sh
just                      # lint + test + doc-check (default, run before pushing)
just fmt                  # auto-format (run if the lint step fails on formatting)
just build                # build all crates
just test                 # run tests via cargo-nextest + doc tests
just lint                 # fmt check + clippy -D warnings
just doc                  # build and open Rust API docs
just doc-check            # build docs with -D warnings (what CI runs)
just generate-changelog   # append new entries to CHANGELOG.md
```

Sanitizer recipes excerpt (slower, not required locally before every PR):

```sh
just sanitize             # run all sanitizers in sequence
just realtime-sanitize    # RTSan: verifies the hot path is allocation- and lock-free
just thread-sanitize      # ThreadSanitizer (requires nightly)
just address-sanitize     # AddressSanitizer (requires nightly)
just miri                 # Miri: fast tests (no `_miri_slow` suffix)
just miri-slow            # Miri: slow tests (thread-spawning / backend lifecycle)
```

## CI pipeline

Every pull request runs the `just` commands in parallel.

## Miri test split: `_miri_slow`

Miri is orders of magnitude slower than a native test run. Tests that involve
thread spawning or backend lifecycle tend to dominate CI time. To keep feedback
fast, these tests are split into two groups:

- **Fast** (`just miri`) — everything without the suffix. Runs by default.
- **Slow** (`just miri-slow`) — tests whose name ends in `_miri_slow`.

In CI the two groups run as separate parallel jobs.

When writing a new Miri-relevant test that spawns threads or exercises the
backend lifecycle, append `_miri_slow` to the function name:

```rust
#[test]
fn my_new_test_miri_slow() {
    // ...
}
```

No attribute or macro is needed — the suffix is the only marker.

## Architecture decisions

Significant architectural choices are recorded as
[Architecture Decision Records](adr/) in MADR format. The index is at
[adr/README.md](adr/README.md).

When a change involves a meaningful design tradeoff — a new data structure,
a change to the hot-path invariants, a new public API shape — add an ADR
documenting the decision and the alternatives considered. Keep ADRs focused on
the *why*, not the *what* (the code already shows what was chosen).

## Building the API docs

```sh
just doc        # builds and opens docs in the browser
just doc-check  # same build but with -D warnings (mirrors CI)
```

The user guide lives in `insomnilog/docs/`.

## Generating a changelog

```sh
just generate-changelog
```

This runs [JReleaser](https://jreleaser.org) against the commit history since
the last tag and appends categorised entries to `CHANGELOG.md` using the
[Conventional Commits](https://www.conventionalcommits.org) preset. Commit
messages must follow that format for entries to appear correctly.
