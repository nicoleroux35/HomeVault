/* =============================================================================
 * File          : contract.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Middleware that keeps the browser client and the server on the same API
 *   contract version. On every /api request it compares the
 *   X-HomeVault-Contract header sent by the client with the server's
 *   contract version and answers 426 Upgrade Required on mismatch, so a
 *   stale cached frontend fails loudly instead of misparsing responses. A
 *   missing header is allowed (older tools keep working) and /api/meta is
 *   exempt because it is the endpoint used to discover the mismatch.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Registered as a layer on the /api routes in src/web/mod.rs; the frontend
 *   sends the header from its API client (frontend/src/lib/services/api.ts).
 * =============================================================================
 */

//! Client/server contract enforcement middleware.
//!
//! Scoped to `/api/*` (see `web::run`), this rejects a request whose
//! `X-HomeVault-Contract` header is present but does not match
//! [`CONTRACT_VERSION`](crate::api_contract::CONTRACT_VERSION) with a
//! **426 Upgrade Required**. A missing header is allowed so older clients and
//! third-party tools that predate the header keep working — only an explicit
//! mismatch (a stale cached SPA bundle talking to a newer server, or the
//! reverse) fails loudly instead of silently misparsing.
//!
//! `GET /api/meta` is exempt: it is the endpoint the client uses to *discover*
//! the mismatch, so it must never be blocked by it.

use crate::api_contract::{client_is_compatible, MetaInfo};
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

/// Header carrying the client's compiled contract version.
pub const CONTRACT_HEADER: &str = "x-homevault-contract";

pub async fn contract_middleware(req: Request, next: Next) -> Response {
    // `/api/meta` is the handshake itself — never gate it.
    if req.uri().path() == "/api/meta" {
        return next.run(req).await;
    }

    let client = req
        .headers()
        .get(CONTRACT_HEADER)
        .and_then(|v| v.to_str().ok());

    if !client_is_compatible(client) {
        let meta = MetaInfo::current();
        return (
            StatusCode::UPGRADE_REQUIRED,
            Json(serde_json::json!({
                "success": false,
                "error": "Client/server contract mismatch — reload the app to pick up the current version.",
                "data": meta,
            })),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_contract::CONTRACT_VERSION;

    #[test]
    fn test_contract_header_name_is_lowercase_for_comparison() {
        // `HeaderName` lookup is case-insensitive, but keeping the constant
        // lowercase documents the wire form the client actually sends.
        assert_eq!(CONTRACT_HEADER, "x-homevault-contract");
    }

    #[test]
    fn test_meta_info_shape_matches_frontend_expectation() {
        let meta = MetaInfo::current();
        assert_eq!(meta.contract_version, CONTRACT_VERSION);
    }
}
