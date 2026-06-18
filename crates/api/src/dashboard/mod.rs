use axum::{response::Html, extract::State, routing::get, Router};

use crate::SharedAppState;

/// Serve the embedded dashboard HTML at GET /
pub fn routes() -> Router<SharedAppState> {
    Router::new().route("/", get(dashboard_handler))
}

async fn dashboard_handler(_state: State<SharedAppState>) -> Html<&'static str> {
    Html(include_str!("index.html"))
}
