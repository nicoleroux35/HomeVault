/* =============================================================================
 * File          : file_requests.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Public file requests: a folder owner mints a link that lets someone
 *   without a HomeVault account upload files into that folder. The link can
 *   carry a password, an expiry and an upload cap; the owner's write grant is
 *   re-checked on every upload, so revoking it stops the request immediately.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - tokio (fs/io)
 *
 * Usage:
 *   /api/explorer/file-requests (owner), /request/{token} (public page) and
 *   /request/{token}/upload (public multipart) in src/web/mod.rs.
 * =============================================================================
 */

//! Public file requests (see docs/guides/explorer.md).

use axum::{
    extract::{Multipart, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

use crate::handlers::share_links::{generate_share_token, token_is_well_formed};
use crate::handlers::AppState;
use crate::storage::{FileRequest, FileRequestRepository};
use crate::utils::ResponseHelper;

/// Largest single file a public request accepts. The router-wide body limit is
/// much larger, so this is the binding ceiling for this route.
pub const MAX_FILE_REQUEST_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Concurrent public uploads, so a link cannot turn into a fork/disk bomb.
const MAX_CONCURRENT_FILE_REQUESTS: usize = 4;

/// Wrong-password budget per token.
const PASSWORD_MAX_FAILURES: u32 = 8;
const PASSWORD_WINDOW: Duration = Duration::from_secs(5 * 60);

static UPLOAD_SEMAPHORE: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_FILE_REQUESTS)));

struct Failures {
    count: u32,
    since: Instant,
}
static PASSWORD_FAILURES: LazyLock<Mutex<HashMap<String, Failures>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn may_try_password(token: &str, now: Instant) -> bool {
    let mut map = PASSWORD_FAILURES.lock();
    map.retain(|_, f| now.duration_since(f.since) < PASSWORD_WINDOW);
    map.get(token)
        .is_none_or(|f| f.count < PASSWORD_MAX_FAILURES)
}

fn note_password_failure(token: &str, now: Instant) {
    let mut map = PASSWORD_FAILURES.lock();
    let entry = map.entry(token.to_string()).or_insert(Failures {
        count: 0,
        since: now,
    });
    entry.count += 1;
}

fn clear_password_failures(token: &str) {
    PASSWORD_FAILURES.lock().remove(token);
}

