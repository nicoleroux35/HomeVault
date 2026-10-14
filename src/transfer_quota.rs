/* =============================================================================
 * File          : transfer_quota.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
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

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;

use crate::groups::GroupService;
use crate::storage::transfer_quota_repo::TransferQuotaRepository;

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

async fn load_applicable(
    pool: Arc<sqlx::SqlitePool>,
    username: &str,
    upload: bool,
) -> Result<Vec<ApplicableQuota>, String> {
    let repo = TransferQuotaRepository::new(pool);
    let mut applicable = Vec::new();

    let user_quota = repo
        .get("user", username)
        .await
        .map_err(|e| format!("Failed to load user quota: {}", e))?;
    let (enabled, limit, used) = if upload {
        (
            user_quota.upload_enabled,
            user_quota.upload_limit_bytes,
            user_quota.upload_used_bytes,
        )
    } else {
        (
            user_quota.download_enabled,
            user_quota.download_limit_bytes,
            user_quota.download_used_bytes,
        )
    };
    if enabled {
        applicable.push(ApplicableQuota {
            subject_type: "user",
            subject_name: username.to_string(),
            limit_bytes: limit,
            used_bytes: used,
        });
    }

    let username_owned = username.to_string();
    let group_names =
        tokio::task::spawn_blocking(move || GroupService::get_user_groups(&username_owned))
            .await
            .map_err(|e| format!("Failed to resolve group membership: {}", e))?
            .unwrap_or_default();

    for group_name in group_names {
        let group_quota = repo
            .get("group", &group_name)
            .await
            .map_err(|e| format!("Failed to load group quota for '{}': {}", group_name, e))?;
        let (enabled, limit, used) = if upload {
            (
                group_quota.upload_enabled,
                group_quota.upload_limit_bytes,
                group_quota.upload_used_bytes,
            )
        } else {
            (
                group_quota.download_enabled,
                group_quota.download_limit_bytes,
                group_quota.download_used_bytes,
            )
        };
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
    for q in quotas {
        let result = if upload {
            repo.add_upload_usage(q.subject_type, &q.subject_name, bytes as i64)
                .await
        } else {
            repo.add_download_usage(q.subject_type, &q.subject_name, bytes as i64)
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
    for q in quotas {
        let delta = -(bytes as i64);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
