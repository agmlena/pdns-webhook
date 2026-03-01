# ── Build stage ────────────────────────────────────────────────────────────────
FROM rust:1.78-slim AS builder

WORKDIR /build

# Cache dependency compilation – only reruns when Cargo.toml/Cargo.lock change
RUN mkdir src && echo "fn main(){}" > src/main.rs
RUN cargo build --release --offline 2>/dev/null; rm src/main.rs

# Build the real binary
COPY src ./src
RUN touch src/main.rs && cargo build --release --offline

# ── Runtime stage ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as non-root
RUN useradd --system --no-create-home --uid 1000 webhook
USER webhook

COPY --from=builder /build/target/release/webhook /usr/local/bin/webhook

# ── Non-sensitive defaults ─────────────────────────────────────────────────────
# PDNS_API_KEY is intentionally absent – supply it via a mounted secret file.
ENV PDNS_API_URL=http://powerdns:8081 \
    PDNS_SERVER_ID=localhost \
    DOMAIN_FILTER="" \
    DEFAULT_TTL=300 \
    PORT=8888 \
    PDNS_API_KEY_FILE=/var/run/secrets/pdns/api-key

RUN mkdir -p /var/run/secrets/pdns

EXPOSE 8888

# ── How to supply the secret ───────────────────────────────────────────────────
# Kubernetes (recommended): mount a Secret volume – see values.yaml.
#
# Docker Swarm:
#   docker secret create pdns_api_key ./api-key.txt
#
# Plain Docker (local dev):
#   echo -n "my-api-key" > /tmp/pdns-api-key
#   docker run --rm \
#     -v /tmp/pdns-api-key:/var/run/secrets/pdns/api-key:ro \
#     -e PDNS_API_URL=http://host.docker.internal:8081 \
#     -p 8888:8888 \
#     ghcr.io/your-org/external-dns-pdns-webhook-rs:latest

ENTRYPOINT ["/usr/local/bin/webhook"]
