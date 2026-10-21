/* =============================================================================
 * File          : app_auth.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Restricted authentication entry point for proxied apps: a browser sent
 *   back by an app's reverse proxy lands here, is authenticated from the
 *   main session cookie, receives a narrowly-scoped app-proxy session
 *   cookie and is redirected to the app. The app token grants proxy access
 *   only — it is meaningless against the API.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - url (>= 2)
 *
 * Usage:
 *   GET /app-auth?app=<store>:<app> (browser navigation), routed in
 *   src/web/mod.rs; the app-proxy auth gate redirects here on missing
 *   sessions.
 * =============================================================================
 */

//! `GET /app-auth` — the server-chosen, restricted auth entry for proxied apps.
//!
//! This is the **second auth entry** in front of the per-app reverse proxy.
//! The proxy itself never reads the API session; when a browser hits a proxied
//! app without a valid app session, the proxy redirects here. This handler:
//!
//! 1. reads the real API session cookie (`hv_session`) — the only place the
//!    app flow touches it;
//! 2. confirms the app actually has a running proxy;
//! 3. mints an app-proxy-scoped session ([`APP_AUTH`]) and sets it as
//!    `hv_app_session`;
//! 4. redirects back to the app's proxy origin.
//!
//! Because cookies are host-scoped (not port-scoped), the cookie set here is
//! carried by the browser to the proxy port on the same host. The token grants
//! proxy access only — it is meaningless against the API — so this is a
//! strictly narrower credential than the `hv_session` the proxy used to read.
//!
//! Under an HTTPS deployment this entry is reached over TLS (the proxy gate's
//! redirect is TLS-aware — see `app_proxy::serve::app_auth_location`) and then
//! redirects to the proxy's plain-HTTP origin: a top-level navigation
//! HTTPS→HTTP is allowed, while the `hv_app_session` cookie must stay
//! non-`Secure` to be delivered there.
//!
//! Deliberately a top-level browser navigation, so it authenticates from the
//! cookie rather than the `x-session-token` header. It is a read-only GET that
//! only issues a redirect and a cookie, so there is no CSRF-relevant state
//! change beyond the user's own app session.

use crate::app_proxy::app_auth::APP_AUTH;
use crate::app_proxy::cookie::{attach_app_session, session_from_cookie_header};
use crate::app_proxy::APP_PROXY;
use crate::handlers::AppState;
use axum::{
    extract::{Query, State},
    http::{header::LOCATION, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// Percent-encode a value for use in a query string.
fn q(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[derive(serde::Deserialize)]
pub struct AppAuthQuery {
    /// Namespaced app id (`<store>:<app>`), matching the proxy key.
    app: String,
}

/// Issues an app-proxy session cookie and redirects to the app origin.
pub async fn handle_app_auth(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AppAuthQuery>,
    headers: HeaderMap,
) -> Response {
    // Authenticate from the browser cookie (this is a top-level navigation).
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(session_from_cookie_header)
        .map(|t| t.to_string());

    let username = match cookie {
        Some(token) => state.auth.get_username_from_token(&token).await,
        None => None,
    };
    let Some(username) = username else {
        // Not signed in to the main UI — send them to login, then back here.
        let next = format!("/app-auth?app={}", q(&query.app));
        let location = format!("/login?next={}", q(&next));
        return redirect(&location);
    };
    // Confirm the account still exists and is not locked.
    if state.auth.get_user(&username).await.is_err() {
        return redirect("/login");
    }

    // The app must have a running proxy to redirect to.
    let Some(info) = APP_PROXY.info(&query.app).await else {
        return (StatusCode::NOT_FOUND, "No running app proxy for this app").into_response();
    };

    let token = APP_AUTH.mint(&username);

    // Preserve the host the browser used; swap to the proxy port. The proxy
    // listener is plain HTTP by design even when the dashboard runs HTTPS:
    // the browser followed a top-level navigation here, and HTTPS→HTTP
    // top-level navigation is allowed (it is not mixed content — only
    // embedded subresources are).
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_string())
        .unwrap_or_else(|| "localhost".to_string());
    let location = format!("http://{host}:{}/", info.proxy_port);

    let resp = redirect(&location);
    attach_app_session(resp, &token)
}

fn redirect(location: &str) -> Response {
    let mut resp = (StatusCode::FOUND, "Redirecting").into_response();
    if let Ok(v) = HeaderValue::from_str(location) {
        resp.headers_mut().insert(LOCATION, v);
    }
    // The redirect carries an auth cookie; no intermediary or browser cache
    // may store it.
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use crate::app_proxy::cookie::session_from_cookie_header;

    #[test]
    fn test_app_auth_reads_only_the_api_session_cookie() {
        assert_eq!(
            session_from_cookie_header("hv_session=abc; x=1"),
            Some("abc")
        );
        assert_eq!(
            session_from_cookie_header("x=1; hv_session=abc"),
            Some("abc")
        );
        assert_eq!(session_from_cookie_header("hv_app_session=abc"), None);
        assert_eq!(session_from_cookie_header("not_hv_session=abc"), None);
    }
}
