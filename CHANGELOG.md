# Changelog

The following sections list the changes between each consecutive version of this library.
The `Highlights` section is a short summary of the `Details` section.

<!-- JRELEASER_CHANGELOG_APPEND - Do not remove or modify this section -->
## 0.1.0 (2026-03-12)

### ✨ Highlights

This is the first release of the library. It focuses on:

- The general setup of the repository
- The first architecture version, such that a first simple example can be run
- The configuration of tooling for future development.

### ⚠️ Known Issues

This version only aimed to run a simple example.
As such, the following things have not been tested or validated:

- Using the logger class in a real-world scenario e.g. instantiating multiple loggers in multiple threads
- Running long-term tests to validate real-time behavior
- Benchmarking against other libraries
- Supporting useful configurations e.g. multiple sinks and formatters.

### 🔎 Details

#### 🚀 Features

- 9010a3e Initialize project base
- aedfaf6 Add `LogLevel` and `LogMetadata`
- 8eb2067 Add binary encoding for log arguments
- cde37e3 Add binary decoding for log arguments
- 4b20505 Add lock-free SPSC ring buffer
- 20c93e8 Add `ConsoleSink` and `PatternFormatter`
- 920943a Add `BackendWorker`
- aae8bfb Add Logger and logging macros
- 15f9d67 Add `basic_usage` example
- 9f72f98 Add `Logger::preallocate()` for explicit per-thread queue init
- bbdedfe Add `RTSan` annotations and integration test

#### 🔄️ Changes

- 0ea01d8 Avoid `transmute` for `LogLevel` conversions

#### 🧪 Tests

- 98955eb Run default usage test every time

#### 🧰 Tasks

- bde7e2e Add `CODEOWNERS` file

#### 🛠  Build

- d56db1a Add `generate-changelog` step with `JReleaser`
- 9aa0165 Add a simple CI pipeline that runs `lint` and `test`
- e3471bb Add auto-approve bot for self reviews
- 9f9ef96 Switch test runner to cargo-nextest
- 4949be7 Enable ANSI color output for all Cargo commands
- 25acfda Add realtime-sanitize job to GitHub Actions
- 3fc1482 Run `ThreadSanitizer` to detect data races
- 99b48a3 Run `AddressSanitizer` to detect memory errors
- b12c3c4 Add an aggregating `just` command to run all sanitizers
- 22ad7ad Fail on warnings in the dedicated lint stage
- 3b3efc7 Add a `doc-check` command
- 3aa44e0 Clean up package metadata before publishing
- 3635d0a Add automatic deployment to `crates.io` on tag build

#### 📝 Documentation

- 6d87199 Add NOTICES file attributing Quill logging library
- a41430b Add architecture overview and quick start to README
- 2bbeb07 Add CLAUDE.md with project context for AI sessions
- 5d5db06 Document `RTSan` validation for insomnilog and integrators
- 22f8d9f Improve docs before publishing first version

### 👥 Contributors

We'd like to thank the following people for their contributions:

- Dwayne Steinke
- csph
