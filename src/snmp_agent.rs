/* =============================================================================
 * File          : snmp_agent.rs
 * Author        : Nicolas Le Roux
 * Created       : 04/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The local SNMP **agent** — the mirror image of `src/snmp.rs`, which is a
 *   client that queries *remote* devices. This module owns the appliance's own
 *   `snmpd`: it renders `/etc/snmp/snmpd.conf`, decides whether to bind the
 *   agent to loopback or to the LAN, and starts/stops the service.
 *
 *   Three rules it exists to keep:
 *   - **Off by default.** An unconfigured appliance answers no SNMP request
 *     at all. Monitoring is a choice, not a shipped default.
 *   - **Never `public`.** The default community is generated, not inherited:
 *     `public` is the first string every scanner tries, and an appliance that
 *     answers it is an appliance whose interface list and disk usage is public.
 *   - **LAN-bound, never internet-facing.** `agentaddress` is either loopback
 *     (local monitoring only) or one configured LAN address — never `udp:*`,
 *     never all interfaces.
 *
 *   Two access methods are supported: SNMPv2c with one generated read-only
 *   community (default), or SNMPv3 with a dedicated user, SHA authentication
 *   and AES privacy. The v3 passphrases are sealed (`hv1:`) before they touch
 *   `snmp-agent.json`; the rendered file is mode 600 either way.
 *
 * Dependencies:
 *   - serde, crate::utils::process (deadlines on every subprocess)
 *
 * Usage:
 *   handlers/snmp.rs — `GET|PUT /api/snmp/agent`.
 * =============================================================================
 */

//! The local SNMP agent (`snmpd`): config rendering, scope rules, service
//! control. See docs/guides/snmp-agent.md.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;

use crate::utils::process::{run_timed, run_timed_with_stdin, HOST_PROBE_TIMEOUT};

/// Reading the daemon's view: it prints its version and exits.
const SNMPD_PROBE_TIMEOUT: std::time::Duration = HOST_PROBE_TIMEOUT;
/// `systemctl start|stop|restart|enable|disable`: can wait on the job queue.
const SNMPD_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Writing the config file through `sudo tee`.
const SNMPD_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Every file this module owns. The `99-` prefix and the `homevault-` name
/// keep the write scoped to our own file, which is also what the sudoers grant
/// and the rollback rely on.
pub const CONF_PATH: &str = "/etc/snmp/snmpd.conf.d/99-homevault-agent.conf";

/// The longest community string `snmpd` accepts, and the length below which
/// a shared secret is not worth calling a secret.
pub const MIN_COMMUNITY_LEN: usize = 8;
pub const MAX_COMMUNITY_LEN: usize = 32;

/// SNMPv3 passphrases: `snmpd` requires at least 8 characters; 64 keeps the
/// rendered line readable and bounds what a secret can inject.
pub const MIN_V3_SECRET_LEN: usize = 8;
pub const MAX_V3_SECRET_LEN: usize = 64;

/// The default dedicated v3 user. Never an account name the appliance uses
/// for SSH or a share: an SNMP user is an SNMP-only identity.
pub const DEFAULT_V3_USERNAME: &str = "homevault";

/// Sealed-secret scopes. Distinct per secret so an authentication blob cannot
/// be opened as a privacy blob (the AAD check fails instead of returning the
/// wrong passphrase).
const V3_AUTH_SCOPE: &str = "snmp-agent:v3-auth";
const V3_PRIV_SCOPE: &str = "snmp-agent:v3-priv";

/// The access method: a v2c community, or a v3 user with auth + privacy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentVersion {
    /// One read-only community string. The historical default.
    #[default]
    V2c,
    /// SNMPv3, authentication SHA, privacy AES.
    V3,
}

/// Where the agent listens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindScope {
    /// `agentaddress 127.0.0.1,[::1]` — only processes on this machine.
    Loopback,
    /// One configured LAN address: monitoring can come from the LAN, never
    /// from the internet. The installer already restricts what the appliance
    /// exposes; this keeps SNMP on the same side of the line.
    Lan,
}

impl BindScope {
    /// The default, and the safe answer for "I am not sure yet".
    pub fn default_scope() -> Self {
        BindScope::Loopback
    }
}

impl Default for BindScope {
    fn default() -> Self {
        Self::default_scope()
    }
}

