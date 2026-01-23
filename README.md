# Gittree

Gittree is a NIP-34 Git service using Nostr transport, with relay-backed repository state
and GRASP-compatible behavior.

## Overview

Core crates:
- gittree-config: typed config loading and validation.
- gittree-core: NIP-34 models, parsing, and policy helpers.
- gittree-storage: Postgres storage and migrations.
- gittree-observability: tracing and metrics wiring.

Services (planned/active):
- relay: nostr-rs-relay wrapper with admission hooks.
- admission: event admission policy service.
- state-service: state and maintainer lookup for hooks.
- git-hook: pre/post-receive helper for git servers.
- coordinator: repo provisioning on announcements.
- sync: periodic reconciliation of git refs to state.

## Config

See `config/example.toml` and `config/example.env` for starting points.

## Development

Run checks and tests per crate:
- `cargo check -p gittree-config`
- `cargo test -p gittree-config`

