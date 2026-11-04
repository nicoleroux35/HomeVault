/* =============================================================================
 * File          : web_push.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Web Push endpoints (phase D2): the VAPID public key and the caller's own
 *   devices, subscribe this device, revoke this device. Every account manages
 *   its own devices; nothing here is admin-only because a subscription is a
 *   personal consent, not a server-wide setting.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::web_push
 *
 * Usage:
 *   Routes under /api/notifications/push in src/web/mod.rs; see
 *   docs/guides/web-push.md.
 * ============================================================================= */

//! The Web Push endpoints, see docs/guides/web-push.md.
//!
//! * `GET    /api/notifications/push`                  — public key + my devices
//! * `POST   /api/notifications/push/subscribe`        — this device
//! * `DELETE /api/notifications/push/subscribe/{id}`   — revoke this device

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use crate::web_push::repo::PushSubscriptionRepository;
use crate::web_push::{self, endpoint_host, MAX_SUBSCRIPTIONS_PER_USER};

/// DNS resolution for an endpoint can block; keep it bounded like every other
/// host probe.
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(10);

/// `GET /api/notifications/push`
pub async fn handle_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    let repo = PushSubscriptionRepository::new(state.db.pool());
    let subscriptions = match repo.list_for_user(&username).await {
        Ok(subscriptions) => subscriptions,
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };

    match web_push::keys().await {
        Ok(keys) => ResponseHelper::json_success(PushGetResponse {
            available: true,
            public_key: Some(keys.public_key.clone()),
            subscriptions: subscriptions.iter().map(DeviceDto::from).collect(),
            max_devices: MAX_SUBSCRIPTIONS_PER_USER,
            error: None,
        }),
        Err(error) => ResponseHelper::json_success(PushGetResponse {
            available: false,
            public_key: None,
            subscriptions: subscriptions.iter().map(DeviceDto::from).collect(),
            max_devices: MAX_SUBSCRIPTIONS_PER_USER,
            error: Some(error),
        }),
    }
}

/// `POST /api/notifications/push/subscribe`
pub async fn handle_subscribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };
    let request: SubscribeRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };

    let validated = tokio::time::timeout(
        VALIDATE_TIMEOUT,
        web_push::validate_subscription(
            &request.endpoint,
            &request.keys.p256dh,
            &request.keys.auth,
        ),
    )
    .await;
    let (endpoint, p256dh, auth) = match validated {
        Ok(Ok(values)) => values,
        Ok(Err(e)) => return ResponseHelper::json_error(e),
        Err(_) => {
            return ResponseHelper::json_error(
                "The push endpoint could not be checked in time; try again.".to_string(),
            )
        }
    };

    let repo = PushSubscriptionRepository::new(state.db.pool());
    match repo.endpoint_registered(&username, &endpoint).await {
        Ok(true) => {}
        Ok(false) => match repo.count_for_user(&username).await {
            Ok(count) if count >= MAX_SUBSCRIPTIONS_PER_USER => {
                return ResponseHelper::json_error(format!(
                    "This account already has {MAX_SUBSCRIPTIONS_PER_USER} devices. \
                     Remove one before adding another."
                ));
            }
            Ok(_) => {}
            Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
        },
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    }

    let label = device_label(&headers);
    match repo
        .upsert(&username, &endpoint, &p256dh, &auth, &label)
        .await
    {
        Ok(subscription) => {
            crate::events::log_event(format!(
                "User '{username}' enabled push notifications on {label}"
            ));
            ResponseHelper::json_success(SubscribeResponse {
                subscription: DeviceDto::from(&subscription),
            })
        }
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// `DELETE /api/notifications/push/subscribe/{id}`
pub async fn handle_unsubscribe(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };
    let repo = PushSubscriptionRepository::new(state.db.pool());
    match repo.delete_for_user(&username, &id).await {
        Ok(true) => {
            crate::events::log_event(format!(
                "User '{username}' revoked a push notification device"
            ));
            ResponseHelper::json_success(serde_json::json!({ "id": id }))
        }
        Ok(false) => ResponseHelper::json_error_with_status(
            axum::http::StatusCode::NOT_FOUND,
            "No such device for this account.".to_string(),
        ),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// A device label from the browser's own User-Agent, bounded and cleaned so a
/// header cannot inject control characters into the screen.
fn device_label(headers: &HeaderMap) -> String {
    let raw = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("Unknown browser");
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    let label = if cleaned.is_empty() {
        "Unknown browser"
    } else {
        cleaned
    };
    web_push::truncate_at(label, 80)
}

#[derive(Debug, Deserialize)]
pub struct SubscribeRequest {
    pub endpoint: String,
    pub keys: SubscribeKeys,
}

#[derive(Debug, Deserialize)]
pub struct SubscribeKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceDto {
    pub id: String,
    pub device_label: String,
    pub created_at: String,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub failure_count: i64,
    /// Only the host; the endpoint path is a capability URL and stays private.
    pub endpoint_host: String,
}

impl From<&crate::web_push::PushSubscription> for DeviceDto {
    fn from(subscription: &crate::web_push::PushSubscription) -> Self {
        Self {
            id: subscription.id.clone(),
            device_label: subscription.device_label.clone(),
            created_at: subscription.created_at.clone(),
            last_success_at: subscription.last_success_at.clone(),
            last_failure_at: subscription.last_failure_at.clone(),
            failure_count: subscription.failure_count,
            endpoint_host: endpoint_host(&subscription.endpoint),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PushGetResponse {
    pub available: bool,
    pub public_key: Option<String>,
    pub subscriptions: Vec<DeviceDto>,
    pub max_devices: i64,
    /// Why the VAPID keys are unavailable; never a browser capability claim.
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SubscribeResponse {
    pub subscription: DeviceDto,
}
