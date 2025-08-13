# GCP Production Deployment Guide (Multi-Service Architecture)

This document provides a high-level strategy for deploying the high-performance CLOB system on Google Cloud Platform (GCP). This guide reflects the final, distributed architecture composed of specialized services.

## Architecture Overview

The system is deployed as a set of independent, scalable microservices that communicate via a high-performance message bus (Redis). This aligns with the `PRODUCT.md`'s description of a physically distributed pipeline.

*   **Ingress & Verification:** A fleet of servers whose only job is to handle client connections and verify signatures.
*   **Message Bus:** A managed Redis instance acts as the ultra-low-latency message bus for passing validated transactions. It also serves as the durable `execution_log`.
*   **Market Matching:** A sharded fleet of servers, where each server (or group of cores) is responsible for a specific set of markets.
*   **Settlement:** A reliable background service that consumes the log and state to produce checkpoints.

### GCP Services Mapping

| Service | GCP Production Equivalent | Purpose |
| :--- | :--- | :--- |
| `ingress-verifier` | **Compute Engine C3D Fleet** | High-throughput TCP termination & signature verification. |
| `market-matcher` | **Compute Engine C3 Fleet** | Ultra-low-latency, single-threaded matching per market. |
| `settlement-plane` | **Compute Engine E2 Cluster** | Reliable, asynchronous state checkpointing. |
| Message Bus / Log | **Memorystore for Redis** | High-performance Pub/Sub and durable transaction log. |
| Load Balancer | **Regional Internal TCP Load Balancer** | Managed, high-performance network load balancing. |

---

## Infrastructure & Deployment Plan

### 1. Network Setup

1.  **Create a VPC:** Provision a dedicated **VPC (Virtual Private Cloud)**.
2.  **Firewall Rules:** Create firewall rules to:
    *   Allow TCP traffic on port `8080` (for `ingress-verifier`) from trusted client IP ranges.
    *   Allow TCP traffic on port `9090` (for the `ingress-verifier` HTTP API) from trusted admin ranges.
    *   Allow all internal traffic within the VPC so the services can communicate with each other and with Redis.

### 2. Provision Managed Services

1.  **Create a Memorystore for Redis Instance:**
    *   Choose a sufficiently large instance to handle the message volume and store the entire execution log in memory.
    *   Place it in the same region as your Compute Engine instances.

### 3. Provision Compute Fleets

Create three distinct **Managed Instance Groups (MIGs)** using Instance Templates. All instances should be in the same region and zone, and ideally within a **Compact Placement Policy** to minimize network latency.

**A. Ingress Verifier Fleet:**

*   **Instance Template:**
    *   **Machine Type:** `c3d-standard-180` (CPU-heavy for signature verification).
    *   **Container Image:** The Docker image for the `ingress-verifier` binary.
*   **Managed Instance Group:**
    *   Create a MIG from this template. Start with 2-3 instances and configure autoscaling based on CPU utilization.
*   **Load Balancer:**
    *   Create a **Regional Internal TCP Load Balancer** and set this MIG as its backend. This provides a single, stable IP address for traders.

**B. Market Matcher Fleet:**

*   **Instance Template:**
    *   **Machine Type:** `c3-highcpu-8` (High clock speed for single-threaded performance).
    *   **Container Image:** The Docker image for the `market-matcher` binary.
*   **Managed Instance Group:**
    *   Create a MIG from this template. The number of instances will depend on how you want to shard your 21,000 markets. You could start with 10 instances, assigning ~2100 markets to each.
    *   **Startup Script:** The script for this instance will need to know which markets it is responsible for. This can be passed via instance metadata. The script would then launch the `market-matcher` binary with the correct market IDs as command-line arguments (e.g., `./market-matcher 1 2 3 ... 2100`).

**C. Settlement Plane Cluster:**

*   **Instance Template:**
    *   **Machine Type:** `e2-standard-4` (Cost-effective for this less-demanding workload).
    *   **Container Image:** The Docker image for the `settlement-plane` binary.
*   **Managed Instance Group:**
    *   Create a MIG from this template with a fixed size of 2 for high availability.

### 4. Configuration

*   Use a **startup script** in your instance templates to pull the latest `config.toml` from a central **GCS Bucket**.
*   The Redis connection string should be passed to the applications via an **environment variable** (`REDIS_ADDR`). This makes it easy to change without rebuilding the container.

### 5. Running the System

Once deployed, the system will be fully operational.
*   Traders connect to the TCP Load Balancer's IP address.
*   The `ingress-verifier` fleet handles the connections and publishes to Redis.
*   The `market-matcher` fleet subscribes to the Redis channels and processes trades.
*   The `settlement-plane` runs in the background, providing verifiability.
*   Independent **Verifiers** can be run from anywhere, connecting to your Redis instance (if you allow public access) or, more realistically, reading the execution log after it has been exported from Redis to a public data store.