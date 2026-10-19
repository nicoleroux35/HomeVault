/* =============================================================================
 * File          : health_probe.rs
 * Author        : Nicolas Le Roux
 * Created       : 17/10/2026
 * Last modified : 17/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Unauthenticated liveness and readiness probes for external monitoring
 *   (systemd, Docker HEALTHCHECK, Zabbix, Uptime Kuma, a reverse proxy).
 *
 *   `GET /api/health` — the authenticated, detailed health payload the
 *   Dashboard reads — answers HTTP 200 even when the caller is unauthenticated
 *   and even when the database is unusable, so a probe built on it can never
 *   detect an outage. These two routes fix that with the standard split:
 *
 *     /api/health/live   — is the process alive and serving? (200 or nothing)
 *     /api/health/ready  — can it actually do its job? (200 / 503)
 *
 *   Both are deliberately minimal: no hostname, no disk names, no version, no
 *   machine id. They are reachable without credentials, so anything they return
 *   is public information.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Public routes in src/web/mod.rs; the detailed `/api/health` stays
 *   admin-authenticated.
 * =============================================================================
 */

//! Public liveness/readiness probes (no authentication, no host details).

use crate::handlers::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Result of the boot-time `PRAGMA quick_check`, recorded once at startup.
///
/// `None` means the check has not run yet (or the database opened cleanly);
/// `Some(reason)` carries SQLite's own message for a damaged file.
static INTEGRITY_FAILURE: once_cell::sync::Lazy<parking_lot::RwLock<Option<String>>> =
    once_cell::sync::Lazy::new(|| parking_lot::RwLock::new(None));

/// Whether the last readiness probe saw a usable database.
static DB_REACHABLE: AtomicBool = AtomicBool::new(true);

/// Records the outcome of the startup integrity check.
pub fn record_integrity_result(result: &Option<String>) {
    *INTEGRITY_FAILURE.write() = result.clone();
    if let Some(reason) = result {
        tracing::error!(
            reason = %reason,
            "Database integrity check failed — the appliance will start in a degraded state"
        );
    }
}

/// The recorded integrity failure, if any.
pub fn integrity_failure() -> Option<String> {
    INTEGRITY_FAILURE.read().clone()
}

/// Cache window for the readiness database probe.
///
/// A monitoring system polling every second must not turn into a `SELECT 1`
/// per second on the appliance's only database, so the answer is reused for a
/// second.
const READY_CACHE: Duration = Duration::from_secs(1);

static LAST_READY: once_cell::sync::Lazy<parking_lot::Mutex<Option<(Instant, bool)>>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(None));

/// `GET /api/health/live` — the process is up and the HTTP stack answers.
///
/// Intentionally trivial: it proves the listener works, nothing more. A
/// supervisor that restarts on a failed liveness check wants exactly this.
pub async fn handle_health_live() -> Response {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}

/// `GET /api/health/ready` — the appliance can actually serve requests.
///
/// Checks the database answers a trivial query and reports `503` otherwise, so
/// a load balancer or systemd can take the instance out of rotation instead of
/// feeding it requests that will fail.
pub async fn handle_health_ready(State(state): State<Arc<AppState>>) -> Response {
    let db_ok = cached_db_probe(&state).await;
    let integrity = integrity_failure();

    let healthy = db_ok && integrity.is_none();

    let body = if healthy {
        serde_json::json!({
            "status": "ready",
            "checks": { "database": "ok", "integrity": "ok" },
        })
    } else {
        serde_json::json!({
            "status": "degraded",
            "checks": {
                "database": if db_ok { "ok" } else { "unreachable" },
                "integrity": integrity.as_deref().unwrap_or("ok"),
            },
        })
    };

    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// `SELECT 1` against the pool, memoised for [`READY_CACHE`].
async fn cached_db_probe(state: &Arc<AppState>) -> bool {
    {
        let cache = LAST_READY.lock();
        if let Some((at, ok)) = *cache {
            if at.elapsed() < READY_CACHE {
                return ok;
            }
        }
    }

    let ok = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&*state.db.pool()),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);

    DB_REACHABLE.store(ok, Ordering::Relaxed);
    *LAST_READY.lock() = Some((Instant::now(), ok));
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integrity_result_round_trip() {
        record_integrity_result(&None);
        assert!(integrity_failure().is_none());

        record_integrity_result(&Some("database disk image is malformed".to_string()));
        assert_eq!(
            integrity_failure().as_deref(),
            Some("database disk image is malformed")
        );

        record_integrity_result(&None);
        assert!(integrity_failure().is_none());
    }
}
