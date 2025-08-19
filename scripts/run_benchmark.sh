#!/usr/bin/env bash

set -euo pipefail

# Build and run the benchmark-client in Docker.
# Default targets (on-net):
# - Ingress-Verifier HTTP: 10.10.0.5:9090
# - Ingress-Verifier TCP:  10.10.0.5:8080
# - Redis:                 redis://10.10.0.2:6379/
# On Linux, we default to --network host so 127.0.0.1 resolves to the host.
# On non-Linux (e.g., macOS/Windows), host networking is unavailable; override
# addresses to use host.docker.internal or set BENCH_NETWORK=bridge and pass
# explicit host addresses.
#
# Environment overrides:
#   IMAGE_NAME       (default: benchmark-client:local)
#   CONTAINER_NAME   (default: benchmark-client)
#   RUST_LOG         (default: info)
#   BENCH_NETWORK    (host|bridge) default: host on Linux, bridge otherwise
#   HTTP_ADDR        (default: 10.10.0.5:9090)
#   TCP_ADDR         (default: 10.10.0.5:8080)
#   REDIS_ADDR       (default: redis://10.10.0.2:6379/)
#   MARKET_ID        (default: 1)
#   NUM_TRADERS      (default: 10)
#   DURATION_SECS    (default: 30)
#   EVENT_SOURCE     (redis|gateway_ws) default: redis
#   GATEWAY_ADDR     (default: 127.0.0.1:9100) used when EVENT_SOURCE=gateway_ws

IMAGE_NAME=${IMAGE_NAME:-benchmark-client:local}
CONTAINER_NAME=${CONTAINER_NAME:-benchmark-client}
RUST_LOG=${RUST_LOG:-info}

# OS detection to pick a sane default network mode
if grep -qiE 'linux' /proc/version 2>/dev/null; then
  BENCH_NETWORK=${BENCH_NETWORK:-host}
else
  BENCH_NETWORK=${BENCH_NETWORK:-bridge}
fi

# Connection targets (overridable)
HTTP_ADDR=${HTTP_ADDR:-10.10.0.5:9090}
TCP_ADDR=${TCP_ADDR:-10.10.0.5:8080}
REDIS_ADDR=${REDIS_ADDR:-redis://10.10.0.2:6379/}
MARKET_ID=${MARKET_ID:-1}
NUM_TRADERS=${NUM_TRADERS:-10}
DURATION_SECS=${DURATION_SECS:-30}
EVENT_SOURCE=${EVENT_SOURCE:-redis}
GATEWAY_ADDR=${GATEWAY_ADDR:-127.0.0.1:9100}

if ! command -v docker >/dev/null 2>&1; then
  if command -v apt-get >/dev/null 2>&1; then
    echo "[benchmark] Installing Docker via apt..."
    . /etc/os-release 2>/dev/null || true
    DISTRO_ID=${ID:-debian}
    CODENAME=${VERSION_CODENAME:-}
    if [[ -z "${CODENAME}" ]] && command -v lsb_release >/dev/null 2>&1; then
      CODENAME=$(lsb_release -cs)
    fi
    # Map distro id to Docker repo path
    case "${DISTRO_ID}" in
      ubuntu) DOCKER_LINUX_PATH=ubuntu ;;
      debian|*) DOCKER_LINUX_PATH=debian ;;
    esac

    sudo apt-get update -y
    sudo apt-get install -y ca-certificates curl gnupg lsb-release
    sudo install -m 0755 -d /etc/apt/keyrings
    curl -fsSL "https://download.docker.com/linux/${DOCKER_LINUX_PATH}/gpg" | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    sudo chmod a+r /etc/apt/keyrings/docker.gpg
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/${DOCKER_LINUX_PATH} ${CODENAME} stable" | sudo tee /etc/apt/sources.list.d/docker.list >/dev/null
    sudo apt-get update -y
    sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
    sudo systemctl enable --now docker || true
  else
    echo "Docker is required but not installed. Please install Docker for your OS." >&2
    exit 1
  fi
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${SCRIPT_DIR%/scripts}"

echo "[benchmark] Building image ${IMAGE_NAME}..."
docker build -f "${REPO_ROOT}/clob/crates/benchmark-client/Dockerfile" -t "${IMAGE_NAME}" "${REPO_ROOT}"

echo "[benchmark] Removing any existing container ${CONTAINER_NAME}..."
docker rm -f "${CONTAINER_NAME}" 2>/dev/null || true

# Docker networking flags
RUN_FLAGS=( -it --rm --name "${CONTAINER_NAME}" -e RUST_LOG="${RUST_LOG}" )
if [[ "${BENCH_NETWORK}" == "host" ]]; then
  RUN_FLAGS+=( --network host )
fi

# Assemble CLI args for benchmark-client
ARGS=(
  --http-addr "${HTTP_ADDR}"
  --tcp-addr "${TCP_ADDR}"
  --redis-addr "${REDIS_ADDR}"
  --market-id "${MARKET_ID}"
  --num-traders "${NUM_TRADERS}"
  --duration-secs "${DURATION_SECS}"
  --event-source "${EVENT_SOURCE}"
)

if [[ -n "${GATEWAY_ADDR}" ]]; then
  ARGS+=( --gateway-addr "${GATEWAY_ADDR}" )
fi

echo "[benchmark] Starting benchmark-client..."
echo "  Network:      ${BENCH_NETWORK}"
echo "  HTTP addr:    ${HTTP_ADDR}"
echo "  TCP addr:     ${TCP_ADDR}"
echo "  Redis addr:   ${REDIS_ADDR}"
echo "  Market ID:    ${MARKET_ID}"
echo "  Traders:      ${NUM_TRADERS}"
echo "  Duration:     ${DURATION_SECS}s"
echo "  Event source: ${EVENT_SOURCE}"
if [[ "${EVENT_SOURCE}" == "gateway_ws" ]]; then
  echo "  Gateway:      ${GATEWAY_ADDR}"
fi

# Run the container and forward any extra args the user provided to the binary
docker run "${RUN_FLAGS[@]}" "${IMAGE_NAME}" "${ARGS[@]}" "$@"

