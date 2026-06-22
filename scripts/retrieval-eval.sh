#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/punchcard-target}"

exec cargo run --quiet --bin retrieval-eval -- "$@"
