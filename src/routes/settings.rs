use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Map, Value};

use crate::{
    core::auth::AuthUser,
    error::{AppError, AppResult},
    services::settings::{get_settings, update_settings},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_user_settings).patch(patch_user_settings))
}

async fn get_user_settings(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let data = get_settings(&state, user.id).await?;
    Ok(Json(json!({ "success": true, "data": data })))
}

async fn patch_user_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    if body.is_empty() {
        return Err(AppError::bad_request("Dữ liệu cập nhật không được để trống."));
    }
    let updated = update_settings(&state, user.id, &body).await?;
    Ok(Json(json!({
        "success": true,
        "message": "Cập nhật cài đặt thành công.",
        "data": updated,
    })))
}
