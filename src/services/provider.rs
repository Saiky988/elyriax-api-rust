use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    error::{AppError, AppResult},
    state::{AppState, CachedValue},
};

const CATALOG_CACHE_TTL_SECS: u64 = 300; // 5 minutes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCatalogItem {
    pub id: String,
    pub name: String,
    pub category_id: String,
    pub category_name: String,
    pub price: f64,
    pub amount: i64,
    pub min: i64,
    pub max: i64,
}

/// Calculates selling price from provider base price + markup percentage (e.g., 5% - 10%),
/// rounded up cleanly to the nearest 100 VND so profit margin is never eroded.
pub fn calculate_markup_price(base_price: f64, markup_percent: f64) -> f64 {
    if !base_price.is_finite() || base_price <= 0.0 {
        return 0.0;
    }
    let pct = markup_percent.clamp(0.0, 500.0);
    let raw = base_price * (1.0 + pct / 100.0);
    (raw / 100.0).ceil() * 100.0
}

fn parse_value_f64(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().replace(',', "").parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn parse_value_i64(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn parse_value_string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub async fn fetch_appmmo_catalog(
    state: &AppState,
    force_refresh: bool,
) -> AppResult<Vec<ProviderCatalogItem>> {
    let cache_key = "appmmo".to_string();
    if !force_refresh {
        if let Some(entry) = state.provider_catalog_cache.get(&cache_key) {
            if entry.is_valid() {
                if let Ok(items) = serde_json::from_value::<Vec<ProviderCatalogItem>>(entry.value.clone()) {
                    return Ok(items);
                }
            }
        }
    }

    let api_key = state.config.provider_appmmo_api_key.trim();
    if api_key.is_empty() {
        return Err(AppError::bad_request(
            "Chưa cấu hình PROVIDER_APPMMO_API_KEY trong .env",
        ));
    }

    let base_url = state.config.provider_appmmo_url.trim();
    let sep = if base_url.contains('?') { "&" } else { "?" };
    let url = format!("{base_url}{sep}api_key={api_key}");

    let resp = state
        .http
        .get(&url)
        .header("Accept", "application/json")
        .header("User-Agent", "Elyriax-API/3.5")
        .send()
        .await
        .map_err(|e| AppError::bad_gateway(format!("Không thể kết nối AppMMO provider: {e}")))?;

    if !resp.status().is_success() {
        return Err(AppError::bad_gateway(format!(
            "AppMMO provider trả về HTTP {}",
            resp.status()
        )));
    }

    let payload: Value = resp
        .json()
        .await
        .map_err(|e| AppError::bad_gateway(format!("Dữ liệu AppMMO không phải JSON hợp lệ: {e}")))?;

    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !status.eq_ignore_ascii_case("success") {
        let msg = payload
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("AppMMO trả về lỗi không xác định");
        return Err(AppError::bad_gateway(format!("AppMMO lỗi: {msg}")));
    }

    let mut items = Vec::new();
    if let Some(categories) = payload.get("categories").and_then(Value::as_array) {
        for cat in categories {
            let cat_id = parse_value_string(cat.get("id"));
            let cat_name = parse_value_string(cat.get("name"));
            let Some(products) = cat.get("products").and_then(Value::as_array) else {
                continue;
            };
            for prod in products {
                let id = parse_value_string(prod.get("id"));
                if id.is_empty() {
                    continue;
                }
                let name = parse_value_string(prod.get("name"));
                let price = parse_value_f64(prod.get("price"));
                let amount = parse_value_i64(prod.get("amount"));
                let min = parse_value_i64(prod.get("min")).max(1);
                let max = parse_value_i64(prod.get("max"));

                items.push(ProviderCatalogItem {
                    id,
                    name,
                    category_id: cat_id.clone(),
                    category_name: cat_name.clone(),
                    price,
                    amount,
                    min,
                    max,
                });
            }
        }
    }

    if let Ok(val) = serde_json::to_value(&items) {
        state.provider_catalog_cache.insert(
            cache_key,
            CachedValue::new(val, Duration::from_secs(CATALOG_CACHE_TTL_SECS)),
        );
    }

    Ok(items)
}

pub async fn list_providers(state: &AppState) -> AppResult<Value> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT provider_name, COUNT(id) AS linked_count
        FROM products
        WHERE provider_name IS NOT NULL AND provider_name <> ''
          AND provider_product_id IS NOT NULL AND provider_product_id <> ''
          AND status <> 'disabled'
        GROUP BY provider_name
        "#,
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut counts: HashMap<String, i64> = HashMap::new();
    for r in rows {
        if let Ok(name) = r.try_get::<String, _>("provider_name") {
            let cnt = r.try_get::<i64, _>("linked_count").unwrap_or(0);
            counts.insert(name.to_lowercase(), cnt);
        }
    }

    let appmmo_configured = !state.config.provider_appmmo_api_key.trim().is_empty();
    let last_run = state.scheduler.last_provider_sync_run.read().await.clone();
    let last_summary = state.scheduler.last_provider_sync_summary.read().await.clone();

    Ok(json!({
        "providers": [
            {
                "key": "appmmo",
                "name": "Shop AppMMO",
                "url": state.config.provider_appmmo_url,
                "configured": appmmo_configured,
                "linked_products": counts.get("appmmo").copied().unwrap_or(0),
                "default_markup_percent": 10.0,
                "sync_interval_hours": state.config.provider_sync_interval_hours,
            }
        ],
        "sync_interval_hours": state.config.provider_sync_interval_hours,
        "last_sync_run": last_run,
        "last_sync_summary": last_summary,
    }))
}

pub async fn get_provider_catalog_filtered(
    state: &AppState,
    provider: &str,
    search: Option<&str>,
    limit: usize,
    force_refresh: bool,
) -> AppResult<Value> {
    let key = provider.trim().to_lowercase();
    let all_items = match key.as_str() {
        "appmmo" => fetch_appmmo_catalog(state, force_refresh).await?,
        _ => {
            return Err(AppError::bad_request(format!(
                "Provider '{provider}' không được hỗ trợ"
            )))
        }
    };

    let total_catalog = all_items.len();
    let q = search.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
    let cap = limit.clamp(1, 300);

    let filtered: Vec<ProviderCatalogItem> = all_items
        .into_iter()
        .filter(|it| match &q {
            None => true,
            Some(term) => {
                it.id.to_lowercase().contains(term)
                    || it.name.to_lowercase().contains(term)
                    || it.category_name.to_lowercase().contains(term)
            }
        })
        .take(cap)
        .collect();

    Ok(json!({
        "provider": key,
        "total_catalog": total_catalog,
        "returned": filtered.len(),
        "items": filtered,
    }))
}

/// Syncs ONLY price (+markup %), provider_base_price, and provider_last_synced_at
/// for products manually linked by Admin. Never creates products or touches name/icon/description/delivery_type.
pub async fn sync_provider_prices(state: &AppState) -> AppResult<Value> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT id, product_code, name, variant_name,
               CAST(price AS DOUBLE) AS price,
               stock, delivery_type,
               provider_name, provider_product_id,
               CAST(COALESCE(provider_markup_percent, 10.00) AS DOUBLE) AS provider_markup_percent
        FROM products
        WHERE provider_name IS NOT NULL AND provider_name <> ''
          AND provider_product_id IS NOT NULL AND provider_product_id <> ''
          AND status <> 'disabled'
        "#,
    )
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(json!({
            "total_linked": 0,
            "updated": 0,
            "stock_updated": 0,
            "unchanged": 0,
            "missing": 0,
            "changes": [],
            "errors": [],
        }));
    }

    // Collect distinct providers used: map provider_product_id -> (base_price, stock_amount)
    let mut catalogs: HashMap<String, HashMap<String, (f64, i64)>> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();

    for r in &rows {
        let p_name = r
            .try_get::<String, _>("provider_name")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if p_name.is_empty() || catalogs.contains_key(&p_name) {
            continue;
        }
        match p_name.as_str() {
            "appmmo" => match fetch_appmmo_catalog(state, true).await {
                Ok(items) => {
                    let map: HashMap<String, (f64, i64)> = items
                        .into_iter()
                        .map(|it| (it.id, (it.price, it.amount.max(0))))
                        .collect();
                    catalogs.insert(p_name, map);
                }
                Err(e) => {
                    errors.push(format!("appmmo: {e}"));
                }
            },
            other => {
                errors.push(format!("Provider không hỗ trợ: {other}"));
            }
        }
    }

    let mut updated = 0usize;
    let mut stock_updated = 0usize;
    let mut unchanged = 0usize;
    let mut missing = 0usize;
    let mut changes: Vec<Value> = Vec::new();

    for r in rows {
        let id: i64 = r.try_get("id").unwrap_or(0);
        let product_code: String = r.try_get("product_code").unwrap_or_default();
        let name: String = r.try_get("name").unwrap_or_default();
        let variant_name: Option<String> = r.try_get("variant_name").ok().flatten();
        let old_price: f64 = r.try_get("price").unwrap_or(0.0);
        let old_stock: i64 = r.try_get("stock").unwrap_or(0);
        let delivery_type: String = r
            .try_get::<Option<String>, _>("delivery_type")
            .ok()
            .flatten()
            .unwrap_or_else(|| "manual".to_string());
        let p_name = r
            .try_get::<String, _>("provider_name")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let p_pid = r
            .try_get::<String, _>("provider_product_id")
            .unwrap_or_default()
            .trim()
            .to_string();
        let markup: f64 = r.try_get("provider_markup_percent").unwrap_or(10.0);

        let Some(cat_map) = catalogs.get(&p_name) else {
            continue;
        };

        let Some(&(base_price, provider_stock)) = cat_map.get(&p_pid) else {
            missing += 1;
            continue;
        };

        if base_price <= 0.0 {
            missing += 1;
            continue;
        }

        let new_price = calculate_markup_price(base_price, markup);
        let price_changed = (new_price - old_price).abs() >= 1.0;
        let is_manual = delivery_type.eq_ignore_ascii_case("manual");
        let new_stock = if is_manual { provider_stock } else { old_stock };
        let stock_changed = is_manual && new_stock != old_stock;

        if price_changed || stock_changed {
            sqlx::query(
                r#"
                UPDATE products
                SET price = ?, stock = ?, provider_base_price = ?, provider_last_synced_at = NOW()
                WHERE id = ?
                "#,
            )
            .bind(new_price)
            .bind(new_stock)
            .bind(base_price)
            .bind(id)
            .execute(pool)
            .await?;

            if price_changed {
                updated += 1;
            }
            if stock_changed {
                stock_updated += 1;
            }
            changes.push(json!({
                "id": id,
                "product_code": product_code,
                "name": name,
                "variant_name": variant_name,
                "provider_name": p_name,
                "provider_product_id": p_pid,
                "base_price": base_price,
                "markup_percent": markup,
                "old_price": old_price,
                "new_price": new_price,
                "old_stock": old_stock,
                "new_stock": new_stock,
            }));
        } else {
            sqlx::query(
                r#"
                UPDATE products
                SET provider_base_price = ?, provider_last_synced_at = NOW()
                WHERE id = ?
                "#,
            )
            .bind(base_price)
            .bind(id)
            .execute(pool)
            .await?;
            unchanged += 1;
        }
    }

    Ok(json!({
        "total_linked": changes.len() + unchanged + missing,
        "updated": updated,
        "stock_updated": stock_updated,
        "unchanged": unchanged,
        "missing": missing,
        "changes": changes,
        "errors": errors,
    }))
}
