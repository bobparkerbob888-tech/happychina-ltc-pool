# ============================================================
# HappyChina LTC Merge Mining Pool - Multi-stage build
# ============================================================

# Stage 1: Build the Rust binary
FROM rust:1.77-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy dependency manifests first for better layer caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to pre-build dependencies
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src

# Copy actual source code
COPY src/ src/

# Touch main.rs to ensure it gets recompiled (not the dummy)
RUN touch src/main.rs

# Build the real binary
RUN cargo build --release

# Stage 2: Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the compiled binary
COPY --from=builder /build/target/release/happychain-pool /usr/local/bin/happychain-pool

# Copy migrations
COPY migrations/ /opt/happychain-pool/migrations/

# Copy web frontend
COPY web/ /opt/happychain-pool/web/

# Config will be mounted as a volume
RUN mkdir -p /etc/happychain-pool

WORKDIR /opt/happychain-pool

EXPOSE 8090 3332 3333 3334 3335

CMD ["happychain-pool", "--config", "/etc/happychain-pool/config.toml"]
