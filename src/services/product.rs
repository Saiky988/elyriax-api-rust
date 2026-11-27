use std::collections::HashSet;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::{MySql, Row, Transaction};

use crate::{
    core::security::generate_slug,
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductVariant {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub product_code: String,
    pub category: Option<String>,
    pub name: String,
    pub variant_name: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub price: f64,
    pub stock: i64,
    pub delivery_type: Option<String>,
    pub require_profile_type: String,
    pub thumbnail: Option<String>,
    pub sort_order: i64,
}

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

pub async fn get_public_categories(state: &AppState) -> AppResult<Vec<String>> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT category
        FROM products
        WHERE status = 'active' AND parent_id IS NULL AND category IS NOT NULL AND category <> ''
        ORDER BY category ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("category").ok())
        .collect())
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
        r#"
        SELECT
            p.id,
            p.product_code,
            p.category,
            p.name,
            p.variant_name,
            p.slug,
            p.description,
            CAST(p.price AS DOUBLE) AS price,
            p.stock,
            p.delivery_type,
            COALESCE(p.require_profile_type, 'none') AS require_profile_type,
            p.thumbnail,
            COALESCE(v.variants_count, 0) AS variants_count,
            CAST(COALESCE(v.min_price, p.price) AS DOUBLE) AS min_price,
            CAST(COALESCE(v.max_price, p.price) AS DOUBLE) AS max_price,
            CAST(CASE WHEN COALESCE(v.variants_count, 0) > 0 THEN COALESCE(v.total_stock, 0) ELSE p.stock END AS SIGNED) AS total_stock
        FROM products p
        LEFT JOIN (
            SELECT
                parent_id,
                COUNT(id) AS variants_count,
                MIN(price) AS min_price,
                MAX(price) AS max_price,
                SUM(stock) AS total_stock
            FROM products
            WHERE status = 'active' AND parent_id IS NOT NULL
            GROUP BY parent_id
        ) v ON v.parent_id = p.id
        WHERE p.status = 'active' AND p.parent_id IS NULL
        "#,
    );
    let mut count_sql = String::from(
        "SELECT COUNT(p.id) AS total FROM products p WHERE p.status = 'active' AND p.parent_id IS NULL",
    );

    if category.is_some() {
        sql.push_str(" AND p.category = ?");
        count_sql.push_str(" AND p.category = ?");
    }
    if search.is_some() {
        sql.push_str(
            " AND (p.name LIKE ? OR p.product_code LIKE ? OR EXISTS (SELECT 1 FROM products c WHERE c.parent_id = p.id AND c.status = 'active' AND (c.name LIKE ? OR c.variant_name LIKE ? OR c.product_code LIKE ?)))",
        );
        count_sql.push_str(
            " AND (p.name LIKE ? OR p.product_code LIKE ? OR EXISTS (SELECT 1 FROM products c WHERE c.parent_id = p.id AND c.status = 'active' AND (c.name LIKE ? OR c.variant_name LIKE ? OR c.product_code LIKE ?)))",
        );
    }
    sql.push_str(&format!(
        " ORDER BY p.sort_order ASC, p.created_at {} LIMIT ? OFFSET ?",
        sort_dir
    ));

    let mut q = sqlx::query(&sql);
    let mut cq = sqlx::query(&count_sql);

    if let Some(cat) = category {
        q = q.bind(cat);
        cq = cq.bind(cat);
    }
    let search_pattern = search.map(|s| format!("%{}%", s));
    if let Some(ref pat) = search_pattern {
        q = q.bind(pat).bind(pat).bind(pat).bind(pat).bind(pat);
        cq = cq.bind(pat).bind(pat).bind(pat).bind(pat).bind(pat);
    }

    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    let count_row = cq.fetch_one(pool).await?;
    let total: i64 = count_row.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let variants_count: i64 = r.try_get("variants_count").unwrap_or(0);
            let min_price: f64 = r.try_get("min_price").unwrap_or(0.0);
            let max_price: f64 = r.try_get("max_price").unwrap_or(min_price);
            let total_stock: i64 = r.try_get("total_stock").unwrap_or(0);

            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "product_code": r.try_get::<String, _>("product_code").unwrap_or_default(),
                "category": r.try_get::<Option<String>, _>("category").ok().flatten(),
                "name": r.try_get::<String, _>("name").unwrap_or_default(),
                "variant_name": r.try_get::<Option<String>, _>("variant_name").ok().flatten(),
                "slug": r.try_get::<Option<String>, _>("slug").ok().flatten(),
                "description": r.try_get::<Option<String>, _>("description").ok().flatten(),
                "price": min_price,
                "min_price": min_price,
                "max_price": max_price,
                "stock": total_stock,
                "total_stock": total_stock,
                "variants_count": variants_count,
                "delivery_type": r.try_get::<Option<String>, _>("delivery_type").ok().flatten(),
                "require_profile_type": r.try_get::<String, _>("require_profile_type").unwrap_or_else(|_| "none".to_string()),
                "thumbnail": r.try_get::<Option<String>, _>("thumbnail").ok().flatten(),
            })
        })
        .collect();

    let categories = get_public_categories(state).await.unwrap_or_default();
    let total_pages = ((total as f64) / (limit as f64)).ceil().max(1.0) as i64;

    Ok(json!({
        "items": items,
        "categories": categories,
        "pagination": {
            "total": total,
            "page": page,
            "limit": limit,
            "total_pages": total_pages,
        }
    }))
}

