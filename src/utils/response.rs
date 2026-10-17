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