#[derive(Debug, Deserialize)]
pub struct FileRequestCreateRequest {
    pub path: String,
    #[serde(default)]
    pub message: Option<String>,
    pub expires_days: Option<i64>,
    pub password: Option<String>,
    pub max_uploads: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct FileRequestView {
    #[serde(flatten)]
    pub request: FileRequest,
    pub request_path: String,
    pub protected: bool,
    pub expired: bool,
    pub exhausted: bool,
}

fn is_expired(request: &FileRequest) -> bool {
    match &request.expires_at {
        Some(expiry) => chrono::DateTime::parse_from_rfc3339(expiry)
            .map(|t| t.timestamp() < chrono::Utc::now().timestamp())
            .unwrap_or(false),
        None => false,
    }
}

fn is_exhausted(request: &FileRequest) -> bool {
    request
        .max_uploads
        .is_some_and(|max| request.upload_count >= max)
}

fn to_view(request: FileRequest) -> FileRequestView {
    let request_path = format!("/request/{}", request.token);
    FileRequestView {
        protected: request.password_hash.is_some(),
        expired: is_expired(&request),
        exhausted: is_exhausted(&request),
        request,
        request_path,
    }
}

fn compute_expiry(days: i64) -> Option<String> {
    if days <= 0 {
        return None;
    }
    Some((chrono::Utc::now() + chrono::Duration::days(days)).to_rfc3339())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn html_response(status: StatusCode, body: String) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// The public page. It explains the request and renders a plain HTML form
/// (password first, then the file) — no JavaScript, so it works in any
/// browser and cannot leak the token into a script.
fn request_page(request: &FileRequest, error: Option<&str>) -> Response {
    let mut body = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Upload files</title></head><body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:3rem auto;padding:0 1rem\">",
    );
    body.push_str("<h1 style=\"font-size:1.35rem\">Upload files</h1>");
    if !request.message.is_empty() {
        body.push_str(&format!(
            "<p style=\"color:#444\">{}</p>",
            escape_html(&request.message)
        ));
    }
    if let Some(error) = error {
        body.push_str(&format!(
            "<p role=\"alert\" style=\"color:#a40000\">{}</p>",
            escape_html(error)
        ));
    }
    body.push_str(&format!(
        "<form method=\"post\" action=\"/request/{}/upload\" enctype=\"multipart/form-data\">",
        request.token
    ));
    if request.password_hash.is_some() {
        body.push_str(
            "<label style=\"display:block;margin:1rem 0 .25rem\">Password<input type=\"password\" name=\"password\" required autocomplete=\"current-password\" style=\"display:block;width:100%;padding:.55rem;margin-top:.25rem\"></label>",
        );
    }
    body.push_str(
        "<label style=\"display:block;margin:1rem 0 .25rem\">Files<input type=\"file\" name=\"file\" multiple required style=\"display:block;margin-top:.25rem\"></label>",
    );
    body.push_str(
        "<button type=\"submit\" style=\"margin-top:1rem;padding:.6rem 1rem\">Upload</button></form></body></html>",
    );
    html_response(StatusCode::OK, body)
}

fn unavailable_page() -> Response {
    html_response(
        StatusCode::NOT_FOUND,
        "<!doctype html><meta charset=\"utf-8\"><p>This upload link is unavailable.</p>"
            .to_string(),
    )
}

/// POST /api/explorer/file-requests — mint a public upload link for a folder.
pub async fn handle_file_request_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<FileRequestCreateRequest>,
) -> Response {
    let session_token =
        match crate::handlers::admin_guard::require_session_token(&state, &headers).await {
            Ok(token) => token,
            Err(resp) => return resp,
        };
    let session = match state.auth.get_session(&session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let Some(path) =
        crate::handlers::explorer::validate_path_access(&state, &req.path, &session).await
    else {
        return ResponseHelper::json_forbidden(
            "Access denied: path outside allowed directory".to_string(),
        );
    };
    if let Some(resp) = crate::handlers::explorer::restricted_response(
        &state,
        &session.username,
        session.is_admin,
        &path,
        false,
        false,
        true,
    )
    .await
    {
        return resp;
    }
    if !tokio::fs::metadata(&path)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return ResponseHelper::json_error("A file request must target a folder".to_string());
    }
    let path = tokio::fs::canonicalize(&path)
        .await
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());

    let message = req.message.unwrap_or_default();
    if message.len() > 500 || message.chars().any(|c| c.is_control() && c != '\n') {
        return ResponseHelper::json_error(
            "The message must be at most 500 characters and contain no control characters"
                .to_string(),
        );
    }
    let expires_days = req.expires_days.unwrap_or(30);
    if !(0..=3650).contains(&expires_days) {
        return ResponseHelper::json_error("expires_days must be between 0 and 3650".to_string());
    }
    if req.max_uploads.is_some_and(|n| !(1..=100_000).contains(&n)) {
        return ResponseHelper::json_error("max_uploads must be between 1 and 100000".to_string());
    }
    if req
        .password
        .as_ref()
        .is_some_and(|p| p.len() < 8 || p.len() > 256)
    {
        return ResponseHelper::json_error(
            "Password must be between 8 and 256 characters".to_string(),
        );
    }
    let password_hash = match req
        .password
        .as_deref()
        .map(|p| state.auth.hash_password(p))
        .transpose()
    {
        Ok(hash) => hash,
        Err(_) => return ResponseHelper::json_error("Could not protect this request".to_string()),
    };

    let repo = FileRequestRepository::new(state.db.pool());
    let token = generate_share_token();
    let expires_at = compute_expiry(expires_days);
    match repo
        .create(
            &session.username,
            &token,
            &path,
            &message,
            password_hash.as_deref(),
            expires_at.as_deref(),
            req.max_uploads,
        )
        .await
    {
        Ok(request) => {
            crate::events::log_event(format!(
                "User '{}' created a file request for '{}'",
                session.username, path
            ));
            ResponseHelper::json_success(to_view(request))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create file request: {e}")),
    }
}

