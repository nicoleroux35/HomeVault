/* =============================================================================
 * File          : notifications.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   System-notification endpoints for the topbar bell panel: list with
 *   pagination, unread count, mark read (one or all), delete, and the
 *   live SSE event stream. The stream authenticates with a short-lived,
 *   single-use ticket because EventSource cannot send session headers.
 *   All endpoints are session-auth; the notifications themselves are
 *   persisted by the writers (see sse/notifications.rs for the live bus).
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

use crate::handlers::admin_guard::require_auth;
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

/// Lists the most recent notifications (newest first).
pub async fn handle_notifications_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = NotificationRepository::new(state.db.pool());
    match repo.list_recent(50).await {
        Ok(list) => ResponseHelper::json_success(list),
        Err(e) => ResponseHelper::json_error(format!("Failed to list notifications: {}", e)),
    }
}

/// Returns the count of unread notifications, for the topbar badge.
pub async fn handle_notifications_unread_count(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = NotificationRepository::new(state.db.pool());
    match repo.unread_count().await {
        Ok(count) => ResponseHelper::json_success(serde_json::json!({ "count": count })),
        Err(e) => ResponseHelper::json_error(format!("Failed to count notifications: {}", e)),
    }
}

/// Marks a single notification as read.
pub async fn handle_notification_mark_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = NotificationRepository::new(state.db.pool());
    match repo.mark_read(&id).await {
        Ok(_) => ResponseHelper::json_success("Marked as read".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to mark notification as read: {}", e)),
    }
}

/// Marks every notification as read.
pub async fn handle_notifications_mark_all_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = NotificationRepository::new(state.db.pool());
    match repo.mark_all_read().await {
        Ok(count) => ResponseHelper::json_success(serde_json::json!({ "updated": count })),
        Err(e) => ResponseHelper::json_error(format!("Failed to mark all as read: {}", e)),
    }
}

/// Deletes a single notification.
pub async fn handle_notification_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = NotificationRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(_) => ResponseHelper::json_success("Notification deleted".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete notification: {}", e)),
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
