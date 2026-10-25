/* =============================================================================
 * File          : transfer_quota.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Enforcement of the configured upload/download transfer quotas for
 *   users and groups: counters tracked against the limits, checked before
 *   transfers proceed. Distinct from the filesystem disk-space quota
 *   system in quota.rs (which caps home-folder size, not network
 *   throughput volume).
 *
 * Dependencies:
 *   - internal storage layer (transfer quota repository)
 *
 * Usage:
 *   Consulted by the transfer-handling paths; limits configured on the
 *   Users/Groups pages via handlers/transfer_quotas.rs.
 * =============================================================================
 */

//! Enforcement for the upload/download transfer quotas configured on the
//! Users/Groups pages (see `src/storage/transfer_quota_repo.rs` for the
//! storage layer). Distinct from the pre-existing filesystem disk-space
//! quota system in `src/quota.rs`.
//!
//! Only File Explorer uploads/downloads go through this — that's the one
//! transfer channel HomeVault's own process fully controls end-to-end.
//! SMB/NFS/FTP/WebDAV shares are served by separate system daemons with no
//! per-request visibility from here, so they are **not** covered by this
//! enforcement.
//!
//! A transfer is checked against the acting user's own quota (if enabled)
//! *and* every group they belong to that has a quota enabled — all of
//! those that apply must have room, and a successful transfer debits usage
//! against all of them. This matches common "personal cap + shared team
//! cap" semantics.
//!
//! ## Counting periods (ROADMAP 23.2)
//!
//! A quota is counted in a [`Period`]: `lifetime` (the default — the counters
//! never reset), or a **calendar** window in the appliance timezone (`day` =
//! midnight to midnight, `week` = Monday 00:00, `month` = the 1st at 00:00).
//! A calendar window is what a home user means by "100 GB/month": it resets on
//! the 1st, not 30 days after their first upload.
//!
//! The window is opened at [`current_window_start`] when the period is saved,
//! and closed by two paths that agree by construction because they share
//! [`window_containing`]: the enforcement path ([`settle_window`], which zeroes
//! a stale row the moment it is consulted) and [`run_reset_loop`] (which keeps
//! the Quotas/Users screens honest even when nobody transfers). Enforcement
//! never depends on the loop having run: a stopped scheduler delays the
//! *display* of the reset, never the reset itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;

use std::time::{Duration, Instant};

use crate::groups::GroupService;

/// How long a resolved group membership is reused.
///
/// ROADMAP 33.1: `load_applicable` forked `id -nG` on **every** quota check,
/// and the resumable-upload path re-checks per chunk — a 1 GB upload in 8 MB
/// chunks forked ~128 times. The lookup is correctly inside `spawn_blocking`
/// (it never blocks the runtime), so this was throughput debt, not a stall.
///
/// Five seconds is the trade: a group added to an account (or removed from
/// one) applies to quota checks within one TTL, which is far below the time a
/// user takes to notice, and it collapses a chunked upload — or a burst of
/// Explorer requests — to a single fork. Deliberately **not** longer: the
/// bounded staleness has to be shorter than "a permission change that looks
/// like it did nothing".
pub const GROUP_MEMBERSHIP_TTL: Duration = Duration::from_secs(5);

/// A cached `id -nG` result, keyed by username.
struct CachedGroups {
    groups: Vec<String>,
    resolved_at: Instant,
}

/// Process-wide cache: one entry per account that has been checked recently.
///
/// Bounded by the number of accounts (the appliance's user table), so it needs
/// no eviction policy beyond the TTL check — but [`prune_group_cache`] drops
/// expired entries anyway so a long-running process does not hold a `Vec` per
/// account that ever transferred a file.
static GROUP_CACHE: LazyLock<Mutex<HashMap<String, CachedGroups>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The cached groups for `username`, if any and still fresh.
fn cached_groups(username: &str, now: Instant) -> Option<Vec<String>> {
    let cache = GROUP_CACHE.lock();
    cache.get(username).and_then(|entry| {
        if now.saturating_duration_since(entry.resolved_at) < GROUP_MEMBERSHIP_TTL {
            Some(entry.groups.clone())
        } else {
            None
        }
    })
}

/// Stores a freshly resolved group list.
fn store_groups(username: &str, groups: Vec<String>, now: Instant) {
    let mut cache = GROUP_CACHE.lock();
    cache.insert(
        username.to_string(),
        CachedGroups {
            groups,
            resolved_at: now,
        },
    );
}

/// Drops every expired entry. Called opportunistically by [`resolve_groups`].
fn prune_group_cache(now: Instant) {
    let mut cache = GROUP_CACHE.lock();
    cache
        .retain(|_, entry| now.saturating_duration_since(entry.resolved_at) < GROUP_MEMBERSHIP_TTL);
}

