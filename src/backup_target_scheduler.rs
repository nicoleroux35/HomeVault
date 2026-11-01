/* =============================================================================
 * File          : backup_target_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Quota watchdog and scheduled integrity check for the backup destination:
 *   measures every tenant repository, revokes (or restores) its access when it
 *   crosses its quota, and runs a test restore on tenants that ask for one once
 *   a day. Only a successful pass advances the bookkeeping.
 *
 * Dependencies:
 *   - tokio (>= 1.52), crate::backup_target, crate::storage, crate::restic
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; see
 *   docs/guides/backup-target.md.
 * =============================================================================
 */

//! Backup-destination watchdog (P2.3).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::backup_target::{self, BackupTenant};
use crate::handlers::AppState;
use crate::storage::{secret_box, BackupTenantRepository};

/// How often quotas are re-measured. A received backup is a slow, daily event,
/// so five minutes of precision is ample and a repository walk is cheap.
const QUOTA_POLL_SECS: u64 = 300;

/// How often a tenant with verification enabled is checked.
const VERIFY_CADENCE_HOURS: i64 = 24;

/// The quota decision for one tenant. Pure, so "crossing revokes, dropping
/// below restores" is testable without a filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaAction {
    /// Access must be revoked.
    Revoke,
    /// Access must be restored (`rest-server` reads the htpasswd file again).
    Restore,
    /// No change.
    Keep,
}

pub fn quota_action(tenant: &BackupTenant, used: u64) -> QuotaAction {
    let blocked = backup_target::quota_verdict(used, tenant.quota_bytes).blocks();
    match (blocked, tenant.is_over_quota()) {
        (true, false) => QuotaAction::Revoke,
        (false, true) => QuotaAction::Restore,
        _ => QuotaAction::Keep,
    }
}

/// Whether a tenant's daily verification is due.
pub fn verification_due(tenant: &BackupTenant, now: DateTime<Utc>) -> bool {
    if tenant.verify_enabled == 0 {
        return false;
    }
    match tenant
        .last_verified_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    {
        Some(last) => {
            now >= last.with_timezone(&Utc) + chrono::Duration::hours(VERIFY_CADENCE_HOURS)
        }
        None => true,
    }
}

pub async fn run_scheduler_loop(state: Arc<AppState>) {
    let mut quota_tick = tokio::time::interval(Duration::from_secs(QUOTA_POLL_SECS));
    let mut verify_tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tokio::select! {
            _ = quota_tick.tick() => {
                if let Err(e) = enforce_quotas(&state).await {
                    tracing::warn!("Backup destination quota pass failed: {e}");
                }
            }
            _ = verify_tick.tick() => {
                if let Err(e) = verify_due(&state).await {
                    tracing::warn!("Backup destination verification pass failed: {e}");
                }
            }
        }
    }
}

/// One quota pass over every tenant.
pub async fn enforce_quotas(state: &Arc<AppState>) -> Result<(), String> {
    let repo = BackupTenantRepository::new(state.db.pool());
    let tenants = repo.list().await.map_err(|e| e.to_string())?;
    if tenants.is_empty() {
        return Ok(());
    }
    let mut changed = false;
    for tenant in &tenants {
        let path = std::path::PathBuf::from(&tenant.repo_path);
        let used = tokio::task::spawn_blocking(move || backup_target::dir_size(&path))
            .await
            .map_err(|e| format!("size task panicked: {e}"))?;
        match quota_action(tenant, used) {
            QuotaAction::Revoke => {
                repo.set_over_quota(tenant.id, true)
                    .await
                    .map_err(|e| e.to_string())?;
                crate::events::log_event(format!(
                    "Backup tenant '{}' is over its quota ({} of {} bytes): access revoked",
                    tenant.name, used, tenant.quota_bytes
                ));
                changed = true;
            }
            QuotaAction::Restore => {
                repo.set_over_quota(tenant.id, false)
                    .await
                    .map_err(|e| e.to_string())?;
                crate::events::log_event(format!(
                    "Backup tenant '{}' is back under its quota ({} of {} bytes): access restored",
                    tenant.name, used, tenant.quota_bytes
                ));
                changed = true;
            }
            QuotaAction::Keep => {}
        }
    }
    if changed {
        let tenants = repo.list().await.map_err(|e| e.to_string())?;
        let settings = backup_target::load_settings();
        // Only the auth file has to change: the daemon re-reads it per request,
        // so a revoked tenant stops working immediately.
        if let Err(e) = backup_target::apply(&settings, &tenants) {
            return Err(format!("the auth file could not be rewritten: {e}"));
        }
    }
    Ok(())
}

