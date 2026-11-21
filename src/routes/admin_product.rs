use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    core::{
        auth::AdminUser,
        response::{send_success, send_success_empty},
    },
    error::{AppError, AppResult},
    services::product::{
        create_product, delete_product, delete_product_item, get_admin_product_items,
        import_product_items, update_product, update_product_item,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/products", post(create_product_ep))
        .route("/products/{prod_id}", patch(update_product_ep).delete(delete_product_ep))
        .route("/products/{prod_id}/items/import", post(import_items_ep))
        .route("/products/{prod_id}/items", get(get_items_ep))
        .route(
            "/product-items/{item_id}",
            patch(update_product_item_ep).delete(delete_product_item_ep),
        )
}

async fn create_product_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Json(body): Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    let id = create_product(&state, &body).await?;
    Ok(send_success("Tạo sản phẩm thành công", json!({ "id": id })))
}

async fn update_product_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(prod_id): Path<i64>,
    Json(body): Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    update_product(&state, prod_id, &body).await?;
    Ok(send_success_empty("Cập nhật sản phẩm thành công"))
}

async fn delete_product_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(prod_id): Path<i64>,
) -> AppResult<Json<Value>> {
    delete_product(&state, prod_id).await?;
    Ok(send_success_empty("Vô hiệu hóa sản phẩm thành công"))
}

#[derive(Deserialize)]
struct ImportItemsBody {
    content: Option<Vec<String>>,
}

async fn import_items_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(prod_id): Path<i64>,
    Json(body): Json<ImportItemsBody>,
) -> AppResult<Json<Value>> {
    let content = body
        .content
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::bad_request("Dữ liệu import không hợp lệ"))?;
    let result = import_product_items(&state, prod_id, content).await?;
    Ok(send_success("Import kho hàng thành công", result))
}

#[derive(Deserialize)]
struct AdminItemsQuery {
    page: Option<i64>,
    limit: Option<i64>,
    status: Option<String>,
    search: Option<String>,
}

async fn get_items_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(prod_id): Path<i64>,
    Query(q): Query<AdminItemsQuery>,
) -> AppResult<Json<Value>> {
    let data = get_admin_product_items(
        &state,
        prod_id,
        q.page.unwrap_or(1),
        q.limit.unwrap_or(20),
        q.status.as_deref(),
        q.search.as_deref(),
    )
    .await?;
    Ok(send_success("Lấy danh sách kho hàng thành công", data))
}

#[derive(Deserialize)]
struct UpdateItemBody {
    content: Option<String>,
}

async fn update_product_item_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(item_id): Path<i64>,
    Json(body): Json<UpdateItemBody>,
) -> AppResult<Json<Value>> {
    let content = body
        .content
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu nội dung cập nhật"))?;
    update_product_item(&state, item_id, &content).await?;
    Ok(send_success_empty("Cập nhật tài nguyên thành công"))
}

async fn delete_product_item_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(item_id): Path<i64>,
) -> AppResult<Json<Value>> {
    delete_product_item(&state, item_id).await?;
    Ok(send_success_empty(
        "Xóa tài nguyên thành công (Đã cập nhật Stock)",
    ))
}