/// The agent's configuration, as persisted and as rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// `false` means "no agent": the file is removed and the service stopped.
    pub enabled: bool,
    pub scope: BindScope,
    /// The LAN address to bind to when `scope == Lan`. Required then, ignored
    /// otherwise (a stale address must not make a loopback agent unbootable).
    pub lan_address: Option<String>,
    /// The read-only community (v2c only). Generated on enable, never `public`.
    pub community: String,
    /// `sysLocation` — where the box physically is.
    pub location: String,
    /// `sysContact` — who to wake up.
    pub contact: String,
    /// Which access method the agent serves.
    #[serde(default)]
    pub version: AgentVersion,
    /// The dedicated SNMPv3 user (v3 only).
    #[serde(default)]
    pub v3_username: String,
    /// The v3 authentication passphrase, in memory only. `skip`: a config is
    /// never serialized as-is — persistence goes through
    /// [`PersistedAgentConfig`], which seals both v3 secrets first.
    #[serde(skip)]
    pub v3_auth_secret: String,
    /// The v3 privacy passphrase, sealed on the same path.
    #[serde(skip)]
    pub v3_priv_secret: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scope: BindScope::default_scope(),
            lan_address: None,
            community: String::new(),
            location: String::new(),
            contact: String::new(),
            version: AgentVersion::default(),
            v3_username: DEFAULT_V3_USERNAME.to_string(),
            v3_auth_secret: String::new(),
            v3_priv_secret: String::new(),
        }
    }
}

/// The on-disk shape: identical to [`AgentConfig`], except the two v3
/// passphrases are `hv1:` sealed blobs. Keeping this a separate type is what
/// stops a plaintext secret from being written by accident: `AgentConfig`
/// skips both fields entirely during serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAgentConfig {
    pub enabled: bool,
    #[serde(default)]
    pub scope: BindScope,
    #[serde(default)]
    pub lan_address: Option<String>,
    #[serde(default)]
    pub community: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub version: AgentVersion,
    #[serde(default)]
    pub v3_username: String,
    /// Sealed authentication passphrase; empty when none is configured.
    #[serde(default)]
    pub v3_auth_secret: String,
    /// Sealed privacy passphrase; empty when none is configured.
    #[serde(default)]
    pub v3_priv_secret: String,
}

fn seal_optional(scope: &str, value: &str) -> Result<String, String> {
    if value.is_empty() {
        Ok(String::new())
    } else {
        crate::storage::secret_box::seal(scope, value)
            .map_err(|e| format!("could not seal the SNMPv3 secret: {e}"))
    }
}

fn open_optional(scope: &str, value: &str) -> Result<String, String> {
    if value.is_empty() {
        Ok(String::new())
    } else {
        crate::storage::secret_box::open(scope, value).map_err(|e| {
            format!(
                "the stored SNMPv3 secret could not be opened ({e}) — re-enter the v3 passphrases"
            )
        })
    }
}

#[cfg(test)]
fn seal_optional_with_key(key: &[u8; 32], scope: &str, value: &str) -> Result<String, String> {
    if value.is_empty() {
        Ok(String::new())
    } else {
        crate::storage::secret_box::seal_with_key(key, scope, value)
            .map_err(|e| format!("could not seal the SNMPv3 secret: {e}"))
    }
}

#[cfg(test)]
fn open_optional_with_key(key: &[u8; 32], scope: &str, value: &str) -> Result<String, String> {
    if value.is_empty() {
        Ok(String::new())
    } else {
        crate::storage::secret_box::open_with_key(key, scope, value).map_err(|e| {
            format!(
                "the stored SNMPv3 secret could not be opened ({e}) — re-enter the v3 passphrases"
            )
        })
    }
}

impl PersistedAgentConfig {
    /// Seal an in-memory config for `snmp-agent.json`.
    pub fn from_config(config: &AgentConfig) -> Result<Self, String> {
        Self::from_config_with(config, |scope, value| seal_optional(scope, value))
    }

    /// Open a stored config back into the runtime shape.
    pub fn into_config(self) -> Result<AgentConfig, String> {
        self.into_config_with(|scope, value| open_optional(scope, value))
    }

    /// The pure core of [`Self::from_config`]: the seal function is injected
    /// so the unit tests exercise the shape without touching the appliance's
    /// data directory.
    fn from_config_with<F>(config: &AgentConfig, seal: F) -> Result<Self, String>
    where
        F: Fn(&str, &str) -> Result<String, String>,
    {
        Ok(Self {
            enabled: config.enabled,
            scope: config.scope,
            lan_address: config.lan_address.clone(),
            community: config.community.clone(),
            location: config.location.clone(),
            contact: config.contact.clone(),
            version: config.version,
            v3_username: config.v3_username.clone(),
            v3_auth_secret: seal(V3_AUTH_SCOPE, &config.v3_auth_secret)?,
            v3_priv_secret: seal(V3_PRIV_SCOPE, &config.v3_priv_secret)?,
        })
    }

    /// The pure core of [`Self::into_config`] (see [`Self::from_config_with`]).
    fn into_config_with<F>(self, open: F) -> Result<AgentConfig, String>
    where
        F: Fn(&str, &str) -> Result<String, String>,
    {
        Ok(AgentConfig {
            enabled: self.enabled,
            scope: self.scope,
            lan_address: self.lan_address,
            community: self.community,
            location: self.location,
            contact: self.contact,
            version: self.version,
            v3_username: if self.v3_username.trim().is_empty() {
                DEFAULT_V3_USERNAME.to_string()
            } else {
                self.v3_username
            },
            v3_auth_secret: open(V3_AUTH_SCOPE, &self.v3_auth_secret)?,
            v3_priv_secret: open(V3_PRIV_SCOPE, &self.v3_priv_secret)?,
        })
    }

