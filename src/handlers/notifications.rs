/* =============================================================================
 * File          : notifications.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   System-notification endpoints for the topbar bell panel: paginated list,
 *   unread count, mark read (one or all, per user), delete, clear-all, and
 *   the live SSE event stream. Read state is per-user (ROADMAP 21.2): the
 *   feed is broadcast, but each account's unread view lives in
 *   `notification_reads`. Deleting the feed is a shared, destructive
 *   operation, so `DELETE /notifications/{id}` and `DELETE /notifications`
 *   are admin-only (least privilege — reads are `require_auth`, mutations
 *   `require_admin`). The stream authenticates with a short-lived,
 *   single-use ticket because EventSource cannot send session headers.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Routes /api/notifications and /api/notifications/events in
 *   src/web/mod.rs.
 * =============================================================================
 */

//! HTTP handlers for system notifications (health alerts, backup results).

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::storage::NotificationRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use std::time::Duration;

/// Query parameters for the paginated list (ROADMAP 21.3). The page size is
/// clamped server-side so no caller can ask for (and pin) an unbounded set.
#[derive(serde::Deserialize)]
pub struct NotificationsListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

const DEFAULT_PAGE_SIZE: i64 = 50;
const MAX_PAGE_SIZE: i64 = 200;

/// Lists the most recent notifications (newest first), paginated, with the
/// caller's read state. Answers an envelope so the client knows whether
/// older rows exist:
/// `{ "notifications": [...], "total": n, "limit": l, "offset": o }`.
pub async fn handle_notifications_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<NotificationsListQuery>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };

    let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let offset = query.offset.unwrap_or(0).max(0);

    let repo = NotificationRepository::new(state.db.pool());
    match repo.list_recent(&username, limit, offset).await {
        Ok(items) => {
            let total = match repo.total_count().await {
                Ok(n) => n,
                Err(e) => {
                    return ResponseHelper::json_internal_error(format!(
                        "Failed to count notifications: {}",
                        e
                    ))
                }
            };
            ResponseHelper::json_success(serde_json::json!({
                "notifications": items,
                "total": total,
                "limit": limit,
                "offset": offset,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list notifications: {}", e)),
    }
}

/// Returns the caller's unread notification count, for the topbar badge.
pub async fn handle_notifications_unread_count(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };

    let repo = NotificationRepository::new(state.db.pool());
    match repo.unread_count_for(&username).await {
        Ok(count) => ResponseHelper::json_success(serde_json::json!({ "count": count })),
        Err(e) => ResponseHelper::json_error(format!("Failed to count notifications: {}", e)),
    }
}

/// Marks a single notification as read **for the caller**.
pub async fn handle_notification_mark_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };

    let repo = NotificationRepository::new(state.db.pool());
    match repo.mark_read(&username, &id).await {
        Ok(true) => ResponseHelper::json_success("Marked as read".to_string()),
        Ok(false) => ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "No such notification".to_string(),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to mark notification as read: {}", e)),
    }
}

/// Marks every notification as read **for the caller only** — one user's
/// "mark all read" never clears another account's badge.
pub async fn handle_notifications_mark_all_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };

    let repo = NotificationRepository::new(state.db.pool());
    match repo.mark_all_read(&username).await {
        Ok(count) => ResponseHelper::json_success(serde_json::json!({ "updated": count })),
        Err(e) => ResponseHelper::json_error(format!("Failed to mark all as read: {}", e)),
    }
}

/// Deletes a single notification. Admin-only (ROADMAP 21.2): the feed is
/// broadcast, so removing an alert-trail entry is a shared, destructive
/// operation; reads stay `require_auth`, mutations of the feed are
/// `require_admin`.
pub async fn handle_notification_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo = NotificationRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(true) => ResponseHelper::json_success("Notification deleted".to_string()),
        Ok(false) => ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "No such notification".to_string(),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete notification: {}", e)),
    }
}

/// Deletes the whole notification feed in one operation (ROADMAP 21.3) —
/// the "clear all" path, replacing the old N-unawaited-DELETEs loop.
/// Admin-only, same rationale as `handle_notification_delete`.
pub async fn handle_notifications_clear_all(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo = NotificationRepository::new(state.db.pool());
    match repo.clear_all().await {
        Ok(count) => ResponseHelper::json_success(serde_json::json!({ "deleted": count })),
        Err(e) => ResponseHelper::json_error(format!("Failed to clear notifications: {}", e)),
    }
}

