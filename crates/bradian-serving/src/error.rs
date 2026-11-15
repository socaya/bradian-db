//! HTTP error envelope: maps `BradianError` to typed JSON responses and status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bradian_core::BradianError;
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

impl From<BradianError> for ApiErrorResponse {
    fn from(e: BradianError) -> Self {
        let (status, code) = match &e {
            BradianError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            BradianError::PermissionDenied(_) => (StatusCode::FORBIDDEN, "permission_denied"),
            BradianError::InvalidValue(_)
            | BradianError::InvalidUid(_)
            | BradianError::InvalidPeriod { .. }
            | BradianError::SchemaMismatch(_)
            | BradianError::TypeError { .. } => (StatusCode::BAD_REQUEST, "invalid_request"),
            BradianError::BudgetExceeded(_) => (StatusCode::PAYLOAD_TOO_LARGE, "budget_exceeded"),
            BradianError::Cancelled => (StatusCode::GONE, "cancelled"),
            BradianError::UnsupportedFeature(_) => (StatusCode::NOT_IMPLEMENTED, "unsupported"),
            BradianError::SegmentCorruption(_)
            | BradianError::Io(_)
            | BradianError::Serialization(_)
            | BradianError::Invariant(_)
            | BradianError::AlreadyExists(_)
            | BradianError::Other(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self {
            status,
            body: ApiError::new(code, e.to_string()),
        }
    }
}