/// GET /api/explorer/file-requests — the caller's own requests.
pub async fn handle_file_requests_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let Ok(username) = crate::handlers::admin_guard::require_auth(&state, &headers).await else {
        return ResponseHelper::json_unauthorized("Authentication required".to_string());
    };
    let repo = FileRequestRepository::new(state.db.pool());
    match repo.list_by_user(&username).await {
        Ok(requests) => {
            ResponseHelper::json_success(requests.into_iter().map(to_view).collect::<Vec<_>>())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list file requests: {e}")),
    }
}

/// DELETE /api/explorer/file-requests/{token} — owner or admin.
pub async fn handle_file_request_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let Ok(username) = crate::handlers::admin_guard::require_auth(&state, &headers).await else {
        return ResponseHelper::json_unauthorized("Authentication required".to_string());
    };
    let repo = FileRequestRepository::new(state.db.pool());
    let Some(request) = repo.find_by_token(&token).await.ok().flatten() else {
        return ResponseHelper::json_error("File request not found".to_string());
    };
    if request.username != username && !state.auth.is_admin(&username).await {
        return ResponseHelper::json_forbidden(
            "You can only revoke your own file requests".to_string(),
        );
    }
    match repo.delete(&token).await {
        Ok(()) => {
            crate::events::log_event(format!(
                "User '{}' revoked the file request for '{}'",
                username, request.path
            ));
            ResponseHelper::json_success(serde_json::json!({ "revoked": true }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke file request: {e}")),
    }
}

/// GET /request/{token} — the public upload page.
pub async fn handle_file_request_page(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Response {
    if !token_is_well_formed(&token) {
        return unavailable_page();
    }
    let repo = FileRequestRepository::new(state.db.pool());
    let Some(request) = repo.find_by_token(&token).await.ok().flatten() else {
        return unavailable_page();
    };
    if is_expired(&request) {
        return unavailable_page();
    }
    if is_exhausted(&request) {
        return html_response(
            StatusCode::TOO_MANY_REQUESTS,
            "<!doctype html><p>This upload link has reached its limit.</p>".to_string(),
        );
    }
    request_page(&request, None)
}

/// POST /request/{token}/upload — public multipart upload.
///
/// The password field is read **before** any file field. A client that puts a
/// file first is refused without writing a byte, so the check can never be
/// bypassed by field ordering.
pub async fn handle_file_request_upload(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    mut multipart: Multipart,
) -> Response {
    if !token_is_well_formed(&token) {
        return unavailable_page();
    }
    let repo = FileRequestRepository::new(state.db.pool());
    let Some(request) = repo.find_by_token(&token).await.ok().flatten() else {
        return unavailable_page();
    };
    if is_expired(&request) {
        return unavailable_page();
    }
    if is_exhausted(&request) {
        return html_response(
            StatusCode::TOO_MANY_REQUESTS,
            "<!doctype html><p>This upload link has reached its limit.</p>".to_string(),
        );
    }

    // The owner's grant is the authority behind this link: a revoked write
    // grant (or a blocked Explorer app) stops the public upload too.
    let owner_is_admin = state.auth.is_admin(&request.username).await;
    let owner_allowed =
        crate::handlers::resource_shares::explorer_app_allowed(&state, &request.username).await
            && crate::handlers::explorer::restricted_response(
                &state,
                &request.username,
                owner_is_admin,
                std::path::Path::new(&request.path),
                false,
                false,
                true,
            )
            .await
            .is_none();
    if !owner_allowed {
        return unavailable_page();
    }
    if !tokio::fs::metadata(&request.path)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return unavailable_page();
    }

    let permission = match UPLOAD_SEMAPHORE.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return html_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "<!doctype html><p>Too many uploads right now, try again.</p>".to_string(),
            )
        }
    };

    let mut password_ok = request.password_hash.is_none();
    let mut uploaded = 0usize;
    loop {
        let next = match multipart.next_field().await {
            Ok(field) => field,
            Err(_) => {
                return request_page(&request, Some("The upload was malformed."));
            }
        };
        let Some(mut field) = next else {
            break;
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "password" {
            let value = match field.text().await {
                Ok(value) => value,
                Err(_) => {
                    return request_page(&request, Some("The password could not be read."));
                }
            };
            let Some(hash) = request.password_hash.as_deref() else {
                password_ok = true;
                continue;
            };
            let now = Instant::now();
            if !may_try_password(&token, now) {
                return html_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "<!doctype html><p>Too many incorrect password attempts.</p>".to_string(),
                );
            }
            if value.is_empty() || value.len() > 256 || !state.auth.verify_password(&value, hash) {
                note_password_failure(&token, now);
                return request_page(&request, Some("The password is incorrect."));
            }
            clear_password_failures(&token);
            password_ok = true;
            continue;
        }
        if name != "file" {
            // Drain an unknown field so the multipart parser can advance.
            while let Ok(Some(_)) = field.chunk().await {}
            continue;
        }
        if !password_ok {
            return html_response(
                StatusCode::UNAUTHORIZED,
                "<!doctype html><p>The password must be sent before the file.</p>".to_string(),
            );
        }

        let raw_name = field.file_name().unwrap_or("upload").to_string();
        let Some(filename) = crate::handlers::explorer::sanitize_upload_filename(&raw_name) else {
            return request_page(&request, Some("That filename is not allowed."));
        };
        match repo.reserve_upload(&token).await {
            Ok(true) => {}
            Ok(false) => {
                return html_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "<!doctype html><p>This request has reached its upload limit.</p>".to_string(),
                )
            }
            Err(_) => {
                return html_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "<!doctype html><p>The upload could not be accepted.</p>".to_string(),
                )
            }
        }
        let temp_name = format!(".hvreq-{}", uuid::Uuid::new_v4());
        let temp_path = std::path::Path::new(&request.path).join(&temp_name);
        let mut file = match tokio::fs::File::create(&temp_path).await {
            Ok(file) => file,
            Err(_) => {
                let _ = repo.finish_upload(&token, false).await;
                return request_page(&request, Some("The destination is not writable."));
            }
        };
        let mut written = 0u64;
        let mut failed = None;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    written += chunk.len() as u64;
                    if written > MAX_FILE_REQUEST_BYTES {
                        failed = Some("The file is too large.");
                        break;
                    }
                    if file.write_all(&chunk).await.is_err() {
                        failed = Some("The file could not be written.");
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    failed = Some("The upload was interrupted.");
                    break;
                }
            }
        }
        if let Some(message) = failed {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_path).await;
            let _ = repo.finish_upload(&token, false).await;
            return request_page(&request, Some(message));
        }
        if written == 0 {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_path).await;
            let _ = repo.finish_upload(&token, false).await;
            return request_page(&request, Some("The file is empty."));
        }
        if file.sync_all().await.is_err() {
            drop(file);
            let _ = tokio::fs::remove_file(&temp_path).await;
            let _ = repo.finish_upload(&token, false).await;
            return request_page(&request, Some("The file could not be saved."));
        }
        drop(file);

        let final_path = crate::browser::non_colliding_path(
            &std::path::Path::new(&request.path).join(&filename),
        );
        if tokio::fs::rename(&temp_path, &final_path).await.is_err() {
            let _ = tokio::fs::remove_file(&temp_path).await;
            let _ = repo.finish_upload(&token, false).await;
            return request_page(&request, Some("The file could not be saved."));
        }
        // The share's own ceiling is enforced after the bytes are on disk
        // (the same read-back the Explorer upload uses), so a public upload
        // cannot silently push a quota-managed share past its limit.
        if crate::handlers::explorer::enforce_share_quota(
            &state,
            std::path::Path::new(&request.path),
            0,
        )
        .await
        .is_err()
        {
            let _ = tokio::fs::remove_file(&final_path).await;
            let _ = repo.finish_upload(&token, false).await;
            return request_page(
                &request,
                Some("The destination folder has reached its size limit."),
            );
        }
        let _ = repo.finish_upload(&token, true).await;
        uploaded += 1;
        crate::events::log_event(format!(
            "A file request from '{}' received '{}'",
            request.username,
            final_path.display()
        ));
    }
    drop(permission);

    if uploaded == 0 {
        return request_page(&request, Some("No file was selected."));
    }
    html_response(
        StatusCode::OK,
        format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Upload complete</title></head><body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:3rem auto;padding:0 1rem\"><h1 style=\"font-size:1.35rem\">Upload complete</h1><p>{uploaded} file(s) sent. You can close this page.</p></body></html>"
        ),
    )
}
