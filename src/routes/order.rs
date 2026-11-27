use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    core::{auth::AuthUser, response::send_success},
    error::AppResult,
    services::order::{cancel_order, checkout, get_order_by_id, get_orders, CheckoutRequest},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(checkout_ep).get(get_orders_ep))
        .route("/checkout", post(checkout_ep))
        .route("/{order_id}", get(get_order_by_id_ep))
        .route("/{order_id}/cancel", post(cancel_order_ep))
}

async fn checkout_ep(
    State(state): State<AppState>,
    user: AuthUser,
    body: Option<Json<CheckoutRequest>>,
) -> AppResult<Json<Value>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let data = checkout(&state, user.id, req).await?;
    Ok(send_success("Thanh toán đơn hàng thành công", data))
}

#[derive(Deserialize)]
struct OrderListQuery {
    page: Option<i64>,
    limit: Option<i64>,
    status: Option<String>,
}

async fn get_orders_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<OrderListQuery>,
) -> AppResult<Json<Value>> {
    let data = get_orders(
        &state,
        user.id,
        q.page.unwrap_or(1),
        q.limit.unwrap_or(20),
        q.status.as_deref(),
    )
    .await?;
    Ok(send_success("Lấy danh sách đơn hàng thành công", data))
}

async fn get_order_by_id_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(order_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let data = get_order_by_id(&state, user.id, order_id).await?;
    Ok(send_success("Lấy chi tiết đơn hàng thành công", data))
}

async fn cancel_order_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(order_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let data = cancel_order(&state, user.id, order_id).await?;
    Ok(send_success("Hủy đơn hàng thành công", data))
}
