use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::{
    core::{
        auth::AdminUser,
        response::{send_success, send_success_empty},
        security::generate_transaction_code,
    },
    error::{AppError, AppResult},
    services::{
        product::{
            create_product, delete_product, delete_product_item, get_admin_product_items,
            get_admin_products, import_product_items, update_product, update_product_item,
        },
        provider::{get_provider_catalog_filtered, list_providers},
        scheduler::{get_scheduler_status, run_provider_price_sync},
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        // Overview
        .route("/overview", get(admin_overview_ep))
        // Market / Products & Variants & Inventory
        .route("/products", get(list_admin_products_ep).post(create_product_ep))
        .route(
            "/products/{prod_id}",
            patch(update_product_ep).delete(delete_product_ep),
        )
        .route("/products/{prod_id}/items/import", post(import_items_ep))
        .route("/products/{prod_id}/items", get(get_items_ep))
        .route(
            "/product-items/{item_id}",
            patch(update_product_item_ep).delete(delete_product_item_ep),
        )
        // Users & Wallets
        .route("/users", get(list_admin_users_ep))
        .route("/users/{user_id}", patch(update_admin_user_ep))
        .route("/users/{user_id}/balance", post(adjust_user_balance_ep))
        // Payments / Transactions
        .route("/transactions", get(list_admin_transactions_ep))
        .route("/transactions/{tx_id}/approve", post(approve_deposit_ep))
        .route("/transactions/{tx_id}/cancel", post(cancel_deposit_ep))
        // Orders
        .route("/orders", get(list_admin_orders_ep))
        .route("/orders/{order_id}", patch(update_admin_order_ep))
        // Providers (AppMMO price-sync)
        .route("/providers", get(list_providers_ep))
        .route("/providers/{provider}/catalog", get(provider_catalog_ep))
        .route("/providers/sync", post(sync_providers_ep))
}

/* ========================= OVERVIEW ========================= */

async fn admin_overview_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;

    let users_row = sqlx::query("SELECT COUNT(id) AS total FROM users")
        .fetch_one(pool)
        .await?;
    let products_row = sqlx::query(
        "SELECT COUNT(id) AS total, SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END) AS active FROM products",
    )
    .fetch_one(pool)
    .await?;
    let orders_row = sqlx::query(
        "SELECT COUNT(id) AS total, CAST(COALESCE(SUM(CASE WHEN status = 'completed' THEN total ELSE 0 END), 0) AS DOUBLE) AS revenue FROM orders",
    )
    .fetch_one(pool)
    .await?;
    let wallet_row = sqlx::query(
        "SELECT CAST(COALESCE(SUM(balance), 0) AS DOUBLE) AS total_balance, CAST(COALESCE(SUM(total_deposit), 0) AS DOUBLE) AS total_deposit FROM wallets",
    )
    .fetch_one(pool)
    .await?;
    let pending_tx_row = sqlx::query(
        "SELECT COUNT(id) AS total FROM transactions WHERE type = 'deposit' AND status = 'pending'",
    )
    .fetch_one(pool)
    .await?;

    let scheduler = get_scheduler_status(&state).await;

    Ok(send_success(
        "Lấy tổng quan quản trị thành công",
        json!({
            "users_total": users_row.try_get::<i64, _>("total").unwrap_or(0),
            "products_total": products_row.try_get::<i64, _>("total").unwrap_or(0),
            "products_active": products_row.try_get::<Option<i64>, _>("active").ok().flatten().unwrap_or(0),
            "orders_total": orders_row.try_get::<i64, _>("total").unwrap_or(0),
            "orders_revenue": orders_row.try_get::<f64, _>("revenue").unwrap_or(0.0),
            "wallets_total_balance": wallet_row.try_get::<f64, _>("total_balance").unwrap_or(0.0),
            "wallets_total_deposit": wallet_row.try_get::<f64, _>("total_deposit").unwrap_or(0.0),
            "pending_deposits": pending_tx_row.try_get::<i64, _>("total").unwrap_or(0),
            "scheduler": scheduler,
        }),
    ))
}

