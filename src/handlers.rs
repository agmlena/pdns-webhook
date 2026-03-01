use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Response},
    Json as BodyJson,
};
use tracing::{debug, error, info, warn};

use crate::{
    dns::{Changes, DomainFilter, Endpoint},
    AppState,
};

// Content-Type required by the external-dns webhook spec
const WEBHOOK_CT: &str = "application/external.dns.webhook+json;version=1";

// ── Annotation name ───────────────────────────────────────────────────────────
//
// Annotate an Ingress or Service with:
//
//   external-dns.alpha.kubernetes.io/provider-specific-pdns-https-target: "1 . alpn=h2,h3"
//
// external-dns strips the "external-dns.alpha.kubernetes.io/provider-specific-"
// prefix and places the remainder ("pdns-https-target") into the endpoint's
// providerSpecific array, which this webhook receives in /adjustendpoints.
//
// The annotation value must be a valid HTTPS SvcParam string (RFC 9460):
//   <priority> <target> [key=value ...]
//
// Examples:
//   "1 . alpn=h2,h3"           – AliasMode targeting the owner, HTTP/2 + HTTP/3
//   "1 . alpn=h2"              – AliasMode, HTTP/2 only
//   "1 svc.example.com."       – ServiceMode with explicit target
//   "0 svc.example.com."       – ServiceMode, priority 0 (alias)
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
// Domain-filter negotiation.

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

// ── POST /adjustendpoints ─────────────────────────────────────────────────────
//
// Called by external-dns before applying changes. For HTTPS endpoints we:
//
//  1. Check providerSpecific for the "pdns-https-target" annotation value.
//     If present, use it as the sole target (explicit override).
//  2. Otherwise fall back to normalising whatever targets are already set
//     so they carry a valid SvcPriority prefix.
//
// Priority of target resolution for HTTPS records:
//
//   annotation (pdns-https-target) > existing targets (normalised) > default

pub async fn adjust_endpoints(
    BodyJson(mut endpoints): BodyJson<Vec<Endpoint>>,
) -> impl IntoResponse {
    for ep in &mut endpoints {
        if ep.record_type != "HTTPS" {
            continue;
        }

        // ── 1. Check for explicit annotation override ─────────────────────
        if let Some(annotation_value) = find_provider_specific(&ep, HTTPS_TARGET_ANNOTATION) {
            let target = validate_https_target(&annotation_value, &ep.dns_name);
            info!(
                "HTTPS {} → target from annotation '{}': {}",
                ep.dns_name, HTTPS_TARGET_ANNOTATION, target
            );
            ep.targets = vec![target];
            continue;
        }

        // ── 2. Normalise existing targets (ensure SvcPriority prefix) ─────
        if ep.targets.is_empty() {
            warn!(
                "HTTPS {} has no targets and no '{}' annotation; skipping",
                ep.dns_name, HTTPS_TARGET_ANNOTATION
            );
            continue;
        }

        ep.targets = ep
            .targets
            .iter()
            .map(|t| normalise_https_target(t, &ep.dns_name))
            .collect();

        debug!("HTTPS {} → normalised targets: {:?}", ep.dns_name, ep.targets);
    }

    (webhook_headers(), Json(endpoints))
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Find a value in an endpoint's providerSpecific list by annotation name.
/// external-dns strips the "external-dns.alpha.kubernetes.io/provider-specific-"
/// prefix before storing, so we match on the bare key only.
fn find_provider_specific(ep: &Endpoint, key: &str) -> Option<String> {
    ep.provider_specific
        .iter()
        .find(|p| p.name == key)
        .map(|p| p.value.clone())
}

/// Validate and normalise an HTTPS SvcParam string from the annotation.
///
/// Accepts both:
///   "1 . alpn=h2,h3"   – already fully formed
///   ". alpn=h2,h3"     – missing priority → prepend "1"
///   "alpn=h2,h3"       – missing priority + target → prepend "1 ."
fn validate_https_target(value: &str, dns_name: &str) -> String {
    let trimmed = value.trim();
    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();

    match parts.as_slice() {
        // Already fully-formed: "<priority> <target> [params]"
        [priority, _target, ..] if priority.parse::<u16>().is_ok() => {
            trimmed.to_string()
        }
        // Missing priority, starts with target: ". alpn=h2" or "svc. alpn=h2"
        [_target, ..] if !parts[0].parse::<u16>().is_ok() => {
            warn!(
                "HTTPS {} annotation missing SvcPriority; prepending '1'",
                dns_name
            );
            format!("1 {trimmed}")
        }
        // Completely bare value (e.g. just params or a hostname)
        _ => {
            warn!(
                "HTTPS {} annotation '{}' has unexpected format; wrapping as '1 . {}'",
                dns_name, value, trimmed
            );
            format!("1 . {trimmed}")
        }
    }
}

/// Ensure an HTTPS target has a SvcPriority prefix.
/// Used for targets that came from sources other than the annotation.
fn normalise_https_target(target: &str, dns_name: &str) -> String {
    let t = target.trim();
    // Already has a numeric priority as first token
    let first = t.split_whitespace().next().unwrap_or("");
    if first.parse::<u16>().is_ok() {
        return t.to_string();
    }
    warn!(
        "HTTPS {} target '{}' missing SvcPriority; prepending '1'",
        dns_name, target
    );
    format!("1 {t}")
}

fn error_response(code: u16, msg: String) -> Response {
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(serde_json::json!({"error": msg})),
    )
        .into_response()
}
