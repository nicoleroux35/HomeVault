/* =============================================================================
 * File          : admin_guard.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The one and only session-permission primitive of the tree: require_auth
 *   validates the caller's session token and returns the username,
 *   require_admin additionally refuses non-admin accounts. Both answer with
 *   a ready-made JSON error when the check fails, so a handler guards itself
 *   with a single match.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   `require_auth(&state, &headers).await` / `require_admin(...)` at the
 *   top of each protected handler — see docs/API.md for the auth model of
 *   every endpoint.
 * =============================================================================
 */

//! Session permission checks shared by every protected handler.
//!
//! `require_auth` resolves and validates the caller's session token and
//! returns the username; `require_admin` additionally refuses non-admin
//! accounts. Both return a ready-to-send JSON error response on failure, so
//! a handler guards itself with a single `match`.
//!
//! **This module is the only place in the tree that may turn an
//! `x-session-token` header into an identity decision** (ROADMAP X.7). Every
//! other module used to carry its own copy — `handlers::auth::require_admin`
//! and private `require_admin` helpers in `syncthing`, `shares`, `vpn`, `ups`,
//! `permissions`, `snmp`, `user_homes` and `downloader` — which is how the
//! error contract drifted: a non-admin caller was answered `400 Bad Request`
//! by `shares`/`permissions`/`snmp` and `403 Forbidden` everywhere else, so a
//! client could not tell "you sent a malformed request" from "you are not
//! allowed to do this". Those copies are now one-line delegations to this
//! module, and the unit tests at the bottom fail if a new copy appears.
//!
//! The status contract is stated once, here and in `docs/API.md`:
//!
//! | Situation                          | Status |
//! |------------------------------------|--------|
//! | `x-session-token` header absent    | `401`  |
//! | Token unknown or expired           | `401`  |
//! | Valid session, account not admin   | `403`  |
//! | Valid session, malformed request   | `400`  |
//!
//! The last row is what gives the other three their meaning: `400` is reserved
//! for a request the server could not understand, so a client can tell "sign in
//! again" from "fix your input" by the status alone.

use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use std::sync::Arc;

/// The header carrying the caller's opaque session token.
pub const SESSION_HEADER: &str = "x-session-token";