/// Forgets one account's cached groups — used after a membership change so the
/// next check is immediate. Public so the Groups screen can call it.
pub fn invalidate_group_cache(username: &str) {
    GROUP_CACHE.lock().remove(username);
}

/// Empties the cache (tests, and a settings import that rewrites accounts).
pub fn clear_group_cache() {
    GROUP_CACHE.lock().clear();
}

/// Resolves `username`'s group membership through the cache, forking `id -nG`
/// only on a miss.
///
/// The fork stays inside `spawn_blocking`: this is called from async handlers
/// and must never run a subprocess on a runtime thread.
async fn resolve_groups(username: &str) -> Result<Vec<String>, String> {
    let now = Instant::now();
    if let Some(groups) = cached_groups(username, now) {
        return Ok(groups);
    }

    let username_owned = username.to_string();
    let groups =
        tokio::task::spawn_blocking(move || GroupService::get_user_groups(&username_owned))
            .await
            .map_err(|e| format!("Failed to resolve group membership: {}", e))?
            .unwrap_or_default();

    // A miss is also the cheap moment to prune: the map is already locked once
    // per miss, and a hit must stay a pure lookup.
    store_groups(username, groups.clone(), now);
    prune_group_cache(now);
    Ok(groups)
}
use crate::storage::transfer_quota_repo::{TransferQuota, TransferQuotaRepository};

/// One quota that applies to the current transfer (the user's own, or one
/// of their groups').
#[derive(Debug, Clone)]
pub struct ApplicableQuota {
    pub subject_type: &'static str,
    pub subject_name: String,
    pub limit_bytes: i64,
    pub used_bytes: i64,
}

/// Error returned when a transfer would exceed one of the applicable
/// quotas. Carries enough detail for a clear, user-facing message.
#[derive(Debug, Clone)]
pub struct QuotaExceeded {
    pub subject_type: &'static str,
    pub subject_name: String,
    pub limit_bytes: i64,
}

impl std::fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Transfer quota exceeded for {} '{}' (limit: {} bytes)",
            self.subject_type, self.subject_name, self.limit_bytes
        )
    }
}

/// The window a quota's counters are counted in.
///
/// `Lifetime` is the default and preserves the behaviour every existing row
/// had before ROADMAP 23.2: the counters only ever grow, until an admin edits
/// the row. The three others are calendar windows — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Lifetime,
    Day,
    Week,
    Month,
}

impl Period {
    /// Every accepted value, in the order the UI offers them.
    pub const ALL: [Period; 4] = [Period::Lifetime, Period::Day, Period::Week, Period::Month];

    /// The value stored in `transfer_quotas.period` and sent over the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Period::Lifetime => "lifetime",
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
        }
    }

    /// The single validation gate for a stored or submitted period. The name
    /// is trimmed and lower-cased first, and an unknown value is a **named
    /// error** listing the accepted ones — never a silent fall back to
    /// `lifetime`, which would quietly turn "per month" into "for ever".
    pub fn parse(value: &str) -> Result<Period, String> {
        let normalised = value.trim().to_ascii_lowercase();
        Period::ALL
            .into_iter()
            .find(|p| p.as_str() == normalised)
            .ok_or_else(|| {
                format!(
                    "period must be one of: {}",
                    Period::ALL.map(Period::as_str).join(", ")
                )
            })
    }
}

/// Converts a local midnight to the absolute instant it happens at in `tz`.
///
/// A local midnight does not always exist: zones that shift their clocks at
/// 00:00 (America/Santiago, Asia/Tehran, …) skip straight to 01:00 on the
/// spring-forward day, and `from_local_datetime` answers `None`. Probing the
/// next two hours and falling back to the UTC reading keeps a quota window
/// from ever panicking on one calendar day a year.
fn local_midnight(tz: Tz, date: NaiveDate) -> DateTime<Utc> {
    let naive = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time of day");
    for hours in 0..3 {
        let candidate = naive + chrono::Duration::hours(hours);
        if let Some(dt) = tz.from_local_datetime(&candidate).earliest() {
            return dt.with_timezone(&Utc);
        }
    }
    naive.and_utc()
}

