#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "missing cargo-llvm-cov (install with: cargo install cargo-llvm-cov)" >&2
  exit 1
fi

tmp_json="$(mktemp -t gittree-storage-cov-json.XXXXXX)"
tmp_text="$(mktemp -t gittree-storage-cov-text.XXXXXX)"
cleanup() {
  rm -f "${tmp_json}" "${tmp_text}"
}
trap cleanup EXIT

max_uncovered_lines="${COV_STORAGE_MAX_UNCOVERED_LINES:-1}"
min_line_pct="${COV_STORAGE_MIN_LINE_PCT:-99.98}"
min_region_pct="${COV_STORAGE_MIN_REGION_PCT:-99}"

if ! command -v docker >/dev/null 2>&1 && [[ -z "${GITTREE_STORAGE_TEST_DATABASE_URL:-}" ]]; then
  export GITTREE_STORAGE_TEST_DATABASE_URL="${COV_STORAGE_DATABASE_URL:-postgres://gittree:gittree@127.0.0.1:5432/gittree}"
fi

cov_cmd=(
  cargo llvm-cov
  -p gittree-storage
  --lib
  --json
  --summary-only
  --output-path "${tmp_json}"
  --fail-uncovered-lines "${max_uncovered_lines}"
  --fail-under-functions 100
  --fail-under-lines "${min_line_pct}"
  --fail-under-regions "${min_region_pct}"
)

./scripts/with-test-postgres.sh "${cov_cmd[@]}"

cargo llvm-cov report \
  -p gittree-storage \
  --text \
  --output-path "${tmp_text}" \
  >/dev/null

python3 - "${tmp_text}" <<'PY'
import sys
from pathlib import Path

report = Path(sys.argv[1]).read_text().splitlines()
current_file = None
uncovered = []

for line in report:
    if line.startswith('/Users/') and line.endswith(':'):
        current_file = line[:-1]
        continue
    if current_file is None:
        continue
    if '/crates/storage/src/' not in current_file:
        continue
    parts = line.split('|', 2)
    if len(parts) < 3:
        continue
    line_no = parts[0].strip()
    count = parts[1].strip()
    if not line_no.isdigit():
        continue
    if count in {'0', '#####'}:
        uncovered.append((current_file, int(line_no), line.rstrip()))

if uncovered:
    print('uncovered executable lines found in storage sources:', file=sys.stderr)
    for file_path, line_no, rendered in uncovered:
        short = file_path.split('/crates/storage/src/', 1)[-1]
        print(f'  {short}:{line_no}: {rendered}', file=sys.stderr)
    sys.exit(1)
PY

echo "storage coverage gate passed"
