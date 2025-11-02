import? '~/.justfile'

set shell := ["bash", "-euo", "pipefail", "-c"]
set windows-shell := ["pwsh", "-NoLogo", "-Command"]

# Show help (same as just running just)
help:
    @just --list

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets --all-features --tests --locked -- -D warnings

lint-fix:
    cargo clippy --workspace --all-targets --all-features --tests --locked --fix --allow-dirty --allow-staged -- -D warnings

test:
    cargo test --workspace --all-features

build:
    cargo build --bin wtg --all-targets --all-features

build-release:
    cargo build --bin wtg --release --all-features

install:
    cargo install --path crates/wtg --all-features --force

check:
    cargo check --workspace --all-targets --all-features

udeps:
    cargo +nightly udeps --workspace --all-features --all-targets --release

# Run the full CI flow locally.
ci:
    just fmt-check
    just lint
    just build
    just test
    just udeps