/// The calendar window that contains `at`, as `(start, end)` where `end` is
/// the start of the *next* window (so `start <= at < end`).
///
/// `None` for [`Period::Lifetime`], which has no window. Both the reset loop
/// and the enforcement path go through this one function, which is why they
/// can never disagree about when a window ends.
pub fn window_containing(
    at: DateTime<Utc>,
    period: Period,
    tz: Tz,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let today = at.with_timezone(&tz).date_naive();
    let (start_date, end_date) = match period {
        Period::Lifetime => return None,
        Period::Day => (today, today.succ_opt()?),
        Period::Week => {
            // Monday-first, the ISO week the UI shows: `num_days_from_monday`
            // is 0 on Monday, so this lands on the current week's Monday.
            let monday = today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()));
            (monday, monday.checked_add_days(chrono::Days::new(7))?)
        }
        Period::Month => (
            NaiveDate::from_ymd_opt(today.year(), today.month(), 1)?,
            if today.month() == 12 {
                NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)?
            } else {
                NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)?
            },
        ),
    };
    Some((local_midnight(tz, start_date), local_midnight(tz, end_date)))
}

/// The instant the window currently containing `now` opened. `now` itself for
/// [`Period::Lifetime`] (there is no window to open).
pub fn current_window_start(now: DateTime<Utc>, period: Period, tz: Tz) -> DateTime<Utc> {
    window_containing(now, period, tz)
        .map(|(start, _)| start)
        .unwrap_or(now)
}

/// Whether a stored window start is behind the window `now` falls in — i.e.
/// the counters it belongs to must be zeroed before they are trusted.
///
/// `None` (a row written before migration 055, or a lifetime row that just
/// gained a period) counts as elapsed: the window is opened on first sight.
/// An unparseable timestamp counts as elapsed too — a hand-edited row must
/// not be able to freeze a counter for ever.
pub fn window_elapsed(
    stored_start: Option<&str>,
    now: DateTime<Utc>,
    period: Period,
    tz: Tz,
) -> bool {
    let Some((current_start, _)) = window_containing(now, period, tz) else {
        return false; // lifetime: nothing ever resets
    };
    let Some(stored) = stored_start
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
    else {
        return true;
    };
    stored < current_start
}

/// When the counters of `quota` will next reset, as an RFC3339 instant —
/// `None` for a lifetime quota. Computed for the wire (the storage layer never
/// writes it); a row whose stored window is already stale reports the end of
/// the *current* window, never a date in the past.
pub fn resets_at(
    period: &str,
    period_started_at: Option<&str>,
    now: DateTime<Utc>,
    tz: Tz,
) -> Option<String> {
    let period = Period::parse(period).ok()?;
    let stored = period_started_at
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    let anchor = match stored {
        Some(start) if !window_elapsed(Some(&start.to_rfc3339()), now, period, tz) => start,
        _ => now,
    };
    window_containing(anchor, period, tz).map(|(_, end)| end.to_rfc3339())
}

/// The appliance timezone (Settings → General, `UTC` when unset or invalid).
/// Pure in-memory read — safe to call from the enforcement path.
pub fn appliance_timezone() -> Tz {
    crate::scheduled_tasks_scheduler::parse_timezone(&crate::handlers::settings::effective_timezone())
        .unwrap_or(chrono_tz::UTC)
}

/// Fills `period_resets_at` on a row about to be serialised to the UI.
pub fn fill_resets_at(quota: &mut TransferQuota, now: DateTime<Utc>, tz: Tz) {
    quota.period_resets_at = resets_at(
        &quota.period,
        quota.period_started_at.as_deref(),
        now,
        tz,
    );
}

