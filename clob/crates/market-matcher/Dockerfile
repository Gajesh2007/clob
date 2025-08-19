# syntax=docker/dockerfile:1

# Build stage
FROM rust:slim-bookworm AS builder
WORKDIR /app/clob

# Copy the entire workspace; for better caching, consider a multi-step copy.
COPY clob /app/clob

# Build only the market-matcher binary in release mode
RUN cargo build -p market-matcher --release

# Runtime stage
FROM debian:bookworm-slim as runtime
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/clob/target/release/market-matcher /usr/local/bin/market-matcher

ENV RUST_LOG=info

# No ports to expose (uses Redis Pub/Sub only)
ENTRYPOINT ["/usr/local/bin/market-matcher"]
