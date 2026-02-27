use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Base URL of the PowerDNS HTTP API, e.g. http://powerdns:8081
    #[serde(default = "default_pdns_url")]
    pub pdns_api_url: String,

    /// PowerDNS API key (X-API-Key header)
    #[serde(default = "default_pdns_key")]
    pub pdns_api_key: String,

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
}

impl Config {
    /// Parse from environment variables (PDNS_API_URL, PDNS_API_KEY, …)
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(envy::from_env::<Config>()?)
    }

    /// Return the domain filter as a Vec<String>, empty if unconfigured.
    pub fn domain_filter_list(&self) -> Vec<String> {
        self.domain_filter
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }
}

fn default_pdns_url()  -> String { "http://localhost:8081".into() }
fn default_pdns_key()  -> String { "changeme".into() }
fn default_server_id() -> String { "localhost".into() }
fn default_ttl()       -> u32    { 300 }
fn default_port()      -> u16    { 8888 }
