#!/usr/bin/env sh
set -eu

crate_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$crate_dir"

cargo fmt -p jetpack -- --check
cargo test --locked -p jetpack --all-targets
cargo test --locked -p jetpack --doc
cargo clippy --locked -p jetpack --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked -p jetpack --no-deps --document-private-items
