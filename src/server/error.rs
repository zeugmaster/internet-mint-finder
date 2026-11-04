use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Unreachable(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (code, message) = match &self {
            Self::InvalidInput(message) => (StatusCode::BAD_REQUEST, message.clone()),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message.clone()),
            Self::Unreachable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Service temporarily unavailable".into(),
            ),
            _ => {
                tracing::error!(error = %self, "Request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not complete this request".into(),
                )
            }
        };
        (code, Json(serde_json::json!({"error": message}))).into_response()
    }
}