/* ========================= PRODUCTS ========================= */

#[derive(Deserialize)]
struct AdminProductsQuery {
    page: Option<i64>,
    limit: Option<i64>,
    search: Option<String>,
    category: Option<String>,
    status: Option<String>,
}

async fn list_admin_products_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<AdminProductsQuery>,
) -> AppResult<Json<Value>> {
    let data = get_admin_products(
        &state,
        q.page.unwrap_or(1),
        q.limit.unwrap_or(100),
        q.search.as_deref(),
        q.category.as_deref(),
        q.status.as_deref(),
    )
    .await?;
    Ok(send_success("Lấy danh sách sản phẩm quản trị thành công", data))
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

/* ========================= USERS & WALLETS ========================= */

#[derive(Deserialize)]
struct AdminUsersQuery {
    page: Option<i64>,
    limit: Option<i64>,
    search: Option<String>,
    status: Option<String>,
}

async fn list_admin_users_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<AdminUsersQuery>,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    let mut sql = String::from(
        r#"
        SELECT
            u.id, u.email, u.username, u.avatar, u.status, u.created_at,
            CAST(COALESCE(w.balance, 0) AS DOUBLE) AS balance,
            CAST(COALESCE(w.total_deposit, 0) AS DOUBLE) AS total_deposit,
            CAST(COALESCE(w.total_spent, 0) AS DOUBLE) AS total_spent,
            (SELECT COUNT(o.id) FROM orders o WHERE o.user_id = u.id) AS orders_count
        FROM users u
        LEFT JOIN wallets w ON w.user_id = u.id
        WHERE 1=1
        "#,
    );
    let mut count_sql = String::from("SELECT COUNT(id) AS total FROM users u WHERE 1=1");

    if q.status.is_some() {
        sql.push_str(" AND u.status = ?");
        count_sql.push_str(" AND u.status = ?");
    }
    if q.search.is_some() {
        sql.push_str(" AND (u.email LIKE ? OR u.username LIKE ?)");
        count_sql.push_str(" AND (u.email LIKE ? OR u.username LIKE ?)");
    }
    sql.push_str(" ORDER BY u.id DESC LIMIT ? OFFSET ?");

    let mut query = sqlx::query(&sql);
    let mut cquery = sqlx::query(&count_sql);

    if let Some(ref st) = q.status {
        query = query.bind(st);
        cquery = cquery.bind(st);
    }
    let pat = q.search.as_ref().map(|s| format!("%{}%", s));
    if let Some(ref p) = pat {
        query = query.bind(p).bind(p);
        cquery = cquery.bind(p).bind(p);
    }

    let rows = query.bind(limit).bind(offset).fetch_all(pool).await?;
    let total: i64 = cquery.fetch_one(pool).await?.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let email: Option<String> = r.try_get("email").ok().flatten();
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            let is_adm = state.config.is_admin_email(email.as_deref());
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "email": email,
                "username": r.try_get::<Option<String>, _>("username").ok().flatten(),
                "avatar": r.try_get::<Option<String>, _>("avatar").ok().flatten(),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "is_admin": is_adm,
                "balance": r.try_get::<f64, _>("balance").unwrap_or(0.0),
                "total_deposit": r.try_get::<f64, _>("total_deposit").unwrap_or(0.0),
                "total_spent": r.try_get::<f64, _>("total_spent").unwrap_or(0.0),
                "orders_count": r.try_get::<i64, _>("orders_count").unwrap_or(0),
                "created_at": created_at,
            })
        })
        .collect();

    let total_pages = ((total as f64) / (limit as f64)).ceil().max(1.0) as i64;
    Ok(send_success(
        "Lấy danh sách người dùng thành công",
        json!({
            "items": items,
            "pagination": { "total": total, "page": page, "limit": limit, "total_pages": total_pages }
        }),
    ))
}

