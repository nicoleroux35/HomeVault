/* =============================================================================
 * File          : firewall.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Firewall management via ufw: rules are never mirrored in the database —
 *   live system state is the source of truth (read from ufw status), with
 *   mandatory self-lockout rules re-asserted on enable and a comment-based
 *   check protecting mandatory rules from deletion. Every value that becomes
 *   an argv entry is validated first, and every call runs under a timeout.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   handlers/firewall.rs; persistence of settings via FirewallRepository.
 * =============================================================================
 */

//! Firewall management via `ufw` (Debian/Ubuntu only, matching the OS check
//! already in `installer/install.sh`). Follows the same "live system state
//! is the source of truth" principle as `src/vpn.rs`'s `wg show`: rules are
//! never mirrored in the database, only read live from `ufw status
//! verbose`/`ufw show added`, so there is no drift between what HomeVault
//! displays and what the kernel is actually enforcing.
//!
//! Self-lockout safety: SSH and HomeVault's own port are always allowed via
//! `ensure_mandatory_rules`, and a rule is only ever treated as "mandatory"
//! (and thus protected from deletion) by matching the rule's own comment
//! string on the live `ufw` output — never from client input — so this
//! can't be bypassed by a raw API call.
//!
//! Two further rules make the module safe to expose:
//!
//! * **Nothing unvalidated reaches argv.** `action`, `protocol`, `port` and
//!   `source` all came straight from a JSON body into `sudo ufw …` before
//!   2026-10-21: `{"action": "delete"}` ran `sudo ufw delete …`, and a port
//!   of `22/tcp` produced `22/tcp/tcp`. See the `validate_*` helpers.
//! * **Deletion reuses the live rule's own spec.** `ufw show added` prints a
//!   command ufw accepts, so `FirewallRule::spec` is that exact argument
//!   string and `ufw delete <spec>` removes precisely the rule the admin is
//!   looking at — including its `from <source>` clause, which the previous
//!   spec-only delete silently dropped (it could not delete a
//!   source-restricted rule at all, and deleted the *global* rule with the
//!   same port when one existed).

use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub const MANDATORY_SSH_COMMENT: &str = "homevault-mandatory-ssh";
pub const MANDATORY_APP_COMMENT: &str = "homevault-mandatory-app";

/// Upper bound on a single `ufw` invocation.
///
/// `ufw` shells out to `iptables`, which takes the xtables lock: on a busy
/// host, or while another firewall tool holds it, `ufw` can block
/// indefinitely. A `Command::output()` in an async handler would then park a
/// Tokio worker for the rest of the process's life.
pub const UFW_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest rule comment accepted (ufw stores it in `/etc/ufw/user.rules`).
const MAX_COMMENT_LEN: usize = 128;

#[derive(Debug)]
pub enum FirewallError {
    /// The request itself is wrong — answer 400 with the message as-is.
    InvalidInput(String),
    /// `ufw` is not installed, or `sudo` refused to run it.
    Unavailable(String),
    /// `ufw` did not answer within [`UFW_TIMEOUT`].
    Timeout,
    /// `ufw` ran and failed, or reported success without doing the work.
    Failed(String),
}

impl std::fmt::Display for FirewallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FirewallError::InvalidInput(m) => write!(f, "{}", m),
            FirewallError::Unavailable(m) => write!(f, "{}", m),
            FirewallError::Timeout => write!(
                f,
                "ufw did not answer within {} seconds",
                UFW_TIMEOUT.as_secs()
            ),
            FirewallError::Failed(m) => write!(f, "{}", m),
        }
    }
}

