//! HTTP error envelope: maps `DapError` to typed JSON responses and status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use dap_core::DapError;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub struct ApiErrorResponse {
    pub status: StatusCode,
    pub body: ApiError,
}

impl IntoResponse for ApiErrorResponse {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<DapError> for ApiErrorResponse {
    fn from(e: DapError) -> Self {
        let (status, code) = match &e {
            DapError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            DapError::PermissionDenied(_) => (StatusCode::FORBIDDEN, "permission_denied"),
            DapError::InvalidValue(_)
            | DapError::InvalidUid(_)
            | DapError::InvalidPeriod { .. }
            | DapError::SchemaMismatch(_)
            | DapError::TypeError { .. } => (StatusCode::BAD_REQUEST, "invalid_request"),
            DapError::BudgetExceeded(_) => (StatusCode::PAYLOAD_TOO_LARGE, "budget_exceeded"),
            DapError::Cancelled => (StatusCode::GONE, "cancelled"),
            DapError::UnsupportedFeature(_) => (StatusCode::NOT_IMPLEMENTED, "unsupported"),
            DapError::SegmentCorruption(_)
            | DapError::Io(_)
            | DapError::Serialization(_)
            | DapError::Invariant(_)
            | DapError::AlreadyExists(_)
            | DapError::Other(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self {
            status,
            body: ApiError::new(code, e.to_string()),
        }
    }
}
