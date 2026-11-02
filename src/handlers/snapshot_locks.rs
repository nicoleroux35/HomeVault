/* =============================================================================
 * File          : snapshot_locks.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Immutable-snapshot lock endpoints: list the active locks (and the recent
 *   history), place a lock for a number of days, release it — normally after
 *   its horizon, or early with a typed confirmation that is journaled as
 *   break-glass. Reads need a session; mutations are admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::snapshot_locks, crate::snapshots
 *
 * Usage:
 *   Routes under /api/snapshots/locks in src/web/mod.rs; see
 *   docs/guides/snapshots.md.
 * =============================================================================
 */

//! Immutable-snapshot lock endpoints, see docs/guides/snapshots.md.
//!
//! * `GET    /api/snapshots/locks?pool=<p>` — active locks + recent history
//! * `POST   /api/snapshots/locks`          — lock a snapshot for N days
//! * `DELETE /api/snapshots/locks/{id}`     — release (typed phrase if early)

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::snapshot_locks::{self, break_glass_phrase, decide_release, ReleaseDecision};
use crate::snapshots::SnapshotService;
use crate::storage::SnapshotLockRepository;
use crate::utils::ResponseHelper;

use super::AppState;

const LOCK_OP_TIMEOUT: Duration = Duration::from_secs(90);

