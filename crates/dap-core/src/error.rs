//! Unified error type. One `DapError` per crate boundary; wrapping is explicit.

use std::io;
use thiserror::Error;

pub type DapResult<T> = Result<T, DapError>;

#[derive(Debug, Error)]
pub enum DapError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("invalid value: {0}")]
    InvalidValue(String),

    #[error("invalid UID: {0}")]
    InvalidUid(String),

    #[error("invalid period '{raw}': {reason}")]
    InvalidPeriod { raw: String, reason: String },

    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    #[error("type error: expected {expected:?}, got {found:?}")]
    TypeError { expected: String, found: String },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("segment corruption: {0}")]
    SegmentCorruption(String),

    #[error("unsupported feature: {0}")]
    UnsupportedFeature(String),

    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),

    #[error("cancelled")]
    Cancelled,

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("invariant violated: {0}")]
    Invariant(String),

    #[error("serialization: {0}")]
    Serialization(String),

    #[error("{0}")]
    Other(String),
}

impl DapError {
    pub fn invalid_value(msg: impl Into<String>) -> Self {
        Self::InvalidValue(msg.into())
    }
    pub fn schema_mismatch(msg: impl Into<String>) -> Self {
        Self::SchemaMismatch(msg.into())
    }
    pub fn corruption(msg: impl Into<String>) -> Self {
        Self::SegmentCorruption(msg.into())
    }
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::UnsupportedFeature(msg.into())
    }
    pub fn invariant(msg: impl Into<String>) -> Self {
        Self::Invariant(msg.into())
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

impl From<serde_json::Error> for DapError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}
