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

# 3. Clone or Update Project
if [ -d "/opt/nasdaq/.git" ]; then
    echo "Repository exists, pulling latest changes..."
    cd /opt/nasdaq
    git pull
else
    echo "Cloning repository..."
    git clone "$REPO_URL" /opt/nasdaq
fi
cd /opt/nasdaq/clob

# Create config.toml
cat > config.toml <<EOF
redis_addr = "redis://${REDIS_IP}:6379/"
[settlement_plane]
checkpoint_interval_seconds = 5
eigenda_proxy_url = "http://10.10.0.3:3100/put?commitment_mode=standard"
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

export RUST_LOG=info

# 4. Create and run systemd service
cargo build --release --bin ingress-verifier

cat > /etc/systemd/system/ingress-verifier.service <<EOF
[Unit]
Description=CLOB Ingress Verifier Service
After=network.target

[Service]
User=root
Group=root
WorkingDirectory=/opt/nasdaq/clob
ExecStart=/opt/nasdaq/clob/target/release/ingress-verifier
Restart=on-failure
RestartSec=5
# Increase OS limits for high-throughput networking
LimitNOFILE=1048576
LimitNPROC=infinity

[Install]
WantedBy=multi-user.target
EOF

# 5. Start the service
systemctl daemon-reload
systemctl enable ingress-verifier.service
systemctl start ingress-verifier.service

echo "Ingress Verifier setup complete and service started via systemd."
