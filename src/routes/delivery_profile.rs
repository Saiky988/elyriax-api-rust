use axum::{
    extract::{Path, Query, State},
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    core::{
        auth::AuthUser,
        response::{send_success, send_success_empty},
    },
    error::AppResult,
    services::delivery_profile::{
        create_delivery_profile, delete_delivery_profile, list_delivery_profiles,
        update_delivery_profile, DeliveryProfileInput,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/delivery-profiles",
            get(list_profiles_ep).post(create_profile_ep),
        )
        .route(
            "/delivery-profiles/{profile_id}",
            put(update_profile_ep).delete(delete_profile_ep),
        )
}

#[derive(Deserialize)]
struct ListProfilesQuery {
    reveal: Option<bool>,
}

async fn list_profiles_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListProfilesQuery>,
) -> AppResult<Json<Value>> {
    let items = list_delivery_profiles(&state, user.id, q.reveal.unwrap_or(false)).await?;
    Ok(send_success(
        "Lấy danh sách thông tin nhận hàng thành công",
        json!({ "items": items }),
    ))
}

async fn create_profile_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<DeliveryProfileInput>,
) -> AppResult<Json<Value>> {
    let created = create_delivery_profile(&state, user.id, body).await?;
    Ok(send_success("Đã lưu thông tin nhận hàng", created))
}

async fn update_profile_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(profile_id): Path<i64>,
    Json(body): Json<DeliveryProfileInput>,
) -> AppResult<Json<Value>> {
    let updated = update_delivery_profile(&state, user.id, profile_id, body).await?;
    Ok(send_success("Đã cập nhật thông tin nhận hàng", updated))
}

async fn delete_profile_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(profile_id): Path<i64>,
) -> AppResult<Json<Value>> {
    delete_delivery_profile(&state, user.id, profile_id).await?;
    Ok(send_success_empty("Đã xóa thông tin nhận hàng"))
}
