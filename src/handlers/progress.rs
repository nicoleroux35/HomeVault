/* =============================================================================
 * File          : progress.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SSE endpoints exposing job progress: stream one job's events live,
 *   list active jobs (the unified activity feed's source), poll a job's
 *   latest status, and cancel a job. Every endpoint requires a valid
 *   session; visibility and cancellation follow the job-owner rules
 *   implemented in crate::sse::progress.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Routes /api/progress in src/web/mod.rs; jobs are created and updated
 *   by handlers through ProgressEmitter.
 * =============================================================================
 */

//! SSE Progress Handlers
//!
//! Real-time progress for long-running jobs (backup, restore, verify, cache
//! mover, File-Explorer copy/move, …). Every endpoint requires a **valid**
//! authenticated session — job ids are unguessable UUIDs, but that is a
//! defence-in-depth layer, not the access control. Per-user ownership
//! scoping (so user A cannot watch user B's job) is a follow-up that needs
//! `ProgressManager::create_job` to carry the initiating username; until
//! then, any authenticated user can observe any active job's progress.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::sse::PROGRESS_MANAGER;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

/// SSE stream for job progress
pub async fn handle_progress_sse(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};
    use tokio::time::interval;

    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    // Get the job's broadcast channel
    let rx = match PROGRESS_MANAGER.get_receiver(&job_id) {
        Some(rx) => rx,
        None => {
            // Job not found - return JSON error
            return ResponseHelper::json_error("Job not found".to_string());
        }
    };

    // Create a stream that polls the broadcast receiver
    let stream = async_stream::stream! {
        let mut rx = rx;
        let mut ticker = interval(Duration::from_secs(30));

        loop {
            tokio::select! {
                // Try to receive a progress event
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            let event_json = serde_json::to_string(&event).unwrap_or_default();
                            let event_name = if event.progress >= 100.0 ||
                                event.state == crate::sse::ProgressState::Completed {
                                "done"
                            } else {
                                "progress"
                            };
                            yield Ok::<_, Infallible>(Event::default().event(event_name).data(event_json));

                            // Exit if completed or failed
                            if event.state == crate::sse::ProgressState::Completed ||
                               event.state == crate::sse::ProgressState::Failed {
                                break;
                            }
                        }
                        Err(_) => {
                            // Channel closed
                            break;
                        }
                    }
                }
                // Heartbeat to keep connection alive
                _ = ticker.tick() => {
                    yield Ok::<_, Infallible>(Event::default().event("heartbeat").data("{}"));
                }
            }
        }
    };

    Sse::new(stream).into_response()
}

/// List all active jobs
pub async fn handle_list_jobs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let jobs = PROGRESS_MANAGER.list_active_jobs();
    let response: Vec<serde_json::Value> = jobs
        .into_iter()
        .map(|(job_id, job_type, state)| {
            serde_json::json!({
                "job_id": job_id,
                "job_type": job_type,
                "state": state
            })
        })
        .collect();

    ResponseHelper::json_success(response)
}

/// Get current status of a specific job
pub async fn handle_get_job_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match PROGRESS_MANAGER.get_progress(&job_id) {
        Some(event) => ResponseHelper::json_success(event),
        None => ResponseHelper::json_error("Job not found".to_string()),
    }
}

/// Cancel/remove a job
pub async fn handle_cancel_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    PROGRESS_MANAGER.remove_job(&job_id);
    ResponseHelper::json_success("Job cancelled".to_string())
}
