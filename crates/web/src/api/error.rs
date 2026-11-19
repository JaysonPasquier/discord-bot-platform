// Les erreurs de l'API JSON : un code stable (pour le frontend) et une phrase lisible.
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub(crate) fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Sign in to continue.",
        )
    }

    pub(crate) fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    pub(crate) fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "This does not exist.")
    }

    pub(crate) fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub(crate) fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub(crate) fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "The platform could not complete this action.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "code": self.code, "message": self.message } });
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    async fn read(error: ApiError) -> (StatusCode, serde_json::Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap_or_default();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    #[tokio::test]
    async fn an_error_is_a_status_with_a_stable_code_and_a_message() {
        let (status, body) = read(ApiError::bad_request("bad_name", "Too short.")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_name");
        assert_eq!(body["error"]["message"], "Too short.");
    }

    #[tokio::test]
    async fn the_common_errors_have_their_own_status() {
        assert_eq!(
            read(ApiError::unauthorized()).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(read(ApiError::not_found()).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            read(ApiError::forbidden("csrf", "x")).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            read(ApiError::internal()).await.0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