#[derive(Deserialize)]
struct UpdateUserBody {
    status: Option<String>,
    username: Option<String>,
}

async fn update_admin_user_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(user_id): Path<i64>,
    Json(body): Json<UpdateUserBody>,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;
    let existing = sqlx::query("SELECT username, status FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = existing else {
        return Err(AppError::not_found("Người dùng không tồn tại"));
    };

    let status = if let Some(ref st) = body.status {
        if !matches!(st.as_str(), "active" | "disabled" | "banned") {
            return Err(AppError::bad_request("Trạng thái không hợp lệ (active | disabled | banned)"));
        }
        st.clone()
    } else {
        row.try_get::<String, _>("status").unwrap_or_else(|_| "active".to_string())
    };

    let username = body
        .username
        .filter(|s| !s.trim().is_empty())
        .or_else(|| row.try_get::<Option<String>, _>("username").ok().flatten());

    sqlx::query("UPDATE users SET username = ?, status = ? WHERE id = ?")
        .bind(&username)
        .bind(&status)
        .bind(user_id)
        .execute(pool)
        .await?;

    Ok(send_success_empty("Cập nhật người dùng thành công"))
}

#[derive(Deserialize)]
struct AdjustBalanceBody {
    amount: f64,
    description: Option<String>,
}

async fn adjust_user_balance_ep(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
    Json(body): Json<AdjustBalanceBody>,
) -> AppResult<Json<Value>> {
    if !body.amount.is_finite() || body.amount == 0.0 {
        return Err(AppError::bad_request("Số tiền điều chỉnh phải khác 0"));
    }

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let u = sqlx::query("SELECT id FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    if u.is_none() {
        return Err(AppError::not_found("Người dùng không tồn tại"));
    }

    sqlx::query("INSERT IGNORE INTO wallets (user_id) VALUES (?)")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    let w_row = sqlx::query(
        "SELECT id, CAST(balance AS DOUBLE) AS balance FROM wallets WHERE user_id = ? FOR UPDATE",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    let wallet_id: i64 = w_row.try_get("id")?;
    let balance_before: f64 = w_row.try_get("balance")?;
    let balance_after = balance_before + body.amount;
    if balance_after < 0.0 {
        return Err(AppError::bad_request("Số dư sau điều chỉnh không thể âm"));
    }

    let deposit_delta = if body.amount > 0.0 { body.amount } else { 0.0 };
    sqlx::query("UPDATE wallets SET balance = ?, total_deposit = total_deposit + ? WHERE id = ?")
        .bind(balance_after)
        .bind(deposit_delta)
        .bind(wallet_id)
        .execute(&mut *tx)
        .await?;

    let tx_code = generate_transaction_code();
    let desc = body
        .description
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("Admin ({}) điều chỉnh số dư", admin.0.username));

    sqlx::query(
        r#"
        INSERT INTO transactions
        (transaction_code, user_id, wallet_id, type, amount, balance_before, balance_after, status, payment_method, description, completed_at)
        VALUES (?, ?, ?, 'adjustment', ?, ?, ?, 'completed', 'admin', ?, NOW())
        "#,
    )
    .bind(&tx_code)
    .bind(user_id)
    .bind(wallet_id)
    .bind(body.amount.abs())
    .bind(balance_before)
    .bind(balance_after)
    .bind(&desc)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(send_success(
        "Điều chỉnh số dư ví thành công",
        json!({
            "user_id": user_id,
            "transaction_code": tx_code,
            "balance_before": balance_before,
            "balance_after": balance_after,
        }),
    ))
}

/* ========================= PAYMENTS / TRANSACTIONS ========================= */

#[derive(Deserialize)]
struct AdminTxQuery {
    page: Option<i64>,
    limit: Option<i64>,
    #[serde(rename = "type")]
    tx_type: Option<String>,
    status: Option<String>,
    search: Option<String>,
}

