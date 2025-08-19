# syntax=docker/dockerfile:1

FROM rust:1.78 as builder
WORKDIR /app/clob
COPY clob /app/clob
RUN cargo build -p market-matcher-gateway --release

FROM debian:bookworm-slim as runtime
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/clob/target/release/market-matcher-gateway /usr/local/bin/market-matcher-gateway
ENV RUST_LOG=info
EXPOSE 9100
ENTRYPOINT ["/usr/local/bin/market-matcher-gateway"]

