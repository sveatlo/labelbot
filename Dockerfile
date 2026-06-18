# syntax=docker/dockerfile:1
FROM rust:bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs \
    && cargo build --release \
    && rm -rf src target/release/labelbot* target/release/deps/labelbot*

COPY . .
RUN cargo build --release

# ── runtime ──────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/labelbot /usr/local/bin/labelbot

RUN mkdir -p /app/db

ENV LABELBOT_CONFIG=/app/config.toml

VOLUME ["/app/db"]

ENTRYPOINT ["labelbot"]
