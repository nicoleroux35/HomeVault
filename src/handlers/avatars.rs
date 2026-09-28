use crate::avatars::{self, AvatarError};
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue},
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

/// Query string accepted by the preview endpoint. `seed` is optional - when
/// missing the server generates a random one on the fly. `size` is also
/// optional and defaults to 128 (matches what the wizard renders).
#[derive(Debug, Deserialize, Default)]
pub struct PreviewQuery {
    #[serde(default)]
    pub seed: Option<String>,
    #[serde(default)]
    pub size: Option<u32>,
    /// When provided, the response is `Cache-Control: no-store` so the wizard
    /// always sees a fresh avatar after a "regenerate" click.
    #[serde(default)]
    pub fresh: Option<bool>,
}

/// Query string accepted by the user avatar endpoint. Only `size` and
/// `fresh` are honored - the seed comes from the `users.avatar_seed` column.
#[derive(Debug, Deserialize, Default)]
pub struct AvatarQuery {
    #[serde(default)]
    pub size: Option<u32>,
    #[serde(default)]
    pub fresh: Option<bool>,
}

fn clamp_size(size: Option<u32>) -> u32 {
    size.unwrap_or(128).clamp(32, 512)
}

fn build_svg_response(svg: String, fresh: bool) -> Response {
    let mut response = Response::new(svg.into());
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml; charset=utf-8"),
    );

    let cache = if fresh {
        "no-store, must-revalidate"
    } else {
        "public, max-age=300, immutable"
    };
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(cache).unwrap_or(HeaderValue::from_static("no-store")),
    );

    response
}

fn error_response(err: AvatarError) -> Response {
    ResponseHelper::json_internal_error(format!("Failed to render avatar: {}", err))
}

/// `GET /api/avatars/preview?seed=<seed>&size=<px>&fresh=true`
///
/// Public (no auth) - used by the setup wizard to preview and regenerate an
/// avatar in real time. The response is the raw SVG payload.
#[axum_macros::debug_handler]
pub async fn handle_avatar_preview(
    State(_state): State<Arc<AppState>>,
    Query(q): Query<PreviewQuery>,
) -> Response {
    let seed = q
        .seed
        .as_deref()
        .and_then(avatars::normalize_seed)
        .unwrap_or_else(avatars::generate_random_seed);

    match avatars::render(&seed, Some(clamp_size(q.size))) {
        Ok(svg) => build_svg_response(svg, q.fresh.unwrap_or(true)),
        Err(e) => error_response(e),
    }
}

/// `GET /api/users/:username/avatar.svg?size=<px>`
///
/// Returns the persistent avatar for a given user. The seed comes from the
/// `users.avatar_seed` column; if it is missing, a random one is generated
/// on the fly (and the response is `no-store`).
#[axum_macros::debug_handler]
pub async fn handle_user_avatar(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    Query(q): Query<AvatarQuery>,
) -> Response {
    let user_repo = crate::storage::UserRepository::new(state.db.pool());
    let stored_seed = match user_repo.find_by_username(&username).await {
        Ok(Some(user)) => user.avatar_seed,
        _ => String::new(),
    };

    let has_persisted_seed = !stored_seed.is_empty();
    let svg = match avatars::render_for(
        if has_persisted_seed {
            Some(stored_seed.as_str())
        } else {
            None
        },
        Some(clamp_size(q.size)),
    ) {
        Ok(s) => s,
        Err(e) => return error_response(e),
    };

    build_svg_response(svg, !has_persisted_seed || q.fresh.unwrap_or(false))
}

/// `PATCH /api/auth/me/avatar` with body `{ "seed": "..." }` -
/// regenerate the avatar for the currently-authenticated user.
#[derive(Debug, Deserialize)]
pub struct UpdateAvatarRequest {
    pub seed: Option<String>,
}

#[axum_macros::debug_handler]
pub async fn handle_update_my_avatar(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateAvatarRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    let seed = req
        .seed
        .as_deref()
        .and_then(avatars::normalize_seed)
        .unwrap_or_else(avatars::generate_random_seed);

    let user_repo = crate::storage::UserRepository::new(state.db.pool());
    if let Err(e) = user_repo.update_avatar_seed(&username, &seed).await {
        return ResponseHelper::json_internal_error(format!("Failed to update avatar: {}", e));
    }

    // Re-render so the client can preview the saved avatar immediately.
    match avatars::render(&seed, Some(128)) {
        Ok(svg) => ResponseHelper::json_success(serde_json::json!({
            "username": username,
            "seed": seed,
            "svg": svg,
            "avatar_url": format!("/api/users/{}/avatar.svg", username),
        })),
        Err(e) => error_response(e),
    }
}

/// Helper used by other handlers (e.g. header integration) to grab the raw
/// SVG bytes for a user without re-rendering.
pub async fn render_user_avatar(
    state: &Arc<AppState>,
    username: &str,
    size: Option<u32>,
) -> Result<String, AvatarError> {
    let user_repo = crate::storage::UserRepository::new(state.db.pool());
    let stored_seed = match user_repo.find_by_username(username).await {
        Ok(Some(user)) => user.avatar_seed,
        _ => String::new(),
    };

    if stored_seed.is_empty() {
        avatars::render_for(None, size)
    } else {
        avatars::render(&stored_seed, size)
    }
}

/// Convenience: return a 404 with a static fallback SVG so callers can keep
/// embedding `<img src="...">` without conditional JS.
pub fn missing_avatar_response() -> Response {
    // The raw string delimiter uses ## to survive `#e5e7eb` inside the
    // SVG payload (a single `#` would close the string prematurely).
    let fallback = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img" aria-label="No avatar"><rect width="64" height="64" fill="#e5e7eb"/><circle cx="32" cy="24" r="10" fill="#9ca3af"/><path d="M12 56c0-11 9-20 20-20s20 9 20 20z" fill="#9ca3af"/></svg>"##;
    let mut response = Response::new(fallback.to_string().into());
    *response.status_mut() = axum::http::StatusCode::NOT_FOUND;
    response
}