/// Brings one stored row's window up to date before its counters are trusted:
/// when the window it was counted in has elapsed, both counters are zeroed in
/// the database and in the returned row, and the window is re-opened at the
/// current calendar boundary.
///
/// The common case writes nothing at all — the staleness check is pure.
async fn settle_window(
    repo: &TransferQuotaRepository,
    mut quota: TransferQuota,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<TransferQuota, String> {
    let period = Period::parse(&quota.period).unwrap_or(Period::Lifetime);
    if !window_elapsed(quota.period_started_at.as_deref(), now, period, tz) {
        return Ok(quota);
    }
    let new_start = current_window_start(now, period, tz).to_rfc3339();
    repo.reset_window(&quota.subject_type, &quota.subject_name, &new_start)
        .await
        .map_err(|e| format!("Failed to reset an elapsed quota window: {e}"))?;
    quota.upload_used_bytes = 0;
    quota.download_used_bytes = 0;
    quota.period_started_at = Some(new_start);
    Ok(quota)
}

/// Load every *enabled* upload quota that applies to `username`: their own
/// (if configured) plus every group they belong to that has one. Group
/// membership is resolved via `id -nG`, the same mechanism
/// `GroupService::get_user_groups` already uses elsewhere — wrapped in
/// `spawn_blocking` since it shells out.
pub async fn applicable_upload_quotas(
    pool: Arc<sqlx::SqlitePool>,
    username: &str,
) -> Result<Vec<ApplicableQuota>, String> {
    load_applicable(pool, username, true).await
}

/// Same as [`applicable_upload_quotas`] but for the download direction.
pub async fn applicable_download_quotas(
    pool: Arc<sqlx::SqlitePool>,
    username: &str,
) -> Result<Vec<ApplicableQuota>, String> {
    load_applicable(pool, username, false).await
}

/// The one sentence every caller uses when the guard itself cannot answer.
/// Identical wording in every File Explorer path on purpose: the user sees
/// one behaviour, and the log line carries the technical detail.
pub fn unavailable_message(detail: &str) -> String {
    format!(
        "Transfer quotas could not be checked, so this transfer was refused ({detail}). Try again in a moment."
    )
}

/// Reads the enabled limits out of a stored row for one direction.
fn direction(quota: &TransferQuota, upload: bool) -> (bool, i64, i64) {
    if upload {
        (
            quota.upload_enabled,
            quota.upload_limit_bytes,
            quota.upload_used_bytes,
        )
    } else {
        (
            quota.download_enabled,
            quota.download_limit_bytes,
            quota.download_used_bytes,
        )
    }
}

async fn load_applicable(
    pool: Arc<sqlx::SqlitePool>,
    username: &str,
    upload: bool,
) -> Result<Vec<ApplicableQuota>, String> {
    let repo = TransferQuotaRepository::new(pool);
    let mut applicable = Vec::new();
    // One instant and one zone for the whole check, so the user's own row and
    // every group row are settled against the same window.
    let now = Utc::now();
    let tz = appliance_timezone();

    let user_quota = repo
        .get("user", username)
        .await
        .map_err(|e| format!("Failed to load user quota: {}", e))?;
    let user_quota = settle_window(&repo, user_quota, now, tz).await?;
    let (enabled, limit, used) = direction(&user_quota, upload);
    if enabled {
        applicable.push(ApplicableQuota {
            subject_type: "user",
            subject_name: username.to_string(),
            limit_bytes: limit,
            used_bytes: used,
        });
    }

    // ROADMAP 33.1: one `id -nG` fork per username per TTL, not per check.
    let group_names = resolve_groups(username).await?;

    for group_name in group_names {
        let group_quota = repo
            .get("group", &group_name)
            .await
            .map_err(|e| format!("Failed to load group quota for '{}': {}", group_name, e))?;
        let group_quota = settle_window(&repo, group_quota, now, tz).await?;
        let (enabled, limit, used) = direction(&group_quota, upload);
        if enabled {
            applicable.push(ApplicableQuota {
                subject_type: "group",
                subject_name: group_name,
                limit_bytes: limit,
                used_bytes: used,
            });
        }
    }

    Ok(applicable)
}

/// Check whether transferring `additional_bytes` more would push any
/// applicable quota over its limit. Returns the first one that would be
/// exceeded, if any.
pub fn check_quota(quotas: &[ApplicableQuota], additional_bytes: u64) -> Result<(), QuotaExceeded> {
    for q in quotas {
        // Compare in u128 so a large `additional_bytes` can never wrap: a
        // plain `additional_bytes as i64` turned any value above i64::MAX
        // negative and silently passed the check. Negative stored counts
        // (never expected, but cheap to guard) clamp to 0.
        let projected = u128::from(q.used_bytes.max(0) as u64) + u128::from(additional_bytes);
        if projected > u128::from(q.limit_bytes.max(0) as u64) {
            return Err(QuotaExceeded {
                subject_type: q.subject_type,
                subject_name: q.subject_name.clone(),
                limit_bytes: q.limit_bytes,
            });
        }
    }
    Ok(())
}

/// `bytes` as an `i64`, saturating instead of wrapping. A `u64` above
/// `i64::MAX` used to become a *negative* debit, which silently *increased*
/// the remaining allowance of the subject it was charged to.
fn bytes_to_i64(bytes: u64) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

/// Record `bytes` of usage against every applicable quota (called once a
/// transfer actually completes — never for a rejected or aborted one).
pub async fn record_usage(
    pool: Arc<sqlx::SqlitePool>,
    quotas: &[ApplicableQuota],
    bytes: u64,
    upload: bool,
) {
    if bytes == 0 || quotas.is_empty() {
        return;
    }
    let repo = TransferQuotaRepository::new(pool);
    let delta = bytes_to_i64(bytes);
    for q in quotas {
        let result = if upload {
            repo.add_upload_usage(q.subject_type, &q.subject_name, delta)
                .await
        } else {
            repo.add_download_usage(q.subject_type, &q.subject_name, delta)
                .await
        };
        if let Err(e) = result {
            tracing::warn!(
                "Failed to record transfer quota usage for {} '{}': {}",
                q.subject_type,
                q.subject_name,
                e
            );
        }
    }
}

/// Undo a previously-recorded reservation, e.g. when a download is abandoned
/// before it completes. Mirrors [`record_usage`] but subtracts instead of
/// adds, so a client that disconnects mid-transfer is not left charged for
/// bytes it never received.
///
/// The counter is floored at 0 by the repository: a refund that arrives after
/// its window has been reset must not push the new window's usage negative.
pub async fn refund_usage(
    pool: Arc<sqlx::SqlitePool>,
    quotas: &[ApplicableQuota],
    bytes: u64,
    upload: bool,
) {
    if bytes == 0 || quotas.is_empty() {
        return;
    }
    let repo = TransferQuotaRepository::new(pool);
    let delta = -bytes_to_i64(bytes);
    for q in quotas {
        let result = if upload {
            repo.add_upload_usage(q.subject_type, &q.subject_name, delta)
                .await
        } else {
            repo.add_download_usage(q.subject_type, &q.subject_name, delta)
                .await
        };
        if let Err(e) = result {
            tracing::warn!(
                "Failed to refund transfer quota usage for {} '{}': {}",
                q.subject_type,
                q.subject_name,
                e
            );
        }
    }
}

/// In-process, per-subject locks serialising a transfer's check-then-debit
/// against every other transfer that touches the same subject(s). Without
/// this, two concurrent uploads/downloads of the same user (or two users
/// sharing a group quota) could both pass the "would this exceed the limit?"
/// check while usage is still low, then both debit — overshooting the cap.
///
/// A transfer holds the guard for its whole duration: the handler for an
/// upload, the response body's stream for a download (released on completion
/// or client disconnect). That serialises a single subject's concurrent
/// transfers, which is the correct behaviour for a hard cap.
pub struct QuotaLockGuard {
    _guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
}

static QUOTA_LOCKS: LazyLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Acquire the per-subject locks for every quota that applies to this
/// transfer. Keys are locked in sorted order so two transfers touching an
/// overlapping set of subjects can never deadlock.
pub async fn acquire_quota_locks(quotas: &[ApplicableQuota]) -> QuotaLockGuard {
    let mut keys: Vec<String> = quotas
        .iter()
        .map(|q| format!("{}:{}", q.subject_type, q.subject_name))
        .collect();
    keys.sort();
    keys.dedup();

    {
        // Drop the mutexes nobody holds any more — one entry per subject that
        // ever transferred used to accumulate for the life of the process.
        // `strong_count == 1` means the map is the only owner: a key that is
        // held by a transfer (or already cloned by another `acquire`) counts
        // at least twice and is kept, so a subject can never end up with two
        // live mutexes.
        let mut map = QUOTA_LOCKS.lock();
        map.retain(|_, lock| Arc::strong_count(lock) > 1);
    }

    let mut guards = Vec::with_capacity(keys.len());
    for key in keys {
        let lock = {
            let mut map = QUOTA_LOCKS.lock();
            map.entry(key)
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        guards.push(lock.lock_owned().await);
    }
    QuotaLockGuard { _guards: guards }
}

/// How often [`run_reset_loop`] re-opens elapsed windows. A minute is well
/// below the shortest period (a day), and the loop writes only when a row is
/// actually stale, so an idle appliance does no work here.
const RESET_POLL_SECS: u64 = 60;

/// Zeroes the counters of every row whose counting window has elapsed, for
/// every period. Returns how many rows were reset.
pub async fn reset_expired_periods(
    pool: Arc<sqlx::SqlitePool>,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<u64, sqlx::Error> {
    let repo = TransferQuotaRepository::new(pool);
    let mut total = 0u64;
    for period in [Period::Day, Period::Week, Period::Month] {
        // The cutoff is the start of the window `now` falls in: any row whose
        // window opened earlier belongs to a previous period, however many
        // periods ago it was (an appliance off for a month still resets once).
        let boundary = current_window_start(now, period, tz).to_rfc3339();
        total += repo
            .reset_expired(period.as_str(), &boundary, &boundary)
            .await?;
    }
    Ok(total)
}

/// Background loop that keeps the stored counters in their current window even
/// when nobody transfers, so the Quotas and Users screens show a reset that
/// actually happened.
///
/// Enforcement does **not** depend on this loop: `load_applicable` settles a
/// stale row itself. A stopped scheduler can therefore only delay what the
/// screens display, never let a transfer through.
pub async fn run_reset_loop(pool: Arc<sqlx::SqlitePool>) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(RESET_POLL_SECS));
    // `Burst` would fire every missed tick back-to-back after a suspend, and
    // each of those would run a write pass for nothing.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match reset_expired_periods(pool.clone(), Utc::now(), appliance_timezone()).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("Transfer quota windows reset for {} row(s)", n),
            Err(e) => tracing::warn!("Transfer quota reset pass failed: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn q(name: &str, limit: i64, used: i64) -> ApplicableQuota {
        ApplicableQuota {
            subject_type: "user",
            subject_name: name.to_string(),
            limit_bytes: limit,
            used_bytes: used,
        }
    }

    #[test]
    fn test_check_quota_no_quotas_always_ok() {
        assert!(check_quota(&[], 0).is_ok());
        assert!(check_quota(&[], u64::MAX).is_ok());
    }

    #[test]
    fn test_check_quota_under_limit_is_ok() {
        let quotas = [q("alice", 1_000, 400)];
        assert!(check_quota(&quotas, 599).is_ok());
    }

    #[test]
    fn test_check_quota_exactly_at_limit_is_ok() {
        // Only strictly *over* the limit is rejected — landing on it is fine.
        let quotas = [q("alice", 1_000, 400)];
        assert!(check_quota(&quotas, 600).is_ok());
    }

    #[test]
    fn test_check_quota_one_byte_over_limit_is_rejected() {
        let quotas = [q("alice", 1_000, 400)];
        let err = check_quota(&quotas, 601).unwrap_err();
        assert_eq!(err.subject_name, "alice");
        assert_eq!(err.subject_type, "user");
        assert_eq!(err.limit_bytes, 1_000);
    }

    #[test]
    fn test_check_quota_already_over_limit_rejects_even_a_zero_byte_transfer() {
        let quotas = [q("alice", 1_000, 1_500)];
        assert!(check_quota(&quotas, 0).is_err());
    }

    #[test]
    fn test_check_quota_returns_the_first_exceeded_quota() {
        // The user's own quota has room; the group quota does not — the
        // group one must be the one reported.
        let quotas = [
            q("alice", 10_000, 100),
            ApplicableQuota {
                subject_type: "group",
                subject_name: "team".to_string(),
                limit_bytes: 500,
                used_bytes: 490,
            },
        ];
        let err = check_quota(&quotas, 50).unwrap_err();
        assert_eq!(err.subject_type, "group");
        assert_eq!(err.subject_name, "team");
    }

    #[test]
    fn test_check_quota_huge_transfer_saturates_instead_of_overflowing() {
        // `used + additional` must not wrap: `u64::MAX` as an i64 is -1, which
        // used to slip under the limit. It must now be rejected.
        let quotas = [q("alice", i64::MAX, 0)];
        assert!(check_quota(&quotas, u64::MAX).is_err());
    }

    #[test]
    fn test_check_quota_multi_terabyte_values_do_not_overflow_i64() {
        // Realistic large transfer: a 2 TiB cap with 1.9 TiB used, and a
        // 200 GiB upload — must be rejected without the i64 add wrapping.
        const TIB: i64 = 1024 * 1024 * 1024 * 1024;
        let quotas = [q("alice", 2 * TIB, 19 * TIB / 10)];
        assert!(check_quota(&quotas, 200 * 1024 * 1024 * 1024).is_err());
        // A 50 GiB upload still fits.
        assert!(check_quota(&quotas, 50 * 1024 * 1024 * 1024).is_ok());
    }

    #[test]
    fn test_quota_exceeded_display_is_human_readable() {
        let e = QuotaExceeded {
            subject_type: "group",
            subject_name: "team".to_string(),
            limit_bytes: 42,
        };
        let s = e.to_string();
        assert!(s.contains("group"));
        assert!(s.contains("team"));
        assert!(s.contains("42"));
    }

    // --- ROADMAP 23.2: counting periods -------------------------------------

    #[test]
    fn test_period_parse_accepts_the_four_documented_values() {
        assert_eq!(Period::parse("lifetime").unwrap(), Period::Lifetime);
        assert_eq!(Period::parse("day").unwrap(), Period::Day);
        assert_eq!(Period::parse("week").unwrap(), Period::Week);
        assert_eq!(Period::parse("month").unwrap(), Period::Month);
        // Tolerant of what a client or an admin hand-edits into the column.
        assert_eq!(Period::parse("  MONTH ").unwrap(), Period::Month);
    }

    #[test]
    fn test_period_parse_refuses_unknown_values_with_a_named_error() {
        for bad in ["", "   ", "yearly", "30d", "lifetime2"] {
            let err = Period::parse(bad).unwrap_err();
            assert!(
                err.contains("lifetime") && err.contains("day") && err.contains("month"),
                "the error must list the accepted values, got: {err}"
            );
        }
    }

    #[test]
    fn test_period_round_trips_through_its_wire_name() {
        for period in Period::ALL {
            assert_eq!(Period::parse(period.as_str()).unwrap(), period);
        }
    }

    #[test]
    fn test_window_containing_is_none_for_a_lifetime_quota() {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        assert_eq!(window_containing(now, Period::Lifetime, chrono_tz::UTC), None);
        assert!(!window_elapsed(None, now, Period::Lifetime, chrono_tz::UTC));
        assert_eq!(resets_at("lifetime", None, now, chrono_tz::UTC), None);
    }

    #[test]
    fn test_day_window_is_midnight_to_midnight() {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        let (start, end) = window_containing(now, Period::Day, chrono_tz::UTC).unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 10, 23, 0, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2026, 10, 24, 0, 0, 0).unwrap());
        assert!(start <= now && now < end);
    }

    #[test]
    fn test_week_window_starts_on_monday() {
        // 2026-10-23 is a Friday; its ISO week opened on Monday the 19th.
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        let (start, end) = window_containing(now, Period::Week, chrono_tz::UTC).unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap());

        // A Monday is its own window start, not the previous Monday's end.
        let monday = Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap();
        assert_eq!(window_containing(monday, Period::Week, chrono_tz::UTC).unwrap().0, monday);
    }

    #[test]
    fn test_month_window_starts_on_the_first() {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        let (start, end) = window_containing(now, Period::Month, chrono_tz::UTC).unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn test_month_window_rolls_the_year_over_in_december() {
        let now = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        let (start, end) = window_containing(now, Period::Month, chrono_tz::UTC).unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn test_windows_follow_the_appliance_timezone_not_utc() {
        let tz = chrono_tz::Europe::Paris;
        // 2026-10-31 23:30 UTC is already 1 November in Paris (CET, +01:00),
        // so a monthly quota has rolled over even though UTC says October.
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 23, 30, 0).unwrap();
        let (start, _) = window_containing(now, Period::Month, tz).unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 10, 31, 23, 0, 0).unwrap());
        // The same instant is still October in UTC.
        let (utc_start, _) = window_containing(now, Period::Month, chrono_tz::UTC).unwrap();
        assert_eq!(utc_start, Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn test_a_window_boundary_across_a_dst_change_keeps_the_local_midnight() {
        // Europe/Paris leaves summer time on 2026-10-25: midnight on the 26th
        // is 23:00 UTC the day before, not 22:00.
        let tz = chrono_tz::Europe::Paris;
        let now = Utc.with_ymd_and_hms(2026, 10, 26, 12, 0, 0).unwrap();
        let (start, end) = window_containing(now, Period::Day, tz).unwrap();
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 10, 25, 23, 0, 0).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2026, 10, 26, 23, 0, 0).unwrap());
    }

    #[test]
    fn test_window_elapsed_compares_against_the_current_window_start() {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        let utc = chrono_tz::UTC;
        // A window opened yesterday is over.
        assert!(window_elapsed(
            Some("2026-10-22T00:00:00+00:00"),
            now,
            Period::Day,
            utc
        ));
        // One opened today is the current one.
        assert!(!window_elapsed(
            Some("2026-10-23T00:00:00+00:00"),
            now,
            Period::Day,
            utc
        ));
        // Never opened, or unreadable: the window must be opened now.
        assert!(window_elapsed(None, now, Period::Day, utc));
        assert!(window_elapsed(Some(""), now, Period::Day, utc));
        assert!(window_elapsed(Some("not a timestamp"), now, Period::Day, utc));
    }

    #[test]
    fn test_a_month_old_row_is_elapsed_however_long_it_sat_idle() {
        // The appliance was off for three months: one reset pass must be
        // enough, and it must land on the current calendar window.
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        let tz = chrono_tz::UTC;
        assert!(window_elapsed(
            Some("2026-07-04T00:00:00+00:00"),
            now,
            Period::Month,
            tz
        ));
        assert_eq!(
            current_window_start(now, Period::Month, tz),
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_resets_at_reports_the_end_of_the_current_window() {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        let utc = chrono_tz::UTC;
        assert_eq!(
            resets_at("month", Some("2026-10-01T00:00:00+00:00"), now, utc).unwrap(),
            "2026-11-01T00:00:00+00:00"
        );
        // A stale stored window reports the *current* window's end, never a
        // date in the past (which would read as "resets yesterday").
        assert_eq!(
            resets_at("month", Some("2026-08-01T00:00:00+00:00"), now, utc).unwrap(),
            "2026-11-01T00:00:00+00:00"
        );
    }

    #[test]
    fn test_fill_resets_at_leaves_a_lifetime_row_alone() {
        let mut quota = TransferQuota::default_for("user", "alice");
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 14, 30, 0).unwrap();
        fill_resets_at(&mut quota, now, chrono_tz::UTC);
        assert_eq!(quota.period_resets_at, None);
    }

    #[test]
    fn test_bytes_to_i64_saturates_instead_of_going_negative() {
        assert_eq!(bytes_to_i64(0), 0);
        assert_eq!(bytes_to_i64(4_096), 4_096);
        assert_eq!(bytes_to_i64(u64::MAX), i64::MAX);
        // The whole point: the saturated value stays positive, so it debits.
        assert!(bytes_to_i64(u64::MAX) > 0);
    }

    #[test]
    fn test_unavailable_message_names_the_transfer_and_the_reason() {
        let msg = unavailable_message("database is locked");
        assert!(msg.contains("refused"), "{msg}");
        assert!(msg.contains("database is locked"), "{msg}");
    }

    // -----------------------------------------------------------------
    // ROADMAP 33.1 — one `id -nG` fork per username per TTL
    // -----------------------------------------------------------------

    /// A second lookup inside the TTL is served from the cache: this is the
    /// whole point of 33.1 — the resumable-upload path re-checks per chunk, so
    /// a 1 GB upload in 8 MB chunks forked ~128 times.
    #[test]
    fn a_fresh_entry_is_served_from_the_cache() {
        clear_group_cache();
        let now = Instant::now();
        store_groups("alice", vec!["homevault_media".into()], now);
        assert_eq!(
            cached_groups("alice", now + Duration::from_secs(1)),
            Some(vec!["homevault_media".to_string()])
        );
        clear_group_cache();
    }

    /// Past the TTL the entry is a miss, so a group added to the account applies
    /// within one window. This is the bounded staleness documented in
    /// `docs/guides/transfer-quotas.md`.
    #[test]
    fn an_expired_entry_is_a_miss() {
        clear_group_cache();
        let now = Instant::now();
        store_groups("bob", vec!["homevault_media".into()], now);
        assert!(
            cached_groups("bob", now + GROUP_MEMBERSHIP_TTL).is_none(),
            "the TTL boundary must be exclusive"
        );
        // …and exactly at the edge minus a nanosecond it is still fresh.
        assert!(
            cached_groups("bob", now + GROUP_MEMBERSHIP_TTL - Duration::from_nanos(1)).is_some()
        );
        clear_group_cache();
    }

    /// Accounts are keyed independently: one user's cached groups never answer
    /// for another (the cross-account leak this cache could introduce).
    #[test]
    fn the_cache_is_keyed_per_username() {
        clear_group_cache();
        let now = Instant::now();
        store_groups("carol", vec!["homevault_media".into()], now);
        store_groups("dave", vec!["homevault_admin".into()], now);
        assert_eq!(
            cached_groups("carol", now),
            Some(vec!["homevault_media".to_string()])
        );
        assert_eq!(
            cached_groups("dave", now),
            Some(vec!["homevault_admin".to_string()])
        );
        assert!(cached_groups("erin", now).is_none());
        clear_group_cache();
    }

    /// An explicit invalidation wins immediately, whatever the TTL says — the
    /// Groups screen calls it after a membership change.
    #[test]
    fn invalidate_drops_one_account_only() {
        clear_group_cache();
        let now = Instant::now();
        store_groups("frank", vec!["homevault_media".into()], now);
        store_groups("grace", vec!["homevault_media".into()], now);
        invalidate_group_cache("frank");
        assert!(cached_groups("frank", now).is_none());
        assert!(cached_groups("grace", now).is_some());
        clear_group_cache();
    }

    /// Pruning drops only the expired entries.
    #[test]
    fn pruning_keeps_the_fresh_entries() {
        clear_group_cache();
        let now = Instant::now();
        store_groups("heidi", vec!["homevault_media".into()], now);
        prune_group_cache(now + GROUP_MEMBERSHIP_TTL);
        assert!(cached_groups("heidi", now + GROUP_MEMBERSHIP_TTL).is_none());
        clear_group_cache();
    }

    /// The TTL has to stay short enough that a permission change is not a
    /// mystery to the user, and long enough to collapse a chunked upload.
    #[test]
    fn the_ttl_is_a_short_bound_that_is_actually_documented() {
        assert!(GROUP_MEMBERSHIP_TTL <= Duration::from_secs(30));
        assert!(GROUP_MEMBERSHIP_TTL >= Duration::from_secs(1));
        assert!(
            include_str!("../docs/guides/transfer-quotas.md").contains("GROUP_MEMBERSHIP_TTL"),
            "the bounded staleness must be documented in the guide"
        );
    }
}
