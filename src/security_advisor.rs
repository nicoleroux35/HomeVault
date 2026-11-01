/* =============================================================================
 * File          : security_advisor.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Read-only Security Advisor (QUICK-WIN QW-10): one answer to "is this box
 *   safely configured?" built by aggregating signals HomeVault already
 *   computes — accounts, 2FA, TLS/ACME, firewall, SMB, shares, snapshots,
 *   backups, updates, plus the QW-05/QW-07 protections. Every check is a pure
 *   function of a gathered `AdvisorFacts`, so both directions are testable
 *   without a live appliance.
 *
 * Dependencies:
 *   - serde, chrono, crate::{auth,tls,acme,firewall,shares,db_backup,snapshot_schedule_repo}
 *
 * Usage:
 *   handlers/security_advisor.rs (GET /security/advisor); the Health screen's
 *   advisor card; see docs/guides/health.md.
 * =============================================================================
 */

//! Security Advisor (see docs/guides/health.md).
//!
//! Health answers "is the machine running?"; the security cards each answer one
//! question about one subsystem. Nobody answered *"is this box safely
//! configured, and what should I do first?"* — which is the question a home user
//! actually has. This module does that, and only that: it **reads** state, it
//! never changes anything.
//!
//! Design rules:
//!
//! 1. **A check that cannot read its signal is never a pass.** Every fact is an
//!    `Option`; an unreadable one becomes [`Severity::Info`] with "could not be
//!    read" as the evidence. A green tick on an unknown state is the one thing
//!    an audit must never produce.
//! 2. **Each finding says what to do and where.** `fix` is a plain sentence and
//!    `deep_link` is the screen that owns the setting, so "TLS is not enabled"
//!    comes with the way to enable it.
//! 3. **The score is arithmetic, not opinion.** Pass and Info (nothing to fix)
//!    are worth 1, Warn ½, Fail 0; the score is the percentage. The counting is
//!    a pure function so the fixture-based test can pin it exactly.
//! 4. **No new plumbing.** Every signal already exists: the checks read the same
//!    services their own screens read, so a finding can never disagree with the
//!    card that owns the setting.

use serde::{Deserialize, Serialize};

/// How bad one finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Nothing to do (a good state, or a protection that is on).
    Pass,
    /// Nothing to fix, but worth knowing (unreadable signal, not applicable).
    Info,
    /// A real weakness, not urgent.
    Warn,
    /// A real exposure.
    Fail,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Pass => "pass",
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Fail => "fail",
        }
    }

    /// Weight in the score: `1.0` (pass/info), `0.5` (warn), `0.0` (fail).
    ///
    /// Info counts as a pass because it means "nothing to fix here" — counting
    /// it as a failure would punish a box for a signal it cannot read.
    pub fn weight(self) -> f64 {
        match self {
            Severity::Pass | Severity::Info => 1.0,
            Severity::Warn => 0.5,
            Severity::Fail => 0.0,
        }
    }
}

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable id (`admin-2fa`, `tls-enabled`, …) — the frontend keys on it and
    /// the tests assert on it, so it is API.
    pub id: String,
    pub severity: Severity,
    /// Plain-English sentence: what is true right now, with the evidence in it.
    pub title: String,
    /// The number/string the verdict is based on.
    pub evidence: String,
    /// What to do, when there is something to do.
    pub fix: Option<String>,
    /// The screen that owns the setting (a `/desktop` app id, or a Settings
    /// category), so the UI can link to it.
    pub link: Option<DeepLink>,
}

/// Where a finding's fix lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepLink {
    /// `settings` | `users` | `shares` | `health` … (`hv-open-app` ids).
    pub app: String,
    /// Settings category, when the target is Settings.
    pub category: Option<String>,
}

impl DeepLink {
    fn settings(category: &str) -> Self {
        Self {
            app: "settings".to_string(),
            category: Some(category.to_string()),
        }
    }

    fn app(app: &str) -> Self {
        Self {
            app: app.to_string(),
            category: None,
        }
    }
}

/// The whole report, as the card renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisorReport {
    /// 0–100, rounded down so "99" never looks like a pass.
    pub score: u8,
    pub counts: ScoreCounts,
    /// Worst first, so the card's first line is the thing to fix.
    pub findings: Vec<Finding>,
    /// RFC 3339, UTC — when this run read the facts.
    pub generated_at: String,
    pub app_version: String,
}

impl AdvisorReport {
    /// The worst severity in the report (`Pass` when everything is fine).
    pub fn worst(&self) -> Severity {
        self.findings
            .iter()
            .map(|f| f.severity)
            .max()
            .unwrap_or(Severity::Pass)
    }

    /// The findings that need action, worst first.
    pub fn actionable(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| matches!(f.severity, Severity::Warn | Severity::Fail))
            .collect()
    }
}

/// How many findings of each severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreCounts {
    pub pass: usize,
    pub info: usize,
    pub warn: usize,
    pub fail: usize,
    pub total: usize,
}

// --------------------------------------------------------------------------
// Facts — every signal is optional, because unreadable must not read as "fine"
// --------------------------------------------------------------------------

/// What the collector managed to read. Every field is `Option`: `None` means
/// "could not be read on this run", never "false".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdvisorFacts {
    // Accounts
    pub admins_total: Option<usize>,
    pub admins_without_2fa: Option<usize>,
    pub has_account_named_default: Option<bool>,
    /// Admin accounts whose password an admin set and the owner never changed.
    pub admins_must_change_password: Option<usize>,

    // Transport
    pub tls_enabled: Option<bool>,
    pub tls_self_signed: Option<bool>,
    pub tls_days_remaining: Option<i64>,
    pub tls_trusted_certificate: Option<bool>,
    pub acme_enabled: Option<bool>,
    pub acme_status: Option<String>,

    // Network exposure
    pub firewall_available: Option<bool>,
    pub firewall_enabled: Option<bool>,
    /// The mandatory allow rules (SSH + the app port) are present.
    pub firewall_mandatory_rules: Option<bool>,

    // SMB
    pub smb_configured: Option<bool>,
    pub smb_guest_allowed: Option<bool>,
    pub smb_signing_required: Option<bool>,
    /// Shortest protocol version (e.g. `SMB2`), when the config names one.
    pub smb_min_protocol: Option<String>,

    // Storage
    pub shares_total: Option<usize>,
    pub shares_world_writable: Option<usize>,
    pub shares_with_quota: Option<usize>,
    pub snapshot_pools_scheduled: Option<usize>,
    pub storage_pools: Option<usize>,

    // Protection features (QW-05 / QW-07 / QW-11 synergy)
    pub auto_block_enabled: Option<bool>,
    pub config_backup_exists: Option<bool>,

    // Updates
    pub update_available: Option<bool>,
    pub latest_version: Option<String>,
}

