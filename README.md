# Gittree

Gittree is a Rust-based NIP-34 Git service built on Nostr transport with
GRASP-compatible repository metadata and a relay-backed state model.

## Overview

Gittree ships as a set of Rust services plus supporting infrastructure:

Rust services:
- `relay`: Nostr relay with NIP-11/42/43 support and GRASP metadata.
- `admission`: admission policy hook for event validation.
- `state`: state lookup service for hooks and coordinators.
- `coordinator`: repo provisioning and Forgejo integration.
- `sync`: reconciliation of git refs into state.
- `git-http`: Git HTTP gateway to Forgejo.
- `webhook`: Forgejo webhook ingestion.
- `control`: control plane (NIP-98 authenticated).
- `auth`: signup and profile API (NIP-98 authenticated).
- `ui`: legacy UI service.
- `app`: static app UI host (Leptos build output).

Infra services:
- `postgres`
- `forgejo`
- `mailpit`
- `nginx`

## Service map and ports

Host ports from `docker-compose.yml`:
- relay: 8080
- admission: 8081
- state: 8082
- coordinator: 8083
- sync: 8084
- git-http: 8085
- ui: 8086
- webhook: 8087
- control: 8088
- auth: 8089
- app: 8090
- postgres: 5432
- mailpit: 1025 / 8025
- nginx: 80
- forgejo: internal 3000 / 22 (not published to host)

## Tenancy and access control

Relay tenancy is host-based. The relay resolves a tenant by the Host header,
and returns `404` for unknown hosts. Create tenants via the control plane
before serving relay traffic.

Per-tenant policy:
- `public_read=false` requires NIP-42 auth plus membership for reads.
- `public_write=false` requires NIP-42 auth plus membership for writes.

Membership is managed with NIP-43 join/leave semantics. Invites are exposed
via membership list and claim events.

## Control plane (NIP-98)

The control service exposes NIP-98 authenticated endpoints:
- `POST /v1/relay/tenants`: create a tenant, generate relay signer material,
  and seed owner membership.
- `POST /v1/repos`: create a repo from a signed NIP-34 announcement.

Tenant creation payload (JSON):
```
{
  "host": "relay.example",
  "name": "Example Relay",
  "description": "...",
  "public_read": false,
  "public_write": false,
  "auth_required": true
}
```

## Auth service (NIP-98)

The auth service provides:
- `POST /v1/signup`: create or reuse an account bound to a Nostr pubkey.
- `GET /v1/profile` and `PATCH /v1/profile`: manage profile data.

## Configuration

All services read from `.env`. Key operator variables include:
- `GITTREE_RELAY_URLS`
- `GITTREE_CONTROL_TOKEN`
- `GITTREE_CONTROL_ADMIN_KEYS`
- `GITTREE_FORGEJO_API_TOKEN`
- `GITTREE_FORGEJO_OWNER`
- `GITTREE_FORGEJO_WEBHOOK_SECRET`
- `GITTREE_STORAGE_READ_URL`
- `GITTREE_STORAGE_WRITE_URL`

See `.env.example` for the full set.

## Logs

Logs are written under `./logs/` by default, with per-service subdirectories
and Docker service output.

## Contributing

Contributions are welcome. Feel free to open issues or pull requests.

## License

Gittree is released into the public domain under the Unlicense. See `LICENSE`.
