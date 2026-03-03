default: lint test

build:
    cargo build

test:
    cargo test

lint:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features

fmt:
    cargo fmt --all

doc:
    cargo doc --document-private-items --no-deps --open

generate-changelog:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo pkgid insomnilog | cut -d'#' -f2)
    JRELEASER_PROJECT_VERSION="$version" JRELEASER_GENERIC_TOKEN=None jreleaser changelog --basedir .
