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

## Run (custom packages)

Override the package list with `COV_PACKAGES`:

```
COV_PACKAGES="gittree-relay gittree-control" ./scripts/llvm-cov.sh
```
