use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use crate::{config::Config, dns::Endpoint};

// ─────────────────────────────────────────────────────────────────────────────
// PowerDNS API shapes (partial – only what we need)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ZoneStub {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Zone {
    pub rrsets: Vec<RrSet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RrSet {
    pub name: String,
    #[serde(rename = "type")]
    pub rrtype: String,
    pub ttl: u32,
    #[serde(default)]
    pub records: Vec<Record>,
    /// Used in PATCH requests; omit on read
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changetype: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub content: String,
    pub disabled: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Client
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PdnsClient {
    http: Client,
    cfg: Config,
}

impl PdnsClient {
    pub fn new(cfg: Config) -> Result<Self> {
        let http = Client::builder()
            .build()
            .context("building reqwest client")?;
        Ok(Self { http, cfg })
    }

    fn base(&self) -> String {
        format!(
            "{}/api/v1/servers/{}",
            self.cfg.pdns_api_url.trim_end_matches('/'),
            self.cfg.pdns_server_id
        )
    }

    fn api_key(&self) -> &str {
        &self.cfg.pdns_api_key
    }

    // ── zones ────────────────────────────────────────────────────────────────

    /// List all zones (stub objects only).
    pub async fn list_zones(&self) -> Result<Vec<ZoneStub>> {
        let url = format!("{}/zones", self.base());
        let resp = self
            .http
            .get(&url)
            .header("X-API-Key", self.api_key())
            .send()
            .await
            .context("GET /zones")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("PowerDNS GET /zones {}: {}", status, body);
        }
        Ok(resp.json().await?)
    }

    /// Fetch a zone with all its RRsets.
    pub async fn get_zone(&self, zone_id: &str) -> Result<Zone> {
        let url = format!("{}/zones/{}", self.base(), zone_id);
        let resp = self
            .http
            .get(&url)
            .header("X-API-Key", self.api_key())
            .send()
            .await
            .context("GET /zones/:id")?;

        if !resp.status().is_success() {
            bail!("PowerDNS GET zone {} → {}", zone_id, resp.status());
        }
        Ok(resp.json().await?)
    }

    /// Walk up the DNS tree to find the best matching zone for `fqdn`.
    pub async fn zone_for(&self, fqdn: &str) -> Result<String> {
        let labels: Vec<&str> = fqdn.trim_end_matches('.').split('.').collect();
        for i in 1..labels.len() {
            let candidate = format!("{}.", labels[i..].join("."));
            let url = format!("{}/zones/{}", self.base(), candidate);
            let resp = self
                .http
                .get(&url)
                .header("X-API-Key", self.api_key())
                .send()
                .await?;
            if resp.status().is_success() {
                debug!("zone_for({fqdn}) → {candidate}");
                return Ok(candidate);
            }
        }
        Err(anyhow!("no PowerDNS zone found for {fqdn}"))
    }

    // ── mutations ────────────────────────────────────────────────────────────

    async fn patch_zone(&self, zone: &str, rrsets: Vec<RrSet>) -> Result<()> {
        let url = format!("{}/zones/{}", self.base(), zone);
        let payload = serde_json::json!({ "rrsets": rrsets });

        let resp = self
            .http
            .patch(&url)
            .header("X-API-Key", self.api_key())
            .json(&payload)
            .send()
            .await
            .context("PATCH /zones/:id")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            error!("PowerDNS PATCH {zone} [{status}]: {body}");
            bail!("PowerDNS PATCH error {status}: {body}");
        }
        Ok(())
    }

    /// Create or replace an RRset for the given endpoint.
    pub async fn upsert(&self, ep: &Endpoint, default_ttl: u32) -> Result<()> {
        let zone = self.zone_for(&ep.dns_name).await?;
        let rrset = build_rrset(ep, default_ttl, "REPLACE");
        info!(
            "UPSERT {rtype} {name} → {zone}",
            rtype = ep.record_type,
            name = ep.dns_name
        );
        self.patch_zone(&zone, vec![rrset]).await
    }

    /// Delete the RRset for the given endpoint.
    pub async fn delete(&self, ep: &Endpoint) -> Result<()> {
        let zone = self.zone_for(&ep.dns_name).await?;
        let rrset = RrSet {
            name: ensure_fqdn(&ep.dns_name),
            rrtype: ep.record_type.clone(),
            ttl: 0,
            records: vec![],
            changetype: Some("DELETE".into()),
            comments: vec![],
        };
        info!(
            "DELETE {rtype} {name} from {zone}",
            rtype = ep.record_type,
            name = ep.dns_name
        );
        self.patch_zone(&zone, vec![rrset]).await
    }

    // ── read ─────────────────────────────────────────────────────────────────

    /// Return all managed endpoints from all zones,
    /// optionally restricted to `domain_filter`.
    pub async fn list_endpoints(
        &self,
        domain_filter: &[String],
    ) -> Result<Vec<Endpoint>> {
        const MANAGED_TYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "HTTPS"];

        let zones = self.list_zones().await?;
        let mut endpoints = Vec::new();

        for zone_stub in zones {
            let zone = match self.get_zone(&zone_stub.id).await {
                Ok(z) => z,
                Err(e) => {
                    error!("skipping zone {}: {e}", zone_stub.id);
                    continue;
                }
            };

            for rrset in zone.rrsets {
                if !MANAGED_TYPES.contains(&rrset.rrtype.as_str()) {
                    continue;
                }

                let name = rrset.name.trim_end_matches('.').to_string();

                if !domain_filter.is_empty()
                    && !domain_filter.iter().any(|d| name.ends_with(d.as_str()))
                {
                    continue;
                }

                let targets: Vec<String> = rrset
                    .records
                    .iter()
                    .filter(|r| !r.disabled)
                    .map(|r| r.content.clone())
                    .collect();

                if targets.is_empty() {
                    continue;
                }

                endpoints.push(Endpoint {
                    dns_name: name,
                    record_type: rrset.rrtype,
                    targets,
                    record_ttl: rrset.ttl,
                    ..Default::default()
                });
            }
        }

        Ok(endpoints)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn ensure_fqdn(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}

/// Format an HTTPS target into PowerDNS wire-text format.
/// If the target already contains a space (SvcPriority is present) leave it
/// alone; otherwise wrap with `1 <target>.`
fn normalise_https_target(target: &str) -> String {
    if target.contains(' ') {
        target.to_string()
    } else {
        format!("1 {}.", target.trim_end_matches('.'))
    }
}

fn build_rrset(ep: &Endpoint, default_ttl: u32, changetype: &str) -> RrSet {
    let ttl = if ep.record_ttl > 0 { ep.record_ttl } else { default_ttl };

    let records: Vec<Record> = ep
        .targets
        .iter()
        .map(|t| {
            let content = if ep.record_type == "HTTPS" {
                normalise_https_target(t)
            } else {
                t.clone()
            };
            Record { content, disabled: false }
        })
        .collect();

    RrSet {
        name: ensure_fqdn(&ep.dns_name),
        rrtype: ep.record_type.clone(),
        ttl,
        records,
        changetype: Some(changetype.to_string()),
        comments: vec![],
    }
}
