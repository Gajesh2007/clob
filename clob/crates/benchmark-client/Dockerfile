# Stage 1: Builder
# Compiles the benchmark-client binary.
FROM rust:slim-bookworm AS builder

WORKDIR /app/clob

# Copy the workspace directory (contains Cargo.toml)
COPY clob /app/clob

# Minimal build dependencies (no OpenSSL needed when using rustls)
RUN apt-get update && \
    apt-get install -y pkg-config && \
    rm -rf /var/lib/apt/lists/*

# Build only the benchmark-client binary in release mode.
RUN cargo build --release --bin benchmark-client


# Stage 2: Final Image
# Creates the minimal runtime image.
FROM debian:bookworm-slim

WORKDIR /app

# No OpenSSL runtime needed; include CA certs for HTTPS
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Copy the compiled binary from the builder stage.
COPY --from=builder /app/clob/target/release/benchmark-client /usr/local/bin/

# Set entrypoint so additional docker run args are forwarded to the binary
ENTRYPOINT ["/usr/local/bin/benchmark-client"]
