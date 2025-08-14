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

REPO_URL=${REPO_URL:-"https://github.com/Gajesh2007/clob.git"}
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

# Resolve repo root: prefer this script's repo; fall back to common locations; else use current dir if valid; else clone
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CANDIDATE_BASE="${SCRIPT_DIR%/scripts}"
if [[ -f "$CANDIDATE_BASE/Cargo.toml" && -d "$CANDIDATE_BASE/crates/benchmark-client" ]]; then
  REPO_ROOT="$CANDIDATE_BASE"
elif [[ -f "$CANDIDATE_BASE/clob/Cargo.toml" && -d "$CANDIDATE_BASE/clob/crates/benchmark-client" ]]; then
  REPO_ROOT="$CANDIDATE_BASE/clob"
elif [[ -f "/opt/nasdaq/clob/Cargo.toml" && -d "/opt/nasdaq/clob/crates/benchmark-client" ]]; then
  REPO_ROOT="/opt/nasdaq/clob"
elif [[ -f "/opt/nasdaq/NASDAQ/clob/Cargo.toml" && -d "/opt/nasdaq/NASDAQ/clob/crates/benchmark-client" ]]; then
  REPO_ROOT="/opt/nasdaq/NASDAQ/clob"
elif [[ -f "$PWD/Cargo.toml" && -d "$PWD/crates/benchmark-client" ]]; then
  REPO_ROOT="$PWD"
else
  echo "No Cargo.toml workspace found; cloning to /opt/nasdaq/NASDAQ/clob..."
  sudo mkdir -p /opt/nasdaq/NASDAQ
  sudo chown -R "$USER":"$USER" /opt/nasdaq/NASDAQ || true
  git clone "$REPO_URL" /opt/nasdaq/NASDAQ/clob
  REPO_ROOT="/opt/nasdaq/NASDAQ/clob"
fi
cd "$REPO_ROOT"

# Ensure Rust toolchain
if ! command -v cargo >/dev/null 2>&1; then
  echo "Installing Rust toolchain..."
  curl https://sh.rustup.rs -sSf | sh -s -- -y
  source "$HOME/.cargo/env"
fi

# Raise file descriptor limit for many concurrent sockets (best-effort)
if command -v prlimit >/dev/null 2>&1; then
  sudo prlimit --pid $$ --nofile=1048576:1048576 || true
else
  ulimit -n 1048576 || true
fi

# Build benchmark client
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
