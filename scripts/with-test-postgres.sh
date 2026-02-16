#!/usr/bin/env bash
set -euo pipefail

check_url_reachable() {
  python3 - "$1" <<'PY'
import struct
import socket
import sys
from urllib.parse import urlparse

url = sys.argv[1]
parsed = urlparse(url)
host = parsed.hostname
port = parsed.port or 5432
username = parsed.username or "gittree"
database = (parsed.path or "/gittree").lstrip("/") or "gittree"

if not host:
    print(f"invalid postgres url (missing host): {url}", file=sys.stderr)
    raise SystemExit(2)

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.settimeout(1.0)
try:
    sock.connect((host, port))
    # Minimal PostgreSQL startup message. If the endpoint is postgres-compatible,
    # the first response message type is usually Authentication ('R') or Error ('E').
    fields = [
        b"user",
        username.encode("utf-8"),
        b"database",
        database.encode("utf-8"),
        b"client_encoding",
        b"UTF8",
    ]
    payload = b"".join(field + b"\x00" for field in fields) + b"\x00"
    body = struct.pack("!I", 196608) + payload
    sock.sendall(struct.pack("!I", len(body) + 4) + body)
    response_type = sock.recv(1)
    if response_type not in {b"R", b"E"}:
        print(
            f"endpoint at {host}:{port} is reachable but not postgres protocol",
            file=sys.stderr,
        )
        raise SystemExit(1)
except OSError as exc:
    print(f"cannot reach postgres at {host}:{port} ({exc})", file=sys.stderr)
    raise SystemExit(1)
finally:
    sock.close()
PY
}

if [[ "${GITTREE_STORAGE_AUTOSTART_DB:-1}" != "1" ]]; then
  "$@"
  exit $?
fi

if [[ -n "${GITTREE_STORAGE_TEST_DATABASE_URL:-}" ]]; then
  if ! check_url_reachable "${GITTREE_STORAGE_TEST_DATABASE_URL}"; then
    echo "set a reachable GITTREE_STORAGE_TEST_DATABASE_URL or unset it to use docker autostart" >&2
    exit 1
  fi
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
