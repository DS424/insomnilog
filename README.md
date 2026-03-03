# insomnilog

Asynchronous Rust Logging Library that never blocks

## Development

This project uses [`just`](https://github.com/casey/just) as a command runner.

**Install:**

```sh
cargo install just
```

**Usage:**

```sh
just                      # list available recipes
just build                # build the project
just test                 # run tests
just fmt                  # format code
just lint                 # run clippy
just doc                  # build and open documentation
just generate-changelog   # append new entries to CHANGELOG.md
```

### Changelog Generation

Changelog entries are appended to `CHANGELOG.md` using [JReleaser](https://jreleaser.org). It parses commits since the last tag and categorizes them using the [Conventional Commits](https://www.conventionalcommits.org) preset.
