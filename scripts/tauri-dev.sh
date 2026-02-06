#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v cargo-tauri >/dev/null 2>&1; then
  echo "missing tauri cli (cargo install tauri-cli)" >&2
  exit 1
fi

cd "$root_dir"

cargo tauri dev --manifest-path crates/app-ui/src-tauri/Cargo.toml
