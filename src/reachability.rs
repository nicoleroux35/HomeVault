/* =============================================================================
 * File          : reachability.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The reachability advisor: one report that says whether this box can be
 *   reached from outside, in the order the pieces depend on each other
 *   (certificate, name, port), each check with what is wrong and what to do.
 *   Everything is derived from settings and the installed certificate — no
 *   external probing, so the report is deterministic and testable.
 *
 * Dependencies:
 *   - crate::tls, crate::acme, crate::ddns
 *
 * Usage:
 *   handlers::reachability serves it; docs/guides/reachability.md explains
 *   each check.
 * =============================================================================
 */

//! The reachability advisor (see docs/guides/reachability.md).
//!
//! The report is deliberately *not* a score: every entry names a concrete
//! condition and a concrete fix, in dependency order. "Your certificate is
//! self-signed" only matters once HTTPS is on, and "port 80 is closed" only
//! matters for the HTTP-01 challenge, so a flat list would give the wrong
//! advice first.

use serde::{Deserialize, Serialize};

use crate::acme::{AcmeSettings, AcmeStatus, ChallengeType};
use crate::ddns::{DdnsSettings, DdnsStatus};
use crate::tls::{TlsMode, TlsSettings};

/// How a single check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    /// Nothing to do.
    Ok,
    /// Works, but externally reachable clients will hit a rough edge.
    Warn,
    /// External access does not work in this configuration.
    Fail,
}

impl CheckState {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckState::Ok => "ok",
            CheckState::Warn => "warn",
            CheckState::Fail => "fail",
        }
    }
}

/// One entry of the report. `id` is stable API (the UI keys its hints off it),
/// `fix` is the actionable sentence shown under the detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: &'static str,
    pub state: CheckState,
    pub detail: String,
    pub fix: Option<String>,
}

