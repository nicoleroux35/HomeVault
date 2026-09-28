//! The `hv_session` cookie. HomeVault's API is header-authenticated
//! (`x-session-token`), but a browser navigating straight to a proxied
//! app's port sends no custom header — only cookies. So login/2FA also
//! drop the session token as an `hv_session` cookie, read *only* by the
//! app proxy's auth gate (`crate::app_proxy::serve`). The API itself never
//! reads this cookie, so no new CSRF surface is introduced there.

use axum::http::{header::SET_COOKIE, HeaderValue};
use axum::response::Response;

pub const SESSION_COOKIE: &str = "hv_session";

/// 30 days, matching the "remember me" session TTL. Without remember-me the
/// cookie is a session cookie (no Max-Age) and the server-side session
/// still expires on its own shorter schedule.
const REMEMBER_MAX_AGE: i64 = 60 * 60 * 24 * 30;

/// Attach `Set-Cookie: hv_session=<token>` to a login/2FA success response.
/// `SameSite=Lax` so a top-level navigation to the app port carries it;
/// `HttpOnly` so page scripts can't read it; not `Secure` because HomeVault
/// is commonly served over plain HTTP on a LAN.
pub fn attach_session(mut resp: Response, token: &str, remember_me: bool) -> Response {
    let cookie = if remember_me {
        format!(
            "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={REMEMBER_MAX_AGE}"
        )
    } else {
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax")
    };
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().append(SET_COOKIE, v);
    }
    resp
}

/// Attach the expiry cookie to a logout response.
pub fn clear_session(mut resp: Response) -> Response {
    if let Ok(v) =
        HeaderValue::from_str(&format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"))
    {
        resp.headers_mut().append(SET_COOKIE, v);
    }
    resp
}

/// Pull the `hv_session` value out of a `Cookie:` header value.
pub fn session_from_cookie_header(header: &str) -> Option<&str> {
    for pair in header.split(';') {
        let pair = pair.trim();
        if let Some(rest) = pair.strip_prefix(SESSION_COOKIE) {
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val);
            }
        }
    }
    None
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
}
