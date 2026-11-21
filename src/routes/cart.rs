use axum::{
    extract::{Path, State},
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    core::{auth::AuthUser, response::send_success},
    error::{AppError, AppResult},
    services::cart::{add_to_cart, clear_cart, get_cart, remove_cart_item, update_cart_item},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_cart_ep).delete(clear_cart_ep))
        .route("/items", post(add_item_ep))
        .route("/items/{item_id}", patch(update_item_ep).delete(remove_item_ep))
}

async fn get_cart_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let cart = get_cart(&state, user.id).await?;
    Ok(send_success("Lấy giỏ hàng thành công", cart))
}

#[derive(Deserialize)]
struct AddCartBody {
    product_code: Option<String>,
    quantity: Option<i64>,
}

async fn add_item_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<AddCartBody>,
) -> AppResult<Json<Value>> {
    let product_code = body
        .product_code
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu product_code"))?;
    let cart = add_to_cart(&state, user.id, &product_code, body.quantity.unwrap_or(1)).await?;
    Ok(send_success("Thêm vào giỏ hàng thành công", cart))
}

#[derive(Deserialize)]
struct UpdateCartBody {
    quantity: Option<i64>,
}

async fn update_item_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(item_id): Path<i64>,
    Json(body): Json<UpdateCartBody>,
) -> AppResult<Json<Value>> {
    let qty = body
        .quantity
        .ok_or_else(|| AppError::bad_request("Thiếu số lượng"))?;
    let cart = update_cart_item(&state, user.id, item_id, qty).await?;
    Ok(send_success("Cập nhật số lượng thành công", cart))
}

async fn remove_item_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(item_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let cart = remove_cart_item(&state, user.id, item_id).await?;
    Ok(send_success("Xóa sản phẩm khỏi giỏ thành công", cart))
}

async fn clear_cart_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let cart = clear_cart(&state, user.id).await?;
    Ok(send_success("Xóa toàn bộ giỏ hàng thành công", cart))
}