/// Issues a short-lived, single-use ticket for opening the `/api/events`
/// SSE stream. A browser `EventSource` can't set a custom
/// `X-Session-Token` header, so it can't be authenticated with
/// `require_auth` directly the way every other endpoint in this file is —
/// this lets the client prove it holds a real session first, over a normal
/// request that *can* carry the header, then hand the resulting one-time
/// ticket to `EventSource` instead of the real token.
pub async fn handle_notifications_events_ticket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let ticket = crate::sse::notifications::issue_ticket();
    ResponseHelper::json_success(serde_json::json!({ "ticket": ticket }))
}

#[derive(serde::Deserialize)]
pub struct NotificationsEventsQuery {
    ticket: Option<String>,
}

/// Real-time SSE stream of newly created notifications, so the bell panel
/// updates instantly instead of waiting for the next 30s poll. Requires a
/// valid ticket from `handle_notifications_events_ticket` (see that
/// handler's doc comment for why).
pub async fn handle_notifications_events(
    Query(query): Query<NotificationsEventsQuery>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use tokio::sync::broadcast::error::RecvError;
    use tokio::time::interval;

    let Some(ticket) = query.ticket.filter(|t| !t.is_empty()) else {
        return (StatusCode::UNAUTHORIZED, "No events ticket provided").into_response();
    };
    if !crate::sse::notifications::consume_ticket(&ticket) {
        return (StatusCode::UNAUTHORIZED, "Invalid or expired events ticket").into_response();
    }

    let mut rx = crate::sse::notifications::NOTIFICATION_BUS.subscribe();

    let stream = async_stream::stream! {
        let mut ticker = interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(notification) => {
                            let json = serde_json::to_string(&notification).unwrap_or_default();
                            yield Ok::<_, std::convert::Infallible>(Event::default().event("notification").data(json));
                        }
                        // A slow client missed some events; nothing to resend, it'll
                        // catch up via its own 30s poll of GET /api/notifications.
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
                _ = ticker.tick() => {
                    yield Ok::<_, std::convert::Infallible>(Event::default().event("heartbeat").data("{}"));
                }
            }
        }
    };

    Sse::new(stream).into_response()
}

/// GET /notifications/events — the event catalog with the admin's current
/// per-event state, plus the webhook sink format. Admin-only: it exposes
/// the notification configuration (who gets emailed what).
pub async fn handle_notification_events_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    let settings = crate::handlers::settings::NOTIFICATION_SETTINGS
        .read()
        .clone();
    let events: Vec<serde_json::Value> = crate::notification_events::EVENT_CATALOG
        .iter()
        .map(|event| {
            serde_json::json!({
                "id": event.id,
                "group": event.group,
                "label_key": event.label_key,
                "enabled": crate::notification_events::is_event_enabled(
                    event.id,
                    settings.event_toggles.as_ref(),
                ),
            })
        })
        .collect();
    ResponseHelper::json_success(serde_json::json!({
        "events": events,
        "webhook_format": settings.webhook_format,
    }))
}

/// PUT /notifications/events — update per-event toggles and/or the webhook
/// sink format. Accepts `{"events": {id: bool, ...}, "webhook_format":
/// "generic"|"slack"|"discord"}` with either part optional. Admin-only.
pub async fn handle_notification_events_config_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<serde_json::Value>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let mut settings = crate::handlers::settings::NOTIFICATION_SETTINGS.write();
    if let Some(serde_json::Value::Object(events)) = req.get("events").cloned() {
        let mut toggles = settings.event_toggles.clone().unwrap_or_default();
        for (id, value) in events {
            let known = crate::notification_events::EVENT_CATALOG
                .iter()
                .any(|e| e.id == id);
            if !known {
                return ResponseHelper::json_error(format!("Unknown event id '{id}'"));
            }
            let Some(enabled) = value.as_bool() else {
                return ResponseHelper::json_error(format!("Event '{id}' must map to a boolean"));
            };
            toggles.insert(id.clone(), enabled);
        }
        settings.event_toggles = Some(toggles);
    }
    if let Some(format) = req.get("webhook_format") {
        let Some(format_str) = format.as_str() else {
            return ResponseHelper::json_error("webhook_format must be a string".to_string());
        };
        if crate::notification_events::WebhookFormat::parse(format_str).is_none() {
            return ResponseHelper::json_error(
                "Invalid webhook format: expected generic, slack or discord".to_string(),
            );
        }
        settings.webhook_format = Some(format_str.to_string());
    }
    drop(settings);
    crate::settings_persistence::save_all_settings();

    crate::events::log_event("Admin updated notification event settings".to_string());
    ResponseHelper::json_success(serde_json::json!({ "success": true }))
}
