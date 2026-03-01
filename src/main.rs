mod config;
mod dns;
mod handlers;
mod pdns;

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::Request,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use tower_http::trace::TraceLayer;
use tracing::{debug, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::{config::Config, pdns::PdnsClient};

// ─────────────────────────────────────────────────────────────────────────────
// Shared application state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub cfg: Config,
    pub pdns: PdnsClient,
}

// ─────────────────────────────────────────────────────────────────────────────
// Request body logging middleware
//
// Only active at DEBUG level or below. Reads the full body into memory,
// logs it, then puts it back so the actual handler can still deserialise it.
// ─────────────────────────────────────────────────────────────────────────────

async fn log_request_body(req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();

    // Collect the body bytes
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
            return next.run(Request::from_parts(parts, Body::empty())).await;
        }
    };

    // Log method, path and body at DEBUG level
    if tracing::enabled!(tracing::Level::DEBUG) {
        let body_str = std::str::from_utf8(&bytes)
            .map(|s| {
                // Pretty-print if it's valid JSON, otherwise show raw
                serde_json::from_str::<serde_json::Value>(s)
                    .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.to_string()))
                    .unwrap_or_else(|_| s.to_string())
            })
            .unwrap_or_else(|_| format!("<{} binary bytes>", bytes.len()));

        debug!(
            method = %parts.method,
            path   = %parts.uri.path(),
            body   = %body_str,
            "← request body"
        );
    }

    // Reconstruct the request with the original bytes and pass it on
    let req = Request::from_parts(parts, Body::from(bytes));
    next.run(req).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = match EnvFilter::try_from_default_env() {
        Ok(f) => {
            eprintln!(
                "[tracing] using RUST_LOG={}",
                std::env::var("RUST_LOG").unwrap_or_default()
            );
            f
        }
        Err(e) => {
            let default = "external_dns_pdns_webhook=debug,tower_http=debug";
            eprintln!("[tracing] RUST_LOG not set or invalid ({e}), defaulting to: {default}");
            EnvFilter::new(default)
        }
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                .with_thread_ids(false)
                .with_ansi(true),
        )
        .init();

    let cfg = Config::from_env()?;
    let port = cfg.port;

    info!("PowerDNS API : {}", cfg.pdns_api_url);
    info!("Server ID    : {}", cfg.pdns_server_id);
    info!(
        "Domain filter: {}",
        if cfg.domain_filter.is_empty() { "(all zones)" } else { &cfg.domain_filter }
    );
    info!("Default TTL  : {}s", cfg.default_ttl);

    let pdns = PdnsClient::new(cfg.clone())?;
    let state = AppState { cfg, pdns };

    let app = Router::new()
        .route("/",                get(handlers::negotiate))
        .route("/healthz",         get(handlers::healthz))
        .route("/records",         get(handlers::get_records))
        .route("/records",         post(handlers::apply_changes))
        .route("/adjustendpoints", post(handlers::adjust_endpoints))
        // log_request_body runs before handlers; only logs at DEBUG level
        .layer(middleware::from_fn(log_request_body))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