async fn list_admin_transactions_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<AdminTxQuery>,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    let mut sql = String::from(
        r#"
        SELECT
            t.id, t.transaction_code, t.user_id, t.wallet_id, t.type,
            CAST(t.amount AS DOUBLE) AS amount,
            CAST(t.balance_before AS DOUBLE) AS balance_before,
            CAST(t.balance_after AS DOUBLE) AS balance_after,
            t.status, t.payment_method, t.reference_id, t.description,
            t.created_at, t.expired_at, t.completed_at,
            u.username, u.email
        FROM transactions t
        LEFT JOIN users u ON u.id = t.user_id
        WHERE 1=1
        "#,
    );
    let mut count_sql = String::from(
        "SELECT COUNT(t.id) AS total FROM transactions t LEFT JOIN users u ON u.id = t.user_id WHERE 1=1",
    );

    if q.tx_type.is_some() {
        sql.push_str(" AND t.type = ?");
        count_sql.push_str(" AND t.type = ?");
    }
    if q.status.is_some() {
        sql.push_str(" AND t.status = ?");
        count_sql.push_str(" AND t.status = ?");
    }
    if q.search.is_some() {
        sql.push_str(" AND (t.transaction_code LIKE ? OR t.reference_id LIKE ? OR u.email LIKE ? OR u.username LIKE ?)");
        count_sql.push_str(" AND (t.transaction_code LIKE ? OR t.reference_id LIKE ? OR u.email LIKE ? OR u.username LIKE ?)");
    }
    sql.push_str(" ORDER BY t.id DESC LIMIT ? OFFSET ?");

    let mut query = sqlx::query(&sql);
    let mut cquery = sqlx::query(&count_sql);

    if let Some(ref tt) = q.tx_type {
        query = query.bind(tt);
        cquery = cquery.bind(tt);
    }
    if let Some(ref st) = q.status {
        query = query.bind(st);
        cquery = cquery.bind(st);
    }
    let pat = q.search.as_ref().map(|s| format!("%{}%", s));
    if let Some(ref p) = pat {
        query = query.bind(p).bind(p).bind(p).bind(p);
        cquery = cquery.bind(p).bind(p).bind(p).bind(p);
    }

    let rows = query.bind(limit).bind(offset).fetch_all(pool).await?;
    let total: i64 = cquery.fetch_one(pool).await?.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            let expired_at: Option<DateTime<Utc>> = r.try_get("expired_at").ok().flatten();
            let completed_at: Option<DateTime<Utc>> = r.try_get("completed_at").ok().flatten();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "transaction_code": r.try_get::<String, _>("transaction_code").unwrap_or_default(),
                "user_id": r.try_get::<i64, _>("user_id").unwrap_or(0),
                "username": r.try_get::<Option<String>, _>("username").ok().flatten(),
                "email": r.try_get::<Option<String>, _>("email").ok().flatten(),
                "wallet_id": r.try_get::<i64, _>("wallet_id").unwrap_or(0),
                "type": r.try_get::<String, _>("type").unwrap_or_default(),
                "amount": r.try_get::<f64, _>("amount").unwrap_or(0.0),
                "balance_before": r.try_get::<Option<f64>, _>("balance_before").ok().flatten(),
                "balance_after": r.try_get::<Option<f64>, _>("balance_after").ok().flatten(),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "payment_method": r.try_get::<Option<String>, _>("payment_method").ok().flatten(),
                "reference_id": r.try_get::<Option<String>, _>("reference_id").ok().flatten(),
                "description": r.try_get::<Option<String>, _>("description").ok().flatten(),
                "created_at": created_at,
                "expired_at": expired_at,
                "completed_at": completed_at,
            })
        })
        .collect();

    let total_pages = ((total as f64) / (limit as f64)).ceil().max(1.0) as i64;
    Ok(send_success(
        "Lấy danh sách giao dịch thành công",
        json!({
            "items": items,
            "pagination": { "total": total, "page": page, "limit": limit, "total_pages": total_pages }
        }),
    ))
}