    /// Seal with an explicit key — used by the unit tests.
    #[cfg(test)]
    fn from_config_sealed(config: &AgentConfig, key: &[u8; 32]) -> Result<Self, String> {
        Self::from_config_with(config, |scope, value| {
            seal_optional_with_key(key, scope, value)
        })
    }

    /// Open with an explicit key — used by the unit tests.
    #[cfg(test)]
    fn into_config_sealed(self, key: &[u8; 32]) -> Result<AgentConfig, String> {
        self.into_config_with(|scope, value| open_optional_with_key(key, scope, value))
    }
}

impl AgentConfig {
    /// The config the agent would run with, without any secret.
    pub fn redacted(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.enabled,
            "scope": self.scope,
            "lan_address": self.lan_address,
            "location": self.location,
            "contact": self.contact,
            "version": self.version,
            "v3_username": self.v3_username,
            // Never the community or the v3 passphrases themselves: this
            // endpoint is what the screen reads on every visit.
            "has_community": !self.community.is_empty(),
            "has_v3_auth_secret": !self.v3_auth_secret.is_empty(),
            "has_v3_priv_secret": !self.v3_priv_secret.is_empty(),
        })
    }
}

/// Why a request to enable the agent was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

/// A generated community: 16 bytes of `/dev/urandom` rendered as lowercase
/// hex. Not base64: `snmpd.conf` treats `#` as a comment and spaces as
/// separators, and keeping the alphabet to `[0-9a-f]` removes the whole class
/// of "the community started with a `#`" bugs.
pub fn generate_community() -> Result<String, ConfigError> {
    let mut bytes = [0u8; 16];
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| ConfigError(format!("Could not read random bytes: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{:02x}", b)).collect())
}

/// Is `community` a usable read-only community string?
///
/// The rules are the ones `snmpd.conf` imposes plus the one this project adds:
/// **`public` is refused by name**, because accepting it means shipping the
/// single most-guessed credential in network management.
pub fn validate_community(community: &str) -> Result<(), ConfigError> {
    let community = community.trim();
    if community.is_empty() {
        return Err(ConfigError("The community must not be empty".to_string()));
    }
    if community.eq_ignore_ascii_case("public") || community.eq_ignore_ascii_case("private") {
        return Err(ConfigError(
            "'public' and 'private' are the two strings every scanner tries first — choose another community"
                .to_string(),
        ));
    }
    if community.chars().count() < MIN_COMMUNITY_LEN {
        return Err(ConfigError(format!(
            "The community must be at least {MIN_COMMUNITY_LEN} characters long"
        )));
    }
    if community.chars().count() > MAX_COMMUNITY_LEN {
        return Err(ConfigError(format!(
            "The community must be at most {MAX_COMMUNITY_LEN} characters long"
        )));
    }
    // `snmpd.conf` is line- and whitespace-oriented: a `#`, a space, a quote
    // or a newline would silently change the meaning of the line.
    if community
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '#' || c == '"' || c == '\'')
    {
        return Err(ConfigError(
            "The community cannot contain spaces, quotes, `#` or control characters".to_string(),
        ));
    }
    Ok(())
}

/// Is `username` a usable SNMPv3 user name?
///
/// `snmpd`'s `createUser` line is whitespace-separated: a space, a quote or a
/// newline would turn the user name into extra directives. The alphabet is
/// deliberately small and the name is never a shell account.
pub fn validate_v3_username(username: &str) -> Result<(), ConfigError> {
    let username = username.trim();
    if username.is_empty() {
        return Err(ConfigError(
            "The SNMPv3 user name must not be empty".to_string(),
        ));
    }
    if username.chars().count() > 32 {
        return Err(ConfigError(
            "The SNMPv3 user name must be at most 32 characters long".to_string(),
        ));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(ConfigError(
            "The SNMPv3 user name may only contain letters, digits, '.', '-' and '_'".to_string(),
        ));
    }
    if !username
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
    {
        return Err(ConfigError(
            "The SNMPv3 user name must start with a letter or a digit".to_string(),
        ));
    }
    Ok(())
}

/// Is `secret` a usable SNMPv3 authentication/privacy passphrase?
///
/// The rendered directive puts the value between double quotes, so a quote, a
/// backslash, whitespace or a control character is refused rather than escaped
/// — the same conservative alphabet rule the community follows. `snmpd`
/// itself requires at least 8 characters.
pub fn validate_v3_secret(kind: &str, secret: &str) -> Result<(), ConfigError> {
    let secret = secret.trim();
    if secret.chars().count() < MIN_V3_SECRET_LEN {
        return Err(ConfigError(format!(
            "The SNMPv3 {kind} passphrase must be at least {MIN_V3_SECRET_LEN} characters long"
        )));
    }
    if secret.chars().count() > MAX_V3_SECRET_LEN {
        return Err(ConfigError(format!(
            "The SNMPv3 {kind} passphrase must be at most {MAX_V3_SECRET_LEN} characters long"
        )));
    }
    if secret
        .chars()
        .any(|c| !c.is_ascii_graphic() || c == '"' || c == '\\')
    {
        return Err(ConfigError(format!(
            "The SNMPv3 {kind} passphrase cannot contain spaces, quotes, backslashes or control characters"
        )));
    }
    Ok(())
}

