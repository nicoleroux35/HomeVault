/* =============================================================================
 * File          : auto_block.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Login auto-block: when one address fails to sign in N times inside a
 *   window, block it — in the database, so the block survives a restart, and
 *   optionally in the firewall so it reaches every service on the box. Plus the
 *   "DoS" knob: a request-rate cap on the public listener.
 *
 * Dependencies:
 *   - serde, chrono, crate::storage::IpBanRepository, crate::firewall
 *
 * Usage:
 *   handlers/auto_block.rs (card), handlers/auth.rs + middleware (enforcement);
 *   see docs/guides/security.md.
 * =============================================================================
 */

//! Login auto-block (see docs/guides/security.md).
//!
//! The existing protections answer two different questions and neither survives
//! a restart in a useful way: the per-account lockout stops a brute force on
//! *one account*, and the in-memory per-IP limiter (`middleware::rate_limit`)
//! forgets everything when the process restarts. This module adds the missing
//! pair:
//!
//! * **A persistent ban per address** (`ip_bans`): N failures inside a window →
//!   a row with an expiry, checked before a password is ever verified, so a
//!   blocked attacker cannot keep counting against an account.
//! * **An optional firewall rule**, so the block reaches SMB/SSH and not only
//!   the web login. It is opt-in: locking an address out of every service is a
//!   bigger hammer than refusing a login.
//!
//! Three invariants that the acceptance criteria hang on:
//!
//! 1. **Never the caller's own address, never loopback.** Blocking the admin's
//!    current address locks them out of the box they are configuring; blocking
//!    `127.0.0.1` breaks local health checks and every reverse proxy.
//! 2. **A ban is per address, never per account.** A banned attacker therefore
//!    cannot lock a legitimate user out from somewhere else — the legitimate
//!    address has its own, clean counter.
//! 3. **A pardon removes both halves.** The row and the firewall rule are
//!    removed together, and the row records whether a rule was installed so the
//!    removal can never touch a rule this feature did not add.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::middleware::rate_limit::RateLimitState;
use crate::storage::database::DbError;
use crate::storage::IpBanRepository;

/// Comment that tags a firewall rule as *ours*, so a pardon deletes exactly one
/// rule and never a hand-written `ufw deny from …`.
pub const BAN_COMMENT_PREFIX: &str = "homevault-autoblock";

/// Bounds for the settings, applied on read *and* on write so a hand-edited row
/// cannot produce a policy the UI cannot express.
pub const MIN_THRESHOLD: u32 = 1;
pub const MAX_THRESHOLD: u32 = 100;
pub const MAX_WINDOW_SECONDS: u32 = 86_400;
pub const MAX_BAN_SECONDS: u32 = 31_536_000;
pub const MAX_DOS_REQUESTS_PER_MINUTE: u32 = 1_000_000;

/// The auto-block policy (`auto_block_settings`, singleton).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoBlockSettings {
    pub enabled: bool,
    /// Failures inside `window_seconds` that trip a ban.
    pub threshold: u32,
    pub window_seconds: u32,
    pub ban_seconds: u32,
    /// Also install (and later remove) a `ufw deny from <ip>` rule.
    pub apply_firewall: bool,
    /// Request-rate cap on the public listener.
    pub dos_enabled: bool,
    pub dos_requests_per_minute: u32,
}

impl Default for AutoBlockSettings {
    /// Off, 5 failures / 5 minutes → 30-minute ban, no firewall rule, and the
    /// DoS cap on at 600 requests/minute.
    ///
    /// The ban is off by default on purpose: a home box behind a NAT is not
    /// scanned the way an exposed one is, and silently locking out a family
    /// member who mistyped a password five times is worse than the threat it
    /// addresses. The layout of the defaults matches the shipped lockout
    /// (`auth::MAX_LOGIN_ATTEMPTS = 5`, five-minute window).
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 5,
            window_seconds: 300,
            ban_seconds: 1800,
            apply_firewall: false,
            dos_enabled: true,
            dos_requests_per_minute: 600,
        }
    }
}

