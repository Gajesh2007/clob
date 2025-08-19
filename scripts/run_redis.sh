#!/usr/bin/env bash

set -euo pipefail

# Tuned Redis runner for Linux (Debian/Ubuntu). Uses host networking and kernel tuning
# for maximum throughput. Supports two modes via REDIS_MODE:
#   fast      -> in-memory only (no persistence), safest for benchmarks
#   durable   -> AOF everysec, RDB disabled (good perf with durability)
#
# Environment overrides:
#   REDIS_MODE       (fast|durable) default: fast
#   REDIS_IMAGE      default: redis:7
#   REDIS_NAME       default: redis-server
#   REDIS_PORT       default: 6379 (used only if host network not available)
#   IO_THREADS       default: 4
#   TMPFS_SIZE       default: 4g (fast mode tmpfs size)
#   BIND_ADDR        default: 0.0.0.0 (listen on all interfaces)
#   PROTECTED_MODE   default: no (must be 'yes' to restrict to localhost)

REDIS_MODE=${REDIS_MODE:-fast}
REDIS_IMAGE=${REDIS_IMAGE:-redis:7}
REDIS_NAME=${REDIS_NAME:-redis-server}
REDIS_PORT=${REDIS_PORT:-6379}
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

# Prefer host networking on Linux to avoid NAT overhead
NETWORK_FLAGS=( --network host )

# Fallback to port mapping if host network not available (non-Linux)
if ! grep -qiE 'linux' /proc/version 2>/dev/null; then
  NETWORK_FLAGS=( -p "${REDIS_PORT}:6379" )
fi

echo "[redis] Starting container ${REDIS_NAME} (mode=${REDIS_MODE})"
docker run -d \
  --name "${REDIS_NAME}" \
  "${NETWORK_FLAGS[@]}" \
  --restart unless-stopped \
  --ulimit nofile=1000000:1000000 \
  --sysctl net.core.somaxconn=65535 \
  ${RUN_FLAGS[@]} \
  "${REDIS_IMAGE}" \
  redis-server \
  ${REDIS_FLAGS[@]} \
  ${PERSIST_FLAGS[@]}

echo "[redis] Waiting for readiness..."
until docker exec "${REDIS_NAME}" redis-cli -u redis://127.0.0.1:6379 ping >/dev/null 2>&1; do
  sleep 0.5
done

echo "[redis] Ready. Connection: redis://127.0.0.1:6379 (mode=${REDIS_MODE})"
echo "[redis] Example CLI: docker exec -it ${REDIS_NAME} redis-cli"

