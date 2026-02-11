#!/usr/bin/env bash
set -euo pipefail

if [[ "${GITTREE_STORAGE_AUTOSTART_DB:-1}" != "1" ]]; then
  "$@"
  exit $?
fi

if [[ -n "${GITTREE_STORAGE_TEST_DATABASE_URL:-}" ]]; then
  "$@"
  exit $?
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found and GITTREE_STORAGE_TEST_DATABASE_URL is unset" >&2
  exit 1
fi

image="${GITTREE_STORAGE_TEST_DB_IMAGE:-postgres:16-alpine}"
database="${GITTREE_STORAGE_TEST_DB_NAME:-gittree}"
username="${GITTREE_STORAGE_TEST_DB_USER:-gittree}"
password="${GITTREE_STORAGE_TEST_DB_PASSWORD:-gittree}"
container_name="gittree-test-db-${RANDOM}-$$"

cleanup() {
  docker rm -f "${container_name}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d --rm \
  --name "${container_name}" \
  -e POSTGRES_DB="${database}" \
  -e POSTGRES_USER="${username}" \
  -e POSTGRES_PASSWORD="${password}" \
  -p 127.0.0.1::5432 \
  "${image}" >/dev/null

for _ in $(seq 1 60); do
  status="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "${container_name}" 2>/dev/null || true)"
  if [[ "${status}" == "healthy" || "${status}" == "running" ]]; then
    break
  fi
  sleep 1
done

port="$(docker inspect -f '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "${container_name}")"
if [[ -z "${port}" ]]; then
  echo "failed to resolve mapped postgres port for ${container_name}" >&2
  docker logs "${container_name}" >&2 || true
  exit 1
fi

export GITTREE_STORAGE_TEST_DATABASE_URL="postgres://${username}:${password}@127.0.0.1:${port}/${database}"
"$@"