impl FirewallError {
    pub fn is_invalid_input(&self) -> bool {
        matches!(self, FirewallError::InvalidInput(_))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FirewallRule {
    pub number: u32,
    pub port: String,
    pub protocol: String,
    pub action: String,
    pub source: Option<String>,
    pub comment: Option<String>,
    pub is_mandatory: bool,
    /// The rule exactly as `ufw show added` printed it, minus the leading
    /// `ufw` and minus its `comment '…'` clause — i.e. the argument string
    /// `ufw delete` takes to remove this rule and nothing else.
    pub spec: String,
}

/// ufw's default-policy triple, as printed by `ufw status verbose`
/// (`Default: deny (incoming), allow (outgoing), disabled (routed)`).
#[derive(Debug, Clone, Serialize)]
pub struct DefaultPolicy {
    pub incoming: String,
    pub outgoing: String,
    pub routed: String,
}

impl Default for DefaultPolicy {
    fn default() -> Self {
        // ufw's own defaults, used when the line cannot be parsed.
        Self {
            incoming: "deny".to_string(),
            outgoing: "allow".to_string(),
            routed: "disabled".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FirewallStatus {
    pub enabled: bool,
    pub default_policy: DefaultPolicy,
    pub rules: Vec<FirewallRule>,
}

/// Outcome of `add_rule`: ufw silently skips a rule that already exists
/// (exit 0, "Skipping adding existing rule"), which the API used to report
/// as a fresh change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddRuleOutcome {
    Added,
    AlreadyPresent,
}

// ---------------------------------------------------------------------------
// Command plumbing
// ---------------------------------------------------------------------------

/// A `sudo ufw` command with the locale forced to `C`.
///
/// `ufw status`'s output is translated (`Status: active` becomes `État :
/// actif` on a French host, etc.), and `get_status()` below parses it for
/// a fixed English string — so on any non-English-locale server the
/// firewall would always read as "disabled" even while active. `sudo`
/// keeps `LANG`/`LANGUAGE`/`LC_*` from its caller's environment (Ubuntu's
/// default `env_keep`), and `LANGUAGE` overrides `LC_ALL`, so both must be
/// pinned here. Rule parsing itself is locale-independent (it reads `ufw
/// show added`'s `ufw allow …` command syntax), but this keeps every call
/// consistent.
fn ufw_command() -> Command {
    let mut cmd = Command::new("sudo");
    cmd.env("LC_ALL", "C")
        .env("LANGUAGE", "")
        .arg("ufw")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    cmd
}

/// Whether the `ufw` binary is installed (mirrors `wireguard_available()`
/// in `src/vpn.rs`).
///
/// Checked via `sudo ufw --version` rather than a bare `Command::new("ufw")`:
/// on Debian/Ubuntu, `ufw` lives in `/usr/sbin`, which is commonly absent
/// from a non-root service account's `PATH` (as `homevault` is) even though
/// `sudo`'s own `secure_path` — used by every other call in this module —
/// always includes it. A bare-PATH check would report "not installed" on a
/// real deployment where `ufw` is present and every actual `sudo ufw ...`
/// call below works fine, permanently hiding the feature from the UI.
///
/// Deliberately **synchronous**: it is used as a `fn() -> bool` probe by
/// `src/dependencies.rs`, and `--version` is a pure read that returns in
/// milliseconds (with `stdin` closed, `sudo` fails immediately rather than
/// waiting for a password). Every call that can block — `status`, `show
/// added`, `enable`, `default`, rule writes — goes through [`run_ufw`],
/// which is async and time-limited.
pub fn ufw_available() -> bool {
    std::process::Command::new("sudo")
        .env("LC_ALL", "C")
        .env("LANGUAGE", "")
        .arg("ufw")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run `sudo ufw <args>` under [`UFW_TIMEOUT`].
async fn run_ufw(args: &[&str]) -> Result<std::process::Output, FirewallError> {
    let mut cmd = ufw_command();
    cmd.args(args);
    match tokio::time::timeout(UFW_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(FirewallError::Unavailable(
                    "ufw is not installed on this server".to_string(),
                ))
            } else {
                Err(FirewallError::Failed(format!("could not run ufw: {}", e)))
            }
        }
        Err(_) => Err(FirewallError::Timeout),
    }
}

/// Run `sudo ufw <args>` and require a zero exit status.
///
/// Returns stdout+stderr so callers can look for ufw's informational
/// messages ("Skipping adding existing rule").
async fn run_ufw_checked(args: &[&str], what: &str) -> Result<String, FirewallError> {
    let output = run_ufw(args).await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(format!("{}{}", stdout, stderr))
    } else {
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        Err(FirewallError::Failed(format!(
            "{}: {}",
            what,
            detail.trim()
        )))
    }
}

// ---------------------------------------------------------------------------
// Input validation
//
// Every one of these values used to be interpolated straight into
// `sudo ufw <action> …`; `action` was the worst, because it is the *verb* of
// the command (`{"action": "delete"}` turned an "add rule" call into a
// delete). The handlers never build an argv from raw client input any more.
// ---------------------------------------------------------------------------

/// `allow` | `deny` | `reject` | `limit` — the four ufw rule verbs.
pub fn validate_action(action: &str) -> Result<String, FirewallError> {
    let action = action.trim().to_ascii_lowercase();
    match action.as_str() {
        "allow" | "deny" | "reject" | "limit" => Ok(action),
        _ => Err(FirewallError::InvalidInput(format!(
            "Action must be one of allow, deny, reject or limit (got '{}')",
            action
        ))),
    }
}

/// `tcp` | `udp` | `any` (the empty string means `any`).
pub fn validate_protocol(protocol: &str) -> Result<String, FirewallError> {
    let protocol = protocol.trim().to_ascii_lowercase();
    match protocol.as_str() {
        "" | "any" => Ok("any".to_string()),
        "tcp" | "udp" => Ok(protocol),
        _ => Err(FirewallError::InvalidInput(format!(
            "Protocol must be tcp, udp or any (got '{}')",
            protocol
        ))),
    }
}

/// A single port (`443`) or an ascending range (`1000:2000`).
///
/// The protocol belongs in its own field: a value like `22/tcp` used to be
/// accepted and then concatenated again (`22/tcp/tcp`), which ufw rejected
/// with an error nobody could act on.
pub fn validate_port(port: &str) -> Result<String, FirewallError> {
    let port = port.trim();
    if port.is_empty() {
        return Err(FirewallError::InvalidInput("Port is required".to_string()));
    }
    if let Some((start, end)) = port.split_once(':') {
        let start = port_number(start)?;
        let end = port_number(end)?;
        if start >= end {
            return Err(FirewallError::InvalidInput(format!(
                "Port range '{}' must be ascending",
                port
            )));
        }
        return Ok(format!("{}:{}", start, end));
    }
    Ok(port_number(port)?.to_string())
}

fn port_number(value: &str) -> Result<u16, FirewallError> {
    let value = value.trim();
    match value.parse::<u32>() {
        Ok(n) if n >= 1 && n <= 65535 => Ok(n as u16),
        _ => Err(FirewallError::InvalidInput(format!(
            "Port '{}' is not a number between 1 and 65535",
            value
        ))),
    }
}

/// An IPv4/IPv6 address or CIDR network, which is what ufw's `from` clause
/// accepts. Anything else (`any`, a hostname, a stray option) is refused:
/// `from any` in particular would silently turn a restricted rule into a
/// global one.
pub fn validate_source(source: &str) -> Result<String, FirewallError> {
    let source = source.trim();
    if source.is_empty() {
        return Err(FirewallError::InvalidInput(
            "Source cannot be empty".to_string(),
        ));
    }
    let (address, prefix) = match source.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (source, None),
    };
    let max_prefix = if address.parse::<std::net::Ipv4Addr>().is_ok() {
        32u8
    } else if address.parse::<std::net::Ipv6Addr>().is_ok() {
        128u8
    } else {
        return Err(FirewallError::InvalidInput(format!(
            "Source '{}' is not an IPv4/IPv6 address or network",
            source
        )));
    };
    if let Some(prefix) = prefix {
        match prefix.trim().parse::<u8>() {
            Ok(n) if n <= max_prefix => {}
            _ => {
                return Err(FirewallError::InvalidInput(format!(
                    "Source '{}' has an invalid prefix length (maximum {})",
                    source, max_prefix
                )))
            }
        }
    }
    Ok(source.to_string())
}

/// Rule comments are stored verbatim by ufw and re-read from `ufw show
/// added`, where they are single-quoted: a `'` in the text would break that
/// round-trip (and make `is_mandatory` unreliable).
pub fn validate_comment(comment: &str) -> Result<String, FirewallError> {
    let comment = comment.trim();
    if comment.is_empty() {
        return Err(FirewallError::InvalidInput(
            "Comment cannot be empty".to_string(),
        ));
    }
    if comment.len() > MAX_COMMENT_LEN {
        return Err(FirewallError::InvalidInput(format!(
            "Comment is longer than {} characters",
            MAX_COMMENT_LEN
        )));
    }
    if comment.contains('\'') || comment.chars().any(|c| c.is_control()) {
        return Err(FirewallError::InvalidInput(
            "Comment cannot contain quotes or control characters".to_string(),
        ));
    }
    Ok(comment.to_string())
}

/// `allow` | `deny` | `reject` for a default policy.
pub fn validate_default_policy(policy: &str) -> Result<String, FirewallError> {
    let policy = policy.trim().to_ascii_lowercase();
    match policy.as_str() {
        "allow" | "deny" | "reject" => Ok(policy),
        _ => Err(FirewallError::InvalidInput(format!(
            "Default policy must be allow, deny or reject (got '{}')",
            policy
        ))),
    }
}

/// `incoming` | `outgoing` | `routed`.
pub fn validate_default_direction(direction: &str) -> Result<String, FirewallError> {
    let direction = direction.trim().to_ascii_lowercase();
    match direction.as_str() {
        "incoming" | "in" => Ok("incoming".to_string()),
        "outgoing" | "out" => Ok("outgoing".to_string()),
        "routed" => Ok("routed".to_string()),
        _ => Err(FirewallError::InvalidInput(format!(
            "Direction must be incoming, outgoing or routed (got '{}')",
            direction
        ))),
    }
}

// ---------------------------------------------------------------------------
// Reading live state
// ---------------------------------------------------------------------------

/// Live firewall status: whether ufw is enabled, its default policy and its
/// current rule set.
///
/// Deliberately reads two separate `ufw` views rather than just `status
/// numbered`: `ufw status` only lists the active rule table while ufw is
/// *enabled* — verified live, a rule added while ufw is disabled (the
/// common case: an admin configuring rules before ever turning the
/// firewall on) is completely invisible from `status numbered`'s output,
/// even though it's genuinely persisted and will apply the moment ufw is
/// enabled. `ufw show added` lists the configured rule set regardless of
/// enabled state, so it's used as the source of truth for the rule list;
/// `ufw status verbose` (unnumbered) provides the enabled flag *and* the
/// default policy. Both need root, delegated via the sudoers rule configured
/// in `src/setup.rs::configure_sudoers()`.
pub async fn get_status() -> Result<FirewallStatus, FirewallError> {
    let status_output = run_ufw(&["status", "verbose"]).await?;
    if !status_output.status.success() {
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        return Err(FirewallError::Failed(format!(
            "ufw status failed: {}",
            stderr.trim()
        )));
    }
    let status_text = String::from_utf8_lossy(&status_output.stdout);
    let enabled = status_text
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("Status: active"));
    let default_policy = parse_default_policy(&status_text).unwrap_or_default();

