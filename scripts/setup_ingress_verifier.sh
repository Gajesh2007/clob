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
cargo build --release --bin ingress-verifier

cat > /etc/systemd/system/ingress-verifier.service <<EOF
[Unit]
Description=CLOB Ingress Verifier Service
After=network.target

[Service]
User=root
Group=root
WorkingDirectory=/opt/nasdaq/clob
Environment="REDIS_ADDR=${REDIS_ADDR}"
ExecStart=/opt/nasdaq/clob/target/release/ingress-verifier
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# 5. Start the service
systemctl daemon-reload
systemctl enable ingress-verifier.service
systemctl start ingress-verifier.service

echo "Ingress Verifier setup complete and service started via systemd."
