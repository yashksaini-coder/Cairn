use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Every failure leaves through here as structured JSON (KPI E5), and every
/// variant carries a stable machine-readable `code` so the client can branch
/// without string-matching prose.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    #[error("audio is {got} bytes; the cap is {cap}")]
    PayloadTooLarge { got: usize, cap: usize },
    #[error("upstream {service} failed: {detail}")]
    Upstream { service: &'static str, detail: String },
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::PayloadTooLarge { .. } => {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
            }
            ApiError::Upstream { .. } => (StatusCode::BAD_GATEWAY, "upstream_failed"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        // Internal errors are logged in full and reported as a bare code:
        // an anyhow chain can name a database path or an upstream URL, and
        // none of that belongs in a public response body.
        let message = match &self {
            ApiError::Internal(e) => {
                tracing::error!(error = ?e, "unhandled internal error");
                "something went wrong on our side".to_string()
            }
            other => other.to_string(),
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