async fn approve_deposit_ep(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(tx_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let tx_row = sqlx::query(
        "SELECT id, wallet_id, type, status, CAST(amount AS DOUBLE) AS amount FROM transactions WHERE id = ? FOR UPDATE",
    )
    .bind(tx_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(tx_row) = tx_row else {
        return Err(AppError::not_found("Giao dịch không tồn tại"));
    };

    let tx_type: String = tx_row.try_get("type")?;
    let status: String = tx_row.try_get("status")?;
    if tx_type != "deposit" || (status != "pending" && status != "expired") {
        return Err(AppError::bad_request(
            "Chỉ có thể duyệt giao dịch nạp tiền đang pending hoặc expired",
        ));
    }

    let wallet_id: i64 = tx_row.try_get("wallet_id")?;
    let amount: f64 = tx_row.try_get("amount")?;

    let w_row = sqlx::query(
        "SELECT CAST(balance AS DOUBLE) AS balance FROM wallets WHERE id = ? FOR UPDATE",
    )
    .bind(wallet_id)
    .fetch_one(&mut *tx)
    .await?;

    let balance_before: f64 = w_row.try_get("balance")?;
    let balance_after = balance_before + amount;
    let ref_id = format!("ADMIN-APPROVE-{}", tx_id);

    sqlx::query(
        r#"
        UPDATE transactions
        SET status = 'completed', balance_before = ?, balance_after = ?,
            completed_at = NOW(), reference_id = COALESCE(reference_id, ?),
            description = CONCAT(COALESCE(description, ''), ' (Duyệt bởi ', ?, ')')
        WHERE id = ?
        "#,
    )
    .bind(balance_before)
    .bind(balance_after)
    .bind(&ref_id)
    .bind(&admin.0.username)
    .bind(tx_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE wallets SET balance = ?, total_deposit = total_deposit + ? WHERE id = ?")
        .bind(balance_after)
        .bind(amount)
        .bind(wallet_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(send_success(
        "Đã duyệt nạp tiền và cộng số dư ví thành công",
        json!({ "id": tx_id, "status": "completed", "balance_after": balance_after }),
    ))
}

async fn cancel_deposit_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(tx_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;
    let res = sqlx::query(
        "UPDATE transactions SET status = 'cancelled', completed_at = NOW() WHERE id = ? AND status IN ('pending', 'processing')",
    )
    .bind(tx_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::bad_request(
            "Giao dịch không tồn tại hoặc không còn ở trạng thái chờ xử lý",
        ));
    }

    Ok(send_success_empty("Đã hủy giao dịch thành công"))
}

/* ========================= ORDERS ========================= */

#[derive(Deserialize)]
struct AdminOrdersQuery {
    page: Option<i64>,
    limit: Option<i64>,
    status: Option<String>,
    search: Option<String>,
}

async fn list_admin_orders_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<AdminOrdersQuery>,
) -> AppResult<Json<Value>> {
    let pool = state.db()?;
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    let mut sql = String::from(
        r#"
        SELECT
            o.id, o.order_code, o.user_id,
            CAST(o.subtotal AS DOUBLE) AS subtotal,
            CAST(o.total AS DOUBLE) AS total,
            o.currency, o.status, o.created_at, o.completed_at,
            u.username, u.email
        FROM orders o
        LEFT JOIN users u ON u.id = o.user_id
        WHERE 1=1
        "#,
    );
    let mut count_sql = String::from(
        "SELECT COUNT(o.id) AS total FROM orders o LEFT JOIN users u ON u.id = o.user_id WHERE 1=1",
    );

    if q.status.is_some() {
        sql.push_str(" AND o.status = ?");
        count_sql.push_str(" AND o.status = ?");
    }
    if q.search.is_some() {
        sql.push_str(" AND (o.order_code LIKE ? OR u.email LIKE ? OR u.username LIKE ?)");
        count_sql.push_str(" AND (o.order_code LIKE ? OR u.email LIKE ? OR u.username LIKE ?)");
    }
    sql.push_str(" ORDER BY o.id DESC LIMIT ? OFFSET ?");

    let mut query = sqlx::query(&sql);
    let mut cquery = sqlx::query(&count_sql);

    if let Some(ref st) = q.status {
        query = query.bind(st);
        cquery = cquery.bind(st);
    }
    let pat = q.search.as_ref().map(|s| format!("%{}%", s));
    if let Some(ref p) = pat {
        query = query.bind(p).bind(p).bind(p);
        cquery = cquery.bind(p).bind(p).bind(p);
    }

    let rows = query.bind(limit).bind(offset).fetch_all(pool).await?;
    let total: i64 = cquery.fetch_one(pool).await?.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            let completed_at: Option<DateTime<Utc>> = r.try_get("completed_at").ok().flatten();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "order_code": r.try_get::<String, _>("order_code").unwrap_or_default(),
                "user_id": r.try_get::<i64, _>("user_id").unwrap_or(0),
                "username": r.try_get::<Option<String>, _>("username").ok().flatten(),
                "email": r.try_get::<Option<String>, _>("email").ok().flatten(),
                "subtotal": r.try_get::<f64, _>("subtotal").unwrap_or(0.0),
                "total": r.try_get::<f64, _>("total").unwrap_or(0.0),
                "currency": r.try_get::<String, _>("currency").unwrap_or_else(|_| "VND".to_string()),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "created_at": created_at,
                "completed_at": completed_at,
            })
        })
        .collect();

    let total_pages = ((total as f64) / (limit as f64)).ceil().max(1.0) as i64;
    Ok(send_success(
        "Lấy danh sách đơn hàng thành công",
        json!({
            "items": items,
            "pagination": { "total": total, "page": page, "limit": limit, "total_pages": total_pages }
        }),
    ))
}