pub async fn get_product_by_code(state: &AppState, code_or_id: &str) -> AppResult<Value> {
    let pool = state.db()?;
    let numeric_id = code_or_id.parse::<i64>().ok();

    let row = if let Some(pid) = numeric_id {
        sqlx::query(
            r#"
            SELECT id, parent_id, product_code, category, name, variant_name, slug, description,
                   CAST(price AS DOUBLE) AS price, stock, delivery_type, COALESCE(require_profile_type, 'none') AS require_profile_type, thumbnail, sort_order
            FROM products WHERE (id = ? OR product_code = ?) AND status = 'active' LIMIT 1
            "#,
        )
        .bind(pid)
        .bind(code_or_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT id, parent_id, product_code, category, name, variant_name, slug, description,
                   CAST(price AS DOUBLE) AS price, stock, delivery_type, COALESCE(require_profile_type, 'none') AS require_profile_type, thumbnail, sort_order
            FROM products WHERE product_code = ? AND status = 'active' LIMIT 1
            "#,
        )
        .bind(code_or_id)
        .fetch_optional(pool)
        .await?
    };

    let Some(requested) = row else {
        return Err(AppError::not_found("Sản phẩm không tồn tại hoặc đã bị ẩn"));
    };

    let req_id: i64 = requested.try_get("id")?;
    let req_parent_id: Option<i64> = requested.try_get("parent_id").ok().flatten();
    let req_code: String = requested.try_get("product_code")?;

    // If a child variant was requested directly, resolve its parent so the UI gets the full variant selector
    let (parent_row, selected_variant_code) = if let Some(pid) = req_parent_id {
        let p = sqlx::query(
            r#"
            SELECT id, parent_id, product_code, category, name, variant_name, slug, description,
                   CAST(price AS DOUBLE) AS price, stock, delivery_type, COALESCE(require_profile_type, 'none') AS require_profile_type, thumbnail, sort_order
            FROM products WHERE id = ? AND status = 'active' LIMIT 1
            "#,
        )
        .bind(pid)
        .fetch_optional(pool)
        .await?;
        match p {
            Some(parent) => (parent, Some(req_code)),
            None => (requested, None),
        }
    } else {
        (requested, None)
    };

    let parent_id: i64 = parent_row.try_get("id")?;
    let variant_rows = sqlx::query(
        r#"
        SELECT id, parent_id, product_code, category, name, variant_name, slug, description,
               CAST(price AS DOUBLE) AS price, stock, delivery_type, COALESCE(require_profile_type, 'none') AS require_profile_type, thumbnail, sort_order
        FROM products
        WHERE parent_id = ? AND status = 'active'
        ORDER BY sort_order ASC, price ASC
        "#,
    )
    .bind(parent_id)
    .fetch_all(pool)
    .await?;

    let variants: Vec<ProductVariant> = variant_rows
        .into_iter()
        .map(|v| ProductVariant {
            id: v.try_get("id").unwrap_or(0),
            parent_id: v.try_get("parent_id").ok().flatten(),
            product_code: v.try_get("product_code").unwrap_or_default(),
            category: v.try_get("category").ok().flatten(),
            name: v.try_get("name").unwrap_or_default(),
            variant_name: v.try_get("variant_name").ok().flatten(),
            slug: v.try_get("slug").ok().flatten(),
            description: v.try_get("description").ok().flatten(),
            price: v.try_get("price").unwrap_or(0.0),
            stock: v.try_get("stock").unwrap_or(0),
            delivery_type: v.try_get("delivery_type").ok().flatten(),
            require_profile_type: v.try_get::<String, _>("require_profile_type").unwrap_or_else(|_| "none".to_string()),
            thumbnail: v.try_get("thumbnail").ok().flatten(),
            sort_order: v.try_get("sort_order").unwrap_or(0),
        })
        .collect();

    let own_price: f64 = parent_row.try_get("price").unwrap_or(0.0);
    let own_stock: i64 = parent_row.try_get("stock").unwrap_or(0);

    let (min_price, max_price, total_stock) = if variants.is_empty() {
        (own_price, own_price, own_stock)
    } else {
        let min_p = variants.iter().map(|v| v.price).fold(f64::INFINITY, f64::min);
        let max_p = variants.iter().map(|v| v.price).fold(0.0_f64, f64::max);
        let sum_s: i64 = variants.iter().map(|v| v.stock).sum();
        (min_p, max_p, sum_s)
    };

    Ok(json!({
        "id": parent_id,
        "requested_id": req_id,
        "selected_variant_code": selected_variant_code,
        "product_code": parent_row.try_get::<String, _>("product_code")?,
        "category": parent_row.try_get::<Option<String>, _>("category").ok().flatten(),
        "name": parent_row.try_get::<String, _>("name")?,
        "variant_name": parent_row.try_get::<Option<String>, _>("variant_name").ok().flatten(),
        "slug": parent_row.try_get::<Option<String>, _>("slug").ok().flatten(),
        "description": parent_row.try_get::<Option<String>, _>("description").ok().flatten(),
        "price": min_price,
        "min_price": min_price,
        "max_price": max_price,
        "stock": total_stock,
        "total_stock": total_stock,
        "variants_count": variants.len(),
        "variants": variants,
        "delivery_type": parent_row.try_get::<Option<String>, _>("delivery_type").ok().flatten(),
        "require_profile_type": parent_row.try_get::<String, _>("require_profile_type").unwrap_or_else(|_| "none".to_string()),
        "thumbnail": parent_row.try_get::<Option<String>, _>("thumbnail").ok().flatten(),
    }))
}

