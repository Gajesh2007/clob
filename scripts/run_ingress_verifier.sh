#!/usr/bin/env bash
set -euo pipefail

# Build and run the ingress-verifier service in Docker.
# Defaults assume Redis is reachable at 10.10.0.2:6379 and we expose
# TCP ingestion on 8080 and HTTP API on 9090.
#
# Environment overrides:
#   IMAGE_NAME     (default: ingress-verifier:local)
#   CONTAINER_NAME (default: ingress-verifier)
#   REDIS_ADDR     (default: redis://10.10.0.2:6379/)
#   TCP_PORT       (default: 8080)
#   HTTP_PORT      (default: 9090)
#   RUST_LOG       (default: info)
#   HOST_BIND      (default: 0.0.0.0) host interface to bind published ports

IMAGE_NAME=${IMAGE_NAME:-ingress-verifier:local}
CONTAINER_NAME=${CONTAINER_NAME:-ingress-verifier}
REDIS_ADDR=${REDIS_ADDR:-redis://10.10.0.2:6379/}
TCP_PORT=${TCP_PORT:-8080}
HTTP_PORT=${HTTP_PORT:-9090}
RUST_LOG=${RUST_LOG:-info}
HOST_BIND=${HOST_BIND:-0.0.0.0}

if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required but not installed. Please install Docker." >&2
  exit 1
fi

echo "[ingress-verifier] Building image ${IMAGE_NAME}..."
docker build -f clob/crates/ingress-verifier/Dockerfile -t "${IMAGE_NAME}" .

echo "[ingress-verifier] Removing any existing container ${CONTAINER_NAME}..."
docker rm -f "${CONTAINER_NAME}" 2>/dev/null || true

# Note: We bind the service to 0.0.0.0 inside the container so ports are exposed.
echo "[ingress-verifier] Starting container..."
docker run -d \
  --name "${CONTAINER_NAME}" \
  --restart unless-stopped \
  -p "${HOST_BIND}:${TCP_PORT}:8080" \
  -p "${HOST_BIND}:${HTTP_PORT}:9090" \
  -e RUST_LOG="${RUST_LOG}" \
  -e CLOB_REDIS_ADDR="${REDIS_ADDR}" \
  -e CLOB_EXECUTION_PLANE__TCP_LISTEN_ADDR="0.0.0.0:8080" \
  -e CLOB_EXECUTION_PLANE__HTTP_LISTEN_ADDR="0.0.0.0:9090" \
  "${IMAGE_NAME}"

# Basic readiness wait: ensure HTTP is listening (best-effort)
echo "[ingress-verifier] Waiting for HTTP to be ready on localhost:${HTTP_PORT}..."
for _ in {1..30}; do
  if command -v curl >/dev/null 2>&1; then
    if curl -sSf "http://127.0.0.1:${HTTP_PORT}/users" -X POST >/dev/null 2>&1; then
      break
    fi
  fi
  sleep 0.5
done

echo "[ingress-verifier] Running."
echo "  TCP ingest: tcp://${HOST_BIND}:${TCP_PORT}"
echo "  HTTP API:   http://${HOST_BIND}:${HTTP_PORT}"
echo "  Redis addr: ${REDIS_ADDR}"
echo "  Logs:       docker logs -f ${CONTAINER_NAME}"
