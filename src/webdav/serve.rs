/* =============================================================================
 * File          : serve.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The built-in WebDAV listener: HTTP Basic authentication against real
 *   HomeVault accounts, per-member share grants resolved once per request,
 *   then the request is handed to the dav-server handler built over
 *   `WebdavFs`. Nothing here runs as root, and a failed auth is a 401 with
 *   the standard challenge — never a pass-through.
 *
 * Dependencies:
 *   - axum (>= 0.8), dav-server (>= 0.11), base64 (>= 0.22)
 *
 * Usage:
 *   Started by src/webdav/mod.rs; the share/config change path rebuilds the
 *   handler and rebinds the listener.
 * ============================================================================= */

//! WebDAV request handling: Basic auth, grant resolution, dav-server dispatch.

use crate::handlers::AppState;
use crate::shares::WebdavConfig;
use crate::storage::resource_share_repo::{ResourceKind, ResourceShareRepository};
use crate::webdav::fs::WebdavShare;
use crate::webdav::vfs::{WebdavCreds, WebdavFs};
use axum::{
    extract::{ConnectInfo, Request, State},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use base64::Engine;
use dav_server::fakels::FakeLs;
use dav_server::memls::MemLs;
use dav_server::{DavHandler, DavMethodSet};
use http::header::WWW_AUTHENTICATE;
use http::StatusCode;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

/// Everything a WebDAV request needs: the app state (auth + DB), the
/// dav-server handler bound to the current share table, and the config.
pub struct WebdavCtx {
    pub state: Arc<AppState>,
    pub dav: DavHandler<WebdavCreds>,
    pub config: WebdavConfig,
}

/// Builds the dav-server handler for a share table and config.
///
/// `locking` selects a real in-memory lock system (so Windows/macOS clients
/// can take locks) or a no-op one; `allowed_methods`, when non-empty, becomes
/// the exact method allow-list; `path_prefix` is stripped from every request
/// path (empty or `/` means "no prefix").
pub fn build_handler(shares: Vec<WebdavShare>, config: &WebdavConfig) -> DavHandler<WebdavCreds> {
    let mut builder = DavHandler::<WebdavCreds>::builder()
        .filesystem(Box::new(WebdavFs::new(shares)))
        // Symlinks inside a share are hidden rather than followed: a link
        // planted in a share must not become a path out of it.
        .hide_symlinks(true)
        .autoindex(true);

    // `MemLs::new()` / `FakeLs::new()` already return a `Box<dyn DavLockSystem>`
    // handle — do not box them again.
    builder = if config.locking {
        builder.locksystem(MemLs::new())
    } else {
        builder.locksystem(FakeLs::new())
    };

    if !config.allowed_methods.is_empty() {
        if let Ok(set) = DavMethodSet::from_vec(config.allowed_methods.clone()) {
            builder = builder.methods(set);
        } else {
            tracing::warn!(
                "WebDAV: ignoring unknown entry in allowed_methods {:?}",
                config.allowed_methods
            );
        }
    }

    if let Some(prefix) = normalized_prefix(&config.path_prefix) {
        builder = builder.strip_prefix(prefix);
    }

    builder.build_handler()
}

/// Normalizes `path_prefix` to something `strip_prefix` accepts, or `None`
/// when the prefix is empty / the bare root (the common case).
pub fn normalized_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let stripped = with_slash.trim_end_matches('/').to_string();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped)
    }
}

/// The axum router for the WebDAV listener: every path is handled by the
/// same dav-server dispatcher. When `compression` is enabled a gzip/brotli
/// layer wraps the responses (already a dependency of the main server).
pub fn router(ctx: Arc<WebdavCtx>) -> Router {
    let compression = ctx.config.compression;
    let router = Router::new().fallback(any(handle_dav)).with_state(ctx);
    if compression {
        router.layer(tower_http::compression::CompressionLayer::new())
    } else {
        router
    }
}

