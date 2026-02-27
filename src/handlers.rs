use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Response},
    Json as BodyJson,
};
use tracing::{error, info};

use crate::{
    dns::{Changes, DomainFilter, Endpoint},
    AppState,
};

// Content-Type required by the external-dns webhook spec
const WEBHOOK_CT: &str = "application/external.dns.webhook+json;version=1";

fn webhook_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        "Content-Type",
        HeaderValue::from_static(WEBHOOK_CT),
    );
    h
}

// ── GET /healthz ─────────────────────────────────────────────────────────────

pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status":"ok"})))
}

// ── GET / ────────────────────────────────────────────────────────────────────
/// Domain-filter negotiation.

pub async fn negotiate(State(state): State<AppState>) -> impl IntoResponse {
    let filter = DomainFilter {
        include: state.cfg.domain_filter_list(),
        exclude: vec![],
    };
    (webhook_headers(), Json(filter))
}

// ── GET /records ─────────────────────────────────────────────────────────────

pub async fn get_records(State(state): State<AppState>) -> Response {
    let domain_filter = state.cfg.domain_filter_list();
    match state.pdns.list_endpoints(&domain_filter).await {
        Ok(eps) => {
            info!("GET /records → {} endpoint(s)", eps.len());
            (webhook_headers(), Json(eps)).into_response()
        }
        Err(e) => {
            error!("GET /records error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}

// ── POST /records ─────────────────────────────────────────────────────────────

pub async fn apply_changes(
    State(state): State<AppState>,
    BodyJson(changes): BodyJson<Changes>,
) -> Response {
    let ttl = state.cfg.default_ttl;

    // Order: deletes → update-old → update-new → creates
    for ep in &changes.delete {
        info!("DELETE {} {}", ep.record_type, ep.dns_name);
        if let Err(e) = state.pdns.delete(ep).await {
            error!("delete {}: {e}", ep.dns_name);
            return error_response(502, e.to_string());
        }
    }

    for ep in &changes.update_old {
        info!("UPDATE-OLD {} {}", ep.record_type, ep.dns_name);
        if let Err(e) = state.pdns.delete(ep).await {
            error!("update_old delete {}: {e}", ep.dns_name);
            return error_response(502, e.to_string());
        }
    }

    for ep in &changes.update_new {
        info!("UPDATE-NEW {} {}", ep.record_type, ep.dns_name);
        if let Err(e) = state.pdns.upsert(ep, ttl).await {
            error!("update_new upsert {}: {e}", ep.dns_name);
            return error_response(502, e.to_string());
        }
    }

    for ep in &changes.create {
        info!("CREATE {} {}", ep.record_type, ep.dns_name);
        if let Err(e) = state.pdns.upsert(ep, ttl).await {
            error!("create {}: {e}", ep.dns_name);
            return error_response(502, e.to_string());
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

// ── POST /adjustendpoints ────────────────────────────────────────────────────
/// Normalises HTTPS targets so they contain a SvcPriority prefix.

pub async fn adjust_endpoints(
    BodyJson(mut endpoints): BodyJson<Vec<Endpoint>>,
) -> impl IntoResponse {
    for ep in &mut endpoints {
        if ep.record_type == "HTTPS" {
            ep.targets = ep
                .targets
                .iter()
                .map(|t| {
                    if t.contains(' ') {
                        t.clone()
                    } else {
                        let host = t.trim_end_matches('.');
                        format!("1 {host}.")
                    }
                })
                .collect();
        }
    }
    (webhook_headers(), Json(endpoints))
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn error_response(code: u16, msg: String) -> Response {
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(serde_json::json!({"error": msg})),
    )
        .into_response()
}
