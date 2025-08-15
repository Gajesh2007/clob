# Stage 1: Builder
# Compiles the ingress-verifier binary.
FROM rust:1-slim-bookworm as builder

WORKDIR /usr/src/clob

# Copy the entire workspace to ensure all local dependencies are available.
COPY . .

# Build only the ingress-verifier binary in release mode.
RUN cargo build --release --bin ingress-verifier


# Stage 2: Final Image
# Creates the minimal runtime image.
FROM debian:bookworm-slim

WORKDIR /app

# Copy the compiled binary from the builder stage.
COPY --from=builder /usr/src/clob/target/release/ingress-verifier /usr/local/bin/

# Copy the configuration file.
COPY config.toml .

# Expose the TCP and HTTP ports.
EXPOSE 8080 9090

# Set the entrypoint.
ENV RUST_LOG=info
CMD ["ingress-verifier"]
