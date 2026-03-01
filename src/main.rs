//! Binary entry point.
//! All modules live in lib.rs (crate `pdns_webhook`).
//! This file only wires them together into a running server.

use pdns_webhook::{
    config::Config,
    handlers,
    pdns::PdnsClient,
    AppState,
};

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

// ─────────────────────────────────────────────────────────────────────────────
// Request body logging middleware
// ─────────────────────────────────────────────────────────────────────────────

async fn log_request_body(req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();

    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
            return next.run(Request::from_parts(parts, Body::empty())).await;
        }
    };

    if tracing::enabled!(tracing::Level::DEBUG) {
        let body_str = std::str::from_utf8(&bytes)
            .map(|s| {
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

    next.run(Request::from_parts(parts, Body::from(bytes))).await
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
            let default = "pdns_webhook=debug,tower_http=debug";
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
        .layer(middleware::from_fn(log_request_body))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
