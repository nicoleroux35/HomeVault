/* =============================================================================
 * File          : cookie.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The hv_session cookie: dropped by login/2FA alongside the header token
 *   and read ONLY by the app-proxy auth gate (browsers navigating straight
 *   to a proxied app's port send cookies, not custom headers). The API
 *   itself never reads it, so no new CSRF surface exists there.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Set in handlers/auth.rs on login; consumed by app_proxy/serve.rs.
 * =============================================================================
 */

//! The `hv_session` cookie. HomeVault's API is header-authenticated
//! (`x-session-token`), but a browser navigating straight to a proxied
//! app's port sends no custom header — only cookies. So login/2FA also
//! drop the session token as an `hv_session` cookie, read *only* by the
//! app proxy's auth gate (`crate::app_proxy::serve`). The API itself never
//! reads this cookie, so no new CSRF surface is introduced there.

use axum::http::{header::SET_COOKIE, HeaderValue};
use axum::response::Response;

pub const SESSION_COOKIE: &str = "hv_session";

/// The app-proxy-scoped session cookie. Set by `GET /app-auth` (on the main
/// origin) after validating the real [`SESSION_COOKIE`], and read *only* by
/// the app proxy's auth gate. It is a distinct, app-proxy-scoped credential:
/// it grants no API access, so leaking it to a proxied app is not a privilege
/// escalation (and the proxy strips it before forwarding anyway).
pub const APP_SESSION_COOKIE: &str = "hv_app_session";

/// 30 days, matching the "remember me" session TTL. Without remember-me the
/// cookie is a session cookie (no Max-Age) and the server-side session
/// still expires on its own shorter schedule.
const REMEMBER_MAX_AGE: i64 = 60 * 60 * 24 * 30;

/// Attach `Set-Cookie: hv_session=<token>` to a login/2FA success response.
///
/// `SameSite=Lax` so a top-level navigation to the app port carries it;
/// `HttpOnly` so page scripts can't read it.
///
/// `Secure` is added when the request actually arrived over TLS (`over_tls`).
/// It cannot be unconditional — HomeVault is commonly served over plain HTTP on
/// a LAN, and a `Secure` cookie is simply dropped by the browser there, which
/// would silently break the app proxy. Conditional is the only correct answer:
/// the flag follows the connection that set it.
pub fn attach_session(
    mut resp: Response,
    token: &str,
    remember_me: bool,
    over_tls: bool,
) -> Response {
    let secure = if over_tls { "; Secure" } else { "" };
    let cookie = if remember_me {
        format!(
            "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={REMEMBER_MAX_AGE}"
        )
    } else {
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{secure}")
    };
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().append(SET_COOKIE, v);
    }
    resp
}

/// Attach the expiry cookie to a logout response.
pub fn clear_session(mut resp: Response) -> Response {
    if let Ok(v) = HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    )) {
        resp.headers_mut().append(SET_COOKIE, v);
    }
    resp
}

/// Pull the `hv_session` value out of a `Cookie:` header value.
pub fn session_from_cookie_header(header: &str) -> Option<&str> {
    cookie_value(header, SESSION_COOKIE)
}

/// Pull the `hv_app_session` value out of a `Cookie:` header value.
pub fn app_session_from_cookie_header(header: &str) -> Option<&str> {
    cookie_value(header, APP_SESSION_COOKIE)
}

/// Attach `Set-Cookie: hv_app_session=<token>` to a response. `HttpOnly` so
/// page scripts can't read it; `SameSite=Lax` so a top-level navigation to the
/// app origin carries it; not `Secure` because HomeVault is commonly served
/// over plain HTTP on a LAN.
pub fn attach_app_session(mut resp: Response, token: &str) -> Response {
    let cookie = format!(
        "{APP_SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={REMEMBER_MAX_AGE}"
    );
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().append(SET_COOKIE, v);
    }
    resp
}

/// Attach the expiry cookie for `hv_app_session` to a logout response.
pub fn clear_app_session(mut resp: Response) -> Response {
    if let Ok(v) = HeaderValue::from_str(&format!(
        "{APP_SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    )) {
        resp.headers_mut().append(SET_COOKIE, v);
    }
    resp
}

/// Extract one named cookie from a `Cookie:` header value. Exact name match
/// only — a cookie whose name merely ends with `name` must not match.
fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    for pair in header.split(';') {
        let pair = pair.trim();
        if let Some(rest) = pair.strip_prefix(name) {
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val);
            }
        }
    }
    None
}

