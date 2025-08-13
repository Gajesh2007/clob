# NASDAQ-CLOB: A High-Performance, Verifiable Trading System

This project is a Rust implementation of a high-performance Central Limit Order Book (CLOB) exchange, designed for ultra-low latency execution and verifiable settlement, based on the architecture described in the `PRODUCT.md` document.

The system is architected with a strict separation between the real-time "hot path" for trading and the asynchronous "warm path" for settlement and verification.

---

## Components

This project is structured as a Cargo workspace with several distinct crates:

*   **`crates/common-types`**: A shared library containing all the core data structures (`Order`, `Trade`, `Account`, `StateSnapshot`, etc.) used across the entire system.
*   **`crates/matching-engine`**: The heart of the exchange. A pure, high-performance library that implements the order book logic, trade execution, and event generation.
*   **`crates/execution-plane`**: The main server application (the "Sequencer"). This is the "hot path" that traders interact with. It handles TCP connections, validates signatures, performs pre-trade risk checks, logs the canonical `Order` history, and serves state snapshots over HTTP.
*   **`crates/settlement-plane`**: The asynchronous "warm path" application. It reads the execution log, submits transaction data to a Data Availability (DA) layer, fetches state snapshots from the execution plane, calculates the Merkle root of the system state, and submits verifiable checkpoints to a simulated L1.
*   **`crates/verifier`**: A completely independent application that acts as a third-party auditor. It downloads the execution log, replays all transactions locally to reconstruct the state, and cryptographically verifies that the state root published by the settlement plane is correct.
*   **`crates/benchmark-client`**: A utility for measuring the end-to-end latency of the system's hot path.

---

## How to Run the System

You will need three separate terminal sessions to run the full system.

### 1. Start the Execution Plane

This is the main server. It will generate a demo user's private key on startup, which you will need for the client.

```bash
# In terminal 1
cd /Users/gaj/Documents/Fun/NASDAQ/clob
cargo run --release -p execution-plane
```
*(Note: The server will print the private key. Copy this key for the next step.)*

### 2. Start the Settlement Plane

This process reads the log and creates the checkpoints.

```bash
# In terminal 2
cd /Users/gaj/Documents/Fun/NASDAQ/clob
cargo run --release -p settlement-plane
```

### 3. (Optional) Generate Activity with a Client

To generate trades, you can use the `benchmark-client`. This requires the private key printed by the `execution-plane` in step 1.

```bash
# In terminal 3
# Replace <PRIVATE_KEY_HEX> with the key from the server's output
cd /Users/gaj/Documents/Fun/NASDAQ/clob
cargo run --release -p benchmark-client -- <PRIVATE_KEY_HEX>
```

---

## How to Verify the System

After the system has been running and has processed some transactions, you can run the verifier. The verifier will read the `execution.log` and the `checkpoint.json` file created by the settlement plane and check if the state is valid.

```bash
# In a new terminal
cd /Users/gaj/Documents/Fun/NASDAQ/clob
cargo run --release -p verifier
```

The verifier will print a `✅ SUCCESS` or `❌ FAILURE` message.
