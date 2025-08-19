# Stage 1: Builder
# Compiles the benchmark-client binary.
FROM rust:slim-bookworm AS builder

WORKDIR /app/clob

# Copy the workspace directory (contains Cargo.toml)
COPY clob /app/clob

# Install build dependencies for OpenSSL
RUN apt-get update && \
    apt-get install -y pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Build only the benchmark-client binary in release mode.
RUN cargo build --release --bin benchmark-client


# Stage 2: Final Image
# Creates the minimal runtime image.
FROM debian:bookworm-slim

WORKDIR /app

# Copy the compiled binary from the builder stage.
COPY --from=builder /app/clob/target/release/benchmark-client /usr/local/bin/

# Set the entrypoint.
CMD ["benchmark-client"]
