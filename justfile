default: lint test

build:
    cargo build

test:
    cargo nextest run
    cargo test --doc

lint:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features

fmt:
    cargo fmt --all

doc:
    cargo doc --document-private-items --no-deps --open

realtime-sanitize:
    RTSAN_ENABLE=1 cargo nextest run -p insomnilog --features rtsan

thread-sanitize:
    RUSTFLAGS="-Z sanitizer=thread" cargo +nightly nextest run -Z build-std --target x86_64-unknown-linux-gnu
    
address-sanitize:
    RUSTFLAGS="-Z sanitizer=address" cargo +nightly nextest run -Z build-std --target x86_64-unknown-linux-gnu

generate-changelog:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo pkgid insomnilog | cut -d'#' -f2)
    JRELEASER_PROJECT_VERSION="$version" JRELEASER_GENERIC_TOKEN=None jreleaser changelog --basedir .
