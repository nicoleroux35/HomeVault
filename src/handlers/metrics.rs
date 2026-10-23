/* =============================================================================
 * File          : metrics.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Historical metrics endpoints: bucketed series for the Metrics app's
 *   charts (1 h / 24 h / 7 d ranges) and the latest stored sample. Auth
 *   users may read (same sensitivity class as the dashboard widgets); there
 *   is deliberately no write path — samples come only from the collector.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/metrics in src/web/mod.rs; data via src/metrics.rs.
 * =============================================================================
 */

//! Metrics history endpoints (read-only, auth).

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::metrics::{bucket_samples, MetricsRange, RawSample};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    /// `1h` (default) | `24h` | `7d`
    pub range: Option<String>,
}

/// GET /api/metrics/history?range=1h|24h|7d — bucketed averages per series.
pub async fn handle_metrics_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<HistoryQuery>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let range = MetricsRange::parse(params.range.as_deref().unwrap_or("1h"));
    let Some(range) = range else {
        return ResponseHelper::json_error("Invalid range: expected 1h, 24h or 7d".to_string());
    };
    let (bucket_width, window) = range.window();
    let now = chrono::Utc::now().timestamp();
    let since = now - window;

    let result = sqlx::query_as::<_, RawSample>(
        "SELECT ts, cpu, mem, load1, net_rx, net_tx, temp FROM metric_samples \
         WHERE ts >= ? AND ts <= ? ORDER BY ts ASC",
    )
    .bind(since)
    .bind(now)
    .fetch_all(state.db.pool().as_ref())
    .await;

    match result {
        Ok(rows) => {
            let points = bucket_samples(&rows, bucket_width, window, now);
            ResponseHelper::json_success(serde_json::json!({
                "range": match range {
                    MetricsRange::OneHour => "1h",
                    MetricsRange::OneDay => "24h",
                    MetricsRange::SevenDays => "7d",
                },
                "bucket_seconds": bucket_width,
                "points": points,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to read metrics: {e}")),
    }
}

/// GET /api/metrics/latest — the most recent stored sample (widgets).
///
/// A fresh install has collected nothing yet. That is a normal first-boot
/// state, not a client error: the answer is `200` with `data: null`, so the
/// caller can tell "nothing measured yet" apart from "your request was wrong"
/// (`ROADMAP 11`).
pub async fn handle_metrics_latest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    match sqlx::query_as::<_, RawSample>(
        "SELECT ts, cpu, mem, load1, net_rx, net_tx, temp FROM metric_samples \
         ORDER BY ts DESC LIMIT 1",
    )
    .fetch_optional(state.db.pool().as_ref())
    .await
    {
        Ok(Some(sample)) => ResponseHelper::json_success(serde_json::json!({
            "ts": sample.ts,
            "cpu": sample.cpu,
            "mem": sample.mem,
            "load1": sample.load1,
            "net_rx": sample.net_rx,
            "net_tx": sample.net_tx,
            "temp": sample.temp,
        })),
        Ok(None) => ResponseHelper::json_success(serde_json::Value::Null),
        Err(e) => ResponseHelper::json_error(format!("Failed to read metrics: {e}")),
    }
}
