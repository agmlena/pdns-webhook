mod config;
mod dns;
mod handlers;
mod pdns;

use std::net::SocketAddr;

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::{config::Config, pdns::PdnsClient};

// ─────────────────────────────────────────────────────────────────────────────
// Shared application state (Arc-wrapped by Axum automatically via Clone)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub cfg: Config,
    pub pdns: PdnsClient,
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Build the env filter from RUST_LOG, warn loudly if it's invalid or absent
    // so the developer knows exactly why they aren't seeing logs.
    // Default: debug for our code, warn for noisy dependencies.
    let filter = match EnvFilter::try_from_default_env() {
        Ok(f) => {
            eprintln!("[tracing] using RUST_LOG={}", std::env::var("RUST_LOG").unwrap_or_default());
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
                .with_target(true)       // shows module path: external_dns_pdns_webhook::pdns
                .with_file(true)         // shows source file: src/pdns.rs
                .with_line_number(true)  // shows line number: :85
                .with_thread_ids(false)
                .with_ansi(true),        // coloured output in terminals
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
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
