//! Firewall management via `ufw` (Debian/Ubuntu only, matching the OS check
//! already in `installer/install.sh`). Follows the same "live system state
//! is the source of truth" principle as `src/vpn.rs`'s `wg show`: rules are
//! never mirrored in the database, only read live from `ufw status
//! numbered`, so there is no drift between what HomeVault displays and what
//! the kernel is actually enforcing.
//!
//! Self-lockout safety: SSH and HomeVault's own port are always allowed via
//! `ensure_mandatory_rules`, and a rule is only ever treated as "mandatory"
//! (and thus protected from deletion) by matching the rule's own comment
//! string on the live `ufw` output — never from client input — so this
//! can't be bypassed by a raw API call.

use serde::Serialize;
use std::process::Command;

pub const MANDATORY_SSH_COMMENT: &str = "homevault-mandatory-ssh";
pub const MANDATORY_APP_COMMENT: &str = "homevault-mandatory-app";

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
    cmd.env("LC_ALL", "C").env("LANGUAGE", "").arg("ufw");
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
pub fn ufw_available() -> bool {
    ufw_command()
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
}

/// Live firewall status: whether ufw is enabled and its current rule set.
///
/// Deliberately reads two separate `ufw` views rather than just `status
/// numbered`: `ufw status` only lists the active rule table while ufw is
/// *enabled* — verified live, a rule added while ufw is disabled (the
/// common case: an admin configuring rules before ever turning the
/// firewall on) is completely invisible from `status numbered`'s output,
/// even though it's genuinely persisted and will apply the moment ufw is
/// enabled. `ufw show added` lists the configured rule set regardless of
/// enabled state, so it's used as the source of truth for the rule list;
/// `ufw status` (unnumbered) is used only for the enabled/disabled flag.
/// Both need root, delegated via the sudoers rule configured in
/// `src/setup.rs::configure_sudoers()`.
pub fn get_status() -> Result<(bool, Vec<FirewallRule>), String> {
    let status_output = ufw_command()
        .arg("status")
        .output()
        .map_err(|e| format!("Failed to run ufw: {}", e))?;
    if !status_output.status.success() {
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        return Err(format!("ufw status failed: {}", stderr));
    }
    let status_text = String::from_utf8_lossy(&status_output.stdout);
    let enabled = status_text
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("Status: active"));

    let added_output = ufw_command()
        .args(["show", "added"])
        .output()
        .map_err(|e| format!("Failed to run ufw: {}", e))?;
    if !added_output.status.success() {
        let stderr = String::from_utf8_lossy(&added_output.stderr);
        return Err(format!("ufw show added failed: {}", stderr));
    }
    let added_text = String::from_utf8_lossy(&added_output.stdout);
    let rules = added_text
        .lines()
        .filter(|l| l.trim().starts_with("ufw "))
        .enumerate()
        .filter_map(|(i, l)| parse_added_rule_line(l, (i + 1) as u32))
        .collect();

    Ok((enabled, rules))
}

/// Parses a single `ufw allow|deny ... [comment 'x']` line as emitted by
/// `ufw show added`, covering both the simple form (`ufw allow 22/tcp`) and
/// the source-restricted form (`ufw allow from <ip> to any port <port>
/// proto <proto>`) produced by `add_rule` below.
fn parse_added_rule_line(line: &str, number: u32) -> Option<FirewallRule> {
    let line = line.trim();
    let rest = line.strip_prefix("ufw ")?;

    let (spec, comment) = match rest.find("comment ") {
        Some(idx) => {
            let (before, after) = rest.split_at(idx);
            let quoted = after.trim_start_matches("comment ").trim();
            (before.trim(), Some(quoted.trim_matches('\'').to_string()))
        }
        None => (rest, None),
    };

    let tokens: Vec<&str> = spec.split_whitespace().collect();
    let action_token = *tokens.first()?;
    let action = action_token.to_uppercase();

    let (port, protocol, source) = if tokens.get(1) == Some(&"from") {
        let source = tokens.get(2).map(|s| s.to_string());
        let port = tokens
            .iter()
            .position(|t| *t == "port")
            .and_then(|i| tokens.get(i + 1))
            .map(|s| s.to_string())
            .unwrap_or_default();
        let protocol = tokens
            .iter()
            .position(|t| *t == "proto")
            .and_then(|i| tokens.get(i + 1))
            .map(|s| s.to_string())
            .unwrap_or_else(|| "any".to_string());
        (port, protocol, source)
    } else {
        let port_spec = tokens.get(1).copied().unwrap_or_default();
        match port_spec.split_once('/') {
            Some((p, proto)) => (p.to_string(), proto.to_string(), None),
            None => (port_spec.to_string(), "any".to_string(), None),
        }
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
    })
}

