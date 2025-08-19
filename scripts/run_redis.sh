#!/usr/bin/env bash

set -euo pipefail

# Tuned Redis runner for Linux (Debian/Ubuntu). Defaults to port mapping so ports
# are visible in `docker ps`. Supports two modes via REDIS_MODE:
#   fast      -> in-memory only (no persistence), safest for benchmarks
#   durable   -> AOF everysec, RDB disabled (good perf with durability)
#
# Environment overrides:
#   REDIS_MODE       (fast|durable) default: fast
#   REDIS_IMAGE      default: redis:latest
#   REDIS_NAME       default: redis-server
#   REDIS_PORT       default: 6379
#   REDIS_HOST_BIND  default: 0.0.0.0 (host bind addr for -p)
#   REDIS_NETWORK    (ports|host) default: ports; host requires Linux and will hide Ports in `docker ps`
#   IO_THREADS       default: 4
#   TMPFS_SIZE       default: 4g (fast mode tmpfs size)
#   BIND_ADDR        default: 0.0.0.0 (listen on all interfaces)
#   PROTECTED_MODE   default: no (must be 'yes' to restrict to localhost)

REDIS_MODE=${REDIS_MODE:-fast}
REDIS_IMAGE=${REDIS_IMAGE:-redis:latest}
REDIS_NAME=${REDIS_NAME:-redis-server}
REDIS_PORT=${REDIS_PORT:-6379}
REDIS_HOST_BIND=${REDIS_HOST_BIND:-0.0.0.0}
REDIS_NETWORK=${REDIS_NETWORK:-ports}
IO_THREADS=${IO_THREADS:-4}
TMPFS_SIZE=${TMPFS_SIZE:-4g}
BIND_ADDR=${BIND_ADDR:-0.0.0.0}
PROTECTED_MODE=${PROTECTED_MODE:-no}

echo "[redis] Mode=${REDIS_MODE} Image=${REDIS_IMAGE} Name=${REDIS_NAME}"

# Ensure Docker is installed and running
if ! command -v docker >/dev/null 2>&1; then
  echo "[redis] Installing Docker..."
  sudo apt-get update -y
  sudo apt-get install -y ca-certificates curl gnupg lsb-release
  curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable" | sudo tee /etc/apt/sources.list.d/docker.list >/dev/null
  sudo apt-get update -y
  sudo apt-get install -y docker-ce docker-ce-cli containerd.io
  sudo systemctl enable --now docker
fi

echo "[redis] Kernel/network tuning (requires sudo)"
# Increase connection backlog and allow memory overcommit for forkless AOF fsync
sudo sysctl -w net.core.somaxconn=65535 >/dev/null
sudo sysctl -w vm.overcommit_memory=1 >/dev/null
# Reuse TIME-WAIT sockets to reduce ephemeral port pressure
sudo sysctl -w net.ipv4.tcp_tw_reuse=1 >/dev/null || true
# Disable Transparent Huge Pages for more predictable latency (best-effort)
if [[ -w /sys/kernel/mm/transparent_hugepage/enabled ]]; then
  echo never | sudo tee /sys/kernel/mm/transparent_hugepage/enabled >/dev/null || true
fi

echo "[redis] Pulling image ${REDIS_IMAGE}"
docker pull "${REDIS_IMAGE}"

echo "[redis] Removing any existing container"
docker rm -f "${REDIS_NAME}" 2>/dev/null || true

# Common redis-server flags
REDIS_FLAGS=(
  --tcp-backlog 65535
  --io-threads "${IO_THREADS}"
  --io-threads-do-reads yes
  --save ""
  --bind "${BIND_ADDR}"
  --protected-mode "${PROTECTED_MODE}"
)

case "${REDIS_MODE}" in
  fast)
    # No persistence; tmpfs for /data for highest performance
    PERSIST_FLAGS=( --appendonly no )
    RUN_FLAGS=(
      --tmpfs /data:rw,nosuid,nodev,noexec,size=${TMPFS_SIZE}
    )
    ;;
  durable)
    # AOF everysec; keep /data on a named volume
    PERSIST_FLAGS=( --appendonly yes --appendfsync everysec )
    RUN_FLAGS=( -v redis-data:/data )
    ;;
  *)
    echo "[redis] Unknown REDIS_MODE='${REDIS_MODE}'. Use 'fast' or 'durable'." >&2
    exit 1
    ;;
esac

# Networking: default to explicit port mapping so ports show up in `docker ps`
NETWORK_FLAGS=()
if grep -qiE 'linux' /proc/version 2>/dev/null; then
  case "${REDIS_NETWORK}" in
    host)
      NETWORK_FLAGS=( --network host )
      ;;
    ports|*)
      NETWORK_FLAGS=( -p "${REDIS_HOST_BIND}:${REDIS_PORT}:6379" )
      ;;
  esac
else
  # Non-Linux (e.g., macOS, Windows): host network not supported; use ports
  NETWORK_FLAGS=( -p "${REDIS_HOST_BIND}:${REDIS_PORT}:6379" )
  REDIS_NETWORK=ports
fi

# Only set container-level sysctls when NOT using host networking. Docker forbids
# setting non-namespaced sysctls (like net.core.somaxconn) in host network namespace.
SYSCTL_DOCKER_FLAGS=()
if [[ "${NETWORK_FLAGS[0]-}" != "--network" || "${NETWORK_FLAGS[1]-}" != "host" ]]; then
  SYSCTL_DOCKER_FLAGS=( --sysctl net.core.somaxconn=65535 )
fi

echo "[redis] Starting container ${REDIS_NAME} (mode=${REDIS_MODE})"
docker run -d \
  --name "${REDIS_NAME}" \
  "${NETWORK_FLAGS[@]}" \
  --restart unless-stopped \
  --ulimit nofile=1000000:1000000 \
  ${SYSCTL_DOCKER_FLAGS[@]} \
  ${RUN_FLAGS[@]} \
  "${REDIS_IMAGE}" \
  redis-server \
  ${REDIS_FLAGS[@]} \
  ${PERSIST_FLAGS[@]}

echo "[redis] Waiting for readiness..."
until docker exec "${REDIS_NAME}" redis-cli -u redis://127.0.0.1:6379 ping >/dev/null 2>&1; do
  sleep 0.5
done

if [[ "${REDIS_NETWORK}" == "host" ]]; then
  CONNECTION_URI="redis://0.0.0.0:6379"
else
  CONNECTION_URI="redis://${REDIS_HOST_BIND}:${REDIS_PORT}"
fi
echo "[redis] Ready. Connection: ${CONNECTION_URI} (mode=${REDIS_MODE}, network=${REDIS_NETWORK})"
echo "[redis] Example CLI: docker exec -it ${REDIS_NAME} redis-cli"

