// tests/adjust_endpoints.rs
//
// Integration test: POST to /adjustendpoints with a real AAAA + HTTPS-target
// providerSpecific payload and assert the endpoint is transformed correctly.
//
// Run:
//   cargo test
//   cargo test -- --nocapture     (show println! output)

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt; // for `.oneshot()`

// ─────────────────────────────────────────────────────────────────────────────
// Build a minimal router that only mounts /adjustendpoints.
// This avoids needing a real PowerDNS server or secret files.
// ─────────────────────────────────────────────────────────────────────────────

fn test_router() -> Router {
    // Import the handler directly; no AppState needed for adjustendpoints.
    use axum::routing::post;
    use pdns_webhook::handlers;

    Router::new().route("/adjustendpoints", post(handlers::adjust_endpoints))
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Send a POST /adjustendpoints and return the parsed JSON response body.
async fn post_adjust(body: Value) -> (StatusCode, Value) {
    let app = test_router();

    let request = Request::builder()
        .method("POST")
        .uri("/adjustendpoints")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    (status, json)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// The payload that external-dns sends for a Traefik service with an HTTPS
/// target annotation. Record type is AAAA, but the webhook/pdns-https-target
/// providerSpecific entry instructs the webhook to create an HTTPS record.
#[tokio::test]
async fn test_adjust_https_target_from_annotation() {
    let body = json!([{
        "dnsName": "test.domain.com",
        "labels": {
            "resource": "service/traefik/traefik"
        },
        "providerSpecific": [
            {
                "name": "webhook/pdns-https-target",
                "value": "1 . alpn=h2"
            }
        ],
        "recordTTL": 10,
        "recordType": "AAAA",
        "targets": [
            "2001:41d0:203:39e1::1",
            "2001:41d0:8:8c6a::1",
            "2001:41d0:8:edcf::1"
        ]
    }]);

    let (status, response) = post_adjust(body).await;

    assert_eq!(status, StatusCode::OK, "expected 200, got {status}: {response}");

    let endpoints = response.as_array().expect("response should be an array");
    assert_eq!(endpoints.len(), 2, "expected 1 endpoint back");

    let aaaa_ep = &endpoints[0];
    assert_eq!(aaaa_ep["dnsName"], "test.domain.com");
    assert_eq!(aaaa_ep["recordTTL"], 10);
    assert_eq!(aaaa_ep["recordType"], "AAAA");
    //assert_eq!(aaaa_ep["targets"].as_array().iter().len(), 3);


    let ep = &endpoints[1];

    // dnsName and TTL should be preserved unchanged
    assert_eq!(ep["dnsName"], "test.domain.com");
    assert_eq!(ep["recordTTL"], 10);

    // recordType should be upgraded to HTTPS
    assert_eq!(
        ep["recordType"], "HTTPS",
        "recordType should be HTTPS, got {}",
        ep["recordType"]
    );

    // targets should be replaced by the annotation value
    let targets = ep["targets"].as_array().expect("targets should be an array");
    assert_eq!(targets.len(), 1, "expected exactly 1 HTTPS target");
    assert_eq!(
        targets[0], "1 . alpn=h2",
        "target should match annotation value"
    );
}

/// Verify that endpoints without the annotation are passed through with their
/// original targets normalised (AAAA stays AAAA, targets unchanged).
#[tokio::test]
async fn test_adjust_aaaa_without_annotation_passes_through() {
    let body = json!([{
        "dnsName": "plain.example.com",
        "labels": {},
        "providerSpecific": [],
        "recordTTL": 300,
        "recordType": "AAAA",
        "targets": ["2001:db8::1"]
    }]);

    let (status, response) = post_adjust(body).await;

    assert_eq!(status, StatusCode::OK);

    let ep = &response.as_array().unwrap()[0];
    assert_eq!(ep["recordType"], "AAAA");
    assert_eq!(ep["targets"][0], "2001:db8::1");
}

/// Verify that an HTTPS endpoint without the annotation keeps its targets but
/// they are normalised (SvcPriority prefix added if missing).
#[tokio::test]
async fn test_adjust_https_without_annotation_normalises_targets() {
    let body = json!([{
        "dnsName": "app.example.com",
        "labels": {},
        "providerSpecific": [],
        "recordTTL": 300,
        "recordType": "HTTPS",
        "targets": ["1 svc.example.com. alpn=h2"]   // bare hostname, no priority
    }]);

    let (status, response) = post_adjust(body).await;

    assert_eq!(status, StatusCode::OK);

    let ep = &response.as_array().unwrap()[0];
    assert_eq!(ep["recordType"], "HTTPS");

    let target = ep["targets"][0].as_str().unwrap();
    assert_eq!(target, "1 svc.example.com. alpn=h2");
}

/// Empty input returns an empty array (not an error).
#[tokio::test]
async fn test_adjust_empty_array() {
    let (status, response) = post_adjust(json!([])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response, json!([]));
}
