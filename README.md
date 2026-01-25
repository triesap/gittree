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

Common environment variables:
- `GITTREE_RELAY_URLS`: Comma-separated list of relay URLs to target.
- `GITTREE_RELAY_COMPAT_MODE`: Compatibility mode (`strict`, `warn`, `allow`).
- `GITTREE_RELAY_PROBE_ACTIVE`: Enable active write/read probe (`true`/`false`).
- `GITTREE_RELAY_PROBE_TIMEOUT_SECS`: Active probe timeout (seconds).
- `GITTREE_RELAY_PROBE_SECRET_KEY`: Optional hex secret key for probe signer.

Service bind and storage configuration live in the same env file used for deployment.

## Relay compatibility

Use the compatibility probe to validate a relay before enabling it:

```
gittree-relay-probe --relay wss://relay.example --active --store
```

Stored compatibility results can be queried from the state service:

```
GET /relay-compatibility?relay_url=wss://relay.example
```

## External relay smoke test

1) Configure `GITTREE_RELAY_URLS` and `GITTREE_RELAY_COMPAT_MODE`.
2) Run `gittree-relay-probe` against the target relay and store the result.
3) Publish a repo announcement + state event and confirm the coordinator provisions a repo.
4) Clone and push via your git transport and confirm state-based enforcement works.

## Development

Run checks and tests per crate:
- `cargo check -p gittree-config`
- `cargo test -p gittree-config`
