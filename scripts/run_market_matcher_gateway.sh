#!/usr/bin/env bash
set -euo pipefail

# Build and run the market-matcher-gateway service in Docker.
# Environment overrides:
#   IMAGE_NAME       (default: market-matcher-gateway:local)
#   CONTAINER_NAME   (default: market-matcher-gateway)
#   REDIS_ADDR       (default: redis://10.10.0.2:6379/)
#   HTTP_PORT        (default: 9100)
#   RUST_LOG         (default: info)

IMAGE_NAME=${IMAGE_NAME:-market-matcher-gateway:local}
CONTAINER_NAME=${CONTAINER_NAME:-market-matcher-gateway}
REDIS_ADDR=${REDIS_ADDR:-redis://10.10.0.2:6379/}
HTTP_PORT=${HTTP_PORT:-9100}
RUST_LOG=${RUST_LOG:-info}

if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required but not installed. Please install Docker." >&2
  exit 1
fi

echo "[gateway] Building image ${IMAGE_NAME}..."
docker build -f clob/crates/market-matcher-gateway/Dockerfile -t "${IMAGE_NAME}" .

echo "[gateway] Removing any existing container ${CONTAINER_NAME}..."
docker rm -f "${CONTAINER_NAME}" 2>/dev/null || true

echo "[gateway] Starting container..."
docker run -d \
  --name "${CONTAINER_NAME}" \
  --restart unless-stopped \
  -p "${HTTP_PORT}:9100" \
  -e RUST_LOG="${RUST_LOG}" \
  -e CLOB_REDIS_ADDR="${REDIS_ADDR}" \
  -e CLOB_GATEWAY__HTTP_LISTEN_ADDR="0.0.0.0:9100" \
  "${IMAGE_NAME}"

echo "[gateway] Running."
echo "  HTTP:       http://127.0.0.1:${HTTP_PORT}"
echo "  Endpoints:  /sse, /ws"
echo "  Redis addr: ${REDIS_ADDR}"
echo "  Logs:       docker logs -f ${CONTAINER_NAME}"


