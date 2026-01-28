# Deploy assets

## Ports
- 80: nginx reverse proxy
- 8080: relay service
- 8081: admission service
- 8082: state service
- 8083: coordinator service
- 8084: sync service
- 8085: git-http service
- 8086: ui service
- 8087: webhook service
- 5432: postgres

## Volumes
- pgdata: postgres data directory mounted at `/var/lib/postgresql/data`

## External relay config

Required env vars for external relay operation:
- `GITTREE_RELAY_URLS`: comma-separated relay URLs (wss://...).
- `GITTREE_RELAY_COMPAT_MODE`: `strict`, `warn`, or `allow`.

Optional probe settings:
- `GITTREE_RELAY_PROBE_ACTIVE`: enable active write/read probe.
- `GITTREE_RELAY_PROBE_TIMEOUT_SECS`: active probe timeout (seconds).
- `GITTREE_RELAY_PROBE_SECRET_KEY`: optional hex secret key for probe signer.

Use `gittree-relay-probe` to validate and store compatibility results before
starting services that enforce compatibility.
