use std::collections::HashSet;
use chrono::NaiveDateTime;
use serde_json::{json, Map, Value};
use sqlx::{MySql, Row, Transaction};

use crate::{
    core::security::generate_slug,
    error::{AppError, AppResult},
    state::AppState,
};

pub async fn sync_stock_tx(tx: &mut Transaction<'_, MySql>, product_id: i64) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE products
        SET stock = (SELECT COUNT(id) FROM product_items WHERE product_id = ? AND status = 'available')
        WHERE id = ?
        "#,
    )
    .bind(product_id)
    .bind(product_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn get_public_products(
    state: &AppState,
    page: i64,
    limit: i64,
    search: Option<&str>,
    category: Option<&str>,
    sort: Option<&str>,
) -> AppResult<Value> {
    let pool = state.db()?;
    let page = page.max(1);
    let limit = limit.clamp(1, 100);
    let offset = (page - 1) * limit;
    let sort_dir = if sort.unwrap_or("DESC").eq_ignore_ascii_case("ASC") {
        "ASC"
    } else {
        "DESC"
    };

    let mut sql = String::from(
        "SELECT product_code, category, name, slug, description, CAST(price AS DOUBLE) AS price, stock, delivery_type, thumbnail FROM products WHERE status = 'active'",
    );
    let mut count_sql = String::from("SELECT COUNT(id) AS total FROM products WHERE status = 'active'");

    if category.is_some() {
        sql.push_str(" AND category = ?");
        count_sql.push_str(" AND category = ?");
    }
    if search.is_some() {
        sql.push_str(" AND name LIKE ?");
        count_sql.push_str(" AND name LIKE ?");
    }
    sql.push_str(&format!(" ORDER BY sort_order ASC, created_at {} LIMIT ? OFFSET ?", sort_dir));

    let mut q = sqlx::query(&sql);
    let mut cq = sqlx::query(&count_sql);

    if let Some(cat) = category {
        q = q.bind(cat);
        cq = cq.bind(cat);
    }
    let search_pattern = search.map(|s| format!("%{}%", s));
    if let Some(ref pat) = search_pattern {
        q = q.bind(pat);
        cq = cq.bind(pat);
    }

    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    let count_row = cq.fetch_one(pool).await?;
    let total: i64 = count_row.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "product_code": r.try_get::<String, _>("product_code").unwrap_or_default(),
                "category": r.try_get::<Option<String>, _>("category").ok().flatten(),
                "name": r.try_get::<String, _>("name").unwrap_or_default(),
                "slug": r.try_get::<Option<String>, _>("slug").ok().flatten(),
                "description": r.try_get::<Option<String>, _>("description").ok().flatten(),
                "price": r.try_get::<f64, _>("price").unwrap_or(0.0),
                "stock": r.try_get::<i64, _>("stock").unwrap_or(0),
                "delivery_type": r.try_get::<Option<String>, _>("delivery_type").ok().flatten(),
                "thumbnail": r.try_get::<Option<String>, _>("thumbnail").ok().flatten(),
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

pub async fn get_product_by_code(state: &AppState, product_code: &str) -> AppResult<Value> {
    let pool = state.db()?;
    let row = sqlx::query(
        r#"
        SELECT product_code, category, name, slug, description, CAST(price AS DOUBLE) AS price, stock, delivery_type, thumbnail
        FROM products WHERE product_code = ? AND status = 'active'
        "#,
    )
    .bind(product_code)
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Err(AppError::not_found("Sản phẩm không tồn tại hoặc đã bị ẩn"));
    };

    Ok(json!({
        "product_code": r.try_get::<String, _>("product_code")?,
        "category": r.try_get::<Option<String>, _>("category").ok().flatten(),
        "name": r.try_get::<String, _>("name")?,
        "slug": r.try_get::<Option<String>, _>("slug").ok().flatten(),
        "description": r.try_get::<Option<String>, _>("description").ok().flatten(),
        "price": r.try_get::<f64, _>("price").unwrap_or(0.0),
        "stock": r.try_get::<i64, _>("stock").unwrap_or(0),
        "delivery_type": r.try_get::<Option<String>, _>("delivery_type").ok().flatten(),
        "thumbnail": r.try_get::<Option<String>, _>("thumbnail").ok().flatten(),
    }))
}

pub async fn create_product(state: &AppState, data: &Map<String, Value>) -> AppResult<i64> {
    let pool = state.db()?;
    let product_code = data
        .get("product_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu thông tin product_code hoặc name"))?;
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu thông tin product_code hoặc name"))?;

    let slug = generate_slug(name);
    let category = data.get("category").and_then(Value::as_str);
    let description = data.get("description").and_then(Value::as_str);
    let price = data.get("price").and_then(Value::as_f64).unwrap_or(0.0);
    let delivery_type = data
        .get("delivery_type")
        .and_then(Value::as_str)
        .unwrap_or("manual");
    let thumbnail = data.get("thumbnail").and_then(Value::as_str);

    let res = sqlx::query(
        r#"
        INSERT INTO products (product_code, category, name, slug, description, price, delivery_type, thumbnail)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(product_code)
    .bind(category)
    .bind(name)
    .bind(&slug)
    .bind(description)
    .bind(price)
    .bind(delivery_type)
    .bind(thumbnail)
    .execute(pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("Duplicate entry") {
            AppError::bad_request("Product Code đã tồn tại")
        } else {
            AppError::Database(e)
        }
    })?;

    Ok(res.last_insert_id() as i64)
}