/// Add an allow/deny rule. `protocol` of `"any"` omits the `/proto` suffix
/// (applies to both tcp and udp, matching ufw's own convention).
pub fn add_rule(
    port: &str,
    protocol: &str,
    action: &str,
    source: Option<&str>,
    comment: &str,
) -> Result<(), String> {
    let port_spec = if protocol.eq_ignore_ascii_case("any") {
        port.to_string()
    } else {
        format!("{}/{}", port, protocol)
    };

    let mut args: Vec<String> = vec![action.to_string()];
    if let Some(src) = source {
        args.push("from".to_string());
        args.push(src.to_string());
        args.push("to".to_string());
        args.push("any".to_string());
        args.push("port".to_string());
        args.push(port.to_string());
        if !protocol.eq_ignore_ascii_case("any") {
            args.push("proto".to_string());
            args.push(protocol.to_string());
        }
    } else {
        args.push(port_spec);
    }
    args.push("comment".to_string());
    args.push(comment.to_string());

    run_sudo_ufw(&args)
}

/// Delete a rule by its exact spec (action/port/protocol) rather than a
/// numeric index — indices shift between calls, so a stale index race could
/// otherwise delete the wrong rule.
pub fn delete_rule_by_spec(port: &str, protocol: &str, action: &str) -> Result<(), String> {
    let port_spec = if protocol.eq_ignore_ascii_case("any") {
        port.to_string()
    } else {
        format!("{}/{}", port, protocol)
    };
    run_sudo_ufw(&["delete".to_string(), action.to_string(), port_spec])
}

/// Idempotently (re-)assert the two rules that must always be present:
/// HomeVault's own port and SSH. Safe to call repeatedly (ufw skips
/// duplicate rules rather than erroring).
pub fn ensure_mandatory_rules(ssh_port: u16, app_port: u16) -> Result<(), String> {
    add_rule(&ssh_port.to_string(), "tcp", "allow", None, MANDATORY_SSH_COMMENT)?;
    add_rule(&app_port.to_string(), "tcp", "allow", None, MANDATORY_APP_COMMENT)?;
    Ok(())
}

pub fn enable() -> Result<(), String> {
    run_sudo_ufw(&["--force".to_string(), "enable".to_string()])
}

pub fn disable() -> Result<(), String> {
    run_sudo_ufw(&["disable".to_string()])
}

fn run_sudo_ufw(args: &[String]) -> Result<(), String> {
    let output = ufw_command()
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run ufw: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("ufw command failed: {}", stderr))
    }
}

/// Best-effort parse of `/etc/ssh/sshd_config`'s `Port` directive.
/// Defaults to 22 (SSH's standard port) if unset/unreadable/multiple.
pub fn detect_ssh_port() -> u16 {
    let Ok(contents) = std::fs::read_to_string("/etc/ssh/sshd_config") else {
        return 22;
    };
    for line in contents.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Port ") {
            if let Ok(port) = rest.trim().parse::<u16>() {
                return port;
            }
        }
    }
    22
}

/// HomeVault's own listen port — reuses the exact same resolution the
/// server itself binds to (`PORT` env var, default 8080), never a
/// client-supplied value.
pub fn detect_app_port() -> u16 {
    crate::config::Settings::load().port
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests for parsing `ufw show added` output and the
    //! mandatory-rule detection. Everything that shells out via `sudo ufw`
    //! needs the real host and is covered by
    //! `tests/e2e/security/security.spec.ts` /
    //! `tests/security_integration.rs`.

    use super::*;

    #[test]
    fn test_parse_simple_allow_rule() {
        let r = parse_added_rule_line("ufw allow 443/tcp", 1).unwrap();
        assert_eq!(r.action, "ALLOW");
        assert_eq!(r.port, "443");
        assert_eq!(r.protocol, "tcp");
        assert_eq!(r.source, None);
        assert!(!r.is_mandatory);
        assert_eq!(r.number, 1);
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
    }

    #[test]
    fn test_parse_rule_with_comment() {
        let line = "ufw allow 22/tcp comment 'homevault-mandatory-ssh'";
        let r = parse_added_rule_line(line, 1).unwrap();
        assert_eq!(r.port, "22");
        assert_eq!(r.comment.as_deref(), Some(MANDATORY_SSH_COMMENT));
        assert!(r.is_mandatory, "the mandatory-ssh comment must flag the rule as protected");
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
        assert!(!r.is_mandatory, "a non-mandatory comment must never protect a rule from deletion");
    }

    #[test]
    fn test_parse_ignores_non_rule_lines() {
        assert!(parse_added_rule_line("Added user rules (see 'ufw status' for running firewall):", 1).is_none());
        assert!(parse_added_rule_line("", 1).is_none());
        assert!(parse_added_rule_line("Status: active", 1).is_none());
    }

    #[test]
    fn test_detect_ssh_port_returns_a_valid_port() {
        // Either the real `Port` directive from /etc/ssh/sshd_config, or the
        // documented fallback of 22 — never 0.
        let p = detect_ssh_port();
        assert!(p > 0, "SSH port must never be 0");
    }
}
