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
    // Initialise tracing – respects RUST_LOG env var
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("external_dns_pdns_webhook=info,tower_http=debug")
        }))
        .with(tracing_subscriber::fmt::layer())
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
        // external-dns webhook contract
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
