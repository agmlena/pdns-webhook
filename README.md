# external-dns PowerDNS HTTPS Webhook (Rust)

A zero-overhead **external-dns webhook provider** written in Rust (Axum + Tokio).
Creates, updates, and deletes DNS records — including `HTTPS` (RFC 9460 SVCB)
records — in [PowerDNS](https://www.powerdns.com/) via its REST API.

## Project layout

```
src/
  main.rs      – Tokio entry point, Axum router, shared AppState
  config.rs    – Typed env-var config via `envy`
  dns.rs       – external-dns webhook data models (Endpoint, Changes, …)
  pdns.rs      – Async PowerDNS API client (reqwest)
  handlers.rs  – Axum route handlers
Cargo.toml
Dockerfile
values.yaml    – Helm sidecar config for external-dns
```

## Webhook endpoints

| Method | Path | Purpose |
|--------|------|---------|
| `GET`  | `/`                  | Domain-filter negotiation |
| `GET`  | `/healthz`           | Liveness / readiness |
| `GET`  | `/records`           | List all managed records |
| `POST` | `/records`           | Apply creates / updates / deletes |
| `POST` | `/adjustendpoints`   | Normalise HTTPS targets |

## HTTPS record format (RFC 9460)

Targets must be in SvcParam wire-text form: `<priority> <target> [key=val …]`

The `/adjustendpoints` handler automatically wraps bare hostnames:

```
app.example.com  →  1 app.example.com.
```

Annotate an Ingress:

```yaml
metadata:
  annotations:
    external-dns.alpha.kubernetes.io/hostname: "app.example.com"
    external-dns.alpha.kubernetes.io/target: "1 . alpn=h2,h3"
```

## Configuration (environment variables)

| Variable         | Default                    | Description |
|------------------|----------------------------|-------------|
| `PDNS_API_URL`   | `http://localhost:8081`    | PowerDNS API base URL |
| `PDNS_API_KEY`   | `changeme`                 | PowerDNS `api-key` |
| `PDNS_SERVER_ID` | `localhost`                | PowerDNS server ID |
| `DOMAIN_FILTER`  | *(all zones)*              | Comma-separated zone list |
| `DEFAULT_TTL`    | `300`                      | TTL fallback |
| `PORT`           | `8888`                     | Listen port |
| `RUST_LOG`       | `…=info`                   | Log filter |

## Build & run

```bash
# Local
cargo build --release
PDNS_API_URL=http://localhost:8081 PDNS_API_KEY=secret \
  ./target/release/webhook

# Docker
docker build -t pdns-webhook-rs .
docker run -p 8888:8888 \
  -e PDNS_API_URL=http://host.docker.internal:8081 \
  -e PDNS_API_KEY=secret \
  pdns-webhook-rs
```

## Kubernetes (Helm)

```bash
kubectl create secret generic powerdns-api-key --from-literal=api-key=<KEY>
helm upgrade --install external-dns external-dns/external-dns -f values.yaml
```

## PowerDNS prerequisites

Enable the HTTP API in `pdns.conf`:

```ini
api=yes
api-key=changeme
webserver=yes
webserver-address=0.0.0.0
webserver-port=8081
webserver-allow-from=0.0.0.0/0
```

Zones must already exist in PowerDNS; the webhook walks up the DNS tree
to find the best-matching zone for each name.
