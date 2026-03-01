use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Response},
    Json as BodyJson,
};
use tracing::{debug, error, info, warn};

use crate::dns::{Changes, DomainFilter, Endpoint};
use crate::AppState;

// Content-Type required by the external-dns webhook spec
const WEBHOOK_CT: &str = "application/external.dns.webhook+json;version=1";

// ── Annotation name ───────────────────────────────────────────────────────────
//
// Annotate a Service or IngressRoute with:
//
//   external-dns.alpha.kubernetes.io/provider-specific-webhook/pdns-https-target: "1 . alpn=h2,h3"
//
// external-dns places the key "webhook/pdns-https-target" into the endpoint's
// providerSpecific array, which this webhook receives in /adjustendpoints.
//
// The annotation value must be a valid HTTPS SvcParam string (RFC 9460):
//   <priority> <target> [key=value ...]
//
// Examples:
//   "1 . alpn=h2,h3"      – AliasMode, HTTP/2 + HTTP/3
//   "1 . alpn=h2"         – AliasMode, HTTP/2 only
//   "1 svc.example.com."  – ServiceMode with explicit target
const HTTPS_TARGET_ANNOTATION: &str = "webhook/pdns-https-target";

fn webhook_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("Content-Type", HeaderValue::from_static(WEBHOOK_CT));
    h
}

// ── GET /healthz ──────────────────────────────────────────────────────────────

pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

// ── GET / ─────────────────────────────────────────────────────────────────────

pub async fn negotiate(State(state): State<AppState>) -> impl IntoResponse {
    let filter = DomainFilter {
        include: state.cfg.domain_filter_list(),
        exclude: vec![],
    };
    (webhook_headers(), Json(filter))
}

// ── GET /records ──────────────────────────────────────────────────────────────

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

// ── POST /adjustendpoints ─────────────────────────────────────────────────────

pub async fn adjust_endpoints(
    BodyJson(mut endpoints): BodyJson<Vec<Endpoint>>,
) -> impl IntoResponse {
    let mut augmented_endpoints: Vec<Endpoint> = vec![];
    for ep in &mut endpoints {
        augmented_endpoints.push(ep.clone());

        let mut https_endpoint = ep.clone();
        if let Some(annotation_value) = find_provider_specific(&https_endpoint, HTTPS_TARGET_ANNOTATION) {
            let target = validate_https_target(&annotation_value, &https_endpoint.dns_name);
            info!(
                "HTTPS {} → target from annotation '{}': {}",
                https_endpoint.dns_name, HTTPS_TARGET_ANNOTATION, target
            );
            https_endpoint.targets = vec![target];
        }

        if https_endpoint.targets.is_empty() {
            warn!(
                "HTTPS {} has no targets and no '{}' annotation; skipping",
                https_endpoint.dns_name, HTTPS_TARGET_ANNOTATION
            );
            continue;
        }

        https_endpoint.record_type = "HTTPS".into();

        https_endpoint.targets = https_endpoint
            .targets
            .iter()
            .map(|t| normalise_https_target(t, &https_endpoint.dns_name))
            .collect();
        augmented_endpoints.push(https_endpoint.clone());

        debug!("HTTPS {} → normalised targets: {:?}", https_endpoint.dns_name, https_endpoint.targets);
    }

    (webhook_headers(), Json(augmented_endpoints))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn find_provider_specific(ep: &Endpoint, key: &str) -> Option<String> {
    ep.provider_specific
        .iter()
        .find(|p| p.name == key)
        .map(|p| p.value.clone())
}

fn validate_https_target(value: &str, dns_name: &str) -> String {
    let trimmed = value.trim();
    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();

    match parts.as_slice() {
        [priority, _target, ..] if priority.parse::<u16>().is_ok() => trimmed.to_string(),
        [_target, ..] => {
            warn!("HTTPS {} annotation missing SvcPriority; prepending '1'", dns_name);
            format!("1 {trimmed}")
        }
        _ => {
            warn!(
                "HTTPS {} annotation '{}' has unexpected format; wrapping as '1 . {}'",
                dns_name, value, trimmed
            );
            format!("1 . {trimmed}")
        }
    }
}

fn normalise_https_target(target: &str, dns_name: &str) -> String {
    let t = target.trim();
    let first = t.split_whitespace().next().unwrap_or("");
    if first.parse::<u16>().is_ok() {
        return t.to_string();
    }
    warn!("HTTPS {} target '{}' missing SvcPriority; prepending '1'", dns_name, target);
    format!("1 {t}")
}

fn error_response(code: u16, msg: String) -> Response {
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(serde_json::json!({"error": msg})),
    )
        .into_response()
}
