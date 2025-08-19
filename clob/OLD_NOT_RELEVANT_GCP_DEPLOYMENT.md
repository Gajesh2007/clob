# GCP Production Deployment Guide (1M TPS Architecture)

This document provides the definitive strategy for deploying the CLOB system on Google Cloud Platform to achieve a target of **1 million transactions per second** across **21,000 markets**.

This guide reflects the final, distributed, multi-service architecture.

## Architecture for 1M TPS

To achieve this level of performance, we must deploy our specialized services (`ingress-verifier`, `market-matcher`) as large, parallel fleets on hardware optimized for each task. The core principle is to eliminate shared resources and contention wherever possible.

### GCP Services Mapping

| Service | GCP Production Equivalent | Purpose & Rationale |
| :--- | :--- | :--- |
| **Load Balancer** | **Regional Internal TCP Load Balancer** | Managed, high-performance network load balancing. The single entry point for all traders. |
| `ingress-verifier` | **Fleet of Compute Engine C3D** | Brute-force signature verification. C3D instances provide the maximum number of cores for parallel processing. |
| **Message Bus** | **Self-Hosted Redis on a dedicated C3D** | **Lowest Latency.** A managed service is too slow. A dedicated instance in a compact placement policy provides microsecond-level latency for the message bus and execution log. |
| `market-matcher` | **Fleet of Compute Engine C3** | Ultra-low-latency, single-threaded matching. C3 instances provide the highest single-core clock speeds. |
| `settlement-plane` | **Compute Engine E2 Cluster** | Reliable, asynchronous state checkpointing. Does not need high-performance hardware. |

---

## Deployment Plan & Instructions

### 1. Network & Placement

1.  **Create a VPC:** Provision a dedicated **VPC (Virtual Private Cloud)**.
2.  **Create a Compact Placement Policy:** In the target region, create a compact placement policy. **All C3 and C3D instances for this system must be launched within this policy.** This is critical for ensuring minimal network latency between services.
3.  **Firewall Rules:** Create firewall rules to:
    *   Allow TCP traffic on port `8080` from your trader/benchmark client IP ranges.
    *   Allow TCP traffic on port `9090` from your admin IP ranges.
    *   Allow all internal TCP and UDP traffic within the VPC.

### 2. Provision the Message Bus

1.  **Create a Dedicated Redis Instance:**
    *   Launch a single **`c3d-standard-30`** (or larger) instance within your compact placement policy.
    *   Install and configure a high-performance Redis server on this instance. This will be your message bus.
    *   Note its internal IP address.

### 3. Provision the Compute Fleets

**A. Ingress Verifier Fleet:**

1.  **Create an Instance Template:**
    *   **Machine Type:** `c3d-standard-180`.
    *   **Container Image:** The Docker image for the `ingress-verifier` binary.
    *   **Environment Variable:** Set `REDIS_ADDR` to the internal IP of your Redis instance (e.g., `redis://10.0.0.5/`).
2.  **Create a Managed Instance Group (MIG):**
    *   Create a MIG from this template with **5-10 instances**.
    *   Configure the **Regional Internal TCP Load Balancer** to use this MIG as its backend for port `8080`.

**B. Market Matcher Fleet:**

1.  **Create an Instance Template:**
    *   **Machine Type:** `c3-highcpu-8` (8 vCPUs, 16 GB RAM).
    *   **Container Image:** The Docker image for the `market-matcher` binary.
    *   **Environment Variable:** Set `REDIS_ADDR` to the internal IP of your Redis instance.
2.  **Create a Managed Instance Group (MIG):**
    *   Create a MIG from this template with **10-15 instances**.
    *   **Startup Script:** This is the key to sharding. The startup script on each instance must determine which markets it is responsible for and launch the `market-matcher` processes accordingly, using `taskset` to pin each process to a specific core.
    *   *Example Startup Script Logic:*
        ```bash
        #!/bin/bash
        # This is a simplified example. A real script would get its shard
        # assignment from instance metadata.
        
        # On machine #1:
        taskset -c 1 ./market-matcher 1 2 3 ... 2100 &
        taskset -c 2 ./market-matcher 2101 2102 ... 4200 &
        # ... and so on for all cores.
        ```

**C. Settlement Plane Cluster:**

*   Deploy the `settlement-plane` binary to a small cluster (2 instances) of `e2-standard-4` machines. They do not need to be in the compact placement policy.

### 4. Benchmarking the Deployed System

Once all services are running, you can measure the true end-to-end performance.

1.  **Provision a Benchmark Client Instance:**
    *   Launch a **`c3d-standard-8`** instance in the **same compact placement policy**.
    *   SSH into the instance and clone the project.
    *   Build the `benchmark-client`: `cargo build --release --bin benchmark-client`.

2.  **Run the Test:**
    *   **Create a User:** From the benchmark instance, call the HTTP API of the `ingress-verifier`'s load balancer to create a user and get a private key.
        ```bash
        curl -X POST http://<LOAD_BALANCER_IP>:9090/users
        ```
    *   **Run the Client:** Execute the benchmark client, passing it the private key.
        ```bash
        ./target/release/benchmark-client <PASTE_PRIVATE_KEY_HERE>
        ```
    *   The client will connect to the load balancer, send a transaction, and print the **true, end-to-end latency** of your production-grade system.
