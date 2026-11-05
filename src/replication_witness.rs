/* =============================================================================
 * File          : replication_witness.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   A minimal replication witness (tasklist P2-15): a lease that a third
 *   party (local file here; an S3 lock is the same shape) grants to one node
 *   at a time. A node may promote only while it holds an unexpired lease, and
 *   two unexpired leases for different nodes are reported as split brain.
 *   This is infrastructure: it does not configure an actual remote witness.
 *
 * Dependencies:
 *   - chrono, serde, crate::storage::atomic::AtomicWriter
 *
 * Usage:
 *   See docs/guides/replication.md.
 * =============================================================================
 */

//! Witness lease and split-brain detection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A lease granted by the witness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WitnessLease {
    pub holder: String,
    pub epoch: u64,
    pub expires_at: String,
}

/// Whether a node may act as primary. With two data sites, losing the witness
/// means no promotion: that is the whole point of the third vote.
pub fn may_promote(lease: Option<&WitnessLease>, node: &str, now: DateTime<Utc>) -> bool {
    match lease {
        Some(lease) => {
            lease.holder == node
                && DateTime::parse_from_rfc3339(&lease.expires_at)
                    .map(|at| at.with_timezone(&Utc) > now)
                    .unwrap_or(false)
        }
        None => false,
    }
}

/// Try to take an unexpired lease through the witness. An existing lease held
/// by another node is refused until it expires; the same holder renews with a
/// higher epoch.
pub fn take_lease(
    current: Option<&WitnessLease>,
    node: &str,
    now: DateTime<Utc>,
    ttl_seconds: i64,
) -> Result<WitnessLease, String> {
    if let Some(lease) = current {
        let unexpired = DateTime::parse_from_rfc3339(&lease.expires_at)
            .map(|at| at.with_timezone(&Utc) > now)
            .unwrap_or(false);
        if unexpired && lease.holder != node {
            return Err(format!(
                "the witness is held by '{}' until {}",
                lease.holder, lease.expires_at
            ));
        }
    }
    Ok(WitnessLease {
        holder: node.to_string(),
        epoch: current.map(|l| l.epoch + 1).unwrap_or(1),
        expires_at: (now + chrono::Duration::seconds(ttl_seconds)).to_rfc3339(),
    })
}

/// Two live leases from different holders mean two primaries: a split brain.
pub fn split_brain(leases: &[WitnessLease], now: DateTime<Utc>) -> Option<(String, String)> {
    let live: Vec<&WitnessLease> = leases
        .iter()
        .filter(|l| {
            DateTime::parse_from_rfc3339(&l.expires_at)
                .map(|at| at.with_timezone(&Utc) > now)
                .unwrap_or(false)
        })
        .collect();
    if live.len() >= 2 && live[0].holder != live[1].holder {
        return Some((live[0].holder.clone(), live[1].holder.clone()));
    }
    None
}

use std::time::Duration;

/// A witness server request times out quickly: replication must not hang on a
/// third site that is down.
const WITNESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Third-site lease configuration, read from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessConfig {
    /// Base URL of the witness service (e.g. `https://witness.example.net`).
    pub endpoint: String,
    /// Optional bearer token shared with the witness.
    pub token: Option<String>,
    /// Name of this node in the lease.
    pub node: String,
    /// Lease duration in seconds.
    pub ttl_seconds: i64,
}

impl WitnessConfig {
    pub fn lease_url(&self) -> String {
        format!("{}/lease", self.endpoint.trim_end_matches('/'))
    }
}

/// Build the configuration from environment values. `None` when no witness is
/// configured; an invalid URL/node is an explicit error, never a silent
/// fallback to "no witness".
pub fn witness_config_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<WitnessConfig>, String> {
    let Some(endpoint) = get("HOMEVAULT_WITNESS_URL").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let endpoint = endpoint.trim().to_string();
    if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
        return Err("HOMEVAULT_WITNESS_URL must start with http:// or https://".to_string());
    }
    let node = get("HOMEVAULT_WITNESS_NODE")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            "HOMEVAULT_WITNESS_NODE is required when a witness URL is set".to_string()
        })?;
    let ttl_seconds = get("HOMEVAULT_WITNESS_TTL")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| (10..=86_400).contains(v))
        .unwrap_or(60);
    Ok(Some(WitnessConfig {
        endpoint,
        token: get("HOMEVAULT_WITNESS_TOKEN").filter(|v| !v.trim().is_empty()),
        node,
        ttl_seconds,
    }))
}

/// HTTP client for a witness that speaks the lease contract:
/// `GET /lease` → lease or 404; `POST /lease {node, ttl_seconds}` → lease;
/// `DELETE /lease {node}` → release.
pub struct WitnessClient {
    config: WitnessConfig,
    http: reqwest::Client,
}

