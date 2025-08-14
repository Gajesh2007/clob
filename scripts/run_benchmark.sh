#!/usr/bin/env bash
set -euo pipefail

# Usage:
#  scripts/run_benchmark.sh \
#    -H 10.10.0.5:9090 \
#    -T 10.10.0.5:8080 \
#    -R redis://10.10.0.2:6379/ \
#    -m 1 \
#    -t 200 \
#    -d 30 \
#    [-l info]
#
# Flags:
#  -H  HTTP address for ingress-verifier (host:port), default: 127.0.0.1:9090
#  -T  TCP address for ingress-verifier (host:port), default: 127.0.0.1:8080
#  -R  Redis address (redis://host:port/), default: redis://127.0.0.1/
#  -m  Market ID (u32), default: 1
#  -t  Number of traders (uint), default: 10
#  -d  Duration seconds (uint), default: 30
#  -l  RUST_LOG level (info|debug|warn), default: info

HTTP_ADDR="127.0.0.1:9090"
TCP_ADDR="127.0.0.1:8080"
REDIS_ADDR="redis://127.0.0.1/"
MARKET_ID=1
NUM_TRADERS=10
DURATION_SECS=30
RUST_LOG_LEVEL="info"

while getopts ":H:T:R:m:t:d:l:" opt; do
  case ${opt} in
    H) HTTP_ADDR="$OPTARG" ;;
    T) TCP_ADDR="$OPTARG" ;;
    R) REDIS_ADDR="$OPTARG" ;;
    m) MARKET_ID="$OPTARG" ;;
    t) NUM_TRADERS="$OPTARG" ;;
    d) DURATION_SECS="$OPTARG" ;;
    l) RUST_LOG_LEVEL="$OPTARG" ;;
    *) echo "Invalid option: -$OPTARG" >&2; exit 2 ;;
  esac
done

# Locate repo root from this script's directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR%/scripts}"
cd "$REPO_ROOT"

# Raise file descriptor limit for many concurrent sockets (best-effort)
if command -v prlimit >/dev/null 2>&1; then
  sudo prlimit --nofile=1048576:1048576 $$ || true
else
  ulimit -n 1048576 || true
fi

# Build benchmark client
if ! command -v cargo >/dev/null 2>&1; then
  echo "Error: cargo not found. Install Rust toolchain first (https://rustup.rs)." >&2
  exit 1
fi

cargo build --release --bin benchmark-client

# Run benchmark
export RUST_LOG="$RUST_LOG_LEVEL"
./target/release/benchmark-client \
  --http-addr "$HTTP_ADDR" \
  --tcp-addr "$TCP_ADDR" \
  --redis-addr "$REDIS_ADDR" \
  --market-id "$MARKET_ID" \
  --num-traders "$NUM_TRADERS" \
  --duration-secs "$DURATION_SECS"
