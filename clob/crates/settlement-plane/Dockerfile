# Stage 1: Builder
# Compiles the settlement-plane binary.
FROM rust:slim-bookworm AS builder

WORKDIR /usr/src/clob

# Copy the entire workspace to ensure all local dependencies are available.
COPY . .

# Build only the settlement-plane binary in release mode.
RUN cargo build --release --bin settlement-plane


# Stage 2: Final Image
# Creates the minimal runtime image.
FROM debian:bookworm-slim

WORKDIR /app

# Copy the compiled binary from the builder stage.
COPY --from=builder /usr/src/clob/target/release/settlement-plane /usr/local/bin/

# Copy the configuration file.
COPY config.toml .

# Set the entrypoint.
CMD ["settlement-plane"]