pub async fn update_product(
    state: &AppState,
    prod_id: i64,
    data: &Map<String, Value>,
) -> AppResult<()> {
    let pool = state.db()?;
    let existing = sqlx::query(
        r#"
        SELECT product_code, category, name, slug, description, CAST(price AS DOUBLE) AS price,
               delivery_type, status, thumbnail, sort_order
        FROM products WHERE id = ?
        "#,
    )
    .bind(prod_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = existing else {
        return Err(AppError::bad_request("Sản phẩm không tồn tại"));
    };

    let product_code = data
        .get("product_code")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| row.try_get("product_code").unwrap_or_default());
    let category = if data.contains_key("category") {
        data.get("category").and_then(Value::as_str).map(String::from)
    } else {
        row.try_get("category").ok().flatten()
    };
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| row.try_get("name").unwrap_or_default());
    let slug = if data.contains_key("name") {
        generate_slug(&name)
    } else {
        row.try_get("slug").unwrap_or_else(|_| generate_slug(&name))
    };
    let description = if data.contains_key("description") {
        data.get("description").and_then(Value::as_str).map(String::from)
    } else {
        row.try_get("description").ok().flatten()
    };
    let price = data
        .get("price")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| row.try_get("price").unwrap_or(0.0));
    let delivery_type = data
        .get("delivery_type")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| row.try_get("delivery_type").unwrap_or_else(|_| "manual".to_string()));
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| row.try_get("status").unwrap_or_else(|_| "active".to_string()));
    let thumbnail = if data.contains_key("thumbnail") {
        data.get("thumbnail").and_then(Value::as_str).map(String::from)
    } else {
        row.try_get("thumbnail").ok().flatten()
    };
    let sort_order = data
        .get("sort_order")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| row.try_get("sort_order").unwrap_or(0));

    sqlx::query(
        r#"
        UPDATE products
        SET product_code = ?, category = ?, name = ?, slug = ?, description = ?,
            price = ?, delivery_type = ?, status = ?, thumbnail = ?, sort_order = ?
        WHERE id = ?
        "#,
    )
    .bind(product_code)
    .bind(category)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(price)
    .bind(delivery_type)
    .bind(status)
    .bind(thumbnail)
    .bind(sort_order)
    .bind(prod_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_product(state: &AppState, prod_id: i64) -> AppResult<()> {
    let pool = state.db()?;
    let res = sqlx::query("UPDATE products SET status = 'disabled' WHERE id = ?")
        .bind(prod_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::bad_request("Sản phẩm không tồn tại"));
    }
    Ok(())
}

