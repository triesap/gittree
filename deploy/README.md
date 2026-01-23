# Deploy assets

## Ports
- 80: nginx reverse proxy
- 8080: relay service
- 8081: admission service
- 8082: state service
- 8083: coordinator service
- 8084: sync service
- 8085: git-http service
- 5432: postgres

## Volumes
- pgdata: postgres data directory mounted at `/var/lib/postgresql/data`