impl Check {
    fn ok(id: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            state: CheckState::Ok,
            detail: detail.into(),
            fix: None,
        }
    }

    fn warn(id: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            id,
            state: CheckState::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(id: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            id,
            state: CheckState::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

/// Everything the assessment needs, gathered once so the logic itself is a
/// pure function of its inputs.
pub struct Inputs<'a> {
    pub tls: &'a TlsSettings,
    pub acme: &'a AcmeSettings,
    pub ddns: &'a DdnsSettings,
    /// Port the plain-HTTP listener uses.
    pub http_port: u16,
    /// Whether the certificate in use is self-signed, when one is readable.
    pub cert_self_signed: Option<bool>,
    /// Days left on the certificate in use, when one is readable.
    pub cert_days_remaining: Option<i64>,
    /// The address this box would publish, when it has one.
    pub current_ip: Option<String>,
}

/// Build the report. Order is the order an admin should work through it.
pub fn assess(inputs: &Inputs) -> Vec<Check> {
    let mut checks = Vec::new();

    // 1. Is anything listening over TLS at all?
    if !inputs.tls.enabled {
        checks.push(Check::fail(
            "tls.enabled",
            "HTTPS is off, so the login form travels in clear text over the network",
            "enable HTTPS/TLS in Settings → Security",
        ));
    } else {
        checks.push(Check::ok(
            "tls.enabled",
            format!("HTTPS is on, port {}", inputs.tls.effective_https_port()),
        ));
    }

    // 2. Is the certificate one a browser will accept?
    if inputs.tls.enabled {
        match (inputs.cert_self_signed, inputs.cert_days_remaining) {
            (Some(true), _) => checks.push(Check::warn(
                "tls.trusted_certificate",
                "the certificate is self-signed, so every client shows a warning",
                "enable certificate automation to get a publicly trusted certificate",
            )),
            (Some(false), Some(days)) if days <= 0 => checks.push(Check::fail(
                "tls.trusted_certificate",
                "the certificate has expired",
                "renew the certificate (Settings → Security → HTTPS/TLS)",
            )),
            (Some(false), Some(days)) if days <= crate::acme::RENEW_WHEN_DAYS_LEFT => {
                checks.push(Check::warn(
                    "tls.trusted_certificate",
                    format!("the certificate expires in {days} day(s)"),
                    "check that renewal is running, or renew it by hand",
                ))
            }
            (Some(false), _) => checks.push(Check::ok(
                "tls.trusted_certificate",
                "the certificate is issued by a certificate authority",
            )),
            (None, _) => checks.push(Check::warn(
                "tls.trusted_certificate",
                "the certificate in use cannot be read",
                "regenerate it, or point HTTPS/TLS at a readable pair",
            )),
        }
    }

    // 3. Will the certificate stay valid without an admin watching?
    if !inputs.acme.enabled {
        checks.push(Check::warn(
            "acme.automation",
            "certificate automation is off, so renewal is manual",
            "enable it so the certificate is renewed a month before it expires",
        ));
    } else {
        match inputs.acme.status {
            AcmeStatus::Error => checks.push(Check::fail(
                "acme.automation",
                inputs
                    .acme
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "the last renewal attempt failed".to_string()),
                "fix the reported reason, then issue the certificate again",
            )),
            AcmeStatus::Pending => checks.push(Check::warn(
                "acme.automation",
                "a DNS record is waiting to be created",
                "create the TXT record shown in HTTPS/TLS, then confirm",
            )),
            AcmeStatus::Valid | AcmeStatus::Expiring => checks.push(Check::ok(
                "acme.automation",
                format!(
                    "automatic renewal is on for {}",
                    if inputs.acme.domain.is_empty() {
                        "the configured name"
                    } else {
                        &inputs.acme.domain
                    }
                ),
            )),
            _ => checks.push(Check::warn(
                "acme.automation",
                "automatic renewal is on but no certificate has been issued yet",
                "issue the certificate once from HTTPS/TLS",
            )),
        }
    }

    // 4. Is there a stable name to put in a browser or a certificate?
    if !inputs.ddns.enabled {
        checks.push(Check::warn(
            "ddns.enabled",
            "no dynamic DNS name is configured",
            "add one so the address stays the same when your ISP changes it",
        ));
    } else if inputs.ddns.status == DdnsStatus::Error {
        checks.push(Check::fail(
            "ddns.enabled",
            inputs
                .ddns
                .last_error
                .clone()
                .unwrap_or_else(|| "the last dynamic DNS update failed".to_string()),
            "check the provider token and the name, then send an update",
        ));
    } else if let Some(current) = inputs.current_ip.as_deref() {
        match inputs.ddns.last_ip.as_deref() {
            Some(published) if published == current => checks.push(Check::ok(
                "ddns.enabled",
                format!("{} points at {current}", inputs.ddns.hostname),
            )),
            Some(published) => checks.push(Check::warn(
                "ddns.enabled",
                format!(
                    "{} still holds {published} but this box now uses {current}",
                    inputs.ddns.hostname
                ),
                "send an update now",
            )),
            None => checks.push(Check::warn(
                "ddns.enabled",
                format!("{} has not been updated yet", inputs.ddns.hostname),
                "send an update now",
            )),
        }
    } else {
        checks.push(Check::ok(
            "ddns.enabled",
            format!("{} is configured", inputs.ddns.hostname),
        ));
    }

    // 5. Do the certificate name and the DNS name agree?
    if inputs.acme.enabled && inputs.ddns.enabled {
        let domain = inputs.acme.domain.trim().to_ascii_lowercase();
        let host = inputs.ddns.hostname.trim().to_ascii_lowercase();
        if !domain.is_empty() && !host.is_empty() && domain != host {
            checks.push(Check::warn(
                "names.agree",
                format!("the certificate is for {domain} but the public name is {host}"),
                "use the same name for the certificate and for dynamic DNS",
            ));
        } else if !domain.is_empty() {
            checks.push(Check::ok("names.agree", format!("both use {domain}")));
        }
    }

    // 6. Ports. HTTP-01 needs port 80 *and* the listener on it.
    if inputs.tls.enabled && inputs.tls.effective_https_port() != 443 {
        checks.push(Check::warn(
            "ports.https",
            format!(
                "HTTPS listens on {}, so links must include the port",
                inputs.tls.effective_https_port()
            ),
            "forward port 443 to this port on the router, or run it on 443",
        ));
    }
    if inputs.acme.enabled && inputs.acme.challenge == ChallengeType::Http01 {
        if inputs.http_port != 80 {
            checks.push(Check::warn(
                "ports.http01",
                format!(
                    "the HTTP-01 challenge is answered on port {} but the authority always uses port 80",
                    inputs.http_port
                ),
                "forward port 80 to this port on the router, or use the DNS-01 challenge",
            ));
        } else {
            checks.push(Check::ok(
                "ports.http01",
                "the HTTP-01 challenge is served on port 80",
            ));
        }
    }
    if inputs.tls.mode == TlsMode::Acme && !inputs.tls.enabled {
        checks.push(Check::warn(
            "tls.mode",
            "a certificate is automated but HTTPS itself is off",
            "turn on HTTPS/TLS to serve it",
        ));
    }

    checks
}

