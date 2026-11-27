use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    core::security::{generate_order_code, generate_transaction_code},
    error::{AppError, AppResult},
    state::AppState,
};

struct LockedProductItem {
    product_id: i64,
    product_code: String,
    product_name: String,
    quantity: i64,
    unit_price: f64,
    subtotal: f64,
    delivery_type: String,
    assigned_ids: Vec<i64>,
}

pub async fn get_order_by_id(state: &AppState, user_id: i64, order_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let order = sqlx::query(
        r#"
        SELECT id, order_code, CAST(subtotal AS DOUBLE) AS subtotal,
               CAST(total AS DOUBLE) AS total, currency, status, created_at, completed_at
        FROM orders WHERE id = ? AND user_id = ?
        "#,
    )
    .bind(order_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let Some(order) = order else {
        return Err(AppError::not_found("Đơn hàng không tồn tại"));
    };

    let order_items = sqlx::query(
        r#"
        SELECT id, product_id, product_code, product_name, quantity,
               CAST(unit_price AS DOUBLE) AS unit_price,
               CAST(subtotal AS DOUBLE) AS subtotal, delivery_type
        FROM order_items WHERE order_id = ?
        "#,
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    let delivered_items = sqlx::query(
        "SELECT id, product_id, content, identifier FROM product_items WHERE order_id = ?",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    let formatted_items: Vec<Value> = order_items
        .into_iter()
        .map(|item| {
            let pid: i64 = item.try_get("product_id").unwrap_or(0);
            let delivery_type: String = item.try_get("delivery_type").unwrap_or_default();
            let belong: Vec<Value> = if delivery_type == "automatic" {
                delivered_items
                    .iter()
                    .filter(|di| di.try_get::<i64, _>("product_id").unwrap_or(-1) == pid)
                    .map(|di| {
                        json!({
                            "id": di.try_get::<i64, _>("id").unwrap_or(0),
                            "identifier": di.try_get::<Option<String>, _>("identifier").ok().flatten(),
                            "content": di.try_get::<Option<String>, _>("content").ok().flatten(),
                        })
                    })
                    .collect()
            } else {
                vec![]
            };

            json!({
                "product_code": item.try_get::<String, _>("product_code").unwrap_or_default(),
                "product_name": item.try_get::<String, _>("product_name").unwrap_or_default(),
                "quantity": item.try_get::<i64, _>("quantity").unwrap_or(0),
                "unit_price": item.try_get::<f64, _>("unit_price").unwrap_or(0.0),
                "subtotal": item.try_get::<f64, _>("subtotal").unwrap_or(0.0),
                "delivery_type": delivery_type,
                "items": belong,
            })
        })
        .collect();

    let created_at: Option<DateTime<Utc>> = order.try_get("created_at").ok().flatten();
    let completed_at: Option<DateTime<Utc>> = order.try_get("completed_at").ok().flatten();

    Ok(json!({
        "id": order.try_get::<i64, _>("id")?,
        "order_code": order.try_get::<String, _>("order_code")?,
        "subtotal": order.try_get::<f64, _>("subtotal").unwrap_or(0.0),
        "total": order.try_get::<f64, _>("total").unwrap_or(0.0),
        "currency": order.try_get::<String, _>("currency").unwrap_or_else(|_| "VND".to_string()),
        "status": order.try_get::<String, _>("status")?,
        "created_at": created_at,
        "completed_at": completed_at,
        "items": formatted_items,
    }))
}

pub async fn checkout(state: &AppState, user_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    // 1. Lock cart items
    let cart_items = sqlx::query(
        r#"
        SELECT ci.id AS cart_item_id, ci.product_id, ci.quantity,
               p.product_code, p.name AS product_name, CAST(p.price AS DOUBLE) AS price,
               p.stock, p.delivery_type, p.status
        FROM cart_items ci
        JOIN products p ON ci.product_id = p.id
        WHERE ci.user_id = ? FOR UPDATE
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;

    if cart_items.is_empty() {
        return Err(AppError::bad_request("Giỏ hàng trống"));
    }

    // 2. Validate products & lock inventory items
    let mut total_amount = 0.0_f64;
    let mut to_process = Vec::with_capacity(cart_items.len());

    for item in cart_items {
        let status: String = item.try_get("status")?;
        let product_name: String = item.try_get("product_name")?;
        if status != "active" {
            return Err(AppError::bad_request(format!(
                "Sản phẩm \"{}\" hiện đã ngừng kinh doanh",
                product_name
            )));
        }

        let product_id: i64 = item.try_get("product_id")?;
        let product_code: String = item.try_get("product_code")?;
        let unit_price: f64 = item.try_get("price")?;
        let product_stock: i64 = item.try_get("stock").unwrap_or(0);
        let qty: i64 = item.try_get("quantity")?;
        let delivery_type: String = item.try_get("delivery_type")?;
        let subtotal = unit_price * (qty as f64);
        total_amount += subtotal;

        let assigned_ids: Vec<i64> = if delivery_type.eq_ignore_ascii_case("manual") {
            if product_stock < qty {
                return Err(AppError::bad_request(format!(
                    "Sản phẩm \"{}\" không đủ số lượng trong kho (còn {})",
                    product_name, product_stock
                )));
            }
            Vec::new()
        } else {
            let available = sqlx::query(
                r#"
                SELECT id FROM product_items
                WHERE product_id = ? AND status = 'available'
                LIMIT ? FOR UPDATE
                "#,
            )
            .bind(product_id)
            .bind(qty)
            .fetch_all(&mut *tx)
            .await?;

            if (available.len() as i64) < qty {
                return Err(AppError::bad_request(format!(
                    "Sản phẩm \"{}\" không đủ số lượng trong kho (còn {})",
                    product_name,
                    available.len()
                )));
            }

            available
                .into_iter()
                .filter_map(|r| r.try_get("id").ok())
                .collect()
        };

        to_process.push(LockedProductItem {
            product_id,
            product_code,
            product_name,
            quantity: qty,
            unit_price,
            subtotal,
            delivery_type,
            assigned_ids,
        });
    }

    // 3. Lock wallet & verify balance
    let wallet = sqlx::query(
        "SELECT id, CAST(balance AS DOUBLE) AS balance FROM wallets WHERE user_id = ? FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(wallet) = wallet else {
        return Err(AppError::bad_request("Ví người dùng không tồn tại"));
    };
    let wallet_id: i64 = wallet.try_get("id")?;
    let balance_before: f64 = wallet.try_get("balance")?;

    if balance_before < total_amount {
        return Err(AppError::bad_request(format!(
            "Số dư ví không đủ (Hiện có: {:.0} VND, Cần: {:.0} VND)",
            balance_before, total_amount
        )));
    }

    let balance_after = balance_before - total_amount;

    // 4. Deduct wallet balance
    sqlx::query("UPDATE wallets SET balance = balance - ?, total_spent = total_spent + ? WHERE id = ?")
        .bind(total_amount)
        .bind(total_amount)
        .bind(wallet_id)
        .execute(&mut *tx)
        .await?;

    // 5. Create order (pending for manual approval items, completed immediately for pure automatic)
    let has_manual = to_process
        .iter()
        .any(|p| p.delivery_type.eq_ignore_ascii_case("manual"));
    let order_code = generate_order_code();
    let order_res = if has_manual {
        sqlx::query(
            r#"
            INSERT INTO orders (order_code, user_id, wallet_id, subtotal, total, currency, status, completed_at)
            VALUES (?, ?, ?, ?, ?, 'VND', 'pending', NULL)
            "#,
        )
        .bind(&order_code)
        .bind(user_id)
        .bind(wallet_id)
        .bind(total_amount)
        .bind(total_amount)
        .execute(&mut *tx)
        .await?
    } else {
        sqlx::query(
            r#"
            INSERT INTO orders (order_code, user_id, wallet_id, subtotal, total, currency, status, completed_at)
            VALUES (?, ?, ?, ?, ?, 'VND', 'completed', CURRENT_TIMESTAMP)
            "#,
        )
        .bind(&order_code)
        .bind(user_id)
        .bind(wallet_id)
        .bind(total_amount)
        .bind(total_amount)
        .execute(&mut *tx)
        .await?
    };
    let order_id = order_res.last_insert_id() as i64;

    // 6. Insert order items & mark inventory sold
    for p in &to_process {
        sqlx::query(
            r#"
            INSERT INTO order_items
            (order_id, product_id, product_code, product_name, quantity, unit_price, subtotal, delivery_type)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(order_id)
        .bind(p.product_id)
        .bind(&p.product_code)
        .bind(&p.product_name)
        .bind(p.quantity)
        .bind(p.unit_price)
        .bind(p.subtotal)
        .bind(&p.delivery_type)
        .execute(&mut *tx)
        .await?;

        for item_id in &p.assigned_ids {
            sqlx::query(
                "UPDATE product_items SET status = 'sold', order_id = ?, sold_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(order_id)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        }

        if p.delivery_type.eq_ignore_ascii_case("manual") {
            sqlx::query("UPDATE products SET stock = GREATEST(0, stock - ?) WHERE id = ?")
                .bind(p.quantity)
                .bind(p.product_id)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE products
                SET stock = (SELECT COUNT(id) FROM product_items WHERE product_id = ? AND status = 'available')
                WHERE id = ?
                "#,
            )
            .bind(p.product_id)
            .bind(p.product_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    // 7. Record transaction
    let tx_code = generate_transaction_code();
    sqlx::query(
        r#"
        INSERT INTO transactions
        (transaction_code, user_id, wallet_id, type, amount, balance_before, balance_after, status, payment_method, reference_id, description, completed_at)
        VALUES (?, ?, ?, 'purchase', ?, ?, ?, 'completed', 'wallet', ?, ?, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(&tx_code)
    .bind(user_id)
    .bind(wallet_id)
    .bind(total_amount)
    .bind(balance_before)
    .bind(balance_after)
    .bind(&order_code)
    .bind(format!("Thanh toán đơn hàng {}", order_code))
    .execute(&mut *tx)
    .await?;

    // 8. Clear cart
    sqlx::query("DELETE FROM cart_items WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    get_order_by_id(state, user_id, order_id).await
}

pub async fn get_orders(
    state: &AppState,
    user_id: i64,
    page: i64,
    limit: i64,
    status: Option<&str>,
) -> AppResult<Value> {
    let pool = state.db()?;
    let page = page.max(1);
    let limit = limit.clamp(1, 100);
    let offset = (page - 1) * limit;

    let mut sql = String::from(
        r#"
        SELECT id, order_code, CAST(subtotal AS DOUBLE) AS subtotal,
               CAST(total AS DOUBLE) AS total, currency, status, created_at, completed_at
        FROM orders WHERE user_id = ?
        "#,
    );
    let mut count_sql = String::from("SELECT COUNT(id) AS total FROM orders WHERE user_id = ?");

    if status.is_some() {
        sql.push_str(" AND status = ?");
        count_sql.push_str(" AND status = ?");
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");

    let mut q = sqlx::query(&sql).bind(user_id);
    let mut cq = sqlx::query(&count_sql).bind(user_id);

    if let Some(st) = status {
        q = q.bind(st);
        cq = cq.bind(st);
    }

    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    let count_row = cq.fetch_one(pool).await?;
    let total: i64 = count_row.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            let completed_at: Option<DateTime<Utc>> = r.try_get("completed_at").ok().flatten();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "order_code": r.try_get::<String, _>("order_code").unwrap_or_default(),
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

    Ok(json!({
        "items": items,
        "pagination": {
            "total": total,
            "page": page,
            "limit": limit,
            "total_pages": total_pages,
        }
    }))
}

pub async fn cancel_order(state: &AppState, user_id: i64, order_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let order = sqlx::query("SELECT id, status FROM orders WHERE id = ? AND user_id = ?")
        .bind(order_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

    let Some(order) = order else {
        return Err(AppError::bad_request("Đơn hàng không tồn tại"));
    };

    let status: String = order.try_get("status")?;
    if status == "completed" {
        return Err(AppError::bad_request(
            "Không thể hủy đơn hàng đã hoàn tất. Vui lòng liên hệ hỗ trợ.",
        ));
    }
    if status == "cancelled" {
        return Err(AppError::bad_request("Đơn hàng đã được hủy trước đó."));
    }

    sqlx::query("UPDATE orders SET status = 'cancelled' WHERE id = ?")
        .bind(order_id)
        .execute(pool)
        .await?;

    Ok(json!({ "id": order_id, "status": "cancelled" }))
}