/// Rebuild a `Cookie:` header value with both HomeVault credentials removed,
/// keeping every other cookie (apps set and read their own). Returns `None`
/// when nothing is left, so the caller can drop the header entirely.
///
/// The gateway strips these before forwarding to an upstream app: a catalog
/// app must never see the main session token (it would let a compromised app
/// impersonate a signed-in admin on the whole dashboard) nor the app session.
pub fn strip_homevault_cookies(header: &str) -> Option<String> {
    let kept: Vec<&str> = header
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or("").trim();
            name != SESSION_COOKIE && name != APP_SESSION_COOKIE
        })
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_from_cookie_header() {
        assert_eq!(
            session_from_cookie_header("foo=1; hv_session=abc123; bar=2"),
            Some("abc123")
        );
        assert_eq!(session_from_cookie_header("hv_session=xyz"), Some("xyz"));
        assert_eq!(session_from_cookie_header("other=1"), None);
        // Must not match a cookie whose name merely ends with hv_session.
        assert_eq!(session_from_cookie_header("not_hv_session=abc"), None);
    }

    #[test]
    fn test_app_session_from_cookie_header() {
        assert_eq!(
            app_session_from_cookie_header("hv_session=api; hv_app_session=app1"),
            Some("app1")
        );
        assert_eq!(
            app_session_from_cookie_header("hv_app_session=app1"),
            Some("app1")
        );
        // The API session must never be mistaken for the app session.
        assert_eq!(app_session_from_cookie_header("hv_session=api"), None);
        assert_eq!(app_session_from_cookie_header("not_hv_app_session=x"), None);
    }

    #[test]
    fn test_strip_homevault_cookies_removes_only_homevault_credentials() {
        assert_eq!(
            strip_homevault_cookies("hv_session=abc; theme=dark"),
            Some("theme=dark".to_string())
        );
        assert_eq!(
            strip_homevault_cookies("app=1; hv_app_session=xyz; other=2"),
            Some("app=1; other=2".to_string())
        );
        assert_eq!(
            strip_homevault_cookies("hv_session=a; hv_app_session=b; keep=c"),
            Some("keep=c".to_string())
        );
        // A cookie whose name merely contains a credential name survives.
        assert_eq!(
            strip_homevault_cookies("not_hv_session=abc"),
            Some("not_hv_session=abc".to_string())
        );
        // Only HomeVault cookies present → header dropped entirely.
        assert_eq!(strip_homevault_cookies(" hv_session=abc "), None);
        assert_eq!(strip_homevault_cookies(""), None);
    }
}

#[cfg(test)]
mod secure_cookie_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::header::SET_COOKIE;

    fn cookie_of(resp: &Response) -> String {
        resp.headers()
            .get(SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// Plain HTTP on a LAN is the common deployment: a `Secure` cookie would be
    /// silently dropped by the browser and the app proxy would break.
    #[test]
    fn test_plain_http_cookie_has_no_secure_flag() {
        let resp = attach_session(Response::new(Body::empty()), "tok", false, false);
        let cookie = cookie_of(&resp);
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(!cookie.contains("Secure"), "got: {cookie}");
    }

    /// Over TLS the flag must be set, otherwise the cookie can be replayed over
    /// a downgraded connection.
    #[test]
    fn test_tls_cookie_carries_the_secure_flag() {
        let resp = attach_session(Response::new(Body::empty()), "tok", false, true);
        let cookie = cookie_of(&resp);
        assert!(cookie.contains("; Secure"), "got: {cookie}");
        assert!(cookie.contains("HttpOnly"));
    }

    #[test]
    fn test_remember_me_keeps_secure_and_max_age_together() {
        let resp = attach_session(Response::new(Body::empty()), "tok", true, true);
        let cookie = cookie_of(&resp);
        assert!(cookie.contains("; Secure"), "got: {cookie}");
        assert!(cookie.contains("Max-Age="), "got: {cookie}");
        assert!(cookie.contains("HttpOnly"));
    }
}
