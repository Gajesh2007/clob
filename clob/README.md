# High-Performance Verifiable Central Limit Order Book (CLOB)

This project is a Rust implementation of the high-performance, verifiably-settled Central Limit Order Book described in `PRODUCT.md`. It simulates a complete, end-to-end financial exchange system, from user creation and deposits to trade execution and independent verification.

## Architecture Overview

The system is a distributed application composed of several distinct services that communicate via network protocols. It is split into two primary planes:

*   **The Real-Time Execution Plane (The "Hot Path"):** Engineered for absolute minimum latency. It handles user interaction, order validation, risk checks, and trade matching. All critical operations occur in-memory for performance.
*   **The Asynchronous Settlement Plane (The "Warm Path"):** Runs in the background to provide verifiability. It consumes data from the execution plane, batches it for a Data Availability (DA) layer, calculates the true state of the system, and commits a cryptographic proof (a Merkle Root) to a public ledger.

## Codebase Structure

The project is organized as a Cargo workspace with several distinct crates, each with a specific responsibility:

*   `clob/`
    *   `crates/`
        *   **`common-types`**: A shared library defining all the core data structures (`Order`, `Trade`, `Account`, `StateSnapshot`, etc.) used across the entire system.
        *   **`matching-engine`**: The heart of the system. A pure, high-performance library that implements the CLOB logic. It is fully unit-tested and benchmarked.
        *   **`execution-plane`**: The main server application. It runs the TCP server for orders, the HTTP server for user management and state snapshots, and the core "sequencer" logic that validates and logs all transactions.
        *   **`settlement-plane`**: A separate server application that runs the background checkpointing process, fetching state, calculating Merkle roots, and creating checkpoints.
        *   **`verifier`**: A standalone application that acts as an independent auditor. It replays the public transaction log to verify the integrity of the checkpoints published by the settlement plane.
        *   **`configuration`**: A shared library that defines and loads all system settings from a `config.toml` file.
        *   **`benchmark-client`**: A command-line tool for measuring the end-to-end latency of the system.
    *   `config.toml`: The main configuration file for the system.

## Hardware Requirements

The different components of the system have vastly different hardware needs.

*   **Execution Plane:** This is the most performance-critical component. For a production deployment, it should run on a "beast of a machine" with a focus on single-core speed and low-latency I/O.
    *   **CPU:** A processor with the highest possible single-core clock speed and a large L3 cache (e.g., an Intel Xeon "Max" Series or AMD EPYC with 3D V-Cache).
    *   **RAM:** Terabytes of the fastest available DRAM.
    *   **Storage:** High-end NVMe drives for the execution log.
    *   **Network:** Enterprise-grade, low-latency network switches (e.g., Arista, Mellanox) with fiber optic connections.

*   **Settlement Plane:** This component is less latency-sensitive but needs to handle significant I/O.
    *   **CPU/RAM:** A standard, good-quality cloud VM or physical server is sufficient.
    *   **Network:** A strong, reliable network connection with good bandwidth is essential for fetching state snapshots and submitting data to the DA layer.

*   **Verifier:** The requirements for the verifier are flexible, as it is run by third parties and is not latency-critical.
    *   **CPU/RAM:** A standard developer machine or a mid-tier cloud VM with enough RAM to hold the entire state of the system in memory is sufficient.

## Deployment Guide

To run the entire system locally, you will need three separate terminal windows and a running EigenDA proxy (or a simple web server that can accept POST requests to simulate it).

### 1. Prerequisites

*   Install Rust and Cargo.
*   Ensure an EigenDA proxy or a mock server is running and accessible at the URL specified in `config.toml`. A simple mock can be created with `python3 -m http.server 3100`.

### 2. Configuration

Review the `config.toml` file in the project root. The default settings should work for a local run.

### 3. Terminal 1: Start the Execution Plane

This is the core server. It must be started first.

```bash
cd /path/to/clob
cargo run --release --bin execution-plane
```

The server will start and print logs to the console. It is now listening for client connections and for requests on its HTTP API.

### 4. Terminal 2: Start the Settlement Plane

This server runs the background checkpointing process.

```bash
cd /path/to/clob
cargo run --release --bin settlement-plane
```

This server will start and, every 5 seconds (by default), it will attempt to fetch the state from the execution plane and create a new checkpoint.

### 5. Terminal 3: Interact with the System

This terminal is used to act as a user.

**A. Create a New User:**

```bash
curl -X POST http://127.0.0.1:9090/users
```

This will return a JSON response containing the new `user_id` and, for this simulation, the user's `private_key_hex`. **Copy the private key.**

**B. Deposit Funds for the New User:**

```bash
curl -X POST http://127.0.0.1:9090/deposit \
-H "Content-Type: application/json" \
-d 
{
    "user_id": { "0": NEW_USER_ID },
    "asset_id": { "0": 1 },
    "amount": "10000.0"
}
```

*(Replace `NEW_USER_ID` with the user ID from the previous step.)*

**C. Submit a Trade:**

Use the `benchmark-client` to submit a signed order.

```bash
# Replace the hex string with the private key you copied
cargo run --release --bin benchmark-client PASTE_PRIVATE_KEY_HEX_HERE
```

You will see logs in the **Execution Plane** terminal as the order is received, validated, and matched.

### 6. Verify the System

After the `settlement-plane` has had time to create at least one checkpoint (after a trade has been logged), you can run the verifier in Terminal 3.

```bash
cargo run --release --bin verifier
```

The verifier will read the `execution.log` and `checkpoint.json` files, replay the history, and print its result:
`✅ SUCCESS: Local state root matches the official checkpoint.`

## Benchmarking

The project includes two types of performance tests.

### Micro-Benchmarks (Core Engine)

These tests measure the performance of the `matching-engine` logic in isolation.

```bash
cd /path/to/clob
cargo bench -p matching-engine
```

This will run several scenarios (simple match, one-to-many match, deep book) and print detailed statistics for each.

### End-to-End Latency (Full System)

This test measures the total "hot path" latency, from a client sending a TCP packet to receiving the UDP multicast event.

1.  Start the `execution-plane` server (see Deployment Step 3).
2.  Run the `benchmark-client` with a valid private key (see Interaction Step C).
3.  The client will print the end-to-end latency upon completion.

```