async fn run_op<T, F>(op: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(LOCK_OP_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(format!("{op} worker failed: {e}")),
        Err(_) => Err(format!(
            "{op} timed out after {}s",
            LOCK_OP_TIMEOUT.as_secs()
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct PoolQuery {
    #[serde(default)]
    pub pool: Option<String>,
}

/// `GET /api/snapshots/locks?pool=<p>`
pub async fn handle_snapshot_locks_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PoolQuery>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let repo = SnapshotLockRepository::new(state.db.pool());
    let now = Utc::now();
    let active = match repo.active(query.pool.as_deref()).await {
        Ok(locks) => locks,
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the locks: {e}")),
    };
    let history = match repo.history(50).await {
        Ok(locks) => locks,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to read the lock history: {e}"))
        }
    };
    let active: Vec<serde_json::Value> = active
        .iter()
        .map(|lock| {
            serde_json::json!({
                "id": lock.id,
                "pool": lock.pool,
                "snapshot": lock.snapshot,
                "tag": lock.tag,
                "locked_until": lock.locked_until,
                "reason": lock.reason,
                "created_by": lock.created_by,
                "created_at": lock.created_at,
                "expired": lock.is_expired(now),
                "break_glass_phrase": break_glass_phrase(&lock.snapshot),
            })
        })
        .collect();
    ResponseHelper::json_success(serde_json::json!({
        "locks": active,
        "history": history,
        "max_lock_days": snapshot_locks::MAX_LOCK_DAYS,
        "tag": snapshot_locks::LOCK_TAG,
    }))
}

/// `POST /api/snapshots/locks`
#[derive(Debug, Deserialize)]
pub struct LockRequest {
    pub pool: String,
    /// `dataset@name` (ZFS) or the bare Btrfs snapshot name.
    pub snapshot: String,
    pub days: i64,
    #[serde(default)]
    pub reason: Option<String>,
}

pub async fn handle_snapshot_lock_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LockRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let horizon = match snapshot_locks::validate_lock_request(&body.snapshot, body.days, Utc::now())
    {
        Ok(horizon) => horizon,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if let Some(reason) = body.reason.as_deref() {
        if reason.chars().count() > 200 || reason.chars().any(|c| c.is_control()) {
            return ResponseHelper::json_error(
                "A lock reason must be one line of at most 200 characters".to_string(),
            );
        }
    }

    let repo = SnapshotLockRepository::new(state.db.pool());
    match repo
        .active_for(&body.pool, &body.snapshot)
        .await
        .map_err(|e| e.to_string())
    {
        Ok(Some(existing)) => {
            return ResponseHelper::json_error(format!(
                "{} is already locked until {}",
                existing.snapshot, existing.locked_until
            ))
        }
        Ok(None) => {}
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to check the existing locks: {e}"))
        }
    }

    // The hold must exist on the pool *before* the row claims it does.
    let pool = body.pool.clone();
    let snapshot = body.snapshot.clone();
    let applied = run_op("apply the snapshot hold", move || {
        let kind = SnapshotService::kind_for(&pool).map_err(|e| e.to_string())?;
        snapshot_locks::apply_hold(kind, &pool, &snapshot)
    })
    .await;
    if let Err(e) = applied {
        return ResponseHelper::json_error(format!("The snapshot could not be locked: {e}"));
    }

    match repo
        .create(
            &body.pool,
            &body.snapshot,
            &horizon.to_rfc3339(),
            body.reason.as_deref(),
            &admin,
        )
        .await
    {
        Ok(lock) => {
            crate::events::log_event(format!(
                "Admin '{}' locked snapshot '{}' on pool '{}' until {}",
                admin, lock.snapshot, lock.pool, lock.locked_until
            ));
            ResponseHelper::json_success(serde_json::json!({
                "lock": lock,
                "break_glass_phrase": break_glass_phrase(&lock.snapshot),
            }))
        }
        Err(e) => {
            // The hold exists but the row could not be written: remove the
            // hold again rather than leaving an invisible lock behind.
            let pool = body.pool.clone();
            let snapshot = body.snapshot.clone();
            let _ = run_op("roll back the snapshot hold", move || {
                let kind = SnapshotService::kind_for(&pool).map_err(|e| e.to_string())?;
                snapshot_locks::release_hold(kind, &pool, &snapshot)
            })
            .await;
            ResponseHelper::json_error(format!("Failed to record the lock: {e}"))
        }
    }
}

/// `DELETE /api/snapshots/locks/{id}`
#[derive(Debug, Deserialize, Default)]
pub struct ReleaseRequest {
    /// The typed phrase, required only before the horizon.
    #[serde(default)]
    pub confirmation: Option<String>,
}

pub async fn handle_snapshot_lock_release(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Option<Json<ReleaseRequest>>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = SnapshotLockRepository::new(state.db.pool());
    let lock = match repo.find(id).await {
        Ok(Some(lock)) => lock,
        Ok(None) => return ResponseHelper::json_error("That lock no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the lock: {e}")),
    };
    if !lock.is_active() {
        return ResponseHelper::json_error("That lock is already released".to_string());
    }

    let confirmation = body.and_then(|Json(b)| b.confirmation);
    let now = Utc::now();
    let decision = decide_release(&lock, confirmation.as_deref(), now);
    if decision == ReleaseDecision::Refused {
        return ResponseHelper::json_error(format!(
            "This lock runs until {}. Releasing it early needs the typed confirmation “{}”.",
            lock.locked_until,
            break_glass_phrase(&lock.snapshot)
        ));
    }
    let break_glass = decision == ReleaseDecision::BreakGlass;

    // Remove the filesystem hold first: a row marked released while the hold
    // remains would report a deletable snapshot that `zfs destroy` refuses.
    let pool = lock.pool.clone();
    let snapshot = lock.snapshot.clone();
    let released = run_op("release the snapshot hold", move || {
        let kind = SnapshotService::kind_for(&pool).map_err(|e| e.to_string())?;
        snapshot_locks::release_hold(kind, &pool, &snapshot)
    })
    .await;
    if let Err(e) = released {
        return ResponseHelper::json_error(format!("The snapshot hold could not be released: {e}"));
    }

    let reason = if break_glass {
        Some("break-glass: released before its horizon with a typed confirmation")
    } else {
        None
    };
    match repo.mark_released(id, &admin, reason).await {
        Ok(()) => {
            if break_glass {
                crate::events::log_event(format!(
                    "BREAK-GLASS: admin '{}' released snapshot lock #{} on '{}' before its horizon ({})",
                    admin, id, lock.snapshot, lock.locked_until
                ));
            } else {
                crate::events::log_event(format!(
                    "Admin '{}' released snapshot lock #{} on '{}'",
                    admin, id, lock.snapshot
                ));
            }
            ResponseHelper::json_success(serde_json::json!({
                "released": true,
                "break_glass": break_glass,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to record the release: {e}")),
    }
}
