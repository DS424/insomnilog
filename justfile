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