/// A fresh v3 passphrase: 16 bytes of `/dev/urandom` as 32 hex characters, the
/// same shape as the generated community (and for the same reason: no `#`,
/// no quote, no whitespace).
pub fn generate_v3_secret() -> Result<String, ConfigError> {
    generate_community()
}

/// Is `address` an address an agent may bind to?
///
/// Only a private, link-local or loopback IPv4 address is accepted: a public
/// address here would publish the appliance's SNMP agent to the internet, and
/// a wildcard (`0.0.0.0`) would listen on every interface at once.
pub fn validate_lan_address(address: &str) -> Result<Ipv4Addr, ConfigError> {
    let parsed: Ipv4Addr = address
        .trim()
        .parse()
        .map_err(|_| ConfigError(format!("'{address}' is not an IPv4 address")))?;
    if parsed.is_unspecified() {
        return Err(ConfigError(
            "The agent cannot bind to 0.0.0.0 — pick the appliance's own LAN address".to_string(),
        ));
    }
    if parsed.is_loopback() {
        return Err(ConfigError(
            "127.0.0.1 is the loopback scope — choose that scope instead".to_string(),
        ));
    }
    if !(parsed.is_private() || parsed.is_link_local()) {
        return Err(ConfigError(format!(
            "{} is not a private LAN address; SNMP must never be published outside the local network",
            parsed
        )));
    }
    Ok(parsed)
}

/// Validate a whole enable request, producing the config that would be saved.
///
/// `community: None` keeps the current one (or generates a fresh one when
/// there is none yet) — the screen never has to round-trip the secret just to
/// change the location string.
pub fn validate_config(
    requested: AgentConfig,
    current: &AgentConfig,
) -> Result<AgentConfig, ConfigError> {
    let mut next = requested;

    if next.enabled {
        // Only an unused address is required; the scope decides whether it is
        // used, so a loopback agent does not need to carry a LAN address.
        if next.scope == BindScope::Lan {
            let address = next.lan_address.clone().unwrap_or_default();
            let parsed = validate_lan_address(&address)?;
            next.lan_address = Some(parsed.to_string());
        } else {
            next.lan_address = None;
        }

        match next.version {
            AgentVersion::V2c => {
                let community = if next.community.trim().is_empty() {
                    if current.community.trim().is_empty() {
                        generate_community()?
                    } else {
                        current.community.clone()
                    }
                } else {
                    next.community.trim().to_string()
                };
                validate_community(&community)?;
                next.community = community;
            }
            AgentVersion::V3 => {
                let username = if next.v3_username.trim().is_empty() {
                    if current.v3_username.trim().is_empty() {
                        DEFAULT_V3_USERNAME.to_string()
                    } else {
                        current.v3_username.clone()
                    }
                } else {
                    next.v3_username.trim().to_string()
                };
                validate_v3_username(&username)?;
                next.v3_username = username;

                // An empty field keeps the stored passphrase, or generates one
                // when there is none yet — the screen never round-trips a
                // secret it cannot read back.
                let auth = if next.v3_auth_secret.trim().is_empty() {
                    if current.v3_auth_secret.trim().is_empty() {
                        generate_v3_secret()?
                    } else {
                        current.v3_auth_secret.clone()
                    }
                } else {
                    next.v3_auth_secret.trim().to_string()
                };
                validate_v3_secret("authentication", &auth)?;

                let privacy = if next.v3_priv_secret.trim().is_empty() {
                    if current.v3_priv_secret.trim().is_empty() {
                        generate_v3_secret()?
                    } else {
                        current.v3_priv_secret.clone()
                    }
                } else {
                    next.v3_priv_secret.trim().to_string()
                };
                validate_v3_secret("privacy", &privacy)?;

                next.v3_auth_secret = auth;
                next.v3_priv_secret = privacy;
            }
        }

        next.location = sanitize_text(&next.location, 128)?;
        next.contact = sanitize_text(&next.contact, 128)?;
    } else {
        // Disabling keeps the community on disk? No: the file is deleted, so
        // the secret goes with it. A re-enable generates a fresh one unless
        // the screen supplied it.
        next.community = current.community.clone();
        next.v3_username = if next.v3_username.trim().is_empty() {
            current.v3_username.clone()
        } else {
            next.v3_username.trim().to_string()
        };
        next.v3_auth_secret = current.v3_auth_secret.clone();
        next.v3_priv_secret = current.v3_priv_secret.clone();
        next.location = sanitize_text(&next.location, 128)?;
        next.contact = sanitize_text(&next.contact, 128)?;
    }
    Ok(next)
}

