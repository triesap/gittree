#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "missing cargo-llvm-cov (install with: cargo install cargo-llvm-cov)" >&2
  exit 1
fi

if [[ -n "${COV_PACKAGES:-}" ]]; then
  read -r -a packages <<< "${COV_PACKAGES}"
else
  packages=(
    gittree-config
    gittree-core
    gittree-storage
    gittree-nostr-auth
    gittree-relay-adapter
    gittree-forgejo
    gittree-relay
    gittree-control
    gittree-auth
  )
fi

args=()
for pkg in "${packages[@]}"; do
  args+=("-p" "$pkg")
done

test_args=()
if [[ "${COV_NOCAPTURE:-0}" == "1" ]]; then
  test_args+=("--" "--nocapture")
fi

if [[ "${#test_args[@]}" -gt 0 ]]; then
  cargo llvm-cov --html "${args[@]}" "${test_args[@]}"
else
  cargo llvm-cov --html "${args[@]}"
fi
