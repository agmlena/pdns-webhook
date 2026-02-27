# ── Build stage ────────────────────────────────────────────────────────────────
FROM rust:1.93.1 AS builder

WORKDIR /build

# Cache dependency compilation – only re-runs when Cargo.toml/Cargo.lock change
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs
RUN cargo build --release 2>/dev/null; rm src/main.rs

# Build the real binary
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ── Runtime stage ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt update && apt install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as non-root
RUN useradd --system --no-create-home --uid 1000 webhook

# Create the secret mount-point directory so Docker volume mounts work cleanly
# even if the orchestrator doesn't pre-create the path.
# The directory is owned by the webhook user so no root access is needed.
RUN mkdir -p /var/run/secrets/pdns && chown webhook /var/run/secrets/pdns

USER webhook

COPY --from=builder /build/target/release/webhook /usr/local/bin/webhook

# ── Non-sensitive defaults ─────────────────────────────────────────────────────
# PDNS_API_KEY is intentionally absent – supply it via a mounted secret file.
ENV PDNS_API_URL=http://powerdns:8081 \
    PDNS_SERVER_ID=localhost \
    DOMAIN_FILTER="" \
    DEFAULT_TTL=300 \
    PORT=8888 \
    # Where the app looks for the API key (matches config.rs default).
    # Override this if you mount your secret at a different path.
    PDNS_API_KEY_FILE=/var/run/secrets/pdns/api-key

EXPOSE 8888

# ── How to supply the secret ───────────────────────────────────────────────────
# Kubernetes (recommended): mount a Secret volume – see values.yaml.
#
# Docker Swarm:
#   docker secret create pdns_api_key ./api-key.txt
#   # In your compose/stack file:
#   #   secrets:
#   #     pdns_api_key:
#   #       external: true
#   #   services:
#   #     webhook:
#   #       secrets:
#   #         - source: pdns_api_key
#   #           target: /var/run/secrets/pdns/api-key
#   #           mode: 0400
#
# Plain Docker (local dev / testing):
#   echo -n "my-api-key" > /tmp/pdns-api-key
#   docker run --rm \
#     -v /tmp/pdns-api-key:/var/run/secrets/pdns/api-key:ro \
#     -e PDNS_API_URL=http://host.docker.internal:8081 \
#     -p 8888:8888 \
#     ghcr.io/your-org/external-dns-pdns-webhook-rs:latest

ENTRYPOINT ["/usr/local/bin/webhook"]
