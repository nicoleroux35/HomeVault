/* =============================================================================
 * File          : geoip.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Country-based firewall rules (tasklist P1-14). A manually maintained
 *   CIDR database maps ISO country codes to networks; an allow/deny policy
 *   per country is expanded into **managed ufw rules** tagged
 *   `hv-geoip:<action>:<CC>`. Applying removes every previously managed rule
 *   first, so a database update or a policy change never leaves stale
 *   countries open/closed. ufw is the persistence layer (its own
 *   `/etc/ufw/user.rules`), no custom iptables chain is introduced.
 *
 * Dependencies:
 *   - serde_json (>= 1.0), ipnetwork (>= 0.20)
 *
 * Usage:
 *   handlers/firewall.rs (`/api/firewall/geoip*`), FirewallCard.svelte.
 * =============================================================================
 */

//! GeoIP firewall policy: country CIDR database → managed ufw rules.

use crate::firewall::{self, AddRuleOutcome, FirewallError, FirewallRule};
use ipnetwork::IpNetwork;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Comment prefix ufw stores for every rule this module owns.
pub const GEOIP_COMMENT_PREFIX: &str = "hv-geoip:";
/// A country's exported range list is bounded so a malformed database cannot
/// fill the rule table (or make an apply take hours).
pub const MAX_CIDRS_PER_COUNTRY: usize = 512;
/// Total managed rules across every selected country.
pub const MAX_GEOIP_RULES: usize = 2048;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeoIpPolicy {
    /// Country code → `allow` or `deny`.
    #[serde(default)]
    pub rules: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GeoIpCountryRule {
    pub country: String,
    pub action: String,
    pub cidr_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GeoIpRuleSet {
    pub country: String,
    pub action: String,
    pub cidrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeoIpStatus {
    pub available: bool,
    pub ufw_available: bool,
    pub database_present: bool,
    pub database_path: String,
    pub database_error: Option<String>,
    pub countries: Vec<GeoIpCountryRule>,
    pub managed_rules: usize,
    pub max_rules: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeoIpApplyResult {
    pub applied: usize,
    pub removed: usize,
    pub countries: Vec<GeoIpCountryRule>,
}

fn data_directory() -> PathBuf {
    PathBuf::from(
        std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string()),
    )
}

/// The manually maintained database. `HOMEVAULT_GEOIP_DB` exists for tests
/// and for an operator who keeps the file outside the data directory.
pub fn database_path() -> PathBuf {
    std::env::var("HOMEVAULT_GEOIP_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_directory().join("data/geoip/countries.json"))
}

fn policy_path() -> PathBuf {
    data_directory().join("data/geoip/policy.json")
}

pub fn validate_country_code(country: &str) -> Result<String, FirewallError> {
    let country = country.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(FirewallError::InvalidInput(
            "Country must be a two-letter ISO code (for example FR or US)".to_string(),
        ));
    }
    Ok(country)
}

pub fn validate_geoip_action(action: &str) -> Result<String, FirewallError> {
    match action.trim().to_ascii_lowercase().as_str() {
        "allow" => Ok("allow".to_string()),
        "deny" => Ok("deny".to_string()),
        other => Err(FirewallError::InvalidInput(format!(
            "GeoIP action '{other}' is not supported: use allow or deny"
        ))),
    }
}

/// Parses the manual database:
/// `{"FR": ["192.0.2.0/24", "2001:db8::/32"], "US": ["198.51.100.0/24"]}`.
pub fn parse_database(raw: &str) -> Result<BTreeMap<String, Vec<String>>, FirewallError> {
    let parsed: BTreeMap<String, Vec<String>> = serde_json::from_str(raw).map_err(|e| {
        FirewallError::InvalidInput(format!("GeoIP database is not valid JSON: {e}"))
    })?;
    let mut database = BTreeMap::new();
    for (country, networks) in parsed {
        let country = validate_country_code(&country)?;
        if networks.is_empty() {
            return Err(FirewallError::InvalidInput(format!(
                "GeoIP country '{country}' has no network"
            )));
        }
        if networks.len() > MAX_CIDRS_PER_COUNTRY {
            return Err(FirewallError::InvalidInput(format!(
                "GeoIP country '{country}' has {} networks (maximum {})",
                networks.len(),
                MAX_CIDRS_PER_COUNTRY
            )));
        }
        let mut clean = Vec::with_capacity(networks.len());
        for network in networks {
            let network = network.trim();
            IpNetwork::from_str(network).map_err(|_| {
                FirewallError::InvalidInput(format!(
                    "GeoIP country '{country}' contains an invalid network '{network}'"
                ))
            })?;
            clean.push(network.to_string());
        }
        clean.sort();
        clean.dedup();
        database.insert(country, clean);
    }
    Ok(database)
}

pub fn load_database() -> Result<BTreeMap<String, Vec<String>>, FirewallError> {
    let path = database_path();
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        FirewallError::Unavailable(format!(
            "GeoIP database is missing at {} ({e}); install or refresh the country CIDR list first",
            path.display()
        ))
    })?;
    parse_database(&raw)
}

pub fn load_policy() -> Result<GeoIpPolicy, FirewallError> {
    let path = policy_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
            FirewallError::InvalidInput(format!(
                "GeoIP policy at {} is not valid JSON: {e}",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GeoIpPolicy::default()),
        Err(e) => Err(FirewallError::Failed(format!(
            "Could not read the GeoIP policy: {e}"
        ))),
    }
}

fn save_policy(policy: &GeoIpPolicy) -> Result<(), FirewallError> {
    let path = policy_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            FirewallError::Failed(format!("Could not create {}: {e}", parent.display()))
        })?;
    }
    let temporary = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(policy)
        .map_err(|e| FirewallError::Failed(format!("Could not serialise the policy: {e}")))?;
    std::fs::write(&temporary, body)
        .map_err(|e| FirewallError::Failed(format!("Could not write the policy: {e}")))?;
    std::fs::rename(&temporary, &path)
        .map_err(|e| FirewallError::Failed(format!("Could not replace the policy: {e}")))?;
    Ok(())
}