/// Everything the report is computed from — pure, so both directions of every
/// check are unit-tested without a box.
pub fn evaluate(facts: &AdvisorFacts, generated_at: &str, app_version: &str) -> AdvisorReport {
    let mut findings = Vec::new();

    // ---- Accounts -------------------------------------------------------
    push(
        &mut findings,
        "admins-count",
        facts.admins_total,
        |admins| {
            if *admins == 0 {
                (
                    Severity::Warn,
                    "No administrator account exists".to_string(),
                    "0 admin accounts".to_string(),
                    Some("Create an administrator account so the box can be managed".to_string()),
                    Some(DeepLink::app("users")),
                )
            } else {
                (
                    Severity::Pass,
                    format!("{admins} administrator account(s)"),
                    format!("{admins} admin account(s)"),
                    None,
                    None,
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The administrator accounts could not be read".to_string(),
                "unreadable".to_string(),
                Some(
                    "Open Settings → Users so the account list can be read and this check \
                     returns a real answer"
                        .to_string(),
                ),
                Some(DeepLink::app("users")),
            )
        },
    );

    push(
        &mut findings,
        "default-admin-name",
        facts.has_account_named_default,
        |named| {
            if *named {
                (
                    Severity::Warn,
                    "An account is named 'admin'".to_string(),
                    "the default name is in use".to_string(),
                    Some(
                        "Rename or remove it: a well-known account name is the first one an \
                         attacker tries"
                            .to_string(),
                    ),
                    Some(DeepLink::app("users")),
                )
            } else {
                (
                    Severity::Pass,
                    "No account uses the default name".to_string(),
                    "no account named 'admin'".to_string(),
                    None,
                    None,
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The account names could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::app("users")),
            )
        },
    );

    push(
        &mut findings,
        "admins-2fa",
        facts.admins_without_2fa,
        |missing| {
            if *missing == 0 {
                (
                    Severity::Pass,
                    "Every administrator uses two-factor authentication".to_string(),
                    "0 admins without 2FA".to_string(),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Warn,
                    format!("{missing} administrator(s) do not use two-factor authentication"),
                    format!("{missing} admin(s) without 2FA"),
                    Some(
                        "Turn on 2FA for every administrator: a password alone is one stolen \
                         secret away from a full takeover"
                            .to_string(),
                    ),
                    Some(DeepLink::app("account")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The two-factor status could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::app("account")),
            )
        },
    );

    push(
        &mut findings,
        "admins-password-change",
        facts.admins_must_change_password,
        |pending| {
            if *pending == 0 {
                (
                    Severity::Pass,
                    "No administrator still uses an administrator-chosen password".to_string(),
                    "0 pending password changes".to_string(),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Warn,
                    format!(
                        "{pending} administrator(s) still use an administrator-chosen password"
                    ),
                    format!("{pending} pending change(s)"),
                    Some("Ask them to sign in and choose their own password".to_string()),
                    Some(DeepLink::app("users")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The password-change state could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::app("users")),
            )
        },
    );

    // ---- Transport ------------------------------------------------------
    push(
        &mut findings,
        "tls-enabled",
        facts.tls_enabled,
        |enabled| {
            if *enabled {
                (
                    Severity::Pass,
                    "HTTPS is enabled".to_string(),
                    "TLS listener on".to_string(),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Warn,
                    "HTTPS is not enabled".to_string(),
                    "sign-ins travel over plain HTTP".to_string(),
                    Some(
                        "Enable HTTPS in Settings → Security: without it, a password typed on the \
                         same network can be read in transit"
                            .to_string(),
                    ),
                    Some(DeepLink::settings("security")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The HTTPS setting could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::settings("security")),
            )
        },
    );

    // The certificate is only worth judging when TLS is on; a self-signed
    // certificate on a LAN is acceptable, an expired one is not.
    if facts.tls_enabled == Some(true) {
        push(
            &mut findings,
            "tls-trusted",
            facts.tls_trusted_certificate,
            |trusted| {
                if *trusted {
                    (
                        Severity::Pass,
                        "The certificate is trusted by browsers".to_string(),
                        "not self-signed".to_string(),
                        None,
                        None,
                    )
                } else {
                    (
                        Severity::Info,
                        "The certificate is self-signed".to_string(),
                        "browsers will warn until a trusted certificate is installed".to_string(),
                        Some(
                            "Fine on a home network; for a public name, let HomeVault get a \
                             trusted certificate automatically"
                                .to_string(),
                        ),
                        Some(DeepLink::settings("security")),
                    )
                }
            },
            |_| {
                (
                    Severity::Info,
                    "The certificate could not be read".to_string(),
                    "unreadable".to_string(),
                    None,
                    Some(DeepLink::settings("security")),
                )
            },
        );

        push(
            &mut findings,
            "tls-expiry",
            facts.tls_days_remaining,
            |days| {
                if *days <= 0 {
                    (
                        Severity::Fail,
                        "The HTTPS certificate has expired".to_string(),
                        format!("expired ({} day(s) past)", -days),
                        Some(
                            "Renew or regenerate the certificate now: browsers refuse an expired \
                             one"
                            .to_string(),
                        ),
                        Some(DeepLink::settings("security")),
                    )
                } else if *days < 14 {
                    (
                        Severity::Warn,
                        format!("The HTTPS certificate expires in {days} day(s)"),
                        format!("{days} day(s) remaining"),
                        Some(
                            "Renew it, or turn on automatic renewal so it never lapses".to_string(),
                        ),
                        Some(DeepLink::settings("security")),
                    )
                } else {
                    (
                        Severity::Pass,
                        format!("The HTTPS certificate is valid for {days} more day(s)"),
                        format!("{days} day(s) remaining"),
                        None,
                        None,
                    )
                }
            },
            |_| {
                (
                    Severity::Info,
                    "The certificate expiry could not be read".to_string(),
                    "unreadable".to_string(),
                    None,
                    Some(DeepLink::settings("security")),
                )
            },
        );

        push(
            &mut findings,
            "acme",
            facts.acme_enabled,
            |enabled| {
                if *enabled {
                    (
                        Severity::Pass,
                        "Automatic certificate renewal is on".to_string(),
                        format!("ACME {}", facts.acme_status.as_deref().unwrap_or("enabled")),
                        None,
                        None,
                    )
                } else {
                    (
                        Severity::Info,
                        "Certificate renewal is manual".to_string(),
                        "ACME automation off".to_string(),
                        Some(
                            "Turn it on for a public name, so the certificate renews itself before \
                             it expires"
                                .to_string(),
                        ),
                        Some(DeepLink::settings("security")),
                    )
                }
            },
            |_| {
                (
                    Severity::Info,
                    "The certificate automation could not be read".to_string(),
                    "unreadable".to_string(),
                    None,
                    Some(DeepLink::settings("security")),
                )
            },
        );
    }

    // ---- Exposure -------------------------------------------------------
    push(
        &mut findings,
        "firewall-enabled",
        facts.firewall_enabled,
        |enabled| {
            if *enabled {
                (
                    Severity::Pass,
                    "The firewall is on".to_string(),
                    "ufw active".to_string(),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Warn,
                    "The firewall is off".to_string(),
                    "every service is reachable from the network".to_string(),
                    Some(
                        "Turn it on in Settings → Security → Firewall; HomeVault keeps the SSH \
                         and web ports open"
                            .to_string(),
                    ),
                    Some(DeepLink::settings("security")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The firewall state could not be read".to_string(),
                "ufw is not installed or did not answer".to_string(),
                None,
                Some(DeepLink::settings("security")),
            )
        },
    );

    // A firewall with its mandatory allow rules missing locks the admin out of
    // their own box, so it is a Fail rather than a Warn.
    if facts.firewall_enabled == Some(true) {
        push(
            &mut findings,
            "firewall-rules",
            facts.firewall_mandatory_rules,
            |present| {
                if *present {
                    (
                        Severity::Pass,
                        "The firewall keeps SSH and the web interface reachable".to_string(),
                        "mandatory allow rules present".to_string(),
                        None,
                        None,
                    )
                } else {
                    (
                        Severity::Fail,
                        "The firewall rule that keeps HomeVault reachable is missing".to_string(),
                        "no allow rule for SSH / the web port".to_string(),
                        Some(
                            "Re-apply the mandatory rules from Settings → Security → Firewall \
                             before you lose access"
                                .to_string(),
                        ),
                        Some(DeepLink::settings("security")),
                    )
                }
            },
            |_| {
                (
                    Severity::Info,
                    "The firewall rules could not be read".to_string(),
                    "unreadable".to_string(),
                    None,
                    Some(DeepLink::settings("security")),
                )
            },
        );
    }

    // ---- SMB ------------------------------------------------------------
    if facts.smb_configured == Some(true) {
        push(
            &mut findings,
            "smb-guest",
            facts.smb_guest_allowed,
            |guest| {
                if *guest {
                    (
                        Severity::Fail,
                        "Guest access is allowed on the SMB shares".to_string(),
                        "allow_guest = yes".to_string(),
                        Some(
                            "Turn guest access off in Shares → SMB: anyone on the network can read \
                             those folders without an account"
                                .to_string(),
                        ),
                        Some(DeepLink::app("shares")),
                    )
                } else {
                    (
                        Severity::Pass,
                        "SMB requires an account".to_string(),
                        "guest access off".to_string(),
                        None,
                        None,
                    )
                }
            },
            |_| {
                (
                    Severity::Info,
                    "The SMB guest setting could not be read".to_string(),
                    "unreadable".to_string(),
                    None,
                    Some(DeepLink::app("shares")),
                )
            },
        );

        push(
            &mut findings,
            "smb-signing",
            facts.smb_signing_required,
            |signing| {
                if *signing {
                    (
                        Severity::Pass,
                        "SMB signing is required".to_string(),
                        "server signing = mandatory".to_string(),
                        None,
                        None,
                    )
                } else {
                    (
                        Severity::Warn,
                        "SMB signing is not required".to_string(),
                        "a network attacker could tamper with file traffic".to_string(),
                        Some(
                            "Add `server signing = mandatory` to the SMB configuration in \
                             Shares → SMB"
                                .to_string(),
                        ),
                        Some(DeepLink::app("shares")),
                    )
                }
            },
            |_| {
                (
                    Severity::Info,
                    "The SMB signing setting could not be read".to_string(),
                    "unreadable".to_string(),
                    None,
                    Some(DeepLink::app("shares")),
                )
            },
        );

        push(
            &mut findings,
            "smb-protocol",
            facts.smb_min_protocol.as_deref(),
            |protocol| {
                let lowest = protocol.trim().to_ascii_uppercase();
                // `NT1` is the protocol name Samba prints for SMB1, so both spellings
                // must be recognised: judging it as "not SMB1" would pass the exact
                // setting the check exists to catch.
                let is_smb1 = lowest.contains("SMB1") || lowest.contains("NT1");
                if is_smb1 && !lowest.contains("SMB2") && !lowest.contains("SMB3") {
                    (
                        Severity::Fail,
                        "SMB1 is still accepted".to_string(),
                        format!("protocol = {protocol}"),
                        Some(
                            "Set the minimum protocol to SMB2 in Shares → SMB: SMB1 is broken by \
                             design and exploited by ransomware"
                                .to_string(),
                        ),
                        Some(DeepLink::app("shares")),
                    )
                } else {
                    (
                        Severity::Pass,
                        "SMB1 is not accepted".to_string(),
                        format!("protocol = {protocol}"),
                        None,
                        None,
                    )
                }
            },
            |_| {
                (
                    Severity::Info,
                    "The SMB protocol setting could not be read".to_string(),
                    "unreadable".to_string(),
                    None,
                    Some(DeepLink::app("shares")),
                )
            },
        );
    }

    // ---- Shares ---------------------------------------------------------
    push(
        &mut findings,
        "shares-world-writable",
        facts.shares_world_writable,
        |world| {
            if *world == 0 {
                (
                    Severity::Pass,
                    "No share is writable by everyone".to_string(),
                    "0 world-writable shares".to_string(),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Warn,
                    format!("{world} share(s) are writable by every account on the box"),
                    format!("{world} world-writable share folder(s)"),
                    Some(
                        "Remove the 'other write' bit from those folders, or give each account its \
                         own share"
                            .to_string(),
                    ),
                    Some(DeepLink::app("shares")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The share permissions could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::app("shares")),
            )
        },
    );

    push(
        &mut findings,
        "shares-quota",
        facts.shares_with_quota,
        |with_quota| {
            if *with_quota > 0 {
                (
                    Severity::Pass,
                    format!("{with_quota} share(s) have a size limit"),
                    format!("{with_quota} share quota(s)"),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Info,
                    "No share has a size limit".to_string(),
                    "0 share quotas".to_string(),
                    Some(
                        "Optional: a size limit stops one runaway folder from filling the pool"
                            .to_string(),
                    ),
                    Some(DeepLink::app("shares")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The share quotas could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::app("shares")),
            )
        },
    );

    // ---- Snapshots and backups ------------------------------------------
    let needs_snapshots = matches!(
        (facts.storage_pools, facts.snapshot_pools_scheduled),
        (Some(pools), Some(0)) if pools > 0
    );
    push(
        &mut findings,
        "snapshots",
        facts.snapshot_pools_scheduled,
        |scheduled| {
            if needs_snapshots || *scheduled == 0 {
                (
                    Severity::Warn,
                    "No storage pool has a snapshot schedule".to_string(),
                    "0 pools scheduled".to_string(),
                    Some(
                        "Schedule snapshots for your data pools in Settings → System, so a bad \
                         delete is recoverable to a point in time"
                            .to_string(),
                    ),
                    Some(DeepLink::settings("system")),
                )
            } else {
                (
                    Severity::Pass,
                    format!("{scheduled} pool(s) are snapshotted on a schedule"),
                    format!("{scheduled} scheduled pool(s)"),
                    None,
                    None,
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The snapshot schedules could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::settings("system")),
            )
        },
    );

    push(
        &mut findings,
        "config-backup",
        facts.config_backup_exists,
        |exists| {
            if *exists {
                (
                    Severity::Pass,
                    "A configuration backup exists".to_string(),
                    "an archive is on disk".to_string(),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Warn,
                    "No configuration backup has been taken".to_string(),
                    "no archive in data/config-backups".to_string(),
                    Some(
                        "Take one in Settings → System → Full configuration backup: it is what \
                         rebuilds this box after a disk failure"
                            .to_string(),
                    ),
                    Some(DeepLink::settings("system")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The configuration backups could not be listed".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::settings("system")),
            )
        },
    );

    // ---- Protections shipped by earlier quick wins -----------------------
    push(
        &mut findings,
        "login-auto-block",
        facts.auto_block_enabled,
        |enabled| {
            if *enabled {
                (
                    Severity::Pass,
                    "Repeated failed sign-ins are blocked".to_string(),
                    "login auto-block on".to_string(),
                    None,
                    None,
                )
            } else {
                (
                    Severity::Info,
                    "Repeated failed sign-ins are not blocked".to_string(),
                    "login auto-block off".to_string(),
                    Some(
                        "Optional, but recommended for a box reachable from the internet: \
                         Settings → Security → Failed sign-in blocking"
                            .to_string(),
                    ),
                    Some(DeepLink::settings("security")),
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "The login-blocking setting could not be read".to_string(),
                "unreadable".to_string(),
                None,
                Some(DeepLink::settings("security")),
            )
        },
    );

    // ---- Updates --------------------------------------------------------
    push(
        &mut findings,
        "updates",
        facts.update_available,
        |available| {
            if *available {
                (
                    Severity::Warn,
                    format!(
                        "A HomeVault update is available ({})",
                        facts.latest_version.as_deref().unwrap_or("newer version")
                    ),
                    format!(
                        "latest {}",
                        facts.latest_version.as_deref().unwrap_or("unknown")
                    ),
                    Some(
                        "Install it from Settings → System → Updates: security fixes ship in \
                         releases"
                            .to_string(),
                    ),
                    Some(DeepLink::settings("system")),
                )
            } else {
                (
                    Severity::Pass,
                    "HomeVault is up to date".to_string(),
                    "no newer release".to_string(),
                    None,
                    None,
                )
            }
        },
        |_| {
            (
                Severity::Info,
                "No update check has run yet".to_string(),
                "not checked".to_string(),
                Some(
                    "Check for updates once in Settings → System, so this answer is real"
                        .to_string(),
                ),
                Some(DeepLink::settings("system")),
            )
        },
    );

    // Worst first; within a severity, the fixed order above (accounts first) is
    // already the order of consequence, so the sort is stable.
    findings.sort_by_key(|f| std::cmp::Reverse(f.severity));

    let mut counts = ScoreCounts::default();
    let mut weighted = 0.0f64;
    for finding in &findings {
        match finding.severity {
            Severity::Pass => counts.pass += 1,
            Severity::Info => counts.info += 1,
            Severity::Warn => counts.warn += 1,
            Severity::Fail => counts.fail += 1,
        }
        weighted += finding.severity.weight();
    }
    counts.total = findings.len();
    let score = if counts.total == 0 {
        100
    } else {
        // Floored, so a "99.6" can never be displayed as a pass-looking 100.
        ((weighted / counts.total as f64) * 100.0).floor() as u8
    };

    AdvisorReport {
        score,
        counts,
        findings,
        generated_at: generated_at.to_string(),
        app_version: app_version.to_string(),
    }
}

/// Where to look when a check could not read its signal, in plain language.
fn where_to_look(link: &DeepLink) -> String {
    match &link.category {
        Some(category) => format!("Open Settings → {category} so this check can read its signal"),
        None => format!(
            "Open the {app} screen so this check can read its signal",
            app = link.app
        ),
    }
}

/// Build one finding from a fact: `Some` → the check's verdict, `None` → the
/// "could not be read" answer, which is never a pass.
fn push<T>(
    findings: &mut Vec<Finding>,
    id: &str,
    fact: Option<T>,
    check: impl FnOnce(&T) -> (Severity, String, String, Option<String>, Option<DeepLink>),
    unknown: impl FnOnce(Option<&T>) -> (Severity, String, String, Option<String>, Option<DeepLink>),
) {
    let (severity, title, evidence, fix, link) = match fact.as_ref() {
        Some(value) => check(value),
        None => {
            let (severity, title, evidence, fix, link) = unknown(fact.as_ref());
            // An unreadable signal must never be a dead end: if the "could not
            // be read" branch did not name a next step, point at the screen
            // that owns the setting so the user can check it themselves.
            let fix = fix.or_else(|| link.as_ref().map(where_to_look));
            (severity, title, evidence, fix, link)
        }
    };
    findings.push(Finding {
        id: id.to_string(),
        severity,
        title,
        evidence,
        fix,
        link,
    });
}

// --------------------------------------------------------------------------
// Collection — the appliance's own signals
// --------------------------------------------------------------------------

/// Login names every scanner tries first. A box that still answers to one of
/// them is one password away from a takeover, whatever the password is.
const DEFAULT_ACCOUNT_NAMES: [&str; 3] = ["admin", "administrator", "root"];

/// `roles.id` of the administrator role (migration `001`).
const ADMIN_ROLE_ID: i64 = 2;

/// Does `name` (in any case) use one of the default account names?
pub fn is_default_account_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    DEFAULT_ACCOUNT_NAMES.contains(&lower.as_str())
}

/// Every port an `allow` rule lets through, as numbers.
///
/// ufw prints `22/tcp`, a bare `8080`, or nothing at all (`allow from
/// 1.2.3.4`). A range (`8000:9000`) is not one port, so it is ignored rather
/// than guessed at.
pub fn allowed_ports(rules: &[crate::firewall::FirewallRule]) -> std::collections::HashSet<u16> {
    let mut ports = std::collections::HashSet::new();
    for rule in rules {
        if !rule.action.eq_ignore_ascii_case("allow") {
            continue;
        }
        for part in rule.port.split(',') {
            let candidate = part.trim().split('/').next().unwrap_or("").trim();
            if let Ok(port) = candidate.parse::<u16>() {
                ports.insert(port);
            }
        }
    }
    ports
}

/// The mandatory allow rules (SSH + the web port) are present, so the firewall
/// cannot lock the administrator out of their own appliance.
pub fn mandatory_rules_present(
    rules: &[crate::firewall::FirewallRule],
    ssh_ports: &[u16],
    app_port: u16,
) -> bool {
    let allowed = allowed_ports(rules);
    allowed.contains(&app_port) && ssh_ports.iter().all(|port| allowed.contains(port))
}

/// `server signing = mandatory` is set in the SMB extra parameters.
pub fn smb_signing_required(config: &crate::shares::SmbConfig) -> bool {
    config.additional_params.iter().any(|line| {
        let mut parts = line.split('=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let value = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        key == "server signing" && value == "mandatory"
    })
}

/// Number of `.hvbak` configuration archives on disk. `None` means the
/// directory exists but could not be read — never "there are none".
fn config_backup_archives() -> Option<usize> {
    let dir = crate::config::paths::data_dir().join("config-backups");
    match std::fs::read_dir(&dir) {
        Ok(entries) => Some(
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .map(|ext| ext == "hvbak")
                        .unwrap_or(false)
                })
                .count(),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(0),
        Err(_) => None,
    }
}

/// The blocking half of the collection: certificate files, the SMB config, the
/// share paths' permissions, the storage pools and the archive directory.
/// Every probe here can touch the filesystem or spawn a tool, so it never runs
/// on the async runtime.
fn collect_host_facts() -> AdvisorFacts {
    use std::os::unix::fs::PermissionsExt;

    let mut facts = AdvisorFacts::default();

    // ---- Transport ----
    let tls = crate::tls::load_settings();
    facts.tls_enabled = Some(tls.enabled);
    let cert = crate::tls::certificate_info(&tls);
    // An unreadable certificate is `None`, not "not self-signed": the check
    // must say "could not be read" rather than tick a box it never saw.
    facts.tls_days_remaining = cert.days_remaining;
    facts.tls_self_signed = if cert.error.is_some() {
        None
    } else {
        Some(cert.self_signed)
    };
    facts.tls_trusted_certificate = if cert.error.is_some() {
        None
    } else {
        Some(!cert.self_signed)
    };
    let acme = crate::acme::load_settings();
    facts.acme_enabled = Some(acme.enabled);
    facts.acme_status = Some(acme.status.as_str().to_string());

    // ---- Shares and SMB ----
    if let Ok(shares) = crate::shares::SharesService::get_shares() {
        facts.shares_total = Some(shares.len());
        facts.smb_configured = Some(
            shares
                .iter()
                .any(|share| share.share_type == crate::shares::ShareType::Smb),
        );
        let mut readable = 0usize;
        let mut world_writable = 0usize;
        for share in &shares {
            if let Ok(meta) = std::fs::metadata(&share.path) {
                readable += 1;
                if meta.permissions().mode() & 0o002 != 0 {
                    world_writable += 1;
                }
            }
        }
        facts.shares_world_writable = if readable == 0 {
            None
        } else {
            Some(world_writable)
        };
    }
    if let Ok(config) = crate::shares::SharesService::get_smb_config() {
        facts.smb_guest_allowed = Some(config.allow_guest);
        facts.smb_signing_required = Some(smb_signing_required(&config));
        facts.smb_min_protocol = Some(config.protocol_version.clone());
    }

    // ---- Storage pools ----
    facts.storage_pools = crate::pools::PoolService::scan_pools()
        .ok()
        .map(|pools| pools.len());

    // ---- Configuration backups ----
    facts.config_backup_exists = config_backup_archives().map(|count| count > 0);

    // ---- Firewall presence (a `sudo ufw --version` probe) ----
    facts.firewall_available = Some(crate::firewall::ufw_available());

    facts
}

/// Read every fact the checks need.
///
/// A signal that cannot be read stays `None`, which [`evaluate`] renders as
/// "could not be read" — never as a pass. Nothing here mutates state, and the
/// update check is read from the cached answer only: opening the advisor never
/// contacts a third party.
pub async fn collect(state: &crate::handlers::AppState) -> AdvisorFacts {
    use crate::storage::{
        IpBanRepository, ShareQuotaRepository, SnapshotScheduleRepository, User2faRepository,
        UserRepository,
    };

    let pool = state.db.pool();
    let mut facts = AdvisorFacts::default();

    // ---- Accounts ----
    if let Ok(users) = UserRepository::new(pool.clone()).find_all().await {
        let admins: Vec<&crate::storage::User> = users
            .iter()
            .filter(|user| user.role_id == ADMIN_ROLE_ID)
            .collect();
        facts.admins_total = Some(admins.len());
        facts.has_account_named_default = Some(
            users
                .iter()
                .any(|user| is_default_account_name(&user.username)),
        );
        facts.admins_must_change_password = Some(
            admins
                .iter()
                .filter(|user| user.must_change_password)
                .count(),
        );
        match User2faRepository::new(pool.clone())
            .list_enabled_users()
            .await
        {
            Ok(enabled) => {
                let enabled: std::collections::HashSet<String> = enabled.into_iter().collect();
                facts.admins_without_2fa = Some(
                    admins
                        .iter()
                        .filter(|user| !enabled.contains(&user.username))
                        .count(),
                );
            }
            Err(_) => facts.admins_without_2fa = None,
        }
    }

    // ---- Share quotas ----
    facts.shares_with_quota = ShareQuotaRepository::new(pool.clone())
        .all()
        .await
        .ok()
        .map(|rows| rows.iter().filter(|(_, _, quota)| *quota > 0).count());

    // ---- Snapshot schedules ----
    facts.snapshot_pools_scheduled = SnapshotScheduleRepository::new(pool.clone())
        .list_enabled()
        .await
        .ok()
        .map(|schedules| schedules.len());

    // ---- Login auto-block ----
    let now = chrono::Utc::now().to_rfc3339();
    facts.auto_block_enabled = IpBanRepository::new(pool.clone())
        .status(&now)
        .await
        .ok()
        .map(|status| status.settings.enabled);

    // ---- Updates: the cached answer only ----
    if let Some(cached) = crate::handlers::settings::CACHED_UPDATE_RESULT
        .read()
        .clone()
    {
        if cached.error.is_none() {
            facts.update_available = Some(!cached.up_to_date);
            facts.latest_version = cached.latest_version;
        }
    }

    // ---- Host probes, off the async runtime ----
    let host = tokio::task::spawn_blocking(collect_host_facts)
        .await
        .unwrap_or_default();
    facts.tls_enabled = host.tls_enabled;
    facts.tls_self_signed = host.tls_self_signed;
    facts.tls_days_remaining = host.tls_days_remaining;
    facts.tls_trusted_certificate = host.tls_trusted_certificate;
    facts.acme_enabled = host.acme_enabled;
    facts.acme_status = host.acme_status;
    facts.smb_configured = host.smb_configured;
    facts.smb_guest_allowed = host.smb_guest_allowed;
    facts.smb_signing_required = host.smb_signing_required;
    facts.smb_min_protocol = host.smb_min_protocol;
    facts.shares_total = host.shares_total;
    facts.shares_world_writable = host.shares_world_writable;
    facts.storage_pools = host.storage_pools;
    facts.config_backup_exists = host.config_backup_exists;
    facts.firewall_available = host.firewall_available;

    // ---- Firewall state (async, bounded by the firewall module) ----
    if facts.firewall_available == Some(true) {
        if let Ok(status) = crate::firewall::get_status().await {
            facts.firewall_enabled = Some(status.enabled);
            if status.enabled {
                facts.firewall_mandatory_rules = Some(mandatory_rules_present(
                    &status.rules,
                    &crate::firewall::detect_ssh_ports(),
                    crate::firewall::detect_app_port(),
                ));
            }
        }
    }

    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-configured box: every check passes.
    fn good_facts() -> AdvisorFacts {
        AdvisorFacts {
            admins_total: Some(1),
            admins_without_2fa: Some(0),
            has_account_named_default: Some(false),
            admins_must_change_password: Some(0),
            tls_enabled: Some(true),
            tls_self_signed: Some(false),
            tls_days_remaining: Some(80),
            tls_trusted_certificate: Some(true),
            acme_enabled: Some(true),
            acme_status: Some("valid".to_string()),
            firewall_available: Some(true),
            firewall_enabled: Some(true),
            firewall_mandatory_rules: Some(true),
            smb_configured: Some(true),
            smb_guest_allowed: Some(false),
            smb_signing_required: Some(true),
            smb_min_protocol: Some("SMB2".to_string()),
            shares_total: Some(2),
            shares_world_writable: Some(0),
            shares_with_quota: Some(2),
            snapshot_pools_scheduled: Some(1),
            storage_pools: Some(1),
            auto_block_enabled: Some(true),
            config_backup_exists: Some(true),
            update_available: Some(false),
            latest_version: Some("1.0.0".to_string()),
        }
    }

    fn report(facts: &AdvisorFacts) -> AdvisorReport {
        evaluate(facts, "2026-10-31T12:00:00+00:00", "1.0.0")
    }

    fn find<'a>(report: &'a AdvisorReport, id: &str) -> &'a Finding {
        report
            .findings
            .iter()
            .find(|f| f.id == id)
            .unwrap_or_else(|| {
                panic!(
                    "no finding '{id}' in {:?}",
                    report.findings.iter().map(|f| &f.id).collect::<Vec<_>>()
                )
            })
    }

    /// The whole finding, owned — for the tests that move its `fix` field.
    fn finding(facts: &AdvisorFacts, id: &str) -> Finding {
        find(&report(facts), id).clone()
    }

    // ---------------------------------------------------------------------
    // A clean box
    // ---------------------------------------------------------------------

    #[test]
    fn a_well_configured_box_scores_one_hundred_with_nothing_actionable() {
        let report = report(&good_facts());
        assert_eq!(report.score, 100);
        assert_eq!(report.worst(), Severity::Pass);
        assert!(report.actionable().is_empty());
        assert_eq!(report.counts.fail, 0);
        assert_eq!(report.counts.warn, 0);
        assert_eq!(report.counts.total, report.findings.len());
        // The report describes itself.
        assert_eq!(report.generated_at, "2026-10-31T12:00:00+00:00");
        assert_eq!(report.app_version, "1.0.0");
    }

    // ---------------------------------------------------------------------
    // Each check, both ways
    // ---------------------------------------------------------------------

    #[test]
    fn admins_without_2fa_warns_and_the_fix_points_at_the_account_page() {
        let mut facts = good_facts();
        facts.admins_without_2fa = Some(2);
        let rep = report(&facts);
        let finding = find(&rep, "admins-2fa");
        assert_eq!(finding.severity, Severity::Warn);
        assert!(finding.title.contains('2'), "{}", finding.title);
        assert!(finding.fix.is_some());
        assert_eq!(finding.link.as_ref().unwrap().app, "account");

        let mut facts = good_facts();
        facts.admins_without_2fa = Some(0);
        assert_eq!(find(&report(&facts), "admins-2fa").severity, Severity::Pass);
    }

    #[test]
    fn the_default_admin_name_is_flagged_both_ways() {
        let mut facts = good_facts();
        facts.has_account_named_default = Some(true);
        assert_eq!(
            find(&report(&facts), "default-admin-name").severity,
            Severity::Warn
        );

        let mut facts = good_facts();
        facts.has_account_named_default = Some(false);
        assert_eq!(
            find(&report(&facts), "default-admin-name").severity,
            Severity::Pass
        );
    }

    #[test]
    fn a_pending_forced_password_change_warns() {
        let mut facts = good_facts();
        facts.admins_must_change_password = Some(1);
        assert_eq!(
            find(&report(&facts), "admins-password-change").severity,
            Severity::Warn
        );
        let mut facts = good_facts();
        facts.admins_must_change_password = Some(0);
        assert_eq!(
            find(&report(&facts), "admins-password-change").severity,
            Severity::Pass
        );
    }

    #[test]
    fn https_off_warns_and_the_fix_links_to_the_security_category() {
        let mut facts = good_facts();
        facts.tls_enabled = Some(false);
        let report = report(&facts);
        let finding = find(&report, "tls-enabled");
        assert_eq!(finding.severity, Severity::Warn);
        assert!(finding.fix.clone().unwrap().contains("Enable HTTPS"));
        let link = finding.link.clone().unwrap();
        assert_eq!(link.app, "settings");
        assert_eq!(link.category.as_deref(), Some("security"));
    }

    #[test]
    fn an_expired_certificate_fails_while_an_expiring_one_only_warns() {
        let mut facts = good_facts();
        facts.tls_days_remaining = Some(0);
        assert_eq!(find(&report(&facts), "tls-expiry").severity, Severity::Fail);

        let mut facts = good_facts();
        facts.tls_days_remaining = Some(5);
        assert_eq!(find(&report(&facts), "tls-expiry").severity, Severity::Warn);

        let mut facts = good_facts();
        facts.tls_days_remaining = Some(45);
        assert_eq!(find(&report(&facts), "tls-expiry").severity, Severity::Pass);
    }

    #[test]
    fn a_self_signed_certificate_is_an_info_not_a_failure() {
        let mut facts = good_facts();
        facts.tls_trusted_certificate = Some(false);
        let finding = finding(&facts, "tls-trusted");
        assert_eq!(
            finding.severity,
            Severity::Info,
            "a self-signed certificate on a home LAN is acceptable, not a defect"
        );
        // …and it still says what would make it better.
        assert!(finding.fix.is_some());
    }

    #[test]
    fn the_certificate_checks_are_skipped_when_https_is_off() {
        let mut facts = good_facts();
        facts.tls_enabled = Some(false);
        facts.tls_days_remaining = None;
        facts.tls_trusted_certificate = None;
        facts.acme_enabled = None;
        let report = report(&facts);
        for id in ["tls-expiry", "tls-trusted", "acme"] {
            assert!(
                report.findings.iter().all(|f| f.id != id),
                "{id} must not be judged when there is no certificate"
            );
        }
    }

    #[test]
    fn a_firewall_that_is_off_warns() {
        let mut facts = good_facts();
        facts.firewall_enabled = Some(false);
        let report = report(&facts);
        let finding = find(&report, "firewall-enabled");
        assert_eq!(finding.severity, Severity::Warn);
        // The rules check is not judged when the firewall is off.
        assert!(report.findings.iter().all(|f| f.id != "firewall-rules"));
    }

    #[test]
    fn a_firewall_missing_its_reachability_rule_fails() {
        // Losing the mandatory allow rule locks the admin out of their own box,
        // so it is a failure, not a warning.
        let mut facts = good_facts();
        facts.firewall_mandatory_rules = Some(false);
        assert_eq!(
            find(&report(&facts), "firewall-rules").severity,
            Severity::Fail
        );
    }

    #[test]
    fn guest_access_and_smb1_fail_while_signing_only_warns() {
        let mut facts = good_facts();
        facts.smb_guest_allowed = Some(true);
        assert_eq!(find(&report(&facts), "smb-guest").severity, Severity::Fail);

        let mut facts = good_facts();
        facts.smb_min_protocol = Some("NT1".to_string());
        assert_eq!(
            find(&report(&facts), "smb-protocol").severity,
            Severity::Fail
        );

        let mut facts = good_facts();
        facts.smb_signing_required = Some(false);
        assert_eq!(
            find(&report(&facts), "smb-signing").severity,
            Severity::Warn
        );

        // `SMB2_10` contains "SMB2" and must pass.
        let mut facts = good_facts();
        facts.smb_min_protocol = Some("SMB2_10".to_string());
        assert_eq!(
            find(&report(&facts), "smb-protocol").severity,
            Severity::Pass
        );
    }

    #[test]
    fn the_smb_checks_are_skipped_when_no_share_uses_smb() {
        let mut facts = good_facts();
        facts.smb_configured = Some(false);
        facts.smb_guest_allowed = None;
        facts.smb_signing_required = None;
        facts.smb_min_protocol = None;
        let report = report(&facts);
        for id in ["smb-guest", "smb-signing", "smb-protocol"] {
            assert!(report.findings.iter().all(|f| f.id != id), "{id} skipped");
        }
    }

    #[test]
    fn world_writable_shares_warn_and_a_quota_is_only_an_info() {
        let mut facts = good_facts();
        facts.shares_world_writable = Some(2);
        assert_eq!(
            find(&report(&facts), "shares-world-writable").severity,
            Severity::Warn
        );

        let mut facts = good_facts();
        facts.shares_with_quota = Some(0);
        assert_eq!(
            find(&report(&facts), "shares-quota").severity,
            Severity::Info,
            "a quota is a safety net, not a security requirement"
        );
    }

    #[test]
    fn missing_snapshots_and_missing_config_backup_warn() {
        let mut facts = good_facts();
        facts.snapshot_pools_scheduled = Some(0);
        assert_eq!(find(&report(&facts), "snapshots").severity, Severity::Warn);

        let mut facts = good_facts();
        facts.config_backup_exists = Some(false);
        assert_eq!(
            find(&report(&facts), "config-backup").severity,
            Severity::Warn
        );
    }

    #[test]
    fn the_login_auto_block_is_recommended_but_optional() {
        let mut facts = good_facts();
        facts.auto_block_enabled = Some(false);
        let finding = finding(&facts, "login-auto-block");
        assert_eq!(finding.severity, Severity::Info);
        assert!(finding.fix.unwrap().contains("recommended"));
    }

    #[test]
    fn an_available_update_warns_and_links_to_the_system_category() {
        let mut facts = good_facts();
        facts.update_available = Some(true);
        facts.latest_version = Some("1.1.0".to_string());
        let finding = finding(&facts, "updates");
        assert_eq!(finding.severity, Severity::Warn);
        assert!(finding.title.contains("1.1.0"), "{}", finding.title);
        let link = finding.link.clone().unwrap();
        assert_eq!(link.category.as_deref(), Some("system"));
    }

    // ---------------------------------------------------------------------
    // Unreadable signals are never a pass
    // ---------------------------------------------------------------------

    #[test]
    fn an_unreadable_signal_is_info_and_never_a_pass() {
        let report = report(&AdvisorFacts::default());
        assert!(
            report.findings.iter().all(|f| f.severity != Severity::Pass),
            "a report built from nothing must not contain a single green tick"
        );
        assert!(report.counts.info > 0);
        // …and the score still reflects "nothing known to be wrong": Info counts
        // as a pass weight, so the number stays high while every line says
        // "unreadable".
        assert_eq!(report.score, 100);
        assert_eq!(report.worst(), Severity::Info);
    }

    #[test]
    fn every_unreadable_finding_says_it_could_not_be_read() {
        let report = report(&AdvisorFacts::default());
        for finding in &report.findings {
            assert!(
                finding.evidence.contains("unreadable")
                    || finding.evidence.contains("not checked")
                    || finding.evidence.contains("not installed"),
                "{} must explain itself, got {:?}",
                finding.id,
                finding.evidence
            );
            assert!(
                finding.fix.is_some(),
                "{} should say where to look",
                finding.id
            );
        }
    }

    // ---------------------------------------------------------------------
    // The score
    // ---------------------------------------------------------------------

    #[test]
    fn the_score_is_the_weighted_percentage_floored() {
        // 12 checks in a clean report; turn one into a failure.
        let mut facts = good_facts();
        facts.smb_guest_allowed = Some(true);
        let report = report(&facts);
        let total = report.counts.total as f64;
        let expected = (((total - 1.0) / total) * 100.0).floor() as u8;
        assert_eq!(report.score, expected);
        // A floor, never a round-up that would read as "perfect".
        assert!(report.score < 100);
        assert_eq!(report.counts.fail, 1);
    }

    #[test]
    fn a_warning_costs_half_a_point() {
        let clean = report(&good_facts());
        let mut facts = good_facts();
        facts.admins_without_2fa = Some(1);
        let warned = report(&facts);
        assert!(warned.score < clean.score);
        // One warn out of N checks: the difference is ~half a check.
        let total = clean.counts.total as f64;
        assert_eq!(warned.score, ((total - 0.5) / total * 100.0).floor() as u8);
    }

    #[test]
    fn the_worst_severity_is_reported_first() {
        let mut facts = good_facts();
        facts.smb_guest_allowed = Some(true); // fail
        facts.admins_without_2fa = Some(1); // warn
        let report = report(&facts);
        assert_eq!(report.findings[0].severity, Severity::Fail);
        let warn_index = report
            .findings
            .iter()
            .position(|f| f.severity == Severity::Warn)
            .expect("a warning");
        assert!(warn_index > 0, "the failure comes first");
        assert_eq!(report.worst(), Severity::Fail);
        assert_eq!(report.actionable().len(), 2);
    }

    #[test]
    fn the_findings_are_ordered_by_consequence_within_a_severity() {
        let report = report(&AdvisorFacts::default());
        // Accounts come before updates among the Info findings.
        let ids: Vec<&str> = report.findings.iter().map(|f| f.id.as_str()).collect();
        let admins = ids.iter().position(|id| *id == "admins-2fa").unwrap();
        let updates = ids.iter().position(|id| *id == "updates").unwrap();
        assert!(admins < updates);
    }

    #[test]
    fn severity_weights_are_what_the_score_assumes() {
        assert_eq!(Severity::Pass.weight(), 1.0);
        assert_eq!(Severity::Info.weight(), 1.0);
        assert_eq!(Severity::Warn.weight(), 0.5);
        assert_eq!(Severity::Fail.weight(), 0.0);
        // Ordering is the one the UI relies on (worst last).
        assert!(Severity::Pass < Severity::Info);
        assert!(Severity::Info < Severity::Warn);
        assert!(Severity::Warn < Severity::Fail);
    }

    // ---------------------------------------------------------------------
    // Collection helpers (pure halves of the collector)
    // ---------------------------------------------------------------------

    fn rule(action: &str, port: &str, mandatory: bool) -> crate::firewall::FirewallRule {
        crate::firewall::FirewallRule {
            number: 1,
            port: port.to_string(),
            protocol: "tcp".to_string(),
            action: action.to_string(),
            source: None,
            comment: None,
            is_mandatory: mandatory,
            spec: format!("{action} {port}/tcp"),
        }
    }

    #[test]
    fn test_is_default_account_name_recognises_the_usual_suspects() {
        for name in ["admin", "Admin", "ADMINISTRATOR", "root", " Root "] {
            assert!(is_default_account_name(name), "{name} must be flagged");
        }
        for name in ["e2eadmin", "admin2", "nicolas", ""] {
            assert!(!is_default_account_name(name), "{name} must not be flagged");
        }
    }

    #[test]
    fn test_allowed_ports_reads_ufw_port_shapes() {
        let rules = vec![
            rule("allow", "22/tcp", true),
            rule("allow", "8080", true),
            rule("allow", "", true), // `allow from 1.2.3.4` — no port
            rule("allow", "8000:9000", false), // a range is not one port
            rule("deny", "23/tcp", false),
        ];
        let ports = allowed_ports(&rules);
        assert!(ports.contains(&22));
        assert!(ports.contains(&8080));
        assert!(!ports.contains(&23), "a deny rule is not an allow");
        assert_eq!(ports.len(), 2, "the range and the empty port are ignored");
    }

    #[test]
    fn test_mandatory_rules_present_needs_ssh_and_the_web_port() {
        let rules = vec![
            rule("allow", "22/tcp", true),
            rule("allow", "8080/tcp", true),
        ];
        assert!(mandatory_rules_present(&rules, &[22], 8080));

        // Only SSH: the web interface is unreachable.
        let only_ssh = vec![rule("allow", "22/tcp", true)];
        assert!(!mandatory_rules_present(&only_ssh, &[22], 8080));

        // Only the web port: SSH is closed.
        let only_web = vec![rule("allow", "8080/tcp", true)];
        assert!(!mandatory_rules_present(&only_web, &[22], 8080));

        // A non-standard SSH port must be the one that is open.
        assert!(!mandatory_rules_present(&rules, &[2222], 8080));
    }

    #[test]
    fn test_smb_signing_required_matches_the_exact_directive() {
        let mut config = crate::shares::SmbConfig::default();
        assert!(!smb_signing_required(&config), "off by default");

        config.additional_params = vec!["server signing = mandatory".to_string()];
        assert!(smb_signing_required(&config));

        config.additional_params = vec!["  Server Signing=Mandatory ".to_string()];
        assert!(
            smb_signing_required(&config),
            "case and spacing are tolerated"
        );

        config.additional_params = vec!["server signing = auto".to_string()];
        assert!(!smb_signing_required(&config), "only `mandatory` counts");

        config.additional_params = vec!["client signing = mandatory".to_string()];
        assert!(
            !smb_signing_required(&config),
            "the client directive is not the server's"
        );
    }
}