impl AutoBlockSettings {
    /// Clamp every field into its documented range.
    ///
    /// Called on read (a row written by an older build, or by hand) and on
    /// write (a request), so the two can never disagree about what is legal.
    pub fn clamped(mut self) -> Self {
        self.threshold = self.threshold.clamp(MIN_THRESHOLD, MAX_THRESHOLD);
        self.window_seconds = self.window_seconds.min(MAX_WINDOW_SECONDS);
        self.ban_seconds = self.ban_seconds.min(MAX_BAN_SECONDS);
        self.dos_requests_per_minute = self
            .dos_requests_per_minute
            .min(MAX_DOS_REQUESTS_PER_MINUTE);
        self
    }

    /// Reject a policy that cannot work, naming the field.
    ///
    /// Only enforced while enabled, so an admin may switch the feature off and
    /// leave the numbers alone.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.threshold < MIN_THRESHOLD {
            return Err("the failure threshold must be at least 1".to_string());
        }
        if self.threshold > MAX_THRESHOLD {
            return Err(format!(
                "the failure threshold must be {MAX_THRESHOLD} or less"
            ));
        }
        if self.ban_seconds == 0 {
            return Err(
                "a ban duration of 0 would block nobody — set at least one second".to_string(),
            );
        }
        if self.window_seconds == 0 {
            return Err("the failure window must be at least one second".to_string());
        }
        if self.window_seconds > MAX_WINDOW_SECONDS {
            return Err(format!(
                "the failure window must be {MAX_WINDOW_SECONDS} seconds or less"
            ));
        }
        if self.ban_seconds > MAX_BAN_SECONDS {
            return Err(format!(
                "the ban duration must be {MAX_BAN_SECONDS} seconds or less"
            ));
        }
        if self.dos_enabled && self.dos_requests_per_minute == 0 {
            return Err(
                "the request-rate cap must be at least 1 per minute (or turn it off)".to_string(),
            );
        }
        Ok(())
    }
}

/// One blocked address (`ip_bans`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct IpBan {
    pub id: i64,
    pub ip: String,
    pub reason: String,
    pub failures: i64,
    pub created_at: String,
    pub expires_at: String,
    pub firewall_rule: bool,
}

impl IpBan {
    /// `now` is an RFC 3339 UTC string; both stamps use the same format, so the
    /// comparison is a plain string comparison (and the SQL side does the same).
    pub fn is_active(&self, now: &str) -> bool {
        self.expires_at.as_str() > now
    }

    /// Seconds left, floored at 0 — what the card shows as "blocked for 12 min".
    pub fn remaining_seconds(&self, now: &str) -> i64 {
        match (parse_rfc3339(&self.expires_at), parse_rfc3339(now)) {
            (Some(expires), Some(now)) => (expires - now).max(0),
            _ => 0,
        }
    }
}

/// The whole picture the Security card renders.
#[derive(Debug, Clone, Serialize)]
pub struct AutoBlockStatus {
    pub settings: AutoBlockSettings,
    pub bans: Vec<IpBan>,
    /// How many rows are still in force (the rest are shown as expired).
    pub active: usize,
    /// How many live bans also carry a firewall rule.
    pub firewall_rules: usize,
}

fn parse_rfc3339(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.timestamp())
}

/// Whether `failures` inside the window is enough to ban.
///
/// Pure, and deliberately `>=`: the threshold is "the number of failures that
/// trips a ban", so `threshold = 5` bans on the fifth.
pub fn should_ban(failures: i64, settings: &AutoBlockSettings) -> bool {
    settings.enabled && failures >= i64::from(settings.threshold)
}

/// Timestamps of a ban starting at `now`.
pub fn ban_window(now: DateTime<Utc>, settings: &AutoBlockSettings) -> (String, String) {
    let expires = now + chrono::Duration::seconds(i64::from(settings.ban_seconds));
    (now.to_rfc3339(), expires.to_rfc3339())
}

/// Addresses this feature must never block, whatever the caller says.
///
/// Loopback and unspecified addresses run the box's own health checks, the
/// reverse proxy in front of it and every local tool; blocking one breaks more
/// than it protects.
pub fn is_protected_address(ip: &str) -> bool {
    match ip.trim().parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_loopback() || v4.is_unspecified(),
        Ok(std::net::IpAddr::V6(v6)) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // An IPv4-mapped loopback (::ffff:127.0.0.1) is still loopback.
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| v4.is_loopback() || v4.is_unspecified())
                    .unwrap_or(false)
        }
        // A value that is not an address at all (the limiter's "unknown") is
        // never banned: there is nothing to block, and a rule would be forged.
        Err(_) => true,
    }
}

