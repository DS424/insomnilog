# Changelog

The following sections list the changes between each consecutive version of this library.
The `Highlights` section is a short summary of the `Details` section.

<!-- JRELEASER_CHANGELOG_APPEND - Do not remove or modify this section -->
## 0.2.0 (2026-05-28)

### ✨ Highlights

The library was completely rewritten with an overall architecture in mind.
It now contains extensive test infrastructure and usage-based documentation.

### ⚠️ Known Issues

The following things have not been tested or validated:

- Running long-term tests to validate real-time behavior
- Benchmarking against other libraries

Currently missing:

- More reference implementations of sinks e.g. different types of file sinks.
- Some features are still undocumented e.g. defining your own decodable type or handling errors.

### 🔎 Details

#### 🚀 Features

- 02ea02d Rewrite queue module
- 9e01d0c 🚨 Move current implementation to a `legacy` module
- 4fefd0a Introduce `queue` from `legacy`
- 762c548 Add `LogLevel` enum
- 5645541 Add `LogMetadata`
- 7fdb972 Add `encode` module
- 9f35f6f Add `RecordHeader` to store record information
- 96e6e69 Add `decode` module
- a6707b6 Add minimal `backend` and `lifecycle` modules
- 1b99a39 Add `formatter` with default pattern implementation
- fdb70f7 Add `Sink` trait and `ConsoleSink` & `NullSink`
- 3a46684 Allow registering and retrieving sinks globally
- eca32e0 Count sink errors and print them during backend shutdown
- 480fdcf Add `Logger` struct
- 78bb8b7 Allow creating and retrieving loggers globally
- 57226f2 Add `logger_ptr` to the  `RecordHeader`
- 6e765f3 Split `DecodedRecord` into `RawDecodedRecord` and `LogRecord`
- 866c2e6 Add `per_thread_queue`
- c1fdbf7 Add `BackendRunner`
- 571e8f9 Use `BackendRunner` in `Backend`
- 4ee4813 Handle dropped records in backend
- 3b30e85 Create producer for each thread in the backend
- 0fb46f7 Allow to retrieve the process wide backend
- 835bf12 Add `frontend`
- fe5644a Allow to preallocate a thread
- 17b45e2 Add logging macros
- 2417122 Retrieve captured output from `ConsoleSink`s that use vec writers
- b160e96 🚨 Remove `legacy` module

#### 🐛 Fixes

- 9fd9a2b Use `Box<[UnsafeCell<u8>]>` for SPSC ring buffer
- 4e8bf0f Implement `Error` for `InvalidPatternError`

#### 🔄️ Changes

- 3b5d418 Swap in the rewritten queue module
- be2992f Use wrapping arithmetic on read/write positions

#### 🧪 Tests

- 87e9761 Add compile-fail doctest for peek/read mutual exclusion
- 13e328f Add test utilities `spin_until` and `RecordingSink`
- 55cebf4 Add end-to-end tests for general usability

#### 🧰 Tasks

- 45af80d Mark `new` of `RecordHeader` as `must_use`
- ed2adc0 Integrate examples into the main crate

#### 🛠  Build

- 0eb6201 Add MIRI check to justfile and GitHub Actions
- 230b2d1 Temporarily allow `dead_code`
- f8c0f1d Abort jobs on force push
- c3b6d35 Split `miri` test execution into `default` and `slow`
- a85ff03 Configure `tagName` for `JReleaser` to correctly diff releases

#### 📝 Documentation

- 08f53ad Add architecture decision records (ADR)
- c65e61c Create usage based docs incl. `about`, `quick_start` and `architecture`
- 6b9631c Add chapter `Preallocating thread queues`
- e683b08 Add chapter `Configuring the formatter`
- b7f3f72 Overhaul the developer and agent documentation
- 282ff9d Use logo in the rendered docs

### 👥 Contributors

We'd like to thank the following people for their contributions:

- Dwayne Steinke

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
