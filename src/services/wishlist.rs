use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

pub async fn get_wishlist(state: &AppState, user_id: i64) -> AppResult<Vec<Value>> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT
            w.id AS wishlist_id,
            p.product_code,
            p.name,
            p.slug,
            CAST(p.price AS DOUBLE) AS price,
            p.stock,
            p.delivery_type,
            p.status,
            p.thumbnail,
            w.created_at
        FROM wishlist w
        JOIN products p ON w.product_id = p.id
        WHERE w.user_id = ? AND p.status != 'disabled'
        ORDER BY w.created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            json!({
                "wishlist_id": r.try_get::<i64, _>("wishlist_id").unwrap_or(0),
                "product_code": r.try_get::<String, _>("product_code").unwrap_or_default(),
                "name": r.try_get::<String, _>("name").unwrap_or_default(),
                "slug": r.try_get::<Option<String>, _>("slug").ok().flatten(),
                "price": r.try_get::<f64, _>("price").unwrap_or(0.0),
                "stock": r.try_get::<i64, _>("stock").unwrap_or(0),
                "delivery_type": r.try_get::<Option<String>, _>("delivery_type").ok().flatten(),
                "status": r.try_get::<Option<String>, _>("status").ok().flatten(),
                "thumbnail": r.try_get::<Option<String>, _>("thumbnail").ok().flatten(),
                "created_at": created_at,
            })
        })
        .collect())
}

pub async fn add_to_wishlist(
    state: &AppState,
    user_id: i64,
    product_code: &str,
) -> AppResult<Vec<Value>> {
    let pool = state.db()?;
    let product = sqlx::query(
        "SELECT id FROM products WHERE product_code = ? AND status != 'disabled'",
    )
    .bind(product_code)
    .fetch_optional(pool)
    .await?;

    let Some(prod) = product else {
        return Err(AppError::bad_request("Sản phẩm không tồn tại"));
    };
    let product_id: i64 = prod.try_get("id")?;

    sqlx::query("INSERT IGNORE INTO wishlist (user_id, product_id) VALUES (?, ?)")
        .bind(user_id)
        .bind(product_id)
        .execute(pool)
        .await?;

    get_wishlist(state, user_id).await
}

pub async fn remove_from_wishlist(
    state: &AppState,
    user_id: i64,
    product_code: &str,
) -> AppResult<Vec<Value>> {
    let pool = state.db()?;
    let product = sqlx::query("SELECT id FROM products WHERE product_code = ?")
        .bind(product_code)
        .fetch_optional(pool)
        .await?;

    let Some(prod) = product else {
        return Err(AppError::bad_request("Sản phẩm không tồn tại"));
    };
    let product_id: i64 = prod.try_get("id")?;

    sqlx::query("DELETE FROM wishlist WHERE user_id = ? AND product_id = ?")
        .bind(user_id)
        .bind(product_id)
        .execute(pool)
        .await?;

    get_wishlist(state, user_id).await
}