/// Free text going into a `snmpd.conf` line: no newlines (they would start a
/// new directive), no control characters, bounded length.
fn sanitize_text(value: &str, max: usize) -> Result<String, ConfigError> {
    let trimmed = value.trim();
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(ConfigError(
            "The location and contact cannot contain line breaks or control characters".to_string(),
        ));
    }
    if trimmed.chars().count() > max {
        return Err(ConfigError(format!(
            "The location and contact must be at most {max} characters long"
        )));
    }
    Ok(trimmed.to_string())
}

/// The `agentaddress` line for a scope.
pub fn agent_address_line(config: &AgentConfig) -> String {
    match config.scope {
        BindScope::Loopback => "agentaddress 127.0.0.1,[::1]".to_string(),
        BindScope::Lan => {
            let address = config.lan_address.clone().unwrap_or_default();
            format!("agentaddress udp:{address}:161")
        }
    }
}

/// Render the whole `snmpd.conf` drop-in.
///
/// What it deliberately does **not** do: grant a write community, accept SNMPv1
/// (`sysservices`/`trap` stay v2c), or widen the default view beyond the two
/// OID subtrees the installer already adds (interfaces and storage) plus the
/// system group. An appliance that answers `inetCidrRouteTable` to anyone with
/// the community is an appliance that publishes its routing table.
pub fn render_config(config: &AgentConfig) -> String {
    let mut s = String::new();
    s.push_str("# Written by HomeVault from the SNMP agent settings.\n");
    s.push_str("# Delete this file and restart snmpd to hand the agent back.\n");
    s.push_str(&match config.version {
        AgentVersion::V2c => "# Read-only, SNMPv2c, no write community, no routing table.\n",
        AgentVersion::V3 => {
            "# Read-only, SNMPv3 (auth SHA, priv AES), no write user, no routing table.\n"
        }
    });
    s.push('\n');
    s.push_str(&agent_address_line(config));
    s.push('\n');
    s.push_str("# What an SNMP reader can see: system, interfaces, storage.\n");
    s.push_str("view systemonly included .1.3.6.1.2.1.1\n");
    s.push_str("view systemonly included .1.3.6.1.2.1.2\n");
    s.push_str("view systemonly included .1.3.6.1.2.1.25.2\n");
    s.push_str("view systemonly excluded .1.3.6.1.2.1.2.2.1.4\n");
    s.push('\n');
    match config.version {
        AgentVersion::V2c => {
            s.push_str("# Read-only community. Never 'public'.\n");
            if config.community.is_empty() {
                // Defensive: a config must never be written without a
                // community, but if one somehow is, the agent answers nobody
                // rather than everybody.
                s.push_str("# (no community configured — the agent answers no request)\n");
            } else {
                // `-V systemonly` is what actually applies the view above; a
                // bare `rocommunity` would grant the default (full) view.
                s.push_str(&format!(
                    "rocommunity {} default -V systemonly\n",
                    config.community
                ));
            }
        }
        AgentVersion::V3 => {
            s.push_str("# Dedicated SNMPv3 user. Read-only: no `rwuser`.\n");
            if config.v3_username.is_empty()
                || config.v3_auth_secret.is_empty()
                || config.v3_priv_secret.is_empty()
            {
                // Defensive: never emit a half-configured v3 user. The agent
                // then answers no request instead of accepting a weak login.
                s.push_str("# (no SNMPv3 credentials configured — the agent answers no request)\n");
            } else {
                s.push_str(&format!(
                    "createUser {} SHA \"{}\" AES \"{}\"\n",
                    config.v3_username, config.v3_auth_secret, config.v3_priv_secret
                ));
                s.push_str(&format!(
                    "rouser {} authpriv systemonly\n",
                    config.v3_username
                ));
            }
        }
    }
    s.push('\n');
    if !config.location.is_empty() {
        s.push_str(&format!("sysLocation {}\n", config.location));
    }
    if !config.contact.is_empty() {
        s.push_str(&format!("sysContact {}\n", config.contact));
    }
    s
}

/// True when the `snmpd` daemon is installed (`/usr/sbin/snmpd`, falling back
/// to `PATH` for a non-standard layout).
pub fn agent_installed() -> bool {
    std::path::Path::new("/usr/sbin/snmpd").exists() || which("snmpd").is_some()
}

