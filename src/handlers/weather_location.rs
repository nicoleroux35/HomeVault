//! Per-account desktop weather-widget location, synced server-side so the
//! same account sees the same chosen city regardless of which browser origin
//! (LAN IP, public IP, ...) it's accessed from - mirrors monitor_layout.rs's
//! shape/reasoning but stays a separate table/endpoint, matching
//! weatherLocation.ts's own deliberate separation from monitorStore.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json as AxumJson};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct WeatherLocationResponse {
    /// Opaque JSON blob (the frontend's own `WeatherLocation` shape); `None`
    /// if this account has never saved a location server-side yet.
    pub location_json: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetWeatherLocationRequest {
    pub location_json: String,
}

/// Fetch the calling account's own saved weather location, if any.
pub async fn handle_get_weather_location(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    match sqlx::query_scalar::<_, String>(
        "SELECT location_json FROM user_weather_location WHERE username = ?",
    )
    .bind(&username)
    .fetch_optional(state.db.pool().as_ref())
    .await
    {
        Ok(location_json) => {
            ResponseHelper::json_success(WeatherLocationResponse { location_json })
        }
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}

/// Upsert the calling account's own weather location.
pub async fn handle_set_weather_location(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<SetWeatherLocationRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let result = sqlx::query(
        "INSERT INTO user_weather_location (username, location_json, updated_at)
         VALUES (?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(username) DO UPDATE SET location_json = excluded.location_json, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(&username)
    .bind(&payload.location_json)
    .execute(state.db.pool().as_ref())
    .await;

    match result {
        Ok(_) => ResponseHelper::json_success(serde_json::json!({})),
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}
