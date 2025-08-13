# High-Performance Verifiable Central Limit Order Book (CLOB)

This project is a Rust implementation of the high-performance, verifiably-settled Central Limit Order Book described in `PRODUCT.md`. It simulates a complete, end-to-end financial exchange system, from user creation and deposits to trade execution and independent verification.

## Architecture Overview

The system is a distributed application composed of several distinct services that communicate via a high-performance message bus (Redis). This architecture is designed for scalability and low latency by separating concerns.

*   **Ingress & Verification (`ingress-verifier`):** A fleet of servers whose only job is to handle client connections, validate signatures, and publish validated transactions to the message bus.
*   **Market Matching (`market-matcher`):** A sharded fleet of servers, where each process is responsible for a single market. It listens for orders for its market on the message bus and performs the matching.
*   **Settlement (`settlement-plane`):** A reliable background service that consumes the transaction log, fetches state snapshots, calculates Merkle roots, and creates verifiable checkpoints.
*   **Message Bus (`Redis`):** Acts as the ultra-low-latency message bus for passing transactions and also serves as the durable `execution_log`.

## Codebase Structure

The project is organized as a Cargo workspace with several distinct crates:

*   `clob/`
    *   `crates/`
        *   **`common-types`**: Defines all core data structures (`Order`, `Trade`, `Transaction`, etc.).
        *   **`matching-engine`**: A pure library implementing the core CLOB logic.
        *   **`ingress-verifier`**: A server application that handles client connections, signature verification, and user management.
        *   **`market-matcher`**: A server application that runs the matching engine for one or more specific markets.
        *   **`settlement-plane`**: A server application that runs the background settlement and checkpointing process.
        *   **`verifier`**: A standalone application for independent auditing.
        *   **`configuration`**: A shared library for loading system settings from `config.toml`.
        *   **`benchmark-client`**: A command-line tool for measuring end-to-end latency.
    *   `config.toml`: The main configuration file for the system.
    *   `GCP_DEPLOYMENT.md`: A guide for deploying the system to production on Google Cloud.

## Deployment Guide (Local Simulation)

To run the entire system locally, you will need a running Redis server and four separate terminal windows.

### 1. Prerequisites

*   Install Rust and Cargo.
*   Install and run Redis on the default port (`redis-server`).

### 2. Configuration

Review the `config.toml` file. The default settings should work for a local run.

### 3. Terminal 1: Start the Ingress Verifier

This server handles all incoming client traffic.

```bash
cd /path/to/clob
cargo run --release --bin ingress-verifier
```

### 4. Terminal 2: Start the Market Matcher

This server runs the matching engine for one or more markets. The market IDs are passed as command-line arguments.

```bash
# Start a matcher for Market ID 1
cd /path/to/clob
cargo run --release --bin market-matcher 1
```

### 5. Terminal 3: Start the Settlement Plane

This server runs the background checkpointing process.

```bash
cd /path/to/clob
cargo run --release --bin settlement-plane
```

### 6. Terminal 4: Interact and Verify

This terminal is used to act as a user and to run the verifier.

**A. Create a New User:**

```bash
curl -X POST http://127.0.0.1:9090/users
```
This will return a JSON response with the new `user_id` and `private_key_hex`. **Copy the private key.**

**B. Deposit Funds:**

```bash
curl -X POST http://127.0.0.1:9090/deposit \
-H "Content-Type: application/json" \
-d '{ 
    "user_id": { "0": NEW_USER_ID },
    "asset_id": { "0": 1 },
    "amount": "10000.0"
}'
```
*(Replace `NEW_USER_ID` with the user ID from the previous step.)*

**C. Submit a Trade:**

Use the `benchmark-client` to submit a signed order.

```bash
# Replace the hex string with the private key you copied
cargo run --release --bin benchmark-client PASTE_PRIVATE_KEY_HEX_HERE
```

**D. Verify the System:**

After the `settlement-plane` has created a checkpoint, run the verifier.

```bash
cargo run --release --bin verifier
```
The verifier will read the log from Redis, replay the history, and print its result.

## Automated Integration Test

The entire multi-service deployment can be tested automatically with a single command. This test will start a temporary Redis server, run all the services, execute a transaction, and verify the result.

```bash
cd /path/to/clob
cargo test --workspace --test integration_test
```

