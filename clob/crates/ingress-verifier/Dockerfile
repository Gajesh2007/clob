# syntax=docker/dockerfile:1

# Build stage
FROM rust:1.78 as builder
WORKDIR /app/clob

# Copy the entire workspace (simple, reliable; acceptable for CI and local builds)
# If you prefer better caching, split Cargo.toml/Cargo.lock copies.
COPY clob /app/clob

# Build only the ingress-verifier binary in release mode
RUN cargo build -p ingress-verifier --release

# Runtime stage
FROM debian:bookworm-slim as runtime
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/clob/target/release/ingress-verifier /usr/local/bin/ingress-verifier

# Default logging level
ENV RUST_LOG=info

# Service listens on these (configurable via env)
EXPOSE 8080 9090

ENTRYPOINT ["/usr/local/bin/ingress-verifier"]
