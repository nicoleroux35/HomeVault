/* =============================================================================
 * File          : email_oauth.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin endpoints for SMTP OAuth2 (P1-16): read the configuration, save the
 *   client registration, start the authorization flow, redeem the callback and
 *   disconnect. Nothing here ever returns a client secret or a token.
 *
 * Dependency:
 *   - axum (>= 0.8), serde, crate::email_oauth, crate::handlers::AppState
 *
 * Usage:
 *   Routed under /api/settings/email/oauth in src/web/mod.rs; the card lives in
 *   Settings → Notifications. See docs/guides/email-oauth.md.
 * =============================================================================
 */

//! SMTP OAuth2 endpoints (see docs/guides/email-oauth.md).

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::AppState;
use crate::email_oauth::{self, EmailOAuthSettings, OAuthProvider};
use crate::utils::ResponseHelper;

/// `GET /api/settings/email/oauth` — the configuration as the card shows it.
pub async fn handle_email_oauth_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let settings = email_oauth::load_settings();
    ResponseHelper::json_success(json!({
        "oauth": settings.public_status(),
        "providers": [
            {
                "id": "google",
                "smtp_host": OAuthProvider::Google.smtp_defaults().0,
                "smtp_port": OAuthProvider::Google.smtp_defaults().1,
                "auth_method": OAuthProvider::Google.auth_method(),
            },
            {
                "id": "microsoft",
                "smtp_host": OAuthProvider::Microsoft.smtp_defaults().0,
                "smtp_port": OAuthProvider::Microsoft.smtp_defaults().1,
                "auth_method": OAuthProvider::Microsoft.auth_method(),
            }
        ]
    }))
}

#[derive(Debug, Default, Deserialize)]
pub struct EmailOAuthUpdate {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    /// Blank/absent keeps the stored secret (the card never shows it back).
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub redirect_url: Option<String>,
}

/// `PUT /api/settings/email/oauth` — save the client registration.
pub async fn handle_email_oauth_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let req: EmailOAuthUpdate = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };

    let current = email_oauth::load_settings();
    let provider = match req.provider.as_deref() {
        Some(value) => match OAuthProvider::parse_strict(value) {
            Ok(provider) => provider,
            Err(e) => return ResponseHelper::json_error(e),
        },
        None => current.provider,
    };
    let client_id = req.client_id.unwrap_or(current.client_id);
    let redirect_url = req.redirect_url.unwrap_or(current.redirect_url);
    let secret = req
        .client_secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match email_oauth::apply_configuration(provider, &client_id, secret, &redirect_url) {
        Ok(settings) => {
            crate::events::log_event(format!(
                "Admin '{admin}' saved the {} OAuth2 registration for outgoing email",
                provider.as_str()
            ));
            ResponseHelper::json_success(json!({ "oauth": settings.public_status() }))
        }
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// `DELETE /api/settings/email/oauth` — forget the registration and the token.
pub async fn handle_email_oauth_disconnect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    if let Err(e) = email_oauth::disconnect() {
        return ResponseHelper::json_error(format!("Could not disconnect: {e}"));
    }
    crate::events::log_event(format!(
        "Admin '{admin}' disconnected OAuth2 for outgoing email"
    ));
    let settings = EmailOAuthSettings::default();
    ResponseHelper::json_success(json!({ "oauth": settings.public_status() }))
}