/// Worst state in the report, for the one-line summary.
pub fn worst(checks: &[Check]) -> CheckState {
    if checks.iter().any(|c| c.state == CheckState::Fail) {
        CheckState::Fail
    } else if checks.iter().any(|c| c.state == CheckState::Warn) {
        CheckState::Warn
    } else {
        CheckState::Ok
    }
}

/// Count of checks per state, for the summary line.
pub fn counts(checks: &[Check]) -> (usize, usize, usize) {
    (
        checks.iter().filter(|c| c.state == CheckState::Ok).count(),
        checks
            .iter()
            .filter(|c| c.state == CheckState::Warn)
            .count(),
        checks
            .iter()
            .filter(|c| c.state == CheckState::Fail)
            .count(),
    )
}

/// Build the report from what is currently configured and installed.
pub fn current_report() -> Vec<Check> {
    let tls = crate::tls::load_settings();
    let acme = crate::acme::load_settings();
    let ddns = crate::ddns::load_settings();
    let cert_info = crate::tls::certificate_info(&tls);
    assess(&Inputs {
        tls: &tls,
        acme: &acme,
        ddns: &ddns,
        http_port: crate::config::Settings::load().port,
        cert_self_signed: cert_info.error.is_none().then_some(cert_info.self_signed),
        cert_days_remaining: cert_info.days_remaining,
        current_ip: crate::ddns::detect_public_ip(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls(enabled: bool) -> TlsSettings {
        TlsSettings {
            enabled,
            https_port: Some(443),
            ..Default::default()
        }
    }

    fn acme_disabled() -> AcmeSettings {
        AcmeSettings::default()
    }

    fn ddns_disabled() -> DdnsSettings {
        DdnsSettings::default()
    }

    fn run(
        tls: &TlsSettings,
        acme: &AcmeSettings,
        ddns: &DdnsSettings,
        http_port: u16,
    ) -> Vec<Check> {
        assess(&Inputs {
            tls,
            acme,
            ddns,
            http_port,
            cert_self_signed: Some(false),
            cert_days_remaining: Some(80),
            current_ip: Some("203.0.113.7".to_string()),
        })
    }

    fn find<'a>(checks: &'a [Check], id: &str) -> &'a Check {
        checks.iter().find(|c| c.id == id).unwrap_or_else(|| {
            panic!(
                "no check {id} in {:?}",
                checks.iter().map(|c| c.id).collect::<Vec<_>>()
            )
        })
    }

    #[test]
    fn test_https_off_is_the_first_failure() {
        let checks = run(&tls(false), &acme_disabled(), &ddns_disabled(), 8080);
        assert_eq!(checks[0].id, "tls.enabled");
        assert_eq!(checks[0].state, CheckState::Fail);
        assert!(checks[0].fix.as_deref().unwrap().contains("HTTPS"));
        assert_eq!(worst(&checks), CheckState::Fail);
    }

    #[test]
    fn test_a_self_signed_certificate_is_a_warning_not_a_failure() {
        let mut acme = acme_disabled();
        acme.enabled = true;
        acme.status = AcmeStatus::Valid;
        let checks = assess(&Inputs {
            tls: &tls(true),
            acme: &acme,
            ddns: &ddns_disabled(),
            http_port: 8080,
            cert_self_signed: Some(true),
            cert_days_remaining: Some(300),
            current_ip: None,
        });
        let cert = find(&checks, "tls.trusted_certificate");
        assert_eq!(cert.state, CheckState::Warn);
        assert!(cert.detail.contains("self-signed"));
    }

    #[test]
    fn test_an_expired_certificate_is_a_failure() {
        let checks = assess(&Inputs {
            tls: &tls(true),
            acme: &acme_disabled(),
            ddns: &ddns_disabled(),
            http_port: 8080,
            cert_self_signed: Some(false),
            cert_days_remaining: Some(-1),
            current_ip: None,
        });
        assert_eq!(
            find(&checks, "tls.trusted_certificate").state,
            CheckState::Fail
        );
    }

    #[test]
    fn test_a_certificate_inside_the_renewal_window_warns() {
        let checks = assess(&Inputs {
            tls: &tls(true),
            acme: &acme_disabled(),
            ddns: &ddns_disabled(),
            http_port: 8080,
            cert_self_signed: Some(false),
            cert_days_remaining: Some(10),
            current_ip: None,
        });
        let cert = find(&checks, "tls.trusted_certificate");
        assert_eq!(cert.state, CheckState::Warn);
        assert!(cert.detail.contains("10 day"));
    }

    #[test]
    fn test_an_acme_error_is_reported_with_its_reason() {
        let mut acme = acme_disabled();
        acme.enabled = true;
        acme.status = AcmeStatus::Error;
        acme.last_error = Some("Invalid response from http://nas.example.com".to_string());
        let checks = run(&tls(true), &acme, &ddns_disabled(), 8080);
        let entry = find(&checks, "acme.automation");
        assert_eq!(entry.state, CheckState::Fail);
        assert!(entry.detail.contains("Invalid response"));
    }

    #[test]
    fn test_automation_off_is_a_warning() {
        let checks = run(&tls(true), &acme_disabled(), &ddns_disabled(), 8080);
        assert_eq!(find(&checks, "acme.automation").state, CheckState::Warn);
    }

    #[test]
    fn test_ddns_reports_a_stale_address() {
        let ddns = DdnsSettings {
            enabled: true,
            hostname: "home.example.com".to_string(),
            status: DdnsStatus::UpToDate,
            last_ip: Some("198.51.100.1".to_string()),
            ..Default::default()
        };
        let checks = run(&tls(true), &acme_disabled(), &ddns, 8080);
        let entry = find(&checks, "ddns.enabled");
        assert_eq!(entry.state, CheckState::Warn);
        assert!(entry.detail.contains("198.51.100.1"));
        assert!(entry.detail.contains("203.0.113.7"));
    }

    #[test]
    fn test_ddns_up_to_date_is_ok() {
        let ddns = DdnsSettings {
            enabled: true,
            hostname: "home.example.com".to_string(),
            status: DdnsStatus::UpToDate,
            last_ip: Some("203.0.113.7".to_string()),
            ..Default::default()
        };
        let checks = run(&tls(true), &acme_disabled(), &ddns, 8080);
        assert_eq!(find(&checks, "ddns.enabled").state, CheckState::Ok);
    }

    #[test]
    fn test_name_mismatch_between_certificate_and_ddns_warns() {
        let ddns = DdnsSettings {
            enabled: true,
            hostname: "home.example.com".to_string(),
            ..Default::default()
        };
        let mut acme = acme_disabled();
        acme.enabled = true;
        acme.domain = "nas.example.com".to_string();
        acme.status = AcmeStatus::Valid;
        let checks = run(&tls(true), &acme, &ddns, 8080);
        let entry = find(&checks, "names.agree");
        assert_eq!(entry.state, CheckState::Warn);
        assert!(entry.detail.contains("nas.example.com"));
        assert!(entry.detail.contains("home.example.com"));
    }

    #[test]
    fn test_http01_on_a_non_standard_port_is_called_out() {
        let mut acme = acme_disabled();
        acme.enabled = true;
        acme.status = AcmeStatus::Valid;
        acme.challenge = ChallengeType::Http01;
        let checks = run(&tls(true), &acme, &ddns_disabled(), 8080);
        let entry = find(&checks, "ports.http01");
        assert_eq!(entry.state, CheckState::Warn);
        assert!(entry.detail.contains("80"));

        let checks = run(&tls(true), &acme, &ddns_disabled(), 80);
        assert_eq!(find(&checks, "ports.http01").state, CheckState::Ok);
    }

    #[test]
    fn test_dns01_does_not_require_port_80() {
        let mut acme = acme_disabled();
        acme.enabled = true;
        acme.status = AcmeStatus::Valid;
        acme.challenge = ChallengeType::Dns01;
        let checks = run(&tls(true), &acme, &ddns_disabled(), 8080);
        assert!(checks.iter().all(|c| c.id != "ports.http01"));
    }

    #[test]
    fn test_a_non_standard_https_port_is_a_warning() {
        let mut t = tls(true);
        t.https_port = Some(8443);
        let checks = run(&t, &acme_disabled(), &ddns_disabled(), 8080);
        let entry = find(&checks, "ports.https");
        assert_eq!(entry.state, CheckState::Warn);
        assert!(entry.detail.contains("8443"));
    }

    #[test]
    fn test_counts_and_worst_track_the_entries() {
        let checks = run(&tls(true), &acme_disabled(), &ddns_disabled(), 80);
        let (ok, warn, fail) = counts(&checks);
        assert_eq!(ok + warn + fail, checks.len());
        assert_eq!(worst(&checks), CheckState::Warn);
    }
}
