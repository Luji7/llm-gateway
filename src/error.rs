use axum::{
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::models::{AnthropicErrorBody, AnthropicErrorResponse};
use crate::translate::TranslateError;

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub error_type: String,
    pub message: String,
}

impl AppError {
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error_type: "invalid_request_error".to_string(),
            message: message.into(),
        }
    }

    pub fn api_error(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            error_type: "api_error".to_string(),
            message: message.into(),
        }
    }

    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: "rate_limit_error".to_string(),
            message: message.into(),
        }
    }

    pub fn from_translate(err: TranslateError) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error_type: err.error_type,
            message: err.message,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let body = AnthropicErrorResponse {
            response_type: "error".to_string(),
            error: AnthropicErrorBody {
                error_type: self.error_type,
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

pub fn map_downstream_error(status: StatusCode, body: &str) -> AppError {
    let mapped = match status.as_u16() {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        500 => "api_error",
        502 | 503 | 504 => "overloaded_error",
        _ => "api_error",
    };

    let message = if body.is_empty() {
        format!("downstream error: {}", status)
    } else {
        format!("downstream error: {}", body)
    };

    AppError {
        status: StatusCode::BAD_GATEWAY,
        error_type: mapped.to_string(),
        message,
    }
}
