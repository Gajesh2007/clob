# Stage 1: Builder
# This stage compiles the Rust application in a full Rust development environment.
FROM rust:1-slim-bookworm as builder

# Set the working directory
WORKDIR /usr/src/clob

# Copy the entire project context
COPY . .

# Install build essentials needed for some dependencies
RUN apt-get update && apt-get install -y build-essential

# Use a build argument to specify which binary to compile
ARG BINARY_NAME
# Build the specified binary in release mode for performance
RUN cargo build --release --bin ${BINARY_NAME}


# Stage 2: Final Image
# This stage creates the final, minimal image for runtime.
FROM debian:bookworm-slim as final

# Set the working directory
WORKDIR /app

# Copy the compiled binary from the builder stage
COPY --from=builder /usr/src/clob/target/release/${BINARY_NAME} /usr/local/bin/

# Copy the configuration file
COPY config.toml .

# Set the entrypoint for the container to run the binary
CMD ["/usr/local/bin/${BINARY_NAME}"]
