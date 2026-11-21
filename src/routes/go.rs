use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    core::auth::GoAdminAuth,
    error::{AppError, AppResult},
    services::go::{create_go_link, get_go_metadata, resolve_redirect},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/go/create", post(create_link_ep))
        .route("/v1/go/{link_id}", get(get_link_metadata_ep))
        .route("/n/{device}/{link_id}", get(redirect_ep))
}

#[derive(Deserialize)]
struct CreateGoBody {
    url: Option<String>,
    device: Option<String>,
    #[serde(rename = "expiresIn")]
    expires_in: Option<i64>,
}

async fn create_link_ep(
    State(state): State<AppState>,
    _admin: GoAdminAuth,
    Json(body): Json<CreateGoBody>,
) -> AppResult<impl IntoResponse> {
    let url = body
        .url
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("url and device are required."))?;
    let device = body
        .device
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("url and device are required."))?;

    let result = create_go_link(&state, &url, &device, body.expires_in.unwrap_or(3600)).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn get_link_metadata_ep(
    State(state): State<AppState>,
    _admin: GoAdminAuth,
    Path(link_id): Path<String>,
) -> Response {
    match get_go_metadata(&state, &link_id).await {
        Some(item) => Json(item).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "error": "Link not found" })),
        )
            .into_response(),
    }
}

async fn redirect_ep(
    State(state): State<AppState>,
    Path((device, link_id)): Path<(String, String)>,
) -> Response {
    match resolve_redirect(&state, &device, &link_id).await {
        Some(dest) => Redirect::temporary(&dest).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "LINK_EXPIRED_OR_INVALID",
                "message": "This link is no longer available or has expired.",
            })),
        )
            .into_response(),
    }
}
