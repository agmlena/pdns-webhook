# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.78-slim AS builder

WORKDIR /build

# Cache dependency compilation
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs
RUN cargo build --release 2>/dev/null; rm src/main.rs

# Build the real binary
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/webhook /usr/local/bin/webhook

ENV PDNS_API_URL=http://powerdns:8081 \
    PDNS_API_KEY=changeme \
    PDNS_SERVER_ID=localhost \
    DOMAIN_FILTER="" \
    DEFAULT_TTL=300 \
    PORT=8888

EXPOSE 8888

ENTRYPOINT ["/usr/local/bin/webhook"]
