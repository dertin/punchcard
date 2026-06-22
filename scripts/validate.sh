#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/punchcard-target}"

cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings

./scripts/agent-assets.sh check
cargo test -p punchcard-mcp stdio_protocol_lists_vertical_slice_tools

cargo run --quiet --bin punchcard -- plugin status
cargo run --quiet --bin punchcard -- rag sync
cargo run --quiet --bin punchcard -- doctor
