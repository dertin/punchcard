#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

cargo install --path crates/punchcard-cli --locked --force
punchcard version
./scripts/agent-assets.sh sync
punchcard plugin install cursor --local-source ./plugins

echo "Installed Punchcard. Run: punchcard init && punchcard rag index && punchcard doctor"
