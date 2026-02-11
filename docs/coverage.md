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
- line coverage: `90`
- function coverage: `85`
- region coverage: `85`
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