/// `POST /api/settings/email/oauth/start` — build the provider URL and bind the
/// flow to this browser with an HttpOnly state cookie.
pub async fn handle_email_oauth_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    over_tls: Option<axum::Extension<crate::middleware::ArrivedOverTls>>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let settings = email_oauth::load_settings();
    if !settings.is_configured() {
        return ResponseHelper::json_error(
            "Save the client ID, client secret and redirect URL before connecting".to_string(),
        );
    }
    let state_value = match email_oauth::new_state() {
        Ok(value) => value,
        Err(e) => return ResponseHelper::json_internal_error(e),
    };
    let url = match email_oauth::authorize_url(
        settings.provider,
        &settings.client_id,
        &settings.redirect_url,
        &state_value,
    ) {
        Ok(url) => url,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let secure = if over_tls.is_some() { "; Secure" } else { "" };
    let cookie = format!(
        "{cookie}={state}; Path=/api/settings/email/oauth; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        cookie = email_oauth::STATE_COOKIE,
        state = state_value,
        max_age = email_oauth::STATE_TTL.as_secs(),
    );
    let mut response = ResponseHelper::json_success(json!({
        "authorization_url": url,
        "expires_in": email_oauth::STATE_TTL.as_secs(),
    }));
    // The state itself is already in the URL the admin is about to open; the
    // cookie is the half a forged link cannot set.
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

#[derive(Debug, Default, Deserialize)]
pub struct EmailOAuthCallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// `GET /api/settings/email/oauth/callback` — public, because the provider
/// redirects the browser here. The state cookie is what proves this callback
/// belongs to the admin's own flow.
pub async fn handle_email_oauth_callback(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<EmailOAuthCallbackQuery>,
) -> Response {
    let Some(state_value) = query
        .state
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return callback_page(
            StatusCode::BAD_REQUEST,
            "Authorization failed",
            "The provider did not return a state value, so the answer cannot be trusted.",
        );
    };
    let cookie = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            crate::app_proxy::cookie::named_cookie_from_header(value, email_oauth::STATE_COOKIE)
        });
    if cookie != Some(state_value) {
        return callback_page(
            StatusCode::BAD_REQUEST,
            "Authorization failed",
            "This browser did not start the authorization flow. Open Settings → Notifications and press Connect again.",
        );
    }
    if query.error.is_some() {
        // A refusal is a legitimate outcome (the admin pressed Cancel). It must
        // not be stored as a scary connection error.
        let _ = email_oauth::take_state(state_value);
        let detail = query
            .error_description
            .as_deref()
            .or(query.error.as_deref())
            .unwrap_or("the provider refused the request");
        return callback_page(
            StatusCode::BAD_REQUEST,
            "Authorization refused",
            &format!("The provider did not grant access: {detail}"),
        );
    }
    let Some(code) = query
        .code
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        let _ = email_oauth::take_state(state_value);
        return callback_page(
            StatusCode::BAD_REQUEST,
            "Authorization failed",
            "The provider did not return an authorization code.",
        );
    };
    if !email_oauth::take_state(state_value) {
        return callback_page(
            StatusCode::BAD_REQUEST,
            "Authorization expired",
            "This authorization link was already used or has expired. Start again from Settings → Notifications.",
        );
    }

    let settings = email_oauth::load_settings();
    if !settings.is_configured() {
        return callback_page(
            StatusCode::BAD_REQUEST,
            "Authorization failed",
            "The OAuth2 configuration is missing. Save the client ID, client secret and redirect URL first.",
        );
    }

    // The token exchange is a blocking HTTP call: run it off the runtime, like
    // every other outbound mail operation.
    let code = code.to_string();
    let exchange =
        tokio::task::spawn_blocking(move || email_oauth::exchange_code_blocking(&settings, &code))
            .await;
    let token = match exchange {
        Ok(Ok(token)) => token,
        Ok(Err(e)) => {
            email_oauth::record_error(&e);
            return callback_page(
                StatusCode::BAD_REQUEST,
                "Authorization failed",
                &format!("The provider refused the token exchange: {e}"),
            );
        }
        Err(e) => {
            return callback_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Authorization failed",
                &format!("The token exchange task stopped: {e}"),
            )
        }
    };

    match token.refresh_token.as_deref() {
        Some(refresh) => {
            if let Err(e) = email_oauth::store_refresh_token_after_exchange(refresh) {
                email_oauth::record_error(&e);
                return callback_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Authorization failed",
                    &format!("The refresh token could not be stored: {e}"),
                );
            }
            crate::events::log_event("OAuth2 was connected for outgoing email".to_string());
            callback_page(
                StatusCode::OK,
                "Email connected",
                "HomeVault can now send mail through the provider. Close this tab and return to Settings.",
            )
        }
        None => callback_page(
            StatusCode::BAD_REQUEST,
            "Authorization incomplete",
            "The provider did not return a refresh token. Remove HomeVault from the provider's authorized apps, then connect again.",
        ),
    }
}

/// A small self-contained result page. No script, no external resource, and
/// every value is escaped: the callback is reachable without a session.
fn callback_page(status: StatusCode, title: &str, message: &str) -> Response {
    let body = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:38rem;margin:4rem auto;padding:0 1.5rem;color:#1f2933;line-height:1.5}}\
         h1{{font-size:1.4rem}}a{{color:#2E72C9}}</style></head>\
         <body><h1>{title}</h1><p>{message}</p>\
         <p><a href=\"/desktop\">Return to HomeVault</a></p></body></html>",
        title = escape_html(title),
        message = escape_html(message),
    );
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    // No content sniffing, no framing, no form submission from this page.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // Drop the one-time state cookie now that it has served its purpose.
    if let Ok(value) = HeaderValue::from_str(&format!(
        "{}=; Path=/api/settings/email/oauth; HttpOnly; SameSite=Lax; Max-Age=0",
        email_oauth::STATE_COOKIE
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

/// The callback query is also used by tests to assert that a provider error is
/// echoed only after being escaped.
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_escape_html_neutralises_the_callback_echo() {
        assert_eq!(
            escape_html("<script>alert('x')</script> & \"q\""),
            "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;q&quot;"
        );
    }

    #[test]
    fn test_callback_page_is_html_and_has_no_store() {
        let response = callback_page(StatusCode::OK, "Email connected", "Done");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
    }

    #[test]
    fn test_state_ttl_is_short() {
        // A started flow that sits unused for a day must be useless.
        assert!(email_oauth::STATE_TTL <= Duration::from_secs(3600));
    }
}