impl WitnessClient {
    pub fn new(config: WitnessConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(WITNESS_TIMEOUT)
            .build()
            .map_err(|e| format!("could not build the witness client: {e}"))?;
        Ok(Self { config, http })
    }

    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let request = self.http.request(method, self.config.lease_url());
        match &self.config.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Current lease, or `None` when the witness has none.
    pub async fn current(&self) -> Result<Option<WitnessLease>, String> {
        let response = self
            .request(reqwest::Method::GET)
            .send()
            .await
            .map_err(|e| format!("witness unreachable: {e}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!("witness answered {}", response.status()));
        }
        response
            .json::<WitnessLease>()
            .await
            .map(Some)
            .map_err(|e| format!("invalid witness lease: {e}"))
    }

    /// Acquire (or renew) the lease for this node. Refused when another node
    /// holds an unexpired lease.
    pub async fn acquire(&self) -> Result<WitnessLease, String> {
        let response = self
            .request(reqwest::Method::POST)
            .json(&serde_json::json!({
                "node": self.config.node,
                "ttl_seconds": self.config.ttl_seconds,
            }))
            .send()
            .await
            .map_err(|e| format!("witness unreachable: {e}"))?;
        if response.status() == reqwest::StatusCode::CONFLICT {
            return Err("the witness is held by another node".to_string());
        }
        if !response.status().is_success() {
            return Err(format!("witness answered {}", response.status()));
        }
        let lease = response
            .json::<WitnessLease>()
            .await
            .map_err(|e| format!("invalid witness lease: {e}"))?;
        if !may_promote(Some(&lease), &self.config.node, Utc::now()) {
            return Err(format!(
                "the witness granted the lease to '{}', not '{}'",
                lease.holder, self.config.node
            ));
        }
        Ok(lease)
    }

    /// Release this node's lease; a refusal is reported but never fatal.
    pub async fn release(&self) -> Result<(), String> {
        let response = self
            .request(reqwest::Method::DELETE)
            .json(&serde_json::json!({ "node": self.config.node }))
            .send()
            .await
            .map_err(|e| format!("witness unreachable: {e}"))?;
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(format!("witness answered {}", response.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            + chrono::Duration::seconds(seconds)
    }

    fn lease(holder: &str, expires: i64, epoch: u64) -> WitnessLease {
        WitnessLease {
            holder: holder.to_string(),
            epoch,
            expires_at: at(expires).to_rfc3339(),
        }
    }

    #[test]
    fn promotion_needs_an_unexpired_lease_in_the_nodes_name() {
        let now = at(0);
        assert!(!may_promote(None, "a", now));
        assert!(may_promote(Some(&lease("a", 60, 1)), "a", now));
        assert!(!may_promote(Some(&lease("b", 60, 1)), "a", now));
        assert!(!may_promote(Some(&lease("a", -1, 1)), "a", now));
    }

    #[test]
    fn a_live_lease_belongs_to_one_node_until_it_expires() {
        let now = at(0);
        let held = lease("a", 60, 1);
        assert!(take_lease(Some(&held), "b", now, 30).is_err());
        let renewed = take_lease(Some(&held), "a", now, 30).unwrap();
        assert_eq!(renewed.epoch, 2);
        let expired = lease("a", -1, 3);
        assert_eq!(
            take_lease(Some(&expired), "b", now, 30).unwrap().holder,
            "b"
        );
    }

    #[test]
    fn two_live_leases_are_a_split_brain() {
        let now = at(0);
        let leases = vec![lease("a", 60, 2), lease("b", 60, 2)];
        assert_eq!(
            split_brain(&leases, now),
            Some(("a".to_string(), "b".to_string()))
        );
        let expired = vec![lease("a", 60, 2), lease("b", -1, 2)];
        assert!(split_brain(&expired, now).is_none());
    }

    #[test]
    fn witness_configuration_is_all_or_nothing() {
        let none = witness_config_from_env(|_| None).unwrap();
        assert!(none.is_none());

        let config = witness_config_from_env(|key| match key {
            "HOMEVAULT_WITNESS_URL" => Some("https://witness.example.net/".to_string()),
            "HOMEVAULT_WITNESS_NODE" => Some("nas-a".to_string()),
            "HOMEVAULT_WITNESS_TOKEN" => Some("secret".to_string()),
            _ => None,
        })
        .unwrap()
        .expect("config");
        assert_eq!(config.lease_url(), "https://witness.example.net/lease");
        assert_eq!(config.node, "nas-a");
        assert_eq!(config.ttl_seconds, 60);

        assert!(witness_config_from_env(|key| match key {
            "HOMEVAULT_WITNESS_URL" => Some("ftp://bad".to_string()),
            _ => None,
        })
        .is_err());
        assert!(witness_config_from_env(|key| match key {
            "HOMEVAULT_WITNESS_URL" => Some("https://witness".to_string()),
            _ => None,
        })
        .is_err());
    }
}
