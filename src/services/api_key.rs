use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    core::security::{generate_hex_token, sha256_hex},
    error::AppResult,
    state::AppState,
};

pub const API_KEY_PREFIX: &str = "sk_syr_";

pub async fn get_or_create_api_key(state: &AppState, user_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let existing = sqlx::query("SELECT created_at, last_used FROM api_keys WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

    if let Some(row) = existing {
        let created_at: Option<DateTime<Utc>> = row.try_get("created_at").ok().flatten();
        let last_used: Option<DateTime<Utc>> = row.try_get("last_used").ok().flatten();
        return Ok(json!({
            "success": true,
            "message": "Bạn đã tạo API Key rồi.",
            "data": {
                "created_at": created_at,
                "last_used": last_used,
            }
        }));
    }

    let raw_key = format!("{}{}", API_KEY_PREFIX, generate_hex_token(24));
    let hashed_key = sha256_hex(&raw_key);

    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind("Default Key")
        .bind(&hashed_key)
        .execute(pool)
        .await?;

    Ok(json!({
        "success": true,
        "api_key": raw_key,
    }))
}

pub async fn get_api_key_info(state: &AppState, user_id: i64) -> AppResult<Option<Value>> {
    let pool = state.db()?;
    let row = sqlx::query("SELECT created_at, last_used FROM api_keys WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| {
        let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
        let last_used: Option<DateTime<Utc>> = r.try_get("last_used").ok().flatten();
        json!({
            "created_at": created_at,
            "last_used": last_used,
        })
    }))
}

pub async fn delete_api_key(state: &AppState, user_id: i64) -> AppResult<bool> {
    let pool = state.db()?;
    let res = sqlx::query("DELETE FROM api_keys WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn validate_api_key(state: &AppState, raw_key: &str) -> AppResult<Option<i64>> {
    if !raw_key.starts_with(API_KEY_PREFIX) {
        return Ok(None);
    }

    let pool = state.db()?;
    let hashed_key = sha256_hex(raw_key);
    let row = sqlx::query("SELECT user_id FROM api_keys WHERE key_hash = ?")
        .bind(&hashed_key)
        .fetch_optional(pool)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let user_id: i64 = row.try_get("user_id")?;

    let pool_clone = pool.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE api_keys SET last_used = CURRENT_TIMESTAMP WHERE user_id = ?")
            .bind(user_id)
            .execute(&pool_clone)
            .await;
    });

    Ok(Some(user_id))
}