    let added_output = run_ufw(&["show", "added"]).await?;
    if !added_output.status.success() {
        let stderr = String::from_utf8_lossy(&added_output.stderr);
        return Err(FirewallError::Failed(format!(
            "ufw show added failed: {}",
            stderr.trim()
        )));
    }
    let added_text = String::from_utf8_lossy(&added_output.stdout);
    let rules = added_text
        .lines()
        .filter(|l| l.trim().starts_with("ufw "))
        .enumerate()
        .filter_map(|(i, l)| parse_added_rule_line(l, (i + 1) as u32))
        .collect();

    Ok(FirewallStatus {
        enabled,
        default_policy,
        rules,
    })
}

/// Parse the `Default: deny (incoming), allow (outgoing), disabled (routed)`
/// line of `ufw status verbose`.
///
/// Without it the page could not answer the one question that makes a rule
/// list meaningful — what happens to traffic no rule matches — and enabling
/// with `allow (incoming)` (the firewall is then a no-op) looked exactly
/// like enabling with `deny (incoming)`.
pub fn parse_default_policy(status_verbose: &str) -> Option<DefaultPolicy> {
    let line = status_verbose
        .lines()
        .find(|l| l.trim_start().starts_with("Default:"))?;
    let rest = line.trim_start().trim_start_matches("Default:").trim();
    if rest.is_empty() {
        return None;
    }
    let mut policy = DefaultPolicy::default();
    for part in rest.split(',') {
        let part = part.trim();
        let Some((value, direction)) = part.split_once('(') else {
            continue;
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        match direction.trim_end_matches(')').trim() {
            "incoming" => policy.incoming = value,
            "outgoing" => policy.outgoing = value,
            "routed" => policy.routed = value,
            _ => {}
        }
    }
    Some(policy)
}

/// Parses a single `ufw allow|deny|reject|limit … [comment 'x']` line as
/// emitted by `ufw show added`.
///
/// Covers the simple form (`ufw allow 22/tcp`), the source-restricted form
/// produced by [`add_rule`] (`ufw allow from <ip> to any port <port> proto
/// <proto>`), interface-scoped rules added from the command line (`ufw allow
/// in on eth0 to any port 80 proto tcp`), rules with no port at all (`ufw
/// allow from 1.2.3.4`) and application profiles (`ufw allow OpenSSH`).
/// `spec` keeps the original tokens so [`delete_rule`] can remove exactly
/// this rule.
fn parse_added_rule_line(line: &str, number: u32) -> Option<FirewallRule> {
    let line = line.trim();
    let rest = line.strip_prefix("ufw ")?;

    // `ufw show added` appends `comment 'text'` last. Split it off *before*
    // tokenising so a comment containing a keyword ("to", "port", "any")
    // can never be read as part of the rule. `find` (not `rfind`) keeps the
    // first clause, which is the real one even if the text repeats it.
    let (spec, comment) = match rest.find(" comment ") {
        Some(idx) => {
            let after = rest[idx + " comment ".len()..].trim();
            (
                rest[..idx].trim(),
                Some(after.trim_matches('\'').to_string()),
            )
        }
        None => (rest.trim(), None),
    };
    if spec.is_empty() {
        return None;
    }

    let tokens: Vec<&str> = spec.split_whitespace().collect();
    let action = tokens.first()?.to_ascii_uppercase();

    let mut port: Option<String> = None;
    let mut protocol: Option<String> = None;
    let mut source: Option<String> = None;
    let mut simple_port_spec: Option<String> = None;

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i] {
            "in" | "out" => {
                i += 1;
                if tokens.get(i) == Some(&"on") {
                    i += 2; // `on <iface>`
                }
            }
            "on" => i += 2,
            "from" => {
                source = tokens.get(i + 1).map(|s| s.to_string());
                i += 2;
            }
            "to" => i += 2,
            "port" => {
                port = tokens.get(i + 1).map(|s| s.to_string());
                i += 2;
            }
            "proto" => {
                protocol = tokens.get(i + 1).map(|s| s.to_string());
                i += 2;
            }
            other => {
                if simple_port_spec.is_none() {
                    simple_port_spec = Some(other.to_string());
                }
                i += 1;
            }
        }
    }

    let (port, protocol) = match (port, simple_port_spec) {
        // `… to any port 5900 proto tcp`
        (Some(port), _) => (port, protocol.unwrap_or_else(|| "any".to_string())),
        // `allow 22/tcp`
        (None, Some(spec)) => match spec.split_once('/') {
            Some((port, proto)) => (port.to_string(), proto.to_string()),
            None => (spec, protocol.unwrap_or_else(|| "any".to_string())),
        },
        // `allow from 1.2.3.4`, `deny out on eth0`: no port at all.
        (None, None) => (String::new(), protocol.unwrap_or_else(|| "any".to_string())),
    };

    let is_mandatory = comment
        .as_deref()
        .map(|c| c == MANDATORY_SSH_COMMENT || c == MANDATORY_APP_COMMENT)
        .unwrap_or(false);

    Some(FirewallRule {
        number,
        port,
        protocol,
        action,
        source,
        comment,
        is_mandatory,
        spec: spec.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Add an allow/deny/reject/limit rule. `protocol` of `"any"` omits the
/// `/proto` suffix (applies to both tcp and udp, matching ufw's own
/// convention). Every argument is validated first.
pub async fn add_rule(
    port: &str,
    protocol: &str,
    action: &str,
    source: Option<&str>,
    comment: &str,
) -> Result<AddRuleOutcome, FirewallError> {
    let action = validate_action(action)?;
    let protocol = validate_protocol(protocol)?;
    let port = validate_port(port)?;
    let source = match source.map(str::trim).filter(|s| !s.is_empty()) {
        Some(source) => Some(validate_source(source)?),
        None => None,
    };
    let comment = validate_comment(comment)?;

    let mut args: Vec<String> = vec![action];
    match &source {
        Some(source) => {
            args.extend([
                "from".to_string(),
                source.clone(),
                "to".to_string(),
                "any".to_string(),
                "port".to_string(),
                port.clone(),
            ]);
            if protocol != "any" {
                args.extend(["proto".to_string(), protocol]);
            }
        }
        None => args.push(if protocol == "any" {
            port
        } else {
            format!("{}/{}", port, protocol)
        }),
    }
    args.extend(["comment".to_string(), comment]);

    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_ufw_checked(&args, "adding the rule").await?;
    if output.to_ascii_lowercase().contains("skipping") {
        Ok(AddRuleOutcome::AlreadyPresent)
    } else {
        Ok(AddRuleOutcome::Added)
    }
}

/// Delete a rule by its **live spec** ([`FirewallRule::spec`]).
///
/// `spec` is never client input: the handler matches the request against the
/// rules returned by [`get_status`] and passes the matching rule's own spec,
/// so the argv is the exact command `ufw show added` printed. That is what
/// makes a source-restricted rule deletable at all (the old
/// `delete <action> <port>/<proto>` form silently missed it) and what keeps
/// the delete from hitting a *different* rule with the same port.
pub async fn delete_rule(spec: &str) -> Result<(), FirewallError> {
    let tokens: Vec<&str> = spec.split_whitespace().collect();
    let Some(action) = tokens.first() else {
        return Err(FirewallError::InvalidInput(
            "Rule spec is empty".to_string(),
        ));
    };
    // Belt and braces: the spec comes from our own parser, but it must still
    // look like a rule (verb first, no comment clause) before it becomes
    // `ufw delete …`.
    validate_action(action)?;
    if tokens.iter().any(|t| *t == "comment" || t.starts_with("--")) {
        return Err(FirewallError::InvalidInput(
            "Rule spec is not a plain ufw rule".to_string(),
        ));
    }

    let mut args: Vec<&str> = vec!["delete"];
    args.extend(tokens);
    run_ufw_checked(&args, "deleting the rule").await?;
    Ok(())
}

/// Idempotently (re-)assert the rules that must always be present:
/// HomeVault's own port and **every** SSH port sshd may listen on. Safe to
/// call repeatedly (ufw skips duplicate rules rather than erroring).
pub async fn ensure_mandatory_rules(
    ssh_ports: &[u16],
    app_port: u16,
) -> Result<(), FirewallError> {
    for port in ssh_ports {
        add_rule(
            &port.to_string(),
            "tcp",
            "allow",
            None,
            MANDATORY_SSH_COMMENT,
        )
        .await?;
    }
    add_rule(
        &app_port.to_string(),
        "tcp",
        "allow",
        None,
        MANDATORY_APP_COMMENT,
    )
    .await?;
    Ok(())
}

/// Enable the firewall and **verify** it.
///
/// `ufw --force enable` exits 0 even when iptables refused to load ("ERROR:
/// problem running iptables"), and the handler used to trust that exit code,
/// record `enabled = true` and report success — on a host where the firewall
/// was not actually filtering anything. The only trustworthy answer is
/// re-reading the live state. If the firewall came up without its mandatory
/// allow rules (so the admin would be locked out of SSH/HomeVault), it is
/// turned back off and the call fails with an explanation, rather than
/// leaving the appliance unreachable.
pub async fn enable_verified(
    ssh_ports: &[u16],
    app_port: u16,
) -> Result<FirewallStatus, FirewallError> {
    let was_enabled = get_status().await.map(|s| s.enabled).unwrap_or(false);

    ensure_mandatory_rules(ssh_ports, app_port).await?;
    run_ufw_checked(&["--force", "enable"], "enabling the firewall").await?;

    let status = get_status().await?;
    if !status.enabled {
        return Err(FirewallError::Failed(
            "ufw reported success but the firewall is still inactive".to_string(),
        ));
    }

    let has = |comment: &str, port: &str| {
        status
            .rules
            .iter()
            .any(|r| r.comment.as_deref() == Some(comment) && r.port == port)
    };
    let mut missing: Vec<String> = ssh_ports
        .iter()
        .filter(|p| !has(MANDATORY_SSH_COMMENT, &p.to_string()))
        .map(|p| format!("SSH ({}/tcp)", p))
        .collect();
    if !has(MANDATORY_APP_COMMENT, &app_port.to_string()) {
        missing.push(format!("HomeVault ({}/tcp)", app_port));
    }

    if !missing.is_empty() {
        let message = format!(
            "the firewall is active but its mandatory allow rule(s) for {} are missing, so \
             enabling it would lock you out",
            missing.join(", ")
        );
        if !was_enabled {
            let _ = run_ufw_checked(&["disable"], "rolling back").await;
        }
        return Err(FirewallError::Failed(format!(
            "{}; the firewall was {}",
            message,
            if was_enabled {
                "left as it was"
            } else {
                "turned back off"
            }
        )));
    }

    Ok(status)
}

/// Disable the firewall and verify that it really is off.
pub async fn disable() -> Result<(), FirewallError> {
    run_ufw_checked(&["disable"], "disabling the firewall").await?;
    let status = get_status().await?;
    if status.enabled {
        return Err(FirewallError::Failed(
            "ufw reported success but the firewall is still active".to_string(),
        ));
    }
    Ok(())
}

/// Change one of ufw's default policies (`ufw default <policy> <direction>`).
pub async fn set_default_policy(direction: &str, policy: &str) -> Result<(), FirewallError> {
    let direction = validate_default_direction(direction)?;
    let policy = validate_default_policy(policy)?;
    run_ufw_checked(
        &["default", &policy, &direction],
        "changing the default policy",
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Port detection
// ---------------------------------------------------------------------------

/// Best-effort parse of `Port` directives from one sshd config file.
///
/// Comments are ignored and the value may be followed by a comment
/// (`Port 2222 # moved off 22`). sshd itself accepts several `Port` lines —
/// it listens on all of them — so every value is kept.
pub fn parse_ssh_ports(contents: &str) -> Vec<u16> {
    contents
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("Port"))
        // `Port` must be followed by whitespace: `Ports 22` is not a directive.
        .filter(|rest| rest.starts_with(' ') || rest.starts_with('\t'))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .collect()
}

/// Every SSH port sshd may listen on.
///
/// `/etc/ssh/sshd_config` alone is not enough: Ubuntu/Debian ship
/// `sshd_config.d/*.conf` drop-ins and `Include` them from the top of the
/// main file, so a `Port 2222` set there is the port SSH actually uses while
/// the main file still says 22 (or says nothing). Protecting only port 22
/// then locks the admin out on the first `ufw enable` — the exact scenario
/// this module exists to prevent. Reading the union of the main file and
/// every drop-in errs towards opening a port that may be unused, never
/// towards closing the one that is.
pub fn detect_ssh_ports() -> Vec<u16> {
    let mut ports = BTreeSet::new();
    for path in sshd_config_paths() {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            ports.extend(parse_ssh_ports(&contents));
        }
    }
    if ports.is_empty() {
        // Unset/unreadable: SSH's standard port.
        ports.insert(22);
    }
    ports.into_iter().collect()
}

/// The main sshd config first, then its `*.conf` drop-ins in lexical order.
fn sshd_config_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/etc/ssh/sshd_config")];
    if let Ok(entries) = std::fs::read_dir("/etc/ssh/sshd_config.d") {
        let mut drop_ins: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("conf"))
            .collect();
        drop_ins.sort();
        paths.extend(drop_ins);
    }
    paths
}