pub async fn import_product_items(
    state: &AppState,
    product_id: i64,
    content_array: Vec<String>,
) -> AppResult<Value> {
    let mut seen = HashSet::new();
    let items: Vec<String> = content_array
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .collect();

    if items.is_empty() {
        return Err(AppError::bad_request("Không có dữ liệu hợp lệ để import"));
    }

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let prod = sqlx::query("SELECT id FROM products WHERE id = ? FOR UPDATE")
        .bind(product_id)
        .fetch_optional(&mut *tx)
        .await?;
    if prod.is_none() {
        return Err(AppError::bad_request("Sản phẩm không tồn tại"));
    }

    let existing_rows = sqlx::query("SELECT content FROM product_items WHERE product_id = ?")
        .bind(product_id)
        .fetch_all(&mut *tx)
        .await?;
    let existing_set: HashSet<String> = existing_rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("content").ok())
        .collect();

    let to_insert: Vec<&String> = items
        .iter()
        .filter(|it| !existing_set.contains(*it))
        .collect();
    let duplicate_count = items.len() - to_insert.len();

    for it in &to_insert {
        let identifier: String = it.chars().take(30).collect();
        sqlx::query(
            "INSERT INTO product_items (product_id, content, identifier, status) VALUES (?, ?, ?, 'available')",
        )
        .bind(product_id)
        .bind(it.as_str())
        .bind(&identifier)
        .execute(&mut *tx)
        .await?;
    }

    sync_stock_tx(&mut tx, product_id).await?;
    tx.commit().await?;

    Ok(json!({
        "inserted": to_insert.len(),
        "duplicate": duplicate_count,
    }))
}

pub async fn update_product_item(state: &AppState, item_id: i64, content: &str) -> AppResult<()> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let row = sqlx::query("SELECT product_id FROM product_items WHERE id = ? FOR UPDATE")
        .bind(item_id)
        .fetch_optional(&mut *tx)
        .await?;
    if row.is_none() {
        return Err(AppError::bad_request("Item không tồn tại"));
    }

    let identifier: String = content.chars().take(30).collect();
    sqlx::query("UPDATE product_items SET content = ?, identifier = ? WHERE id = ?")
        .bind(content)
        .bind(&identifier)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn delete_product_item(state: &AppState, item_id: i64) -> AppResult<()> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let row = sqlx::query("SELECT product_id FROM product_items WHERE id = ? FOR UPDATE")
        .bind(item_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        return Err(AppError::bad_request("Item không tồn tại"));
    };
    let product_id: i64 = row.try_get("product_id")?;

    sqlx::query("UPDATE product_items SET status = 'deleted' WHERE id = ?")
        .bind(item_id)
        .execute(&mut *tx)
        .await?;

    sync_stock_tx(&mut tx, product_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn get_admin_product_items(
    state: &AppState,
    product_id: i64,
    page: i64,
    limit: i64,
    status: Option<&str>,
    search: Option<&str>,
) -> AppResult<Value> {
    let pool = state.db()?;
    let page = page.max(1);
    let limit = limit.clamp(1, 200);
    let offset = (page - 1) * limit;

    let mut sql = String::from(
        "SELECT id, identifier, content, status, order_id, created_at, sold_at FROM product_items WHERE product_id = ?",
    );
    let mut count_sql =
        String::from("SELECT COUNT(id) AS total FROM product_items WHERE product_id = ?");

    if status.is_some() {
        sql.push_str(" AND status = ?");
        count_sql.push_str(" AND status = ?");
    }
    if search.is_some() {
        sql.push_str(" AND (identifier LIKE ? OR content LIKE ?)");
        count_sql.push_str(" AND (identifier LIKE ? OR content LIKE ?)");
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");

    let mut q = sqlx::query(&sql).bind(product_id);
    let mut cq = sqlx::query(&count_sql).bind(product_id);

    if let Some(st) = status {
        q = q.bind(st);
        cq = cq.bind(st);
    }
    let search_pat = search.map(|s| format!("%{}%", s));
    if let Some(ref pat) = search_pat {
        q = q.bind(pat).bind(pat);
        cq = cq.bind(pat).bind(pat);
    }

    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    let count_row = cq.fetch_one(pool).await?;
    let total: i64 = count_row.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let created_at: Option<NaiveDateTime> = r.try_get("created_at").ok().flatten();
            let sold_at: Option<NaiveDateTime> = r.try_get("sold_at").ok().flatten();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "identifier": r.try_get::<Option<String>, _>("identifier").ok().flatten(),
                "content": r.try_get::<Option<String>, _>("content").ok().flatten(),
                "status": r.try_get::<Option<String>, _>("status").ok().flatten(),
                "order_id": r.try_get::<Option<i64>, _>("order_id").ok().flatten(),
                "created_at": created_at.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
                "sold_at": sold_at.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
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
