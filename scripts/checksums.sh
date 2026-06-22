#!/usr/bin/env bash
# Generate SHA-256 checksums for release artifacts.
#
# Usage:
#   ./scripts/checksums.sh [artifact_dir]
#
# Writes CHECKSUMS.sha256 inside the artifact directory (default: ./dist),
# covering every regular file there except the checksum file itself. Paths are
# stored relative to the artifact directory so the file verifies with
# `sha256sum --check CHECKSUMS.sha256` from inside that directory. This is a
# release artifact and is intentionally git-ignored.
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact_dir="${1:-${repository_root}/dist}"
checksum_file="CHECKSUMS.sha256"

if [[ ! -d "${artifact_dir}" ]]; then
  echo "artifact directory not found: ${artifact_dir}" >&2
  exit 1
fi

cd "${artifact_dir}"
mapfile -t artifacts < <(find . -type f ! -name "${checksum_file}" -printf '%P\n' | sort)
if [[ "${#artifacts[@]}" -eq 0 ]]; then
  echo "no artifacts found in ${artifact_dir}" >&2
  exit 1
fi

sha256sum "${artifacts[@]}" >"${checksum_file}"
echo "wrote ${artifact_dir}/${checksum_file} (${#artifacts[@]} artifact(s))"