/// Handles one WebDAV request: authenticate, resolve grants, dispatch.
pub async fn handle_dav(
    State(ctx): State<Arc<WebdavCtx>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    // Reject a path outside the configured prefix before it reaches
    // dav-server, so the prefix is a hard mount point and not a hint.
    if let Some(prefix) = normalized_prefix(&ctx.config.path_prefix) {
        let path = req.uri().path();
        if path != prefix && !path.starts_with(&format!("{prefix}/")) {
            return StatusCode::NOT_FOUND.into_response();
        }
    }

    if !ctx.config.require_auth {
        // Open mode (explicitly configured): every reachable share is served
        // to anyone, which is the meaning of turning authentication off.
        let creds = WebdavCreds {
            username: "anonymous".to_string(),
            is_admin: true,
            granted: Arc::new(HashSet::new()),
        };
        return into_axum(
            ctx.dav
                .handle_guarded(req, "anonymous".to_string(), creds)
                .await,
        );
    }

    let Some((username, password)) = basic_credentials(req.headers()) else {
        return unauthorized();
    };

    let ip = addr.ip().to_string();
    match ctx
        .state
        .auth
        .authenticate(&username, &password, &ip, Some("webdav"))
        .await
    {
        Ok(true) => {}
        Ok(false) => return unauthorized(),
        Err(_) => return unauthorized(),
    }

    let creds = match build_creds(&ctx.state, &username).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("WebDAV: could not resolve grants for {username}: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    into_axum(ctx.dav.handle_guarded(req, username, creds).await)
}

/// Converts a dav-server response (its own body type) into an axum response
/// without buffering — `dav_server::body::Body` implements `http_body::Body`
/// with `Data = Bytes`, which is all `axum::body::Body::new` needs.
fn into_axum(resp: http::Response<dav_server::body::Body>) -> Response {
    let (parts, body) = resp.into_parts();
    Response::from_parts(parts, axum::body::Body::new(body))
}

/// Builds the per-request credentials: admin flag plus the names of the
/// shares this member holds a `share` grant for.
async fn build_creds(state: &Arc<AppState>, username: &str) -> Result<WebdavCreds, String> {
    let is_admin = state.auth.is_admin(username).await;
    let granted = if is_admin {
        HashSet::new()
    } else {
        let repo = ResourceShareRepository::new(state.db.pool());
        repo.list_for_member(username, Some(ResourceKind::Share))
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|grant| grant.resource)
            .collect()
    };
    Ok(WebdavCreds {
        username: username.to_string(),
        is_admin,
        granted: Arc::new(granted),
    })
}

/// Extracts `(username, password)` from an `Authorization: Basic` header.
///
/// Returns `None` for any other scheme, a malformed base64 payload, or a
/// decoded value without a `:` separator — all of which are a 401, never an
/// authenticated request.
fn basic_credentials(headers: &http::HeaderMap) -> Option<(String, String)> {
    let raw = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = raw
        .strip_prefix("Basic ")
        .or_else(|| raw.strip_prefix("basic "))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    if user.is_empty() {
        return None;
    }
    Some((user.to_string(), pass.to_string()))
}

fn unauthorized() -> Response {
    let mut resp = StatusCode::UNAUTHORIZED.into_response();
    resp.headers_mut().insert(
        WWW_AUTHENTICATE,
        http::HeaderValue::from_static("Basic realm=\"HomeVault WebDAV\""),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn basic(user: &str, pass: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        headers.insert(
            http::header::AUTHORIZATION,
            format!("Basic {encoded}").parse().unwrap(),
        );
        headers
    }

    #[test]
    fn test_basic_credentials_decodes_user_and_password() {
        let headers = basic("alice", "s3cret:pw");
        assert_eq!(
            basic_credentials(&headers),
            Some(("alice".to_string(), "s3cret:pw".to_string()))
        );
    }

    #[test]
    fn test_basic_credentials_rejects_missing_or_other_scheme() {
        assert_eq!(basic_credentials(&HeaderMap::new()), None);

        let mut bearer = HeaderMap::new();
        bearer.insert(http::header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(basic_credentials(&bearer), None);
    }

    #[test]
    fn test_basic_credentials_rejects_malformed_base64_and_empty_user() {
        let mut bad = HeaderMap::new();
        bad.insert(
            http::header::AUTHORIZATION,
            "Basic !!!notbase64".parse().unwrap(),
        );
        assert_eq!(basic_credentials(&bad), None);

        // ":pw" decodes fine but has no username.
        let mut empty = HeaderMap::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(":pw");
        empty.insert(
            http::header::AUTHORIZATION,
            format!("Basic {encoded}").parse().unwrap(),
        );
        assert_eq!(basic_credentials(&empty), None);
    }

    #[test]
    fn test_normalized_prefix() {
        assert_eq!(normalized_prefix(""), None);
        assert_eq!(normalized_prefix("/"), None);
        assert_eq!(normalized_prefix("  "), None);
        assert_eq!(normalized_prefix("dav"), Some("/dav".to_string()));
        assert_eq!(normalized_prefix("/dav"), Some("/dav".to_string()));
        assert_eq!(normalized_prefix("/dav/"), Some("/dav".to_string()));
    }
}
