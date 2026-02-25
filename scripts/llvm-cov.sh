#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "missing cargo-llvm-cov (install with: cargo install cargo-llvm-cov)" >&2
  exit 1
fi

if [[ "${COV_CLEAN:-1}" == "1" ]]; then
  cargo llvm-cov clean --workspace
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
needs_storage_db=0
for pkg in "${packages[@]}"; do
  if [[ "${pkg}" == "gittree-storage" ]]; then
    needs_storage_db=1
    break
  fi
done

test_args=()
if [[ "${COV_NOCAPTURE:-0}" == "1" ]]; then
  test_args+=("--" "--nocapture")
fi

if [[ "${#test_args[@]}" -gt 0 ]]; then
  cov_cmd=(cargo llvm-cov --html "${args[@]}" "${test_args[@]}")
else
  cov_cmd=(cargo llvm-cov --html "${args[@]}")
fi

if [[ "${needs_storage_db}" == "1" ]]; then
  if ! command -v docker >/dev/null 2>&1 && [[ -z "${GITTREE_STORAGE_TEST_DATABASE_URL:-}" ]]; then
    export GITTREE_STORAGE_TEST_DATABASE_URL="${COV_STORAGE_DATABASE_URL:-postgres://gittree:gittree@127.0.0.1:5432/gittree}"
  fi
  ./scripts/with-test-postgres.sh "${cov_cmd[@]}"
else
  "${cov_cmd[@]}"
fi
