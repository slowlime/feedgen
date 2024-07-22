use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, Clone)]
pub struct FeedCannotBeUpdated {
    pub name: String,
}

impl IntoResponse for FeedCannotBeUpdated {
    fn into_response(self) -> Response {
        let name = self.name;

        IntoResponse::into_response((
            StatusCode::FORBIDDEN,
            format!("Updates for the feed `{name}` were disabled in the config"),
        ))
    }
}
