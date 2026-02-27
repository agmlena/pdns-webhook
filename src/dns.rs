use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// external-dns webhook contract types
// ─────────────────────────────────────────────────────────────────────────────

/// A provider-specific property attached to an endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSpecific {
    pub name: String,
    pub value: String,
}

/// One DNS endpoint as external-dns understands it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub dns_name: String,
    pub record_type: String,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub record_ttl: u32,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub provider_specific: Vec<ProviderSpecific>,
    #[serde(default)]
    pub set_identifier: String,
}

/// The payload sent by external-dns to POST /records.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    #[serde(default)]
    pub create: Vec<Endpoint>,
    #[serde(default)]
    pub update_old: Vec<Endpoint>,
    #[serde(default)]
    pub update_new: Vec<Endpoint>,
    #[serde(default)]
    pub delete: Vec<Endpoint>,
}

/// Domain-filter response for GET /
#[derive(Debug, Serialize)]
pub struct DomainFilter {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}
