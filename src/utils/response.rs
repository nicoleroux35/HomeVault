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
