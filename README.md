# NASDAQ-CLOB: A High-Performance, Verifiable Trading System

This repository contains the reference implementation of a high-performance Central Limit Order Book (CLOB) exchange, designed for ultra-low latency execution and verifiable settlement.

The full architectural vision, detailing the hybrid real-time and asynchronous settlement model, is described in the [**`PRODUCT.md`**](./PRODUCT.md) document. This document outlines the "hot path" for trading, the "warm path" for settlement, and the multi-level verification process.

## The Implementation

The practical implementation of this system is contained within the `clob/` directory, which is a self-contained Rust workspace. It is structured to mirror the architecture described in the product document, with separate crates for the matching engine, execution plane, settlement plane, and verifier.

For detailed technical information, build instructions, and steps to run the complete system, please refer to the [**`clob/README.md`**](./clob/README.md).
-
