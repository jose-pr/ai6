//! A refusal as the app shows it: `{"error": TEXT}` with its status.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::ApiError;

/// What a route answers when it does not do what was asked.
#[derive(Debug)]
pub(crate) struct Refused {
    pub(crate) status: StatusCode,
    pub(crate) body: Value,
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<ApiError> for Refused {
    fn from(error: ApiError) -> Self {
        refused(error.status, error.message)
    }
}

pub(crate) fn refused(status: StatusCode, error: impl Into<String>) -> Refused {
    Refused {
        status,
        body: json!({"error": error.into()}),
    }
}
