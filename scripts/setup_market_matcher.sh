#!/usr/bin/env bash
set -euxo pipefail

# --- User Configuration ---
REPO_URL="https://github.com/Gajesh2007/clob.git"
# The internal IP of the Redis VM you created
REDIS_IP="10.10.0.2" # Example: 10.10.0.2
# ------------------------

# 1. Install Dependencies
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y curl build-essential pkg-config libssl-dev git

# 2. Install Rust
curl https://sh.rustup.rs -sSf | sh -s -- -y
source /root/.cargo/env

# 3. Clone and Build Project
git clone "$REPO_URL" /opt/nasdaq
cd /opt/nasdaq/clob

# 4. Create and run systemd service
export REDIS_ADDR="redis://${REDIS_IP}:6379/"
cargo build --release --bin market-matcher

# This example starts a single matcher for market 1.
# A real script would use instance metadata to determine its shard.
MARKET_IDS="1"

cat > /etc/systemd/system/market-matcher.service <<EOF
[Unit]
Description=CLOB Market Matcher Service
After=network.target

[Service]
User=root
Group=root
WorkingDirectory=/opt/nasdaq/clob
Environment="REDIS_ADDR=${REDIS_ADDR}"
ExecStart=/opt/nasdaq/clob/target/release/market-matcher ${MARKET_IDS}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# 5. Start the service
systemctl daemon-reload
systemctl enable market-matcher.service
systemctl start market-matcher.service

echo "Market Matcher setup complete and service started for market(s) ${MARKET_IDS} via systemd."
