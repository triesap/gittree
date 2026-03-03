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

needs_storage_db=0
for pkg in "${packages[@]}"; do
  if [[ "${pkg}" == "gittree-storage" ]]; then
    needs_storage_db=1
    break
  fi
done

if [[ "${COV_STRICT_STORAGE:-1}" == "1" && "${needs_storage_db}" == "1" ]]; then
  ./scripts/coverage-gate-storage.sh
  original_packages=("${packages[@]}")
  packages=()
  for pkg in "${original_packages[@]}"; do
    if [[ "${pkg}" != "gittree-storage" ]]; then
      packages+=("${pkg}")
    fi
  done
  if [[ "${#packages[@]}" -eq 0 ]]; then
    echo "coverage gate passed"
    exit 0
  fi
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

lines="${COV_FAIL_UNDER_LINES:-100}"
functions="${COV_FAIL_UNDER_FUNCTIONS:-100}"
regions="${COV_FAIL_UNDER_REGIONS:-100}"
include_bin_mains="${COV_INCLUDE_BIN_MAINS:-0}"

cov_args=(
  --json
  --summary-only
  --fail-under-lines "${lines}"
  --fail-under-functions "${functions}"
  --fail-under-regions "${regions}"
)

if [[ "${include_bin_mains}" != "1" ]]; then
  ignore_regex="${COV_IGNORE_FILENAME_REGEX:-.*/src/main\\.rs$}"
  cov_args+=(--ignore-filename-regex "${ignore_regex}")
elif [[ -n "${COV_IGNORE_FILENAME_REGEX:-}" ]]; then
  cov_args+=(--ignore-filename-regex "${COV_IGNORE_FILENAME_REGEX}")
fi

if [[ "${#test_args[@]}" -gt 0 ]]; then
  cov_cmd=(
    cargo llvm-cov
    "${cov_args[@]}"
    "${args[@]}"
    "${test_args[@]}"
  )
else
  cov_cmd=(
    cargo llvm-cov
    "${cov_args[@]}"
    "${args[@]}"
  )
fi

if [[ "${needs_storage_db}" == "1" ]]; then
  if ! command -v docker >/dev/null 2>&1 && [[ -z "${GITTREE_STORAGE_TEST_DATABASE_URL:-}" ]]; then
    export GITTREE_STORAGE_TEST_DATABASE_URL="${COV_STORAGE_DATABASE_URL:-postgres://gittree:gittree@127.0.0.1:5432/gittree}"
  fi
  ./scripts/with-test-postgres.sh "${cov_cmd[@]}"
else
  "${cov_cmd[@]}"
fi