/// Whether the ban decision must be skipped for this pair of addresses.
///
/// Two reasons, both about not shooting ourselves in the foot:
/// * the address is loopback/unspecified/not an address (`is_protected_address`);
/// * it is the address of the request that would be blocked — an admin who
///   mistypes their own password five times must not lock themselves out of the
///   box they are standing in front of.
pub fn must_not_ban(candidate: &str, caller: &str) -> bool {
    is_protected_address(candidate) || candidate.trim() == caller.trim()
}

// --------------------------------------------------------------------------
// Enforcement
// --------------------------------------------------------------------------

/// The outcome of checking one address before either a login or any API call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BanCheck {
    /// Not banned (or the feature is off).
    Allowed,
    /// Banned until `expires_at`, with the reason to show the caller.
    Blocked { expires_at: String, reason: String },
}

/// Look up a live ban, sweeping expired rows first.
///
/// The sweep is what makes the ban *self-healing*: an expired row is deleted
/// (and its firewall rule removed) the next time anyone is checked, so a box
/// that is never rebooted still forgets an old ban. It runs on the read path
/// on purpose — a scheduled task would add a scheduler for something this
/// cheap, and a ban that outlives its expiry because a timer did not fire is
/// exactly the bug this feature must not have.
pub async fn check(state: &crate::handlers::AppState, ip: &str) -> BanCheck {
    if ip.trim().is_empty() {
        return BanCheck::Allowed;
    }
    let repo = IpBanRepository::new(state.db.pool());
    let now = Utc::now().to_rfc3339();
    if let Err(e) = sweep_expired(&repo, &now).await {
        // A failed sweep must not break authentication: the lookup below still
        // uses `now`, so an expired row simply does not match.
        tracing::warn!("Auto-block: could not sweep expired bans: {e}");
    }
    match repo.find_active(ip, &now).await {
        Ok(Some(ban)) => BanCheck::Blocked {
            expires_at: ban.expires_at,
            reason: ban.reason,
        },
        Ok(None) => BanCheck::Allowed,
        Err(e) => {
            // Fail open, loudly: a database hiccup must not lock the box.
            tracing::error!("Auto-block: could not read bans, allowing the request: {e}");
            BanCheck::Allowed
        }
    }
}

/// Remove every expired ban, dropping the firewall rule of each one that had
/// it. Returns how many rows were removed.
pub async fn sweep_expired(repo: &IpBanRepository, now: &str) -> Result<usize, DbError> {
    let expired = repo.expired(now).await?;
    let mut removed = 0;
    for ban in expired {
        if ban.firewall_rule {
            // Best effort: the box may have no ufw, or the rule was removed by
            // hand. Either way the *row* must go, or the address stays
            // unblockable-by-list forever.
            if let Err(e) = crate::firewall::unban_source(&ban.ip).await {
                tracing::warn!(
                    "Auto-block: could not remove the firewall rule for {}: {e}",
                    ban.ip
                );
            }
        }
        if repo.delete(&ban.ip).await? {
            removed += 1;
        }
        crate::events::log_event(format!("Login ban for '{}' expired and was lifted", ban.ip));
    }
    Ok(removed)
}

/// Count the failures a ban decision uses: failed sign-ins from this address
/// inside the **configured** window.
///
/// The window is the admin's, not the repo default: a policy of "3 failures in
/// 60 seconds" must count 60 seconds, otherwise the number the card shows and
/// the number that trips the ban describe different things.
async fn failures_in_window(
    state: &crate::handlers::AppState,
    ip: &str,
    settings: &AutoBlockSettings,
) -> i64 {
    let repo = crate::storage::LoginAttemptRepository::new(state.db.pool());
    match repo
        .get_attempts_count_by_ip(ip, i64::from(settings.window_seconds))
        .await
    {
        Ok(count) => count,
        Err(e) => {
            // Fail closed *for the ban*: an unreadable counter means we cannot
            // prove the threshold was crossed, so nobody is banned on a guess.
            tracing::warn!("Auto-block: could not count failures for {ip}: {e}");
            0
        }
    }
}

