/* =============================================================================
 * File          : snapshot_lock_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Background loop for immutable snapshots: releases the filesystem hold of
 *   every lock whose retention horizon has passed (then marks the row
 *   released), and forgets a lock whose snapshot no longer exists. Polls
 *   hourly; only a successful release advances the row, so a wedged pool is
 *   retried instead of silently reported as unlocked.
 *
 * Dependencies:
 *   - tokio (>= 1.52), crate::snapshot_locks, crate::storage
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; see docs/guides/snapshots.md.
 * =============================================================================
 */

//! Lock-release loop for immutable snapshots (P2.5).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::handlers::AppState;
use crate::snapshot_locks::{self, SnapshotLock};
use crate::snapshots::SnapshotService;
use crate::storage::SnapshotLockRepository;

/// Poll interval: a lock lapses on a day boundary, so an hour is precise
/// enough and cheap.
const POLL_INTERVAL_SECS: u64 = 3600;

/// Which active locks this tick must release. Pure, so the "an unexpired lock
/// is never released" rule is testable.
pub fn due_for_release<'a>(
    locks: &'a [SnapshotLock],
    now: chrono::DateTime<Utc>,
) -> Vec<&'a SnapshotLock> {
    locks
        .iter()
        .filter(|lock| lock.is_active() && lock.is_expired(now))
        .collect()
}

pub async fn run_release_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    // The first tick fires immediately; a fresh boot should reconcile expired
    // locks at once (an appliance may have been off for a month).
    loop {
        interval.tick().await;
        if let Err(e) = tick(&state).await {
            tracing::warn!("Snapshot lock release failed: {e}");
        }
    }
}

async fn tick(state: &Arc<AppState>) -> Result<(), String> {
    let repo = SnapshotLockRepository::new(state.db.pool());
    let locks = repo.active(None).await.map_err(|e| e.to_string())?;
    if locks.is_empty() {
        return Ok(());
    }
    for lock in due_for_release(&locks, Utc::now()) {
        let pool = lock.pool.clone();
        let snapshot = lock.snapshot.clone();
        let id = lock.id;
        let released = tokio::task::spawn_blocking(move || {
            let kind = SnapshotService::kind_for(&pool).map_err(|e| e.to_string())?;
            snapshot_locks::release_hold(kind, &pool, &snapshot)
        })
        .await
        .map_err(|e| format!("release task panicked: {e}"))?;
        match released {
            Ok(()) => {
                if let Err(e) = repo
                    .mark_released(id, "system", Some("retention horizon reached"))
                    .await
                {
                    tracing::warn!("Could not mark lock #{id} released: {e}");
                    continue;
                }
                crate::events::log_event(format!(
                    "Snapshot lock #{} on '{}' expired and was released",
                    id, lock.snapshot
                ));
            }
            Err(e) => {
                // A snapshot that no longer exists (deleted out-of-band) has
                // nothing to release: forget the row instead of retrying for
                // ever. Any other failure keeps the row and is retried.
                if e.contains("does not exist") || e.contains("No such") {
                    let _ = repo
                        .forget(
                            &lock.pool,
                            &lock.snapshot,
                            "system",
                            "snapshot no longer exists",
                        )
                        .await;
                    crate::events::log_event(format!(
                        "Snapshot lock #{} forgotten: '{}' no longer exists",
                        id, lock.snapshot
                    ));
                } else {
                    tracing::warn!(
                        "Snapshot lock #{} on '{}' could not be released: {e}",
                        id,
                        lock.snapshot
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, TimeZone};

    fn lock(id: i64, until: chrono::DateTime<Utc>, released: bool) -> SnapshotLock {
        SnapshotLock {
            id,
            pool: "tank".to_string(),
            snapshot: format!("snap-{id}"),
            tag: snapshot_locks::LOCK_TAG.to_string(),
            locked_until: until.to_rfc3339(),
            reason: None,
            created_by: "admin".to_string(),
            created_at: "2026-10-31T00:00:00+00:00".to_string(),
            released_at: released.then(|| "2026-10-31T01:00:00+00:00".to_string()),
            released_by: None,
            release_reason: None,
        }
    }

    #[test]
    fn only_lapsed_unreleased_locks_are_due() {
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let locks = vec![
            lock(1, now - ChronoDuration::days(1), false),
            lock(2, now + ChronoDuration::days(1), false),
            lock(3, now - ChronoDuration::days(1), true),
        ];
        let due = due_for_release(&locks, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, 1);
    }

    #[test]
    fn a_corrupt_horizon_is_never_released_automatically() {
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let mut broken = lock(1, now, false);
        broken.locked_until = "garbage".to_string();
        assert!(due_for_release(&[broken], now).is_empty());
    }
}
