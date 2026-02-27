use serde::Deserialize;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Raw env-var config (non-sensitive values only)
// ─────────────────────────────────────────────────────────────────────────────

/// Non-sensitive configuration loaded from environment variables.
#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
    /// Base URL of the PowerDNS HTTP API, e.g. http://powerdns:8081
    #[serde(default = "default_pdns_url")]
    pub pdns_api_url: String,

    /// PowerDNS server-id, almost always "localhost"
    #[serde(default = "default_server_id")]
    pub pdns_server_id: String,

    /// Comma-separated list of zones to manage; empty = manage all
    #[serde(default)]
    pub domain_filter: String,

    /// Default TTL when the endpoint doesn't specify one
    #[serde(default = "default_ttl")]
    pub default_ttl: u32,

    /// Port to listen on
    #[serde(default = "default_port")]
    pub port: u16,

    // ── Secret resolution ────────────────────────────────────────────────────
    //
    // Secrets are loaded from files, not plain env vars.
    // Set the *_FILE path to point at a mounted Kubernetes Secret volume.
    //
    // Fallback order for the API key:
    //   1. PDNS_API_KEY_FILE   – path to a file containing the key  (preferred)
    //   2. PDNS_API_KEY        – inline value                        (dev only)

    /// Path to a file that contains the PowerDNS API key.
    #[serde(default = "default_api_key_file")]
    pub pdns_api_key_file: String,

    /// Inline API key – used only when PDNS_API_KEY_FILE does not exist.
    #[serde(default)]
    pub pdns_api_key: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public config (secrets already resolved to in-memory values)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub pdns_api_url: String,
    /// The resolved API key – never stored in an env var at runtime.
    pub pdns_api_key: String,
    pub pdns_server_id: String,
    pub domain_filter: String,
    pub default_ttl: u32,
    pub port: u16,
}

impl Config {
    /// Load configuration.
    ///
    /// Non-sensitive values come from environment variables.
    /// The PowerDNS API key is read from the file pointed at by
    /// `PDNS_API_KEY_FILE` (default `/var/run/secrets/pdns/api-key`).
    /// If that file does not exist the `PDNS_API_KEY` env var is used as a
    /// fallback so local development still works without a mounted secret.
    pub fn from_env() -> anyhow::Result<Self> {
        let raw: RawConfig = envy::from_env()?;

        let pdns_api_key = resolve_secret(
            &raw.pdns_api_key_file,
            &raw.pdns_api_key,
            "PDNS_API_KEY",
        )?;

        Ok(Self {
            pdns_api_url: raw.pdns_api_url,
            pdns_api_key,
            pdns_server_id: raw.pdns_server_id,
            domain_filter: raw.domain_filter,
            default_ttl: raw.default_ttl,
            port: raw.port,
        })
    }

    /// Return the domain filter as a `Vec<String>`, empty if unconfigured.
    pub fn domain_filter_list(&self) -> Vec<String> {
        self.domain_filter
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Secret resolution helper
// ─────────────────────────────────────────────────────────────────────────────

/// Read a secret value from `file_path` if it exists, otherwise fall back to
/// `inline`.  Leading/trailing whitespace (including newlines) is stripped from
/// file contents so secrets can be stored one-per-line without extra care.
fn resolve_secret(file_path: &str, inline: &str, name: &str) -> anyhow::Result<String> {
    let path = Path::new(file_path);
    if path.exists() {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading secret file {file_path}: {e}"))?;
        let value = raw.trim().to_string();
        if value.is_empty() {
            anyhow::bail!("secret file {file_path} is empty");
        }
        tracing::debug!("loaded {name} from file {file_path}");
        Ok(value)
    } else if !inline.is_empty() {
        tracing::warn!(
            "{name}: falling back to env-var value (not recommended for production)"
        );
        Ok(inline.to_string())
    } else {
        anyhow::bail!(
            "secret {name} not configured: set {name}_FILE to a mounted secret path \
             or {name} for local dev"
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Defaults
// ─────────────────────────────────────────────────────────────────────────────

fn default_pdns_url()     -> String { "http://localhost:8081".into() }
fn default_server_id()    -> String { "localhost".into() }
fn default_ttl()          -> u32    { 300 }
fn default_port()         -> u16    { 8888 }
fn default_api_key_file() -> String { "/var/run/secrets/pdns/api-key".into() }
