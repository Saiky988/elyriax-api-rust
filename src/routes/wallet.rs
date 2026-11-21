use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    core::{auth::AuthUser, response::send_success},
    error::{AppError, AppResult},
    services::wallet::{
        create_deposit_info, get_or_create_wallet, get_transaction_by_id, get_transactions,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_wallet_ep))
        .route("/deposit", post(create_deposit_ep))
        .route("/deposit/{transaction_code}", get(get_deposit_status_ep))
        .route("/transactions", get(get_transactions_ep))
        .route("/transactions/{tx_id}", get(get_transaction_by_id_ep))
}

async fn get_wallet_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let wallet = get_or_create_wallet(&state, user.id).await?;
    Ok(send_success("Lấy thông tin ví thành công", wallet))
}

#[derive(Deserialize)]
struct DepositBody {
    amount: Option<f64>,
}

async fn create_deposit_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<DepositBody>,
) -> AppResult<Json<Value>> {
    let amount = body
        .amount
        .ok_or_else(|| AppError::bad_request("Số tiền nạp không hợp lệ"))?;
    let data = create_deposit_info(&state, user.id, amount).await?;
    Ok(send_success("Tạo yêu cầu nạp tiền thành công", data))
}

async fn get_deposit_status_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(transaction_code): Path<String>,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;
    let row = sqlx::query(
        "SELECT status FROM transactions WHERE user_id = ? AND transaction_code = ?",
    )
    .bind(user.id)
    .bind(&transaction_code)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Err(AppError::not_found("Không tìm thấy giao dịch"));
    };
    let status: String = row.try_get("status")?;
    Ok(send_success("Trạng thái giao dịch", json!({ "status": status })))
}

#[derive(Deserialize)]
struct TxListQuery {
    page: Option<i64>,
    limit: Option<i64>,
    #[serde(rename = "type")]
    tx_type: Option<String>,
    status: Option<String>,
    sort: Option<String>,
}

async fn get_transactions_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<TxListQuery>,
) -> AppResult<Json<Value>> {
    let data = get_transactions(
        &state,
        user.id,
        q.page.unwrap_or(1),
        q.limit.unwrap_or(10),
        q.tx_type.as_deref(),
        q.status.as_deref(),
        q.sort.as_deref(),
    )
    .await?;
    Ok(send_success("Lấy lịch sử giao dịch thành công", data))
}

async fn get_transaction_by_id_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(tx_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let data = get_transaction_by_id(&state, user.id, tx_id).await?;
    Ok(send_success("Chi tiết giao dịch", data))
}
