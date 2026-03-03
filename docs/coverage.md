# Coverage (llvm-cov)

This repo uses `cargo-llvm-cov` for Rust coverage reporting.

## Install

```
cargo install cargo-llvm-cov
```

## Run (default packages)

```
./scripts/llvm-cov.sh
```

By default the script targets the core crates and services and skips wasm/tauri
packages. HTML output is written to `target/llvm-cov/html`.

To pass through `--nocapture` to Rust tests:

```
COV_NOCAPTURE=1 ./scripts/llvm-cov.sh
```

When `gittree-storage` is included, coverage scripts automatically provision a
temporary Postgres instance via Docker if
`GITTREE_STORAGE_TEST_DATABASE_URL` is not set.

Disable auto-provisioning:

```
GITTREE_STORAGE_AUTOSTART_DB=0 ./scripts/llvm-cov.sh
```

## Run (custom packages)

Override the package list with `COV_PACKAGES`:

```
COV_PACKAGES="gittree-relay gittree-control" ./scripts/llvm-cov.sh
```

## Coverage Gate

Use the gate script to enforce minimum coverage thresholds on the relevant
security-critical packages.

```
./scripts/coverage-gate.sh
```

Defaults:
- line coverage: `100`
- function coverage: `100`
- region coverage: `100`
- binary `main.rs` files are excluded from gate calculations by default
  (`.*/src/main\\.rs$`)

Override thresholds:

```
COV_FAIL_UNDER_LINES=92 COV_FAIL_UNDER_FUNCTIONS=88 ./scripts/coverage-gate.sh
```

Include binary `main.rs` files in gate calculations:

```
COV_INCLUDE_BIN_MAINS=1 ./scripts/coverage-gate.sh
```

Override the ignore regex directly:

```
COV_IGNORE_FILENAME_REGEX='.*/src/bin/experimental_.*\\.rs$' ./scripts/coverage-gate.sh
```

### Strict storage mode

Run the storage crate with stricter defaults:

```
COV_PACKAGES="gittree-storage" COV_STRICT_STORAGE=1 ./scripts/coverage-gate.sh
```

This path delegates to `scripts/coverage-gate-storage.sh`, which enforces:
- storage function coverage: `100%`
- storage line coverage: `100%`
- storage region coverage: `100%`
- storage uncovered lines: `0`

If Docker is unavailable and `GITTREE_STORAGE_TEST_DATABASE_URL` is unset, coverage
scripts fall back to:

```
postgres://gittree:gittree@127.0.0.1:5432/gittree
```

Override with:

```
COV_STORAGE_DATABASE_URL="postgres://..." ./scripts/coverage-gate-storage.sh
```

## Coverage Contract (Current Stage)

Current stage contract for this repo requires:

- **100% line/function/region coverage for `gittree-storage`** (strict mode).
- **100% line/function/region coverage for security-critical packages in the core stack**:
  - `gittree-config`
  - `gittree-core`
  - `gittree-nostr-auth`
  - `gittree-relay-adapter`
  - `gittree-forgejo`
  - `gittree-relay`
  - `gittree-control`
  - `gittree-auth`

This matches the default set used by `./scripts/coverage-gate.sh`.

### Stage-0 frontend exception

For this stage, `gittree-app-ui` is not in the 100/100/100 scope because frontend
coverage is not yet required.

Use these commands for the required checks:

```bash
# 1) Storage hardening requirement.
COV_PACKAGES="gittree-storage" COV_STRICT_STORAGE=1 ./scripts/coverage-gate.sh

# 2) Security-critical rust stack baseline.
COV_PACKAGES="gittree-config gittree-core gittree-nostr-auth gittree-relay-adapter gittree-forgejo gittree-relay gittree-control gittree-auth" ./scripts/coverage-gate.sh
```

When frontend coverage is required in a later stage, add `gittree-app-ui` and
feature-aligned SSR/non-SSR checks to this contract with staged threshold
increases.