/// Record one failed sign-in for `ip` and ban it when the policy says so.
///
/// Returns `Some(ban)` when a *new* ban was created, so the caller can tell the
/// user why they are now refused. Never bans the caller's own address or
/// loopback (`must_not_ban`).
pub async fn record_failure(
    state: &crate::handlers::AppState,
    ip: &str,
    caller: &str,
) -> Option<IpBan> {
    if must_not_ban(ip, caller) {
        return None;
    }
    let repo = IpBanRepository::new(state.db.pool());
    let settings = repo.settings().await.ok()?.clamped();
    if !settings.enabled {
        return None;
    }
    let failures = failures_in_window(state, ip, &settings).await;
    if !should_ban(failures, &settings) {
        return None;
    }

    let now = Utc::now();
    let (created_at, expires_at) = ban_window(now, &settings);
    let reason = format!(
        "{} failed sign-ins in {} minutes",
        failures,
        (settings.window_seconds / 60).max(1)
    );

    // The firewall rule comes first: if it fails, the row still records the
    // ban (web access is refused either way) and `firewall_rule` stays 0, which
    // is exactly what a later pardon must know.
    let mut installed = false;
    if settings.apply_firewall {
        match crate::firewall::ban_source(ip, &ban_comment(ip)).await {
            Ok(()) => installed = true,
            Err(e) => tracing::warn!(
                "Auto-block: banned {} in the database but could not add a firewall rule: {e}",
                ip
            ),
        }
    }

    if let Err(e) = repo
        .upsert(ip, &reason, failures, &created_at, &expires_at, installed)
        .await
    {
        tracing::error!("Auto-block: could not store the ban for {ip}: {e}");
        return None;
    }

    crate::events::log_event_warn(format!(
        "Login ban: '{}' is blocked until {} ({})",
        ip, expires_at, reason
    ));
    repo.find_active(ip, &created_at).await.ok().flatten()
}

/// The ufw comment for one ban: prefixed so a pardon (and a human) can tell our
/// rules from a hand-written one.
pub fn ban_comment(ip: &str) -> String {
    // ufw comments are single-quoted when read back, so they must not contain a
    // quote; an address never does.
    format!("{BAN_COMMENT_PREFIX}-{ip}")
}

/// Lift a ban: firewall rule first, then the row. Returns whether a row existed.
///
/// Order matters: if the firewall removal fails, the caller still gets the row
/// removed (so the card and the enforcement agree) and the error is reported —
/// a leftover `ufw deny` is a *visible* problem, a leftover row is a silent one.
pub async fn pardon(state: &crate::handlers::AppState, ip: &str) -> Result<bool, String> {
    let repo = IpBanRepository::new(state.db.pool());
    let existing = repo
        .find_active(ip, &Utc::now().to_rfc3339())
        .await
        .map_err(|e| format!("could not read the ban: {e}"))?;
    let had_rule = existing.map(|b| b.firewall_rule).unwrap_or(false);
    let had_row = repo
        .delete(ip)
        .await
        .map_err(|e| format!("could not remove the ban: {e}"))?;

    let mut warning = None;
    if had_rule {
        if let Err(e) = crate::firewall::unban_source(ip).await {
            warning = Some(format!(
                "the ban was lifted but its firewall rule could not be removed: {e}"
            ));
        }
    }
    if had_row {
        crate::events::log_event(format!("Admin pardoned the login ban on '{ip}'"));
    }
    match warning {
        Some(reason) => Err(reason),
        None => Ok(had_row),
    }
}

/// Purge every ban (the "clear the list" action). Returns how many were lifted.
pub async fn pardon_all(state: &crate::handlers::AppState) -> Result<usize, String> {
    let repo = IpBanRepository::new(state.db.pool());
    let bans = repo.list().await.map_err(|e| e.to_string())?;
    let mut lifted = 0;
    for ban in bans {
        if ban.firewall_rule {
            let _ = crate::firewall::unban_source(&ban.ip).await;
        }
        if repo.delete(&ban.ip).await.unwrap_or(false) {
            lifted += 1;
        }
    }
    if lifted > 0 {
        crate::events::log_event(format!("Admin pardoned every login ban ({lifted})"));
    }
    Ok(lifted)
}

/// Seconds left until `expires_at` (RFC 3339), for a `Retry-After` header.
///
/// `None` when the stamp cannot be parsed — a header with a nonsense number is
/// worse than no header.
pub fn remaining_seconds(expires_at: &str) -> Option<u64> {
    let expires = parse_rfc3339(expires_at)?;
    Some((expires - Utc::now().timestamp()).max(0) as u64)
}