/// The first SSH port — kept for the settings cache and the preview's
/// `ssh_port` field. [`detect_ssh_ports`] is the authoritative list.
pub fn detect_ssh_port() -> u16 {
    detect_ssh_ports().first().copied().unwrap_or(22)
}

/// HomeVault's own listen port — reuses the exact same resolution the
/// server itself binds to (`PORT` env var, default 8080), never a
/// client-supplied value.
pub fn detect_app_port() -> u16 {
    crate::config::Settings::load().port
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests for the argument validators, the `ufw` output
    //! parsers and the mandatory-rule detection. Everything that shells out
    //! via `sudo ufw` needs the real host and is covered by
    //! `tests/e2e/security/security.spec.ts` /
    //! `tests/security_integration.rs`.

    use super::*;

    // -- rule line parsing ------------------------------------------------

    #[test]
    fn test_parse_simple_allow_rule() {
        let r = parse_added_rule_line("ufw allow 443/tcp", 1).unwrap();
        assert_eq!(r.action, "ALLOW");
        assert_eq!(r.port, "443");
        assert_eq!(r.protocol, "tcp");
        assert_eq!(r.source, None);
        assert!(!r.is_mandatory);
        assert_eq!(r.number, 1);
        assert_eq!(r.spec, "allow 443/tcp");
    }

    #[test]
    fn test_parse_deny_rule() {
        let r = parse_added_rule_line("ufw deny 23/tcp", 4).unwrap();
        assert_eq!(r.action, "DENY");
        assert_eq!(r.port, "23");
    }

    #[test]
    fn test_parse_rule_without_protocol_suffix() {
        // `ufw allow 8080` (no `/proto`) applies to any protocol.
        let r = parse_added_rule_line("ufw allow 8080", 2).unwrap();
        assert_eq!(r.port, "8080");
        assert_eq!(r.protocol, "any");
    }

    #[test]
    fn test_parse_source_restricted_rule() {
        let line = "ufw allow from 192.168.1.0/24 to any port 5900 proto tcp";
        let r = parse_added_rule_line(line, 3).unwrap();
        assert_eq!(r.action, "ALLOW");
        assert_eq!(r.source.as_deref(), Some("192.168.1.0/24"));
        assert_eq!(r.port, "5900");
        assert_eq!(r.protocol, "tcp");
        // The spec keeps the `from` clause: dropping it is what made a
        // source-restricted rule impossible to delete.
        assert_eq!(
            r.spec,
            "allow from 192.168.1.0/24 to any port 5900 proto tcp"
        );
    }

    #[test]
    fn test_parse_interface_scoped_rule() {
        // Added from the CLI: `ufw allow in on eth0 to any port 80 proto tcp`.
        let r = parse_added_rule_line("ufw allow in on eth0 to any port 80 proto tcp", 1).unwrap();
        assert_eq!(r.action, "ALLOW");
        assert_eq!(r.port, "80");
        assert_eq!(r.protocol, "tcp");
        assert_eq!(r.source, None);
    }

    #[test]
    fn test_parse_source_only_rule_has_no_port() {
        // `ufw allow from 1.2.3.4` allows every port from one host.
        let r = parse_added_rule_line("ufw allow from 1.2.3.4", 1).unwrap();
        assert_eq!(r.port, "");
        assert_eq!(r.source.as_deref(), Some("1.2.3.4"));
        assert_eq!(r.protocol, "any");
    }

    #[test]
    fn test_parse_application_profile_rule() {
        let r = parse_added_rule_line("ufw allow OpenSSH", 1).unwrap();
        assert_eq!(r.port, "OpenSSH");
        assert_eq!(r.spec, "allow OpenSSH");
    }

    #[test]
    fn test_parse_limit_rule() {
        let r = parse_added_rule_line("ufw limit 2222/tcp", 1).unwrap();
        assert_eq!(r.action, "LIMIT");
        assert_eq!(r.port, "2222");
    }

    #[test]
    fn test_parse_rule_with_comment() {
        let line = "ufw allow 22/tcp comment 'homevault-mandatory-ssh'";
        let r = parse_added_rule_line(line, 1).unwrap();
        assert_eq!(r.port, "22");
        assert_eq!(r.comment.as_deref(), Some(MANDATORY_SSH_COMMENT));
        assert!(
            r.is_mandatory,
            "the mandatory-ssh comment must flag the rule as protected"
        );
        // The comment is not part of the delete spec.
        assert_eq!(r.spec, "allow 22/tcp");
    }

    #[test]
    fn test_parse_mandatory_app_rule_is_protected() {
        let line = "ufw allow 8080/tcp comment 'homevault-mandatory-app'";
        let r = parse_added_rule_line(line, 2).unwrap();
        assert!(r.is_mandatory);
    }

    #[test]
    fn test_parse_custom_comment_is_not_mandatory() {
        let line = "ufw allow 443/tcp comment 'homevault-custom-allow-443'";
        let r = parse_added_rule_line(line, 1).unwrap();
        assert_eq!(r.comment.as_deref(), Some("homevault-custom-allow-443"));
        assert!(
            !r.is_mandatory,
            "a non-mandatory comment must never protect a rule from deletion"
        );
    }

    #[test]
    fn test_parse_comment_text_cannot_be_read_as_a_rule_keyword() {
        // A comment containing `to any port` must not shift the rule's fields.
        let line = "ufw allow 443/tcp comment 'to any port 22 proto tcp'";
        let r = parse_added_rule_line(line, 1).unwrap();
        assert_eq!(r.port, "443");
        assert_eq!(r.protocol, "tcp");
        assert_eq!(r.source, None);
        assert_eq!(r.comment.as_deref(), Some("to any port 22 proto tcp"));
    }

    #[test]
    fn test_parse_ignores_non_rule_lines() {
        assert!(parse_added_rule_line(
            "Added user rules (see 'ufw status' for running firewall):",
            1
        )
        .is_none());
        assert!(parse_added_rule_line("", 1).is_none());
        assert!(parse_added_rule_line("Status: active", 1).is_none());
    }

    // -- default policy ---------------------------------------------------

    #[test]
    fn test_parse_default_policy_from_verbose_status() {
        let verbose = "Status: active\nLogging: on (low)\nDefault: deny (incoming), allow (outgoing), disabled (routed)\nNew profiles: skip\n";
        let p = parse_default_policy(verbose).unwrap();
        assert_eq!(p.incoming, "deny");
        assert_eq!(p.outgoing, "allow");
        assert_eq!(p.routed, "disabled");
    }

    #[test]
    fn test_parse_default_policy_reject_incoming() {
        let verbose = "Status: inactive\nDefault: reject (incoming), allow (outgoing), disabled (routed)\n";
        let p = parse_default_policy(verbose).unwrap();
        assert_eq!(p.incoming, "reject");
    }

    #[test]
    fn test_parse_default_policy_missing_line_falls_back() {
        assert!(parse_default_policy("Status: active\n").is_none());
        let fallback = DefaultPolicy::default();
        assert_eq!(fallback.incoming, "deny");
        assert_eq!(fallback.outgoing, "allow");
        assert_eq!(fallback.routed, "disabled");
    }

    // -- port validation --------------------------------------------------

    #[test]
    fn test_validate_port_accepts_single_ports_and_ranges() {
        assert_eq!(validate_port("443").unwrap(), "443");
        assert_eq!(validate_port("  8080 ").unwrap(), "8080");
        assert_eq!(validate_port("1").unwrap(), "1");
        assert_eq!(validate_port("65535").unwrap(), "65535");
        assert_eq!(validate_port("1000:2000").unwrap(), "1000:2000");
    }

    #[test]
    fn test_validate_port_rejects_out_of_range_and_non_numeric() {
        for bad in ["", "   ", "0", "65536", "-1", "abc", "22/tcp", "22 33", "1:"] {
            assert!(
                validate_port(bad).is_err(),
                "'{}' should be refused",
                bad
            );
        }
    }

    #[test]
    fn test_validate_port_rejects_descending_or_empty_range() {
        assert!(validate_port("2000:1000").is_err());
        assert!(validate_port("1000:1000").is_err());
        assert!(validate_port("0:100").is_err());
        assert!(validate_port("100:70000").is_err());
    }

    // -- action / protocol ------------------------------------------------

    #[test]
    fn test_validate_action_accepts_the_four_ufw_verbs() {
        for action in ["allow", "deny", "reject", "limit", "ALLOW", " Deny "] {
            assert!(validate_action(action).is_ok(), "{action}");
        }
        assert_eq!(validate_action("ALLOW").unwrap(), "allow");
    }

    #[test]
    fn test_validate_action_rejects_command_verbs_and_options() {
        // `action` is the *verb* of `sudo ufw <action> …`: anything but the
        // four rule verbs would run a different ufw command entirely.
        for bad in [
            "delete",
            "reset",
            "enable",
            "disable",
            "--force",
            "-h",
            "allow delete",
            "",
        ] {
            assert!(validate_action(bad).is_err(), "'{}' should be refused", bad);
        }
    }

    #[test]
    fn test_validate_protocol() {
        assert_eq!(validate_protocol("tcp").unwrap(), "tcp");
        assert_eq!(validate_protocol("UDP").unwrap(), "udp");
        assert_eq!(validate_protocol("any").unwrap(), "any");
        assert_eq!(validate_protocol("").unwrap(), "any");
        for bad in ["icmp", "tcp/udp", "tcp udp", "6"] {
            assert!(validate_protocol(bad).is_err(), "'{}'", bad);
        }
    }

    // -- source validation ------------------------------------------------

    #[test]
    fn test_validate_source_accepts_addresses_and_networks() {
        for good in [
            "192.168.1.10",
            "192.168.1.0/24",
            "0.0.0.0/0",
            "::1",
            "fd00::/8",
            " 10.0.0.5 ",
        ] {
            assert!(validate_source(good).is_ok(), "'{}' should be accepted", good);
        }
    }

    #[test]
    fn test_validate_source_rejects_any_and_malformed_values() {
        // `from any` would turn a restricted rule into a global one.
        for bad in [
            "",
            "   ",
            "any",
            "localhost",
            "1.2.3",
            "192.168.1.0/33",
            "::1/129",
            "1.2.3.4/8/8",
            "1.2.3.4 to any",
        ] {
            assert!(validate_source(bad).is_err(), "'{}' should be refused", bad);
        }
    }

    // -- comment validation -----------------------------------------------

    #[test]
    fn test_validate_comment_rejects_quotes_and_control_characters() {
        assert!(validate_comment("homevault-custom-allow-443").is_ok());
        assert!(validate_comment("").is_err());
        // A quote would break `ufw show added`'s single-quoted round-trip.
        assert!(validate_comment("it's mine").is_err());
        assert!(validate_comment("two\nlines").is_err());
        assert!(validate_comment(&"x".repeat(MAX_COMMENT_LEN + 1)).is_err());
    }

    // -- default policy validation ----------------------------------------

    #[test]
    fn test_validate_default_policy_and_direction() {
        assert_eq!(validate_default_policy("DENY").unwrap(), "deny");
        assert!(validate_default_policy("disabled").is_err());
        assert!(validate_default_policy("").is_err());

        assert_eq!(validate_default_direction("in").unwrap(), "incoming");
        assert_eq!(validate_default_direction("outgoing").unwrap(), "outgoing");
        assert_eq!(validate_default_direction("routed").unwrap(), "routed");
        assert!(validate_default_direction("forward").is_err());
        assert!(validate_default_direction("").is_err());
    }

    // -- sshd port detection ----------------------------------------------

    #[test]
    fn test_parse_ssh_ports_reads_directives_and_ignores_comments() {
        let config = "#Port 22\nPort 2222\n  Port 2200  # secondary\nPorts 99\n";
        assert_eq!(parse_ssh_ports(config), vec![2222, 2200]);
    }

    #[test]
    fn test_parse_ssh_ports_ignores_invalid_and_zero() {
        let config = "Port abc\nPort 0\nPort 70000\nPort\n";
        assert!(parse_ssh_ports(config).is_empty());
    }

    #[test]
    fn test_detect_ssh_ports_never_returns_an_empty_list() {
        // Either the real drop-ins/main file, or the documented fallback 22.
        let ports = detect_ssh_ports();
        assert!(!ports.is_empty(), "at least one SSH port must be protected");
        assert!(ports.iter().all(|p| *p > 0));
        // Sorted and deduplicated.
        let mut sorted = ports.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ports, sorted);
        // The single-port helper agrees with the list.
        assert_eq!(detect_ssh_port(), ports[0]);
    }
}
