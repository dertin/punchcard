#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/setup.sh [--check|--install]

--check    Verify that the host already has the required system packages.
--install  Install missing system packages with apt.
EOF
}

mode="install"
if [[ $# -gt 1 ]]; then
  usage >&2
  exit 2
fi
if [[ $# -eq 1 ]]; then
  case "$1" in
    --check)
      mode="check"
      ;;
    --install)
      mode="install"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
fi

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

if [[ ! -r /etc/os-release ]]; then
  echo "setup is supported on Debian and Ubuntu only" >&2
  exit 1
fi

# shellcheck disable=SC1091
source /etc/os-release
case "${ID:-}:${ID_LIKE:-}" in
  ubuntu:*|debian:*|*:ubuntu*|*:debian*)
    ;;
  *)
    echo "setup is supported on Debian and Ubuntu only" >&2
    exit 1
    ;;
esac

apt_packages=(
  ca-certificates
  curl
  git
  build-essential
  pkg-config
  libssl-dev
  poppler-utils
)

missing_packages=()
for package in "${apt_packages[@]}"; do
  if ! dpkg -s "$package" >/dev/null 2>&1; then
    missing_packages+=("$package")
  fi
done

missing_tools=()
for tool in cargo rustc; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing_tools+=("$tool")
  fi
done

if ! cargo fmt --version >/dev/null 2>&1; then
  missing_tools+=("cargo fmt")
fi
if ! cargo clippy --version >/dev/null 2>&1; then
  missing_tools+=("cargo clippy")
fi

if (( ${#missing_packages[@]} == 0 )) && (( ${#missing_tools[@]} == 0 )); then
  echo "Setup already satisfied for Debian/Ubuntu."
  echo "Validated platform: Ubuntu 24.04.x."
  exit 0
fi

if (( ${#missing_packages[@]} > 0 )); then
  echo "Missing apt packages: ${missing_packages[*]}"
fi
if (( ${#missing_tools[@]} > 0 )); then
  echo "Missing Rust tooling: ${missing_tools[*]}"
  echo "Install Rust 1.91+ with rustup and add the rustfmt/clippy components."
fi

if [[ "$mode" == "check" ]]; then
  exit 1
fi

if (( ${#missing_packages[@]} > 0 )); then
  if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
    apt-get update
    apt-get install -y "${missing_packages[@]}"
  else
    if command -v sudo >/dev/null 2>&1; then
      sudo apt-get update
      sudo apt-get install -y "${missing_packages[@]}"
    else
      echo "Need root or sudo to install: ${missing_packages[*]}" >&2
      exit 1
    fi
  fi
fi

echo "System packages are ready. If Rust tooling was missing, install it separately."
echo "Next steps: ./scripts/install-local.sh and ./scripts/validate.sh"
