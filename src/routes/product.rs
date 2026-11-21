use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    core::response::send_success,
    error::AppResult,
    services::product::{get_product_by_code, get_public_products},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_public_products_ep))
        .route("/{product_code}", get(get_product_detail_ep))
}

#[derive(Deserialize)]
struct ProductListQuery {
    page: Option<i64>,
    limit: Option<i64>,
    search: Option<String>,
    category: Option<String>,
    sort: Option<String>,
}

async fn get_public_products_ep(
    State(state): State<AppState>,
    Query(q): Query<ProductListQuery>,
) -> AppResult<Json<Value>> {
    let data = get_public_products(
        &state,
        q.page.unwrap_or(1),
        q.limit.unwrap_or(12),
        q.search.as_deref(),
        q.category.as_deref(),
        q.sort.as_deref(),
    )
    .await?;
    Ok(send_success("Lấy danh sách sản phẩm thành công", data))
}

async fn get_product_detail_ep(
    State(state): State<AppState>,
    Path(product_code): Path<String>,
) -> AppResult<Json<Value>> {
    let data = get_product_by_code(&state, &product_code).await?;
    Ok(send_success("Lấy chi tiết sản phẩm thành công", data))
}
