use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

pub async fn get_cart(state: &AppState, user_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT
            ci.id,
            ci.product_id,
            p.product_code,
            p.name AS product_name,
            p.slug,
            CAST(p.price AS DOUBLE) AS unit_price,
            ci.quantity,
            p.stock,
            p.delivery_type,
            p.status AS product_status,
            p.thumbnail,
            CAST(p.price * ci.quantity AS DOUBLE) AS subtotal
        FROM cart_items ci
        JOIN products p ON ci.product_id = p.id
        WHERE ci.user_id = ?
        ORDER BY ci.created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut total = 0.0_f64;
    let mut total_items = 0_i64;
    let mut formatted_items = Vec::with_capacity(rows.len());

    for r in rows {
        let subtotal: f64 = r.try_get("subtotal").unwrap_or(0.0);
        let unit_price: f64 = r.try_get("unit_price").unwrap_or(0.0);
        let qty: i64 = r.try_get("quantity").unwrap_or(0);
        let stock: i64 = r.try_get("stock").unwrap_or(0);
        let product_status: String = r.try_get("product_status").unwrap_or_default();
        let is_active = product_status == "active";

        if is_active {
            total += subtotal;
            total_items += qty;
        }

        formatted_items.push(json!({
            "id": r.try_get::<i64, _>("id").unwrap_or(0),
            "product_code": r.try_get::<String, _>("product_code").unwrap_or_default(),
            "product_name": r.try_get::<String, _>("product_name").unwrap_or_default(),
            "slug": r.try_get::<Option<String>, _>("slug").ok().flatten(),
            "unit_price": format!("{:.2}", unit_price),
            "quantity": qty,
            "stock": stock,
            "delivery_type": r.try_get::<Option<String>, _>("delivery_type").ok().flatten(),
            "thumbnail": r.try_get::<Option<String>, _>("thumbnail").ok().flatten(),
            "subtotal": format!("{:.2}", subtotal),
            "is_available": is_active && stock >= qty,
        }));
    }

    Ok(json!({
        "items": formatted_items,
        "summary": {
            "total_items": total_items,
            "total_amount": format!("{:.2}", total),
            "currency": "VND",
        }
    }))
}

pub async fn add_to_cart(
    state: &AppState,
    user_id: i64,
    product_code: &str,
    quantity: i64,
) -> AppResult<Value> {
    if quantity <= 0 {
        return Err(AppError::bad_request("Số lượng sản phẩm không hợp lệ"));
    }

    let pool = state.db()?;
    let product = sqlx::query(
        "SELECT id, name, stock, status FROM products WHERE product_code = ?",
    )
    .bind(product_code)
    .fetch_optional(pool)
    .await?;

    let Some(prod) = product else {
        return Err(AppError::bad_request(
            "Sản phẩm không tồn tại hoặc đã ngừng kinh doanh",
        ));
    };
    let status: String = prod.try_get("status")?;
    if status != "active" {
        return Err(AppError::bad_request(
            "Sản phẩm không tồn tại hoặc đã ngừng kinh doanh",
        ));
    }

    let product_id: i64 = prod.try_get("id")?;
    let stock: i64 = prod.try_get("stock")?;

    let existing = sqlx::query(
        "SELECT id, quantity FROM cart_items WHERE user_id = ? AND product_id = ?",
    )
    .bind(user_id)
    .bind(product_id)
    .fetch_optional(pool)
    .await?;

    let new_qty = if let Some(ref ex) = existing {
        let cur_qty: i64 = ex.try_get("quantity")?;
        cur_qty + quantity
    } else {
        quantity
    };

    if new_qty > stock {
        return Err(AppError::bad_request(format!(
            "Kho hàng chỉ còn {} sản phẩm",
            stock
        )));
    }

    if let Some(ex) = existing {
        let ex_id: i64 = ex.try_get("id")?;
        sqlx::query("UPDATE cart_items SET quantity = ? WHERE id = ?")
            .bind(new_qty)
            .bind(ex_id)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("INSERT INTO cart_items (user_id, product_id, quantity) VALUES (?, ?, ?)")
            .bind(user_id)
            .bind(product_id)
            .bind(quantity)
            .execute(pool)
            .await?;
    }

    get_cart(state, user_id).await
}

pub async fn update_cart_item(
    state: &AppState,
    user_id: i64,
    cart_item_id: i64,
    quantity: i64,
) -> AppResult<Value> {
    if quantity <= 0 {
        return Err(AppError::bad_request("Số lượng phải lớn hơn 0"));
    }

    let pool = state.db()?;
    let item = sqlx::query(
        r#"
        SELECT ci.id, ci.product_id, p.stock, p.status
        FROM cart_items ci
        JOIN products p ON ci.product_id = p.id
        WHERE ci.id = ? AND ci.user_id = ?
        "#,
    )
    .bind(cart_item_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let Some(item) = item else {
        return Err(AppError::bad_request("Mục giỏ hàng không tồn tại"));
    };

    let status: String = item.try_get("status")?;
    if status != "active" {
        return Err(AppError::bad_request("Sản phẩm này hiện không khả dụng"));
    }
    let stock: i64 = item.try_get("stock")?;
    if quantity > stock {
        return Err(AppError::bad_request(format!(
            "Kho hàng chỉ còn {} sản phẩm",
            stock
        )));
    }

    sqlx::query("UPDATE cart_items SET quantity = ? WHERE id = ?")
        .bind(quantity)
        .bind(cart_item_id)
        .execute(pool)
        .await?;

    get_cart(state, user_id).await
}

pub async fn remove_cart_item(
    state: &AppState,
    user_id: i64,
    cart_item_id: i64,
) -> AppResult<Value> {
    let pool = state.db()?;
    let res = sqlx::query("DELETE FROM cart_items WHERE id = ? AND user_id = ?")
        .bind(cart_item_id)
        .bind(user_id)
        .execute(pool)
        .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::bad_request("Mục giỏ hàng không tồn tại"));
    }

    get_cart(state, user_id).await
}

pub async fn clear_cart(state: &AppState, user_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    sqlx::query("DELETE FROM cart_items WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;

    Ok(json!({
        "items": [],
        "summary": {
            "total_items": 0,
            "total_amount": "0.00",
            "currency": "VND",
        }
    }))
}
