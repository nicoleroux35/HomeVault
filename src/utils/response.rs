/* =============================================================================
 * File          : response.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Builds the uniform JSON answer shape used by every API endpoint:
 *   a success flag, an optional data payload and an optional message.
 *   ResponseHelper offers one constructor per common HTTP status so all
 *   handlers stay consistent, and JsonResponse converts itself into an
 *   axum response (error answers default to 400 Bad Request).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Inside a handler: `ResponseHelper::json_success(data)` (or the status
 *   / error variants); `JsonResponse` implements `IntoResponse`.
 * =============================================================================
 */

//! The uniform JSON envelope every API endpoint answers with:
//! `{ "success": bool, "data"?: …, "message"?: String }` plus ready-made
//! constructors for the common status codes (200/400/401/403/500 and the
//! 2FA-required shape), so handlers never hand-roll response JSON.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json as AxumJson,
};
use serde::Serialize;

#[derive(Serialize)]
pub struct JsonResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T: Serialize> JsonResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            message: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            message: Some(message),
        }
    }
}

impl<T: Serialize> IntoResponse for JsonResponse<T> {
    fn into_response(self) -> Response {
        let status = if self.success {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        };
        (status, AxumJson(self)).into_response()
    }
}

pub struct ResponseHelper;

impl ResponseHelper {
    pub fn json<T: Serialize>(status: StatusCode, data: T) -> Response {
        (status, AxumJson(data)).into_response()
    }

    pub fn json_success<T: Serialize>(data: T) -> Response {
        (StatusCode::OK, AxumJson(JsonResponse::success(data))).into_response()
    }

    /// `202 Accepted` for work that continues after the response — the caller
    /// gets the handle it can poll instead of waiting for the result.
    pub fn json_accepted<T: Serialize>(data: T) -> Response {
        (StatusCode::ACCEPTED, AxumJson(JsonResponse::success(data))).into_response()
    }

    pub fn json_error(message: String) -> Response {
        (
            StatusCode::BAD_REQUEST,
            AxumJson(JsonResponse::<()>::error(message)),
        )
            .into_response()
    }

    pub fn json_unauthorized(message: String) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            AxumJson(JsonResponse::<()>::error(message)),
        )
            .into_response()
    }

    pub fn json_forbidden(message: String) -> Response {
        (
            StatusCode::FORBIDDEN,
            AxumJson(JsonResponse::<()>::error(message)),
        )
            .into_response()
    }

    /// A structured error carrying an explicit status (429, 409, …).
    pub fn json_error_with_status(status: StatusCode, message: String) -> Response {
        (status, AxumJson(JsonResponse::<()>::error(message))).into_response()
    }

    /// **The answer for a capability this host cannot provide** (P0.2).
    ///
    /// A missing `clamscan`, `docker`, `ufw`, `restic`, `dnsmasq`, `upsd`, hwmon
    /// channel or kernel module is not a client error: the request was well
    /// formed, the server simply cannot carry it out. It used to be answered
    /// `400`, which made "fix your input" indistinguishable from "install a
    /// package" for every caller.
    ///
    /// `503` — the server is temporarily unable to handle the request — and the
    /// message must **name the missing piece**, so the screen can tell the user
    /// what to install rather than showing a generic failure.
    ///
    /// See `docs/API.md` → "Capability and unsupported-capability contract".
    pub fn json_unavailable(missing: impl std::fmt::Display) -> Response {
        Self::json_error_with_status(StatusCode::SERVICE_UNAVAILABLE, missing.to_string())
    }

    /// **The answer for a route this build cannot perform at all** (P0.2).
    ///
    /// Distinct from [`Self::json_unavailable`]: `501` means no installation
    /// will help, the feature is not implemented, and the message must say what
    /// to do instead. An unauthenticated caller must never reach this — the
    /// guard runs first.
    pub fn json_not_implemented(message: String) -> Response {
        Self::json_error_with_status(StatusCode::NOT_IMPLEMENTED, message)
    }

    pub fn json_internal_error(message: String) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            AxumJson(JsonResponse::<()>::error(message)),
        )
            .into_response()
    }

    pub fn json_2fa_required() -> Response {
        let response = serde_json::json!({
            "success": false,
            "data": {
                "requires_2fa": true
            },
            "message": "Two-factor authentication required"
        });
        (StatusCode::UNAUTHORIZED, AxumJson(response)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;

    async fn status_and_body(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// A missing tool is `503`, never `400` (P0.2). `400` is reserved for a
    /// request the server could not understand, so answering it for "this host
    /// has no `clamscan`" tells the caller to fix input that is already fine.
    #[tokio::test]
    async fn json_unavailable_answers_503_and_names_the_missing_piece() {
        let (status, body) =
            status_and_body(ResponseHelper::json_unavailable("ufw is not installed")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["success"], serde_json::json!(false));
        assert_eq!(body["message"], serde_json::json!("ufw is not installed"));
    }

    /// `Display`, not `&str`: a formatted sentence must work the same way.
    #[tokio::test]
    async fn json_unavailable_accepts_a_formatted_sentence() {
        let (status, body) = status_and_body(ResponseHelper::json_unavailable(format!(
            "'{}' is not installed on this server",
            "7z"
        )))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body["message"],
            serde_json::json!("'7z' is not installed on this server")
        );
    }

    /// An unimplemented feature is `501`, distinct from `503`: no installation
    /// fixes it, so the two can never be folded together.
    #[tokio::test]
    async fn json_not_implemented_answers_501() {
        let (status, body) = status_and_body(ResponseHelper::json_not_implemented(
            "not built yet".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["message"], serde_json::json!("not built yet"));
    }

    /// The three capability statuses must stay three.
    #[test]
    fn capability_statuses_are_distinct() {
        assert_ne!(StatusCode::SERVICE_UNAVAILABLE, StatusCode::NOT_IMPLEMENTED);
        assert_ne!(StatusCode::SERVICE_UNAVAILABLE, StatusCode::BAD_REQUEST);
        assert_ne!(StatusCode::NOT_IMPLEMENTED, StatusCode::BAD_REQUEST);
    }
}
