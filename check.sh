#!/usr/bin/env bash
# This scripts runs various CI-like checks in a convenient way.
set -eux

cargo check --workspace --all-targets --locked
# The web build is experimental: only checked when the target is installed.
if rustup target list --installed | grep -q '^wasm32-unknown-unknown$'; then
    cargo check --workspace --all-features --lib --target wasm32-unknown-unknown --locked
fi
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings -W clippy::all
cargo test --workspace --all-targets --all-features --locked
cargo test --workspace --doc --locked