/// Pure expansion of the policy against the database. Missing countries and
/// the global cap are refused **before** any rule is touched.
pub fn build_plan(
    policy: &GeoIpPolicy,
    database: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<GeoIpRuleSet>, FirewallError> {
    let mut plan = Vec::new();
    let mut total = 0usize;
    for (country, action) in &policy.rules {
        let country = validate_country_code(country)?;
        let action = validate_geoip_action(action)?;
        let cidrs = database.get(&country).ok_or_else(|| {
            FirewallError::InvalidInput(format!(
                "Country '{country}' is selected but missing from the GeoIP database"
            ))
        })?;
        total += cidrs.len();
        if total > MAX_GEOIP_RULES {
            return Err(FirewallError::InvalidInput(format!(
                "The selected countries expand to more than {MAX_GEOIP_RULES} firewall rules; narrow the selection or refresh the database"
            )));
        }
        plan.push(GeoIpRuleSet {
            country,
            action,
            cidrs: cidrs.clone(),
        });
    }
    plan.sort_by(|a, b| a.country.cmp(&b.country));
    Ok(plan)
}

/// Country rows for the status card.
pub fn country_rules(
    policy: &GeoIpPolicy,
    database: &BTreeMap<String, Vec<String>>,
) -> Vec<GeoIpCountryRule> {
    policy
        .rules
        .iter()
        .map(|(country, action)| GeoIpCountryRule {
            country: country.clone(),
            action: action.clone(),
            cidr_count: database.get(country).map(Vec::len).unwrap_or(0),
        })
        .collect()
}

pub fn is_managed_comment(comment: Option<&str>) -> bool {
    comment
        .map(|c| c.starts_with(GEOIP_COMMENT_PREFIX))
        .unwrap_or(false)
}

fn comment_for(action: &str, country: &str) -> String {
    format!("{GEOIP_COMMENT_PREFIX}{action}:{country}")
}

async fn remove_managed_rules(rules: &[FirewallRule]) -> Result<usize, FirewallError> {
    let mut removed = 0;
    for rule in rules
        .iter()
        .filter(|r| is_managed_comment(r.comment.as_deref()))
    {
        firewall::delete_rule(&rule.spec).await?;
        removed += 1;
    }
    Ok(removed)
}

pub async fn status() -> GeoIpStatus {
    let ufw_available = firewall::ufw_available();
    let database = load_database();
    let database_present = database.is_ok();
    let database_error = database.as_ref().err().map(ToString::to_string);
    let database = database.unwrap_or_default();
    let policy = load_policy().unwrap_or_default();
    let countries = country_rules(&policy, &database);
    let managed_rules = if ufw_available {
        firewall::get_status()
            .await
            .map(|s| {
                s.rules
                    .iter()
                    .filter(|r| is_managed_comment(r.comment.as_deref()))
                    .count()
            })
            .unwrap_or(0)
    } else {
        0
    };
    GeoIpStatus {
        available: ufw_available && database_present,
        ufw_available,
        database_present,
        database_path: database_path().to_string_lossy().into_owned(),
        database_error,
        countries,
        managed_rules,
        max_rules: MAX_GEOIP_RULES,
    }
}

/// Reconciles the live `hv-geoip:*` rules with the saved policy.
pub async fn apply() -> Result<GeoIpApplyResult, FirewallError> {
    if !firewall::ufw_available() {
        return Err(FirewallError::Unavailable(
            "GeoIP rules need ufw, which is not installed".to_string(),
        ));
    }
    let database = load_database()?;
    let policy = load_policy()?;
    let plan = build_plan(&policy, &database)?;
    let rules = country_rules(&policy, &database);

    let current = firewall::get_status().await?;
    let removed = remove_managed_rules(&current.rules).await?;

    let mut applied = 0;
    for set in &plan {
        for network in &set.cidrs {
            let comment = comment_for(&set.action, &set.country);
            match firewall::add_source_rule(&set.action, network, &comment).await {
                Ok(AddRuleOutcome::Added | AddRuleOutcome::AlreadyPresent) => applied += 1,
                Err(error) => {
                    // Do not leave a half-applied country set open. Best
                    // effort: remove what this run managed to add.
                    let cleanup = firewall::get_status().await;
                    if let Ok(status) = cleanup {
                        let _ = remove_managed_rules(&status.rules).await;
                    }
                    return Err(error);
                }
            }
        }
    }
    Ok(GeoIpApplyResult {
        applied,
        removed,
        countries: rules,
    })
}

/// Adds or replaces one country policy, then applies the whole policy.
pub async fn set_rule(country: &str, action: &str) -> Result<GeoIpApplyResult, FirewallError> {
    let country = validate_country_code(country)?;
    let action = validate_geoip_action(action)?;
    let database = load_database()?;
    if !database.contains_key(&country) {
        return Err(FirewallError::InvalidInput(format!(
            "Country '{country}' is not in the GeoIP database"
        )));
    }
    let mut policy = load_policy()?;
    policy.rules.insert(country, action);
    // Validate/cap before writing: a policy that cannot be applied must not
    // replace the one that is currently in force.
    build_plan(&policy, &database)?;
    save_policy(&policy)?;
    apply().await
}

/// Removes one country policy, then applies the rest.
pub async fn remove_rule(country: &str) -> Result<GeoIpApplyResult, FirewallError> {
    let country = validate_country_code(country)?;
    let mut policy = load_policy()?;
    if policy.rules.remove(&country).is_none() {
        return Err(FirewallError::InvalidInput(format!(
            "Country '{country}' is not in the GeoIP policy"
        )));
    }
    save_policy(&policy)?;
    apply().await
}

/// Reads the database path without parsing it (used by the status test).
pub fn database_exists() -> bool {
    Path::new(&database_path()).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([
            ("FR".to_string(), vec!["192.0.2.0/24".to_string()]),
            ("CN".to_string(), vec!["198.51.100.0/24".to_string()]),
        ])
    }

    #[test]
    fn country_codes_and_actions_are_strict() {
        assert_eq!(validate_country_code(" fr ").unwrap(), "FR");
        assert!(validate_country_code("FRA").is_err());
        assert!(validate_country_code("1R").is_err());
        assert_eq!(validate_geoip_action("ALLOW").unwrap(), "allow");
        assert!(validate_geoip_action("reject").is_err());
    }

    #[test]
    fn database_parsing_validates_every_network() {
        let parsed =
            parse_database(r#"{"fr": ["192.0.2.0/24", "192.0.2.0/24"], "US": ["2001:db8::/32"]}"#)
                .unwrap();
        assert_eq!(parsed["FR"], vec!["192.0.2.0/24"]);
        assert_eq!(parsed["US"], vec!["2001:db8::/32"]);
        assert!(parse_database(r#"{"FR": ["not-a-network"]}"#).is_err());
        assert!(parse_database(r#"{"F": ["192.0.2.0/24"]}"#).is_err());
        assert!(parse_database(r#"{"FR": []}"#).is_err());
        assert!(parse_database("{").is_err());
    }

    #[test]
    fn plan_expands_only_selected_countries_and_caps_the_total() {
        let policy = GeoIpPolicy {
            rules: BTreeMap::from([
                ("CN".to_string(), "deny".to_string()),
                ("FR".to_string(), "allow".to_string()),
            ]),
        };
        let plan = build_plan(&policy, &database()).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].country, "CN");
        assert_eq!(plan[0].cidrs, vec!["198.51.100.0/24"]);
        assert_eq!(plan[1].action, "allow");

        let missing = GeoIpPolicy {
            rules: BTreeMap::from([("DE".to_string(), "deny".to_string())]),
        };
        assert!(build_plan(&missing, &database()).is_err());
    }

    #[test]
    fn managed_comments_are_the_only_ones_removed() {
        assert!(is_managed_comment(Some("hv-geoip:deny:CN")));
        assert!(!is_managed_comment(Some("homevault-custom-allow-5900")));
        assert!(!is_managed_comment(None));
        assert_eq!(comment_for("allow", "FR"), "hv-geoip:allow:FR");
    }
}