/// One verification pass over the tenants whose daily check is due.
pub async fn verify_due(state: &Arc<AppState>) -> Result<(), String> {
    let repo = BackupTenantRepository::new(state.db.pool());
    let tenants = repo.list().await.map_err(|e| e.to_string())?;
    let now = Utc::now();
    for tenant in tenants.iter().filter(|t| verification_due(t, now)) {
        let password = match secret_box::open(&tenant.secret_scope(), &tenant.repo_password_sealed)
        {
            Ok(password) => password,
            Err(e) => {
                let _ = repo.record_verification(tenant.id, false, &e).await;
                continue;
            }
        };
        let repository = tenant.repo_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            backup_target::verify_repository(&repository, &password).map(|o| o.message)
        })
        .await
        .map_err(|e| format!("verification task panicked: {e}"))?;
        match result {
            Ok(message) => {
                let _ = repo.record_verification(tenant.id, true, &message).await;
            }
            Err(e) => {
                let _ = repo.record_verification(tenant.id, false, &e).await;
                crate::events::log_event(format!(
                    "Backup tenant '{}' failed its integrity check: {}",
                    tenant.name, e
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn tenant(quota: i64, over: bool) -> BackupTenant {
        BackupTenant {
            id: 1,
            name: "family".to_string(),
            auth_hash: String::new(),
            token_hash: String::new(),
            repo_password_sealed: String::new(),
            repo_path: "/var/lib/homevault/backup-target/family".to_string(),
            quota_bytes: quota,
            over_quota: i64::from(over),
            verify_enabled: 1,
            last_seen_at: None,
            last_verified_at: None,
            last_verify_status: None,
            last_verify_error: None,
            created_by: "admin".to_string(),
            created_at: "2026-10-31T00:00:00+00:00".to_string(),
            updated_at: "2026-10-31T00:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn crossing_the_quota_revokes_and_dropping_below_restores() {
        // Under the quota, not blocked: nothing to do.
        assert_eq!(quota_action(&tenant(100, false), 50), QuotaAction::Keep);
        // Crossing revokes exactly once.
        assert_eq!(quota_action(&tenant(100, false), 100), QuotaAction::Revoke);
        assert_eq!(quota_action(&tenant(100, true), 120), QuotaAction::Keep);
        // A raised quota (or freed space) restores access exactly once.
        assert_eq!(quota_action(&tenant(200, true), 150), QuotaAction::Restore);
        // Unlimited never blocks.
        assert_eq!(quota_action(&tenant(0, false), 10_000), QuotaAction::Keep);
        // An unlimited tenant that was somehow flagged is restored.
        assert_eq!(quota_action(&tenant(0, true), 1), QuotaAction::Restore);
    }

    #[test]
    fn verification_is_due_daily_and_only_when_enabled() {
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let mut t = tenant(0, false);
        assert!(verification_due(&t, now), "never verified ⇒ due");
        t.last_verified_at = Some((now - chrono::Duration::hours(23)).to_rfc3339());
        assert!(!verification_due(&t, now), "23h ago is not due yet");
        t.last_verified_at = Some((now - chrono::Duration::hours(24)).to_rfc3339());
        assert!(verification_due(&t, now));
        // A corrupt timestamp is treated as "never verified".
        t.last_verified_at = Some("garbage".to_string());
        assert!(verification_due(&t, now));
        // Disabled verification never runs.
        t.verify_enabled = 0;
        t.last_verified_at = None;
        assert!(!verification_due(&t, now));
    }
}