/// Extracts the raw `x-session-token` value, or an empty string when absent.
///
/// Only this module calls it: a handler never needs the token itself, only
/// the username `require_auth` / `require_admin` return.
fn session_token(headers: &HeaderMap) -> &str {
    headers
        .get(SESSION_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
}

/// The message a denial carries, derived from its status alone.
///
/// Handlers that answer a JSON envelope instead of a `Response` (Syncthing,
/// Downloader, VPN) need the wording without consuming the body. Deriving it
/// from the status keeps the *decision* here: those modules cannot invent a
/// different rule, only a different envelope.
pub fn denial_message(status: StatusCode) -> String {
    match status {
        StatusCode::FORBIDDEN => "Admin privileges required for this operation".to_string(),
        StatusCode::UNAUTHORIZED => "Invalid or expired session".to_string(),
        _ => "No session token provided".to_string(),
    }
}

/// Ensures the caller is authenticated AND an admin.
/// Returns `Ok(username)` on success, `Err(response)` otherwise.
// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
pub async fn require_admin(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, Response> {
    let username = require_auth(state, headers).await?;

    if state.auth.is_admin(&username).await {
        Ok(username)
    } else {
        Err(ResponseHelper::json_forbidden(
            "Admin privileges required for this operation".to_string(),
        ))
    }
}

/// An authenticated caller: who they are, and the opaque token that proved it.
///
/// `token` exists only for server-side comparisons — "is this row my current
/// session?", "revoke everything except this one". It is never serialized: the
/// self-service session list answers with `session_public_id(token)` hashes so
/// the response can never be replayed as a credential (ROADMAP 14.1).
#[derive(Debug, Clone)]
pub struct Caller {
    pub username: String,
    pub token: String,
}

/// Ensures the caller is authenticated (admin or regular user).
/// Returns `Ok(username)` on success, `Err(response)` otherwise.
// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
pub async fn require_auth(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, Response> {
    require_auth_caller(state, headers)
        .await
        .map(|caller| caller.username)
}

/// Same check as [`require_auth`], but also returns the caller's own token.
///
/// Only the self-service session endpoints need it, and only to compare it
/// against the caller's other sessions — never to hand it back to the client.
// `Response` is axum's canonical error type; see the note on `require_auth`.
#[allow(clippy::result_large_err)]
pub async fn require_auth_caller(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<Caller, Response> {
    let token = session_token(headers);

    if token.is_empty() {
        // `401`, not `400`: the caller did not authenticate. A client that
        // branches on the status must be able to tell "you are not signed in"
        // from "your request is malformed" (ROADMAP X.8).
        return Err(ResponseHelper::json_unauthorized(
            "No session token provided".to_string(),
        ));
    }

    match state.auth.get_username_from_token(token).await {
        Some(username) => Ok(Caller {
            username,
            token: token.to_string(),
        }),
        None => Err(ResponseHelper::json_unauthorized(
            "Invalid or expired session".to_string(),
        )),
    }
}

/// Validates the caller and returns only their session token.
///
/// Some handlers still resolve the session themselves after the guard (the
/// Explorer's path guard needs the `Session` row, not just the name). They call
/// this instead of parsing the header, so the `401` contract stays in one place
/// (ROADMAP X.8).
// `Response` is axum's canonical error type; see the note on `require_auth`.
#[allow(clippy::result_large_err)]
pub async fn require_session_token(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<String, Response> {
    require_auth_caller(state, headers)
        .await
        .map(|caller| caller.token)
}

#[cfg(test)]
mod tests {
    //! Source guards for ROADMAP X.7 — one auth primitive, no local copies.
    //!
    //! These read the handler sources from disk (`CARGO_MANIFEST_DIR` is baked
    //! in at compile time), so they cost nothing at runtime and cannot go
    //! stale: every `src/handlers/*.rs` is scanned, not just a hand-kept list.

    use super::denial_message;
    use super::SESSION_HEADER;
    use axum::http::StatusCode;
    use std::path::{Path, PathBuf};

    fn handlers_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/handlers")
    }

    fn handler_sources() -> Vec<(String, String)> {
        let mut out = Vec::new();
        let dir = handlers_dir();
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("utf-8 file name")
                .to_string();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            out.push((name, text));
        }
        assert!(
            !out.is_empty(),
            "no handler source found — the guard is inert"
        );
        out
    }

    /// A module that needs a different envelope may keep a one-line helper,
    /// but it must delegate. No file other than this one defines the check.
    #[test]
    fn no_module_defines_its_own_auth_primitive() {
        // Built with `concat!` so this file's own text does not contain the
        // needle (a source guard would otherwise match itself).
        let define_admin = concat!("async fn require_", "admin");
        let define_auth = concat!("async fn require_", "auth");

        for (name, text) in handler_sources() {
            if name == "admin_guard.rs" {
                continue;
            }
            assert!(
                !text.contains(define_admin),
                "{name} defines its own `require_admin` — delegate to \
                 admin_guard::require_admin instead (ROADMAP X.7)"
            );
            assert!(
                !text.contains(define_auth),
                "{name} defines its own `require_auth` — delegate to \
                 admin_guard::require_auth instead (ROADMAP X.7)"
            );
        }
    }

    /// The eight modules X.7 named must no longer touch the header at all:
    /// their guard helper delegates, so the raw string is gone from the file.
    #[test]
    fn delegated_guard_modules_never_parse_the_header() {
        let modules = [
            "syncthing.rs",
            "shares.rs",
            "vpn.rs",
            "ups.rs",
            "permissions.rs",
            "snmp.rs",
            "user_homes.rs",
            "downloader.rs",
        ];
        let sources = handler_sources();
        for module in modules {
            let (_, text) = sources
                .iter()
                .find(|(name, _)| name == module)
                .unwrap_or_else(|| panic!("{module} not found under src/handlers"));
            assert!(
                !text.contains(SESSION_HEADER),
                "{module} still parses `{SESSION_HEADER}` itself — call \
                 admin_guard::require_admin instead (ROADMAP X.7)"
            );
            assert!(
                text.contains("admin_guard::require_admin"),
                "{module} must delegate its guard to admin_guard"
            );
        }
    }

    /// `security.rs` was the named surface of X.7: it parsed the header six
    /// times and rebuilt the same three branches. It must now go through the
    /// shared guard for every endpoint.
    #[test]
    fn security_module_never_parses_the_header_itself() {
        let text = std::fs::read_to_string(handlers_dir().join("security.rs"))
            .expect("read src/handlers/security.rs");

        assert!(
            !text.contains(SESSION_HEADER),
            "security.rs must not read `{SESSION_HEADER}` — use \
             admin_guard::require_auth / require_admin (ROADMAP X.7)"
        );
        assert!(
            text.contains("require_auth") && text.contains("require_admin"),
            "security.rs must guard its handlers with the shared primitive"
        );
    }

    /// The message a non-`Response` module relays is derived from the status,
    /// so it can never contradict the shared decision.
    #[test]
    fn denial_messages_match_their_status() {
        assert_eq!(
            denial_message(StatusCode::FORBIDDEN),
            "Admin privileges required for this operation"
        );
        assert_eq!(
            denial_message(StatusCode::UNAUTHORIZED),
            "Invalid or expired session"
        );
        assert_eq!(
            denial_message(StatusCode::BAD_REQUEST),
            "No session token provided"
        );
    }
}
