#!/usr/bin/env bash
set -euo pipefail

# Build and run the market-matcher service in Docker.
# Requires at least one market id as argument(s).
# Defaults to Redis at 10.10.0.2:6379.
#
# Usage:
#   ./scripts/run_market_matcher.sh 1 2 3
#
# Environment overrides:
#   IMAGE_NAME       (default: market-matcher:local)
#   CONTAINER_NAME   (default: market-matcher)
#   REDIS_ADDR       (default: redis://10.10.0.2:6379/)
#   RUST_LOG         (default: info)

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <market_id_1> [market_id_2 ...]" >&2
  exit 1
fi

IMAGE_NAME=${IMAGE_NAME:-market-matcher:local}
CONTAINER_NAME=${CONTAINER_NAME:-market-matcher}
REDIS_ADDR=${REDIS_ADDR:-redis://10.10.0.2:6379/}
RUST_LOG=${RUST_LOG:-info}

if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is required but not installed. Please install Docker." >&2
  exit 1
fi

echo "[market-matcher] Building image ${IMAGE_NAME}..."
docker build -f clob/crates/market-matcher/Dockerfile -t "${IMAGE_NAME}" .

echo "[market-matcher] Removing any existing container ${CONTAINER_NAME}..."
docker rm -f "${CONTAINER_NAME}" 2>/dev/null || true

# Prepare arguments
MARKET_ARGS=("$@")

# Start container.
echo "[market-matcher] Starting container..."
docker run -d \
  --name "${CONTAINER_NAME}" \
  --restart unless-stopped \
  -e RUST_LOG="${RUST_LOG}" \
  -e CLOB_REDIS_ADDR="${REDIS_ADDR}" \
  "${IMAGE_NAME}" \
  "${MARKET_ARGS[@]}"

echo "[market-matcher] Running."
echo "  Markets:     ${MARKET_ARGS[*]}"
echo "  Redis addr:  ${REDIS_ADDR}"
echo "  Logs:        docker logs -f ${CONTAINER_NAME}"
