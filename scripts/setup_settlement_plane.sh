#!/usr/bin/env bash
set -euxo pipefail

# --- User Configuration ---
REPO_URL="https://github.com/Gajesh2007/clob.git" 
# The internal IP of the EigenDA proxy VM you created
EIGENDA_PROXY_IP="10.10.0.3" # Example: 10.10.0.3
REDIS_IP="10.10.0.2"
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

# Create config.toml with the internal EigenDA proxy IP
cat > config.toml <<EOF
redis_addr = "redis://${REDIS_IP}:6379/"
[settlement_plane]
checkpoint_interval_seconds = 5
eigenda_proxy_url = "http://${EIGENDA_PROXY_IP}:3100/put?commitment_mode=standard"
checkpoint_file_path = "checkpoint.json"
[verifier]
checkpoint_file_path = "checkpoint.json"
execution_log_path = "execution.log"
[execution_plane]
tcp_listen_addr = "0.0.0.0:8080"
http_listen_addr = "0.0.0.0:9090"
execution_log_path = "execution.log"
[multicast]
addr = "239.0.0.1:9000"
bind_addr = "0.0.0.0:0"
EOF

cargo build --release --bin settlement-plane

# 4. Create and run systemd service
cat > /etc/systemd/system/settlement-plane.service <<EOF
[Unit]
Description=CLOB Settlement Plane Service
After=network.target

[Service]
User=root
Group=root
WorkingDirectory=/opt/nasdaq/clob
ExecStart=/opt/nasdaq/clob/target/release/settlement-plane
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# 5. Start the service
systemctl daemon-reload
systemctl enable settlement-plane.service
systemctl start settlement-plane.service

echo "Settlement Plane setup complete and service started via systemd."