/// How long a ban has left, in whole minutes (for the card's wording).
pub fn remaining_minutes(remaining_seconds: i64) -> i64 {
    (remaining_seconds + 59) / 60
}

/// The "temporarily blocked" answer, shared by the login handler and the
/// middleware so the wording can never drift.
pub fn blocked_message(expires_at: &str) -> String {
    let remaining = parse_rfc3339(expires_at)
        .map(|expires| expires - Utc::now().timestamp())
        .unwrap_or(0)
        .max(0);
    format!(
        "This address is temporarily blocked after repeated failed sign-ins. Try again in about {} \
         minute(s), or ask an administrator to lift the block.",
        remaining_minutes(remaining)
    )
}

/// The request-rate cap for the public listener, as a limiter deadline.
///
/// `RateLimitState` is a fixed-window counter, so "N per minute" is exactly one
/// window of 60 s. `None` means the DoS row is off.
pub fn dos_limiter(settings: &AutoBlockSettings) -> Option<Duration> {
    if !settings.dos_enabled || settings.dos_requests_per_minute == 0 {
        return None;
    }
    Some(Duration::from_secs(60))
}

/// The live in-memory counter for the DoS row.
///
/// Rebuilt whenever the configured rate changes, because `RateLimitState` fixes
/// its budget at construction: a policy saved in the card must take effect
/// without a restart, and re-creating the counter is the only honest way to say
/// "the budget is now N".
/// `(configured requests per minute, counter for that budget)`.
type DosState = parking_lot::Mutex<Option<(u32, std::sync::Arc<RateLimitState>)>>;

static DOS_STATE: std::sync::LazyLock<DosState> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(None));

