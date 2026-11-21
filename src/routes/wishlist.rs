use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    core::{auth::AuthUser, response::send_success},
    error::{AppError, AppResult},
    services::wishlist::{add_to_wishlist, get_wishlist, remove_from_wishlist},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_wishlist_ep))
        .route("/items", post(add_wishlist_item_ep))
        .route("/items/{product_code}", delete(remove_wishlist_item_ep))
}

async fn get_wishlist_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let data = get_wishlist(&state, user.id).await?;
    Ok(send_success("Lấy danh sách yêu thích thành công", data))
}

#[derive(Deserialize)]
struct WishlistBody {
    product_code: Option<String>,
}

async fn add_wishlist_item_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<WishlistBody>,
) -> AppResult<Json<Value>> {
    let code = body
        .product_code
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu product_code"))?;
    let data = add_to_wishlist(&state, user.id, &code).await?;
    Ok(send_success("Đã thêm vào danh sách yêu thích", data))
}

async fn remove_wishlist_item_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(product_code): Path<String>,
) -> AppResult<Json<Value>> {
    let data = remove_from_wishlist(&state, user.id, &product_code).await?;
    Ok(send_success("Đã xóa khỏi danh sách yêu thích", data))
}