#[derive(Deserialize)]
struct UpdateOrderBody {
    status: String,
}

async fn update_admin_order_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(order_id): Path<i64>,
    Json(body): Json<UpdateOrderBody>,
) -> AppResult<Json<Value>> {
    if !matches!(
        body.status.as_str(),
        "pending" | "processing" | "completed" | "cancelled" | "failed" | "refunded"
    ) {
        return Err(AppError::bad_request("Trạng thái đơn hàng không hợp lệ"));
    }

    let pool = state.db()?;
    let res = sqlx::query(
        "UPDATE orders SET status = ?, completed_at = IF(? = 'completed', COALESCE(completed_at, NOW()), completed_at) WHERE id = ?",
    )
    .bind(&body.status)
    .bind(&body.status)
    .bind(order_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::not_found("Đơn hàng không tồn tại"));
    }

    Ok(send_success_empty("Cập nhật trạng thái đơn hàng thành công"))
}

/* ========================= PROVIDERS ========================= */

async fn list_providers_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> AppResult<Json<Value>> {
    let data = list_providers(&state).await?;
    Ok(send_success("Lấy danh sách nhà cung cấp thành công", data))
}

#[derive(Deserialize)]
struct ProviderCatalogQuery {
    q: Option<String>,
    limit: Option<usize>,
    refresh: Option<bool>,
}

async fn provider_catalog_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(provider): Path<String>,
    Query(q): Query<ProviderCatalogQuery>,
) -> AppResult<Json<Value>> {
    let data = get_provider_catalog_filtered(
        &state,
        &provider,
        q.q.as_deref(),
        q.limit.unwrap_or(60),
        q.refresh.unwrap_or(false),
    )
    .await?;
    Ok(send_success("Lấy danh mục sản phẩm nhà cung cấp thành công", data))
}

async fn sync_providers_ep(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> AppResult<Json<Value>> {
    let summary = run_provider_price_sync(state).await;
    Ok(send_success("Hoàn tất đồng bộ giá từ nhà cung cấp", summary))
}