fn which(binary: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

/// `systemctl <action> snmpd`, under a deadline.
fn systemctl(action: &str) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["systemctl", action, "snmpd"]);
    let output = run_timed(&mut cmd, SNMPD_CONTROL_TIMEOUT)
        .map_err(|e| format!("could not run systemctl {action} snmpd: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "systemctl {} snmpd failed: {}",
            action,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Write the drop-in and restart the agent. Called after the config was
/// validated and persisted by the handler.
pub fn write_config(config: &AgentConfig) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["tee", CONF_PATH]);
    let body = render_config(config);
    let output = run_timed_with_stdin(&mut cmd, SNMPD_WRITE_TIMEOUT, body.as_bytes())
        .map_err(|e| format!("could not write {CONF_PATH}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not write {}: {}",
            CONF_PATH,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // Our file carries a secret: root-only, like the netplan files this
    // project writes.
    let mut chmod = Command::new("sudo");
    chmod.args(["chmod", "600", CONF_PATH]);
    let _ = run_timed(&mut chmod, SNMPD_WRITE_TIMEOUT);
    Ok(())
}

/// Remove the drop-in (the disabled state) and stop the agent.
pub fn remove_config() -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(["rm", "-f", CONF_PATH]);
    let output = run_timed(&mut cmd, SNMPD_WRITE_TIMEOUT)
        .map_err(|e| format!("could not remove {CONF_PATH}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not remove {}: {}",
            CONF_PATH,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Apply a config to the host: write (or remove) the file, then restart or
/// stop the service. Every step is reported, so the screen can say exactly
/// what state the appliance is in instead of a bare "saved".
pub fn apply_to_host(config: &AgentConfig) -> Result<&'static str, String> {
    if config.enabled {
        write_config(config)?;
        systemctl("enable")?;
        systemctl("restart")?;
        Ok("running")
    } else {
        remove_config()?;
        // Stop first, then disable: a stopped-but-enabled daemon comes back at
        // the next boot, which is exactly the state "off" must not leave.
        let _ = systemctl("stop");
        let _ = systemctl("disable");
        Ok("stopped")
    }
}

/// Whether the agent is currently serving SNMP queries.
pub fn is_serving() -> bool {
    let mut cmd = Command::new("systemctl");
    cmd.args(["is-active", "snmpd"]);
    matches!(run_timed(&mut cmd, SNMPD_PROBE_TIMEOUT), Ok(out) if out.status.success())
}

/// The addresses this appliance could bind the agent to: its own private and
/// link-local IPv4 addresses, so the screen offers a real choice instead of a
/// free-text field nobody can fill correctly.
pub fn lan_bind_candidates() -> Vec<String> {
    crate::network::list_interfaces()
        .into_iter()
        .filter_map(|iface| iface.ipv4)
        .filter(|ip| {
            ip.parse::<IpAddr>()
                .map(|addr| match addr {
                    IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified(),
                    IpAddr::V6(_) => false,
                })
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(community: &str, scope: BindScope) -> AgentConfig {
        AgentConfig {
            enabled: true,
            scope,
            lan_address: None,
            community: community.to_string(),
            location: "Rack, garage".to_string(),
            contact: "admin@example.local".to_string(),
            version: AgentVersion::V2c,
            v3_username: DEFAULT_V3_USERNAME.to_string(),
            v3_auth_secret: String::new(),
            v3_priv_secret: String::new(),
        }
    }

    fn v3_enabled(auth: &str, privacy: &str) -> AgentConfig {
        let mut config = enabled("", BindScope::Loopback);
        config.version = AgentVersion::V3;
        config.v3_auth_secret = auth.to_string();
        config.v3_priv_secret = privacy.to_string();
        config
    }

    #[test]
    fn test_community_never_accepts_public_or_private() {
        for bad in ["public", "PUBLIC", "Private", "  public  "] {
            assert!(
                validate_community(bad).is_err(),
                "accepted the default community {bad:?}"
            );
        }
    }

    #[test]
    fn test_community_bounds_and_characters() {
        assert!(validate_community("short").is_err(), "too short");
        assert!(validate_community(&"a".repeat(MAX_COMMUNITY_LEN + 1)).is_err());
        assert!(validate_community(&"a".repeat(MAX_COMMUNITY_LEN)).is_ok());
        for bad in [
            "has space",
            "has\ttab",
            "line\nbreak",
            "quote\"d",
            "hash#tag",
            "ctrl\u{7}",
        ] {
            assert!(
                validate_community(bad).is_err(),
                "accepted {bad:?} — it would change the meaning of the snmpd line"
            );
        }
    }

    #[test]
    fn test_lan_address_must_be_private_and_never_a_wildcard() {
        assert!(validate_lan_address("192.168.1.10").is_ok());
        assert!(validate_lan_address("10.0.0.5").is_ok());
        assert!(validate_lan_address("172.16.4.4").is_ok());
        assert!(validate_lan_address("169.254.3.3").is_ok());
        // Wildcard, loopback, public and garbage are all refused.
        assert!(validate_lan_address("0.0.0.0").is_err());
        assert!(validate_lan_address("127.0.0.1").is_err());
        assert!(validate_lan_address("8.8.8.8").is_err());
        assert!(validate_lan_address("not-an-ip").is_err());
    }

    #[test]
    fn test_loopback_scope_ignores_a_stale_lan_address() {
        let current = enabled("", BindScope::Loopback);
        let mut requested = enabled("", BindScope::Loopback);
        requested.lan_address = Some("192.168.1.10".to_string());
        let next = validate_config(requested, &current).expect("valid");
        assert_eq!(
            next.lan_address, None,
            "a loopback agent carries no address"
        );
        assert!(validate_community(&next.community).is_ok());
    }

    #[test]
    fn test_lan_scope_requires_a_usable_address() {
        let current = enabled("", BindScope::Loopback);
        let mut requested = enabled("", BindScope::Lan);
        requested.lan_address = Some("0.0.0.0".to_string());
        assert!(validate_config(requested, &current).is_err());

        let mut ok = enabled("", BindScope::Lan);
        ok.lan_address = Some("192.168.1.10".to_string());
        let next = validate_config(ok, &current).expect("valid");
        assert_eq!(next.lan_address.as_deref(), Some("192.168.1.10"));
    }

    #[test]
    fn test_an_empty_community_is_generated_and_kept() {
        let current = AgentConfig::default();
        let next = validate_config(enabled("", BindScope::Loopback), &current).expect("valid");
        assert_eq!(next.community.len(), 32, "16 bytes of hex");
        assert_ne!(next.community, "public");

        // A later save without a community keeps the one in use, so the screen
        // never has to round-trip the secret.
        let mut again = enabled("", BindScope::Loopback);
        again.location = "Attic".to_string();
        let next2 = validate_config(again, &next).expect("valid");
        assert_eq!(next2.community, next.community);
    }

    #[test]
    fn test_rendered_config_is_read_only_and_bounded() {
        let config = enabled("0123456789abcdef", BindScope::Loopback);
        let rendered = render_config(&config);
        assert!(
            rendered.contains("agentaddress 127.0.0.1,[::1]"),
            "{rendered}"
        );
        assert!(
            rendered.contains("rocommunity 0123456789abcdef default -V systemonly"),
            "the declared view must actually be applied: {rendered}"
        );
        assert!(rendered.contains("sysLocation Rack, garage"), "{rendered}");
        // Never a write community, never the routing table, never `udp:*`.
        assert!(!rendered.contains("rwcommunity"), "{rendered}");
        assert!(!rendered.contains("inetCidrRouteTable"), "{rendered}");
        assert!(!rendered.contains("udp:*"), "{rendered}");
        assert!(!rendered.contains("0.0.0.0"), "{rendered}");
        // The agent must not answer the interfaces' own addresses either — the
        // view excludes `ifInetAddress`-adjacent columns? No: it is the
        // ifPhysAddress (MAC) column that stays hidden.
        assert!(
            rendered.contains("view systemonly excluded .1.3.6.1.2.1.2.2.1.4"),
            "{rendered}"
        );
    }

    #[test]
    fn test_lan_scope_renders_one_address_never_a_wildcard() {
        let mut config = enabled("0123456789abcdef", BindScope::Lan);
        config.lan_address = Some("192.168.1.10".to_string());
        let rendered = render_config(&config);
        assert!(
            rendered.contains("agentaddress udp:192.168.1.10:161"),
            "{rendered}"
        );
    }

    #[test]
    fn test_a_config_without_a_community_answers_nobody() {
        let mut config = enabled("", BindScope::Loopback);
        config.community = String::new();
        let rendered = render_config(&config);
        assert!(!rendered.contains("rocommunity"), "{rendered}");
        assert!(rendered.contains("answers no request"), "{rendered}");
    }

    #[test]
    fn test_redacted_config_never_carries_the_community() {
        let config = enabled("0123456789abcdef", BindScope::Lan);
        let redacted = config.redacted();
        let text = redacted.to_string();
        assert!(!text.contains("0123456789abcdef"), "{text}");
        assert_eq!(redacted["has_community"], serde_json::json!(true));
        assert_eq!(redacted["enabled"], serde_json::json!(true));
    }

    #[test]
    fn test_v3_username_is_a_small_safe_alphabet() {
        for good in ["homevault", "monitor_1", "nas-eu.2"] {
            assert!(validate_v3_username(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "has space",
            "quote\"d",
            "line\nbreak",
            "-leading-dash",
            "user#1",
            "é",
            &"a".repeat(33),
        ] {
            assert!(
                validate_v3_username(bad).is_err(),
                "accepted the v3 user name {bad:?}"
            );
        }
    }

    #[test]
    fn test_v3_passphrases_are_bounded_and_cannot_inject_a_directive() {
        assert!(validate_v3_secret("authentication", "short").is_err());
        assert!(validate_v3_secret("authentication", &"a".repeat(MAX_V3_SECRET_LEN + 1)).is_err());
        assert!(validate_v3_secret("authentication", &"a".repeat(MIN_V3_SECRET_LEN)).is_ok());
        // The value sits between double quotes in `createUser`.
        for bad in [
            "has space",
            "has\ttab",
            "line\nbreak",
            "quote\"d",
            "back\\slash",
            "ctrl\u{7}",
        ] {
            assert!(
                validate_v3_secret("privacy", bad).is_err(),
                "accepted the v3 passphrase {bad:?}"
            );
        }
    }

    #[test]
    fn test_v3_renders_a_dedicated_authpriv_user_limited_to_the_view() {
        let config = v3_enabled("authpass123", "privpass456");
        let rendered = render_config(&config);
        assert!(
            rendered.contains("createUser homevault SHA \"authpass123\" AES \"privpass456\""),
            "{rendered}"
        );
        assert!(
            rendered.contains("rouser homevault authpriv systemonly"),
            "{rendered}"
        );
        // No v2c community, no write user, no full view.
        assert!(!rendered.contains("rocommunity"), "{rendered}");
        assert!(!rendered.contains("rwuser"), "{rendered}");
    }

    #[test]
    fn test_v3_without_credentials_answers_nobody() {
        let config = v3_enabled("", "");
        let rendered = render_config(&config);
        assert!(!rendered.contains("createUser"), "{rendered}");
        assert!(!rendered.contains("rouser"), "{rendered}");
        assert!(
            rendered.contains("answers no request"),
            "a half-configured v3 user must never be rendered: {rendered}"
        );
    }

    #[test]
    fn test_v3_enable_generates_and_keeps_both_passphrases() {
        let current = AgentConfig::default();
        let next = validate_config(v3_enabled("", ""), &current).expect("valid");
        assert_eq!(next.version, AgentVersion::V3);
        assert_eq!(next.v3_auth_secret.len(), 32);
        assert_eq!(next.v3_priv_secret.len(), 32);
        assert_ne!(next.v3_auth_secret, next.v3_priv_secret);

        // A later save without passphrases keeps the ones in use.
        let mut again = v3_enabled("", "");
        again.location = "Attic".to_string();
        let next2 = validate_config(again, &next).expect("valid");
        assert_eq!(next2.v3_auth_secret, next.v3_auth_secret);
        assert_eq!(next2.v3_priv_secret, next.v3_priv_secret);
    }

    #[test]
    fn test_v3_secrets_are_sealed_at_rest_and_round_trip() {
        const KEY: [u8; 32] = [9u8; 32];
        let config = v3_enabled("authpass123", "privpass456");
        let stored = PersistedAgentConfig::from_config_sealed(&config, &KEY).expect("sealed");
        let json = serde_json::to_string(&stored).expect("json");
        assert!(!json.contains("authpass123"), "{json}");
        assert!(!json.contains("privpass456"), "{json}");
        assert!(stored.v3_auth_secret.starts_with("hv1:"), "{json}");
        assert!(stored.v3_priv_secret.starts_with("hv1:"), "{json}");

        let reopened = stored.into_config_sealed(&KEY).expect("opened");
        assert_eq!(reopened.v3_auth_secret, "authpass123");
        assert_eq!(reopened.v3_priv_secret, "privpass456");
        assert_eq!(reopened.v3_username, "homevault");
    }

    #[test]
    fn test_v3_secrets_do_not_survive_a_scope_swap() {
        const KEY: [u8; 32] = [9u8; 32];
        let config = v3_enabled("authpass123", "privpass456");
        let mut stored = PersistedAgentConfig::from_config_sealed(&config, &KEY).expect("sealed");
        // The authentication blob is not a privacy blob: swapping them must
        // fail authentication instead of handing back the wrong passphrase.
        std::mem::swap(&mut stored.v3_auth_secret, &mut stored.v3_priv_secret);
        assert!(
            stored.into_config_sealed(&KEY).is_err(),
            "a v3 secret must be bound to its own scope"
        );
    }

    #[test]
    fn test_v3_redacted_never_carries_a_passphrase() {
        let config = v3_enabled("authpass123", "privpass456");
        let redacted = config.redacted();
        let text = redacted.to_string();
        assert!(!text.contains("authpass123"), "{text}");
        assert!(!text.contains("privpass456"), "{text}");
        assert_eq!(redacted["version"], serde_json::json!("v3"));
        assert_eq!(redacted["v3_username"], serde_json::json!("homevault"));
        assert_eq!(redacted["has_v3_auth_secret"], serde_json::json!(true));
        assert_eq!(redacted["has_v3_priv_secret"], serde_json::json!(true));
    }

    #[test]
    fn test_an_old_config_without_v3_fields_still_loads() {
        let json = r#"{"enabled":true,"scope":"loopback","lan_address":null,
            "community":"0123456789abcdef","location":"","contact":""}"#;
        let stored: PersistedAgentConfig = serde_json::from_str(json).expect("old config");
        let config = stored.into_config().expect("opened");
        assert_eq!(config.version, AgentVersion::V2c);
        assert_eq!(config.v3_username, DEFAULT_V3_USERNAME);
        assert!(config.v3_auth_secret.is_empty());
        assert!(config.v3_priv_secret.is_empty());
    }

    #[test]
    fn test_location_and_contact_cannot_inject_a_directive() {
        let current = AgentConfig::default();
        let mut config = enabled("0123456789abcdef", BindScope::Loopback);
        config.location = "Rack\nrocommunity attacker default".to_string();
        assert!(validate_config(config, &current).is_err());

        let mut crowded = enabled("0123456789abcdef", BindScope::Loopback);
        crowded.contact = "x".repeat(200);
        assert!(validate_config(crowded, &current).is_err());
    }
}