pub async fn get_admin_products(
    state: &AppState,
    page: i64,
    limit: i64,
    search: Option<&str>,
    category: Option<&str>,
    status: Option<&str>,
) -> AppResult<Value> {
    let pool = state.db()?;
    let page = page.max(1);
    let limit = limit.clamp(1, 200);
    let offset = (page - 1) * limit;

    let mut sql = String::from(
        r#"
        SELECT id, parent_id, product_code, category, name, variant_name, slug, description,
               CAST(price AS DOUBLE) AS price, stock, delivery_type, COALESCE(require_profile_type, 'none') AS require_profile_type, status, thumbnail, sort_order,
               provider_name, provider_product_id,
               CAST(COALESCE(provider_markup_percent, 10.00) AS DOUBLE) AS provider_markup_percent,
               CAST(provider_base_price AS DOUBLE) AS provider_base_price,
               provider_last_synced_at, created_at
        FROM products WHERE 1=1
        "#,
    );
    let mut count_sql = String::from("SELECT COUNT(id) AS total FROM products WHERE 1=1");

    if category.is_some() {
        sql.push_str(" AND category = ?");
        count_sql.push_str(" AND category = ?");
    }
    if status.is_some() {
        sql.push_str(" AND status = ?");
        count_sql.push_str(" AND status = ?");
    }
    if search.is_some() {
        sql.push_str(" AND (name LIKE ? OR product_code LIKE ? OR variant_name LIKE ?)");
        count_sql.push_str(" AND (name LIKE ? OR product_code LIKE ? OR variant_name LIKE ?)");
    }
    sql.push_str(" ORDER BY COALESCE(parent_id, id) ASC, parent_id IS NOT NULL ASC, sort_order ASC, id ASC LIMIT ? OFFSET ?");

    let mut q = sqlx::query(&sql);
    let mut cq = sqlx::query(&count_sql);

    if let Some(cat) = category {
        q = q.bind(cat);
        cq = cq.bind(cat);
    }
    if let Some(st) = status {
        q = q.bind(st);
        cq = cq.bind(st);
    }
    let search_pat = search.map(|s| format!("%{}%", s));
    if let Some(ref pat) = search_pat {
        q = q.bind(pat).bind(pat).bind(pat);
        cq = cq.bind(pat).bind(pat).bind(pat);
    }

    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    let count_row = cq.fetch_one(pool).await?;
    let total: i64 = count_row.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            let provider_last_synced_at: Option<DateTime<Utc>> = r.try_get("provider_last_synced_at").ok().flatten();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "parent_id": r.try_get::<Option<i64>, _>("parent_id").ok().flatten(),
                "product_code": r.try_get::<String, _>("product_code").unwrap_or_default(),
                "category": r.try_get::<Option<String>, _>("category").ok().flatten(),
                "name": r.try_get::<String, _>("name").unwrap_or_default(),
                "variant_name": r.try_get::<Option<String>, _>("variant_name").ok().flatten(),
                "slug": r.try_get::<Option<String>, _>("slug").ok().flatten(),
                "description": r.try_get::<Option<String>, _>("description").ok().flatten(),
                "price": r.try_get::<f64, _>("price").unwrap_or(0.0),
                "stock": r.try_get::<i64, _>("stock").unwrap_or(0),
                "delivery_type": r.try_get::<Option<String>, _>("delivery_type").ok().flatten(),
                "require_profile_type": r.try_get::<String, _>("require_profile_type").unwrap_or_else(|_| "none".to_string()),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "thumbnail": r.try_get::<Option<String>, _>("thumbnail").ok().flatten(),
                "sort_order": r.try_get::<i64, _>("sort_order").unwrap_or(0),
                "provider_name": r.try_get::<Option<String>, _>("provider_name").ok().flatten(),
                "provider_product_id": r.try_get::<Option<String>, _>("provider_product_id").ok().flatten(),
                "provider_markup_percent": r.try_get::<f64, _>("provider_markup_percent").unwrap_or(10.0),
                "provider_base_price": r.try_get::<Option<f64>, _>("provider_base_price").ok().flatten(),
                "provider_last_synced_at": provider_last_synced_at,
                "created_at": created_at,
            })
        })
        .collect();

    let total_pages = ((total as f64) / (limit as f64)).ceil().max(1.0) as i64;
    Ok(json!({
        "items": items,
        "pagination": { "total": total, "page": page, "limit": limit, "total_pages": total_pages }
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
    let parent_id = data.get("parent_id").and_then(Value::as_i64);
    let variant_name = data
        .get("variant_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let category = data.get("category").and_then(Value::as_str).unwrap_or("account");
    let description = data.get("description").and_then(Value::as_str);
    let price = data.get("price").and_then(Value::as_f64).unwrap_or(0.0);
    let stock = data.get("stock").and_then(Value::as_i64).unwrap_or(0).max(0);
    let delivery_type = data
        .get("delivery_type")
        .and_then(Value::as_str)
        .unwrap_or("manual");
    let require_profile_type = match data
        .get("require_profile_type")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .trim()
        .to_lowercase()
        .as_str()
    {
        "email" => "email",
        "account" => "account",
        "any" => "any",
        _ => "none",
    };
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("active");
    let thumbnail = data.get("thumbnail").and_then(Value::as_str);
    let sort_order = data.get("sort_order").and_then(Value::as_i64).unwrap_or(0);
    let provider_name = data
        .get("provider_name")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let provider_product_id = data
        .get("provider_product_id")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let provider_markup_percent = data
        .get("provider_markup_percent")
        .and_then(Value::as_f64)
        .unwrap_or(10.0)
        .clamp(0.0, 500.0);
    let provider_base_price = data.get("provider_base_price").and_then(Value::as_f64);

    let res = sqlx::query(
        r#"
        INSERT INTO products
        (parent_id, product_code, category, name, variant_name, slug, description, price, stock, delivery_type, require_profile_type, status, thumbnail, sort_order, provider_name, provider_product_id, provider_markup_percent, provider_base_price)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(parent_id)
    .bind(product_code)
    .bind(category)
    .bind(name)
    .bind(variant_name)
    .bind(&slug)
    .bind(description)
    .bind(price)
    .bind(stock)
    .bind(delivery_type)
    .bind(require_profile_type)
    .bind(status)
    .bind(thumbnail)
    .bind(sort_order)
    .bind(provider_name)
    .bind(provider_product_id)
    .bind(provider_markup_percent)
    .bind(provider_base_price)
    .execute(pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("Duplicate entry") {
            AppError::bad_request("Product Code hoặc Slug đã tồn tại")
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
        SELECT parent_id, product_code, category, name, variant_name, slug, description,
               CAST(price AS DOUBLE) AS price, stock, delivery_type, COALESCE(require_profile_type, 'none') AS require_profile_type, status, thumbnail, sort_order,
               provider_name, provider_product_id,
               CAST(COALESCE(provider_markup_percent, 10.00) AS DOUBLE) AS provider_markup_percent,
               CAST(provider_base_price AS DOUBLE) AS provider_base_price
        FROM products WHERE id = ?
        "#,
    )
    .bind(prod_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = existing else {
        return Err(AppError::bad_request("Sản phẩm không tồn tại"));
    };

    let parent_id = if data.contains_key("parent_id") {
        data.get("parent_id").and_then(Value::as_i64)
    } else {
        row.try_get("parent_id").ok().flatten()
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
    let variant_name = if data.contains_key("variant_name") {
        data.get("variant_name")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        row.try_get("variant_name").ok().flatten()
    };
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
    let existing_stock: i64 = row.try_get("stock").unwrap_or(0);
    let delivery_type = data
        .get("delivery_type")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| row.try_get("delivery_type").unwrap_or_else(|_| "manual".to_string()));
    let require_profile_type = if let Some(rpt) = data.get("require_profile_type").and_then(Value::as_str) {
        match rpt.trim().to_lowercase().as_str() {
            "email" => "email".to_string(),
            "account" => "account".to_string(),
            "any" => "any".to_string(),
            _ => "none".to_string(),
        }
    } else {
        row.try_get("require_profile_type").unwrap_or_else(|_| "none".to_string())
    };
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
    let provider_name = if data.contains_key("provider_name") {
        data.get("provider_name")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
    } else {
        row.try_get("provider_name").ok().flatten()
    };
    let provider_product_id = if data.contains_key("provider_product_id") {
        data.get("provider_product_id")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        row.try_get("provider_product_id").ok().flatten()
    };
    let provider_markup_percent = data
        .get("provider_markup_percent")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| row.try_get("provider_markup_percent").unwrap_or(10.0))
        .clamp(0.0, 500.0);
    let provider_base_price = if data.contains_key("provider_base_price") {
        data.get("provider_base_price").and_then(Value::as_f64)
    } else {
        row.try_get("provider_base_price").ok().flatten()
    };
    let stock = if delivery_type.eq_ignore_ascii_case("manual") {
        data.get("stock")
            .and_then(Value::as_i64)
            .map(|v| v.max(0))
            .unwrap_or(existing_stock)
    } else {
        existing_stock
    };

    sqlx::query(
        r#"
        UPDATE products
        SET parent_id = ?, product_code = ?, category = ?, name = ?, variant_name = ?,
            slug = ?, description = ?, price = ?, stock = ?, delivery_type = ?, require_profile_type = ?, status = ?, thumbnail = ?, sort_order = ?,
            provider_name = ?, provider_product_id = ?, provider_markup_percent = ?, provider_base_price = ?
        WHERE id = ?
        "#,
    )
    .bind(parent_id)
    .bind(product_code)
    .bind(category)
    .bind(name)
    .bind(variant_name)
    .bind(slug)
    .bind(description)
    .bind(price)
    .bind(stock)
    .bind(delivery_type)
    .bind(require_profile_type)
    .bind(status)
    .bind(thumbnail)
    .bind(sort_order)
    .bind(provider_name)
    .bind(provider_product_id)
    .bind(provider_markup_percent)
    .bind(provider_base_price)
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
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            let sold_at: Option<DateTime<Utc>> = r.try_get("sold_at").ok().flatten();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "identifier": r.try_get::<Option<String>, _>("identifier").ok().flatten(),
                "content": r.try_get::<Option<String>, _>("content").ok().flatten(),
                "status": r.try_get::<Option<String>, _>("status").ok().flatten(),
                "order_id": r.try_get::<Option<i64>, _>("order_id").ok().flatten(),
                "created_at": created_at,
                "sold_at": sold_at,
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