/// Whether this address is still inside its request budget.
///
/// **Unauthenticated calls only** (the caller decides that): a signed-in user is
/// already trusted, and a family behind one NAT address shares one budget — a
/// cap that also counted authenticated polling would throttle the appliance's
/// own dashboard before it stopped an attacker.
pub async fn dos_allows(state: &crate::handlers::AppState, ip: &str) -> bool {
    let repo = IpBanRepository::new(state.db.pool());
    let Ok(settings) = repo.settings().await else {
        // A database hiccup must not throttle anybody.
        return true;
    };
    let settings = settings.clamped();
    if dos_limiter(&settings).is_none() {
        return true;
    }
    let rpm = settings.dos_requests_per_minute;
    let limiter = {
        let mut guard = DOS_STATE.lock();
        match guard.as_ref() {
            Some((current, limiter)) if *current == rpm => limiter.clone(),
            _ => {
                let limiter = std::sync::Arc::new(
                    crate::middleware::rate_limit::RateLimitState::new(rpm, 60),
                );
                *guard = Some((rpm, limiter.clone()));
                limiter
            }
        }
    };
    limiter.check_rate_limit(ip).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AutoBlockSettings {
        AutoBlockSettings {
            enabled: true,
            ..AutoBlockSettings::default()
        }
    }

    // ---------------------------------------------------------------------
    // The ban decision
    // ---------------------------------------------------------------------

    #[test]
    fn the_threshold_is_the_number_of_failures_that_trips_the_ban() {
        let s = settings(); // threshold 5
        assert!(!should_ban(4, &s));
        assert!(should_ban(5, &s), "the fifth failure trips it");
        assert!(should_ban(6, &s));
    }

    #[test]
    fn a_disabled_policy_never_bans_whatever_the_count() {
        let s = AutoBlockSettings {
            enabled: false,
            ..AutoBlockSettings::default()
        };
        assert!(!should_ban(9_999, &s));
    }

    #[test]
    fn the_ban_window_starts_now_and_lasts_the_configured_time() {
        let s = AutoBlockSettings {
            ban_seconds: 1800,
            ..settings()
        };
        let now = DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (created, expires) = ban_window(now, &s);
        assert_eq!(created, "2026-10-31T12:00:00+00:00");
        assert_eq!(expires, "2026-10-31T12:30:00+00:00");
        assert!(expires > created, "RFC 3339 UTC sorts lexicographically");
    }

    // ---------------------------------------------------------------------
    // Never block ourselves
    // ---------------------------------------------------------------------

    #[test]
    fn loopback_and_unspecified_addresses_are_never_banned() {
        for ip in [
            "127.0.0.1",
            "127.0.0.53",
            "::1",
            "0.0.0.0",
            "::",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_protected_address(ip), "{ip} must be protected");
        }
        for ip in ["192.168.1.20", "10.0.0.5", "203.0.113.9", "2001:db8::1"] {
            assert!(!is_protected_address(ip), "{ip} may be banned");
        }
    }

    #[test]
    fn a_value_that_is_not_an_address_is_never_banned() {
        // The limiter's "unknown" and an empty string have nothing to block; a
        // firewall rule built from them would be forged input.
        for ip in ["", "unknown", "not-an-ip", "192.168.1.1/24"] {
            assert!(is_protected_address(ip), "{ip:?} must not be banned");
        }
    }

    #[test]
    fn the_caller_never_bans_its_own_address() {
        // An admin mistyping their password five times must not lock themselves
        // out of the box they are configuring.
        assert!(must_not_ban("203.0.113.9", "203.0.113.9"));
        assert!(must_not_ban(" 203.0.113.9 ", "203.0.113.9"));
        assert!(!must_not_ban("198.51.100.4", "203.0.113.9"));
        // …and the protected addresses are protected whatever the caller is.
        assert!(must_not_ban("127.0.0.1", "203.0.113.9"));
    }

    // ---------------------------------------------------------------------
    // Settings
    // ---------------------------------------------------------------------

    #[test]
    fn the_defaults_are_off_with_a_thirty_minute_ban_and_a_dos_cap() {
        let s = AutoBlockSettings::default();
        assert!(!s.enabled, "a home box is not scanned by default");
        assert_eq!(s.threshold, 5);
        assert_eq!(s.window_seconds, 300);
        assert_eq!(s.ban_seconds, 1800);
        assert!(!s.apply_firewall, "locking every service is opt-in");
        assert!(s.dos_enabled);
        assert_eq!(s.dos_requests_per_minute, 600);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn a_disabled_policy_may_keep_any_numbers() {
        let s = AutoBlockSettings {
            enabled: false,
            threshold: 0,
            ban_seconds: 0,
            window_seconds: 0,
            dos_enabled: true,
            dos_requests_per_minute: 0,
            ..AutoBlockSettings::default()
        };
        assert!(s.validate().is_ok(), "off is always valid");
    }

    #[test]
    fn an_enabled_policy_is_validated_field_by_field() {
        let cases: Vec<(&str, AutoBlockSettings)> = vec![
            (
                "threshold 0",
                AutoBlockSettings {
                    threshold: 0,
                    ..settings()
                },
            ),
            (
                "threshold above the cap",
                AutoBlockSettings {
                    threshold: MAX_THRESHOLD + 1,
                    ..settings()
                },
            ),
            (
                "a zero ban blocks nobody",
                AutoBlockSettings {
                    ban_seconds: 0,
                    ..settings()
                },
            ),
            (
                "a zero window",
                AutoBlockSettings {
                    window_seconds: 0,
                    ..settings()
                },
            ),
            (
                "a window above the cap",
                AutoBlockSettings {
                    window_seconds: MAX_WINDOW_SECONDS + 1,
                    ..settings()
                },
            ),
            (
                "a ban above the cap",
                AutoBlockSettings {
                    ban_seconds: MAX_BAN_SECONDS + 1,
                    ..settings()
                },
            ),
            (
                "a zero rate cap while the DoS row is on",
                AutoBlockSettings {
                    dos_enabled: true,
                    dos_requests_per_minute: 0,
                    ..settings()
                },
            ),
        ];
        for (label, s) in cases {
            assert!(s.validate().is_err(), "{label} must be refused");
        }
        // A zero cap is fine once the DoS row is off.
        let dos_off = AutoBlockSettings {
            dos_enabled: false,
            dos_requests_per_minute: 0,
            ..settings()
        };
        assert!(dos_off.validate().is_ok());
    }

    #[test]
    fn clamped_brings_a_hand_edited_row_back_into_range() {
        let s = AutoBlockSettings {
            enabled: true,
            threshold: 0,
            window_seconds: u32::MAX,
            ban_seconds: u32::MAX,
            dos_requests_per_minute: u32::MAX,
            ..AutoBlockSettings::default()
        }
        .clamped();
        assert_eq!(s.threshold, MIN_THRESHOLD);
        assert_eq!(s.window_seconds, MAX_WINDOW_SECONDS);
        assert_eq!(s.ban_seconds, MAX_BAN_SECONDS);
        assert_eq!(s.dos_requests_per_minute, MAX_DOS_REQUESTS_PER_MINUTE);
        assert!(s.validate().is_ok(), "clamped values are always legal");
    }

    // ---------------------------------------------------------------------
    // Ban rows
    // ---------------------------------------------------------------------

    fn ban(expires_at: &str, firewall_rule: bool) -> IpBan {
        IpBan {
            id: 1,
            ip: "203.0.113.9".to_string(),
            reason: "5 failed sign-ins in 5 minutes".to_string(),
            failures: 5,
            created_at: "2026-10-31T12:00:00+00:00".to_string(),
            expires_at: expires_at.to_string(),
            firewall_rule,
        }
    }

    #[test]
    fn a_ban_is_active_only_before_its_expiry() {
        let now = "2026-10-31T12:10:00+00:00";
        assert!(ban("2026-10-31T12:30:00+00:00", false).is_active(now));
        // Exactly at the expiry it is over: a ban that lingers one extra second
        // is a ban the admin has to clear by hand.
        assert!(!ban("2026-10-31T12:10:00+00:00", false).is_active(now));
        assert!(!ban("2026-10-31T11:00:00+00:00", false).is_active(now));
    }

    #[test]
    fn the_remaining_time_is_reported_and_never_negative() {
        let now = "2026-10-31T12:10:00+00:00";
        assert_eq!(
            ban("2026-10-31T12:30:00+00:00", false).remaining_seconds(now),
            1200
        );
        assert_eq!(
            ban("2026-10-31T11:00:00+00:00", false).remaining_seconds(now),
            0
        );
        // An unparsable stamp reports 0 rather than a nonsense number.
        assert_eq!(ban("whenever", false).remaining_seconds(now), 0);
    }

    #[test]
    fn remaining_minutes_rounds_up_so_a_partial_minute_still_says_one() {
        assert_eq!(remaining_minutes(0), 0);
        assert_eq!(remaining_minutes(1), 1);
        assert_eq!(remaining_minutes(60), 1);
        assert_eq!(remaining_minutes(61), 2);
        assert_eq!(remaining_minutes(1800), 30);
    }

    // ---------------------------------------------------------------------
    // Wording and the DoS cap
    // ---------------------------------------------------------------------

    #[test]
    fn the_blocked_message_names_the_wait_and_how_to_lift_it() {
        let soon = (Utc::now() + chrono::Duration::minutes(12)).to_rfc3339();
        let message = blocked_message(&soon);
        assert!(message.contains("temporarily blocked"), "{message}");
        assert!(message.contains("minute"), "{message}");
        assert!(
            message.contains("administrator"),
            "a user must know a human can lift it: {message}"
        );
        // An expired or unparsable stamp never produces a negative wait.
        let past = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert!(blocked_message(&past).contains("about 0 minute"));
        assert!(blocked_message("nonsense").contains("about 0 minute"));
    }

    #[test]
    fn the_dos_cap_is_one_minute_window_or_nothing() {
        let on = settings();
        assert_eq!(dos_limiter(&on), Some(Duration::from_secs(60)));
        let off = AutoBlockSettings {
            dos_enabled: false,
            ..settings()
        };
        assert_eq!(dos_limiter(&off), None);
        let zero = AutoBlockSettings {
            dos_enabled: true,
            dos_requests_per_minute: 0,
            ..settings()
        };
        assert_eq!(
            dos_limiter(&zero),
            None,
            "0 is off, not 'no requests at all'"
        );
    }

    #[test]
    fn a_firewall_comment_can_never_break_out_of_its_quotes() {
        let comment = ban_comment("203.0.113.9");
        assert!(comment.starts_with(BAN_COMMENT_PREFIX));
        assert!(comment.contains("203.0.113.9"));
        // ufw re-reads comments single-quoted; a quote in ours would break the
        // round-trip and make the pardon unable to find the rule.
        assert!(!comment.contains('\''));
    }
}
