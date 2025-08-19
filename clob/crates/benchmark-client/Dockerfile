# Stage 1: Builder
# Compiles the benchmark-client binary.
FROM rust:slim-bookworm AS builder

WORKDIR /usr/src/clob

# Copy the entire workspace to ensure all local dependencies are available.
COPY . .

# Build only the benchmark-client binary in release mode.
RUN cargo build --release --bin benchmark-client


# Stage 2: Final Image
# Creates the minimal runtime image.
FROM debian:bookworm-slim

WORKDIR /app

# Copy the compiled binary from the builder stage.
COPY --from=builder /usr/src/clob/target/release/benchmark-client /usr/local/bin/

# Set the entrypoint.
CMD ["benchmark-client"]
