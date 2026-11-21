use axum::http::HeaderMap;
use sqlx::Row;

use crate::{
    core::{
        auth::AuthUser,
        security::{generate_hex_token, sha256_hex},
    },
    error::AppResult,
    state::AppState,
};

pub fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
}

pub async fn create_session(
    state: &AppState,
    user_id: i64,
    headers: &HeaderMap,
    remember_me: bool,
) -> AppResult<String> {
    let pool = state.db()?;
    let session_token = generate_hex_token(32);
    let token_hash = sha256_hex(&session_token);
    let ip = extract_client_ip(headers);
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let days: i32 = if remember_me { 365 } else { 7 };

    sqlx::query(
        r#"
        INSERT INTO sessions (user_id, token_hash, ip, user_agent, remember_me, expires_at)
        VALUES (?, ?, ?, ?, ?, DATE_ADD(NOW(), INTERVAL ? DAY))
        "#,
    )
    .bind(user_id)
    .bind(&token_hash)
    .bind(ip)
    .bind(user_agent)
    .bind(remember_me)
    .bind(days)
    .execute(pool)
    .await?;

    Ok(session_token)
}

pub async fn verify_session(state: &AppState, session_token: &str) -> AppResult<Option<AuthUser>> {
    if session_token.is_empty() {
        return Ok(None);
    }

    let pool = state.db()?;
    let token_hash = sha256_hex(session_token);

    let row = sqlx::query(
        r#"
        SELECT user_id FROM sessions
        WHERE token_hash = ? AND revoked = FALSE AND expires_at > NOW()
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let user_id: i64 = row.try_get("user_id")?;

    // Best-effort update last_seen asynchronously without blocking auth path
    let pool_clone = pool.clone();
    let hash_clone = token_hash.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE sessions SET last_seen = NOW() WHERE token_hash = ?")
            .bind(hash_clone)
            .execute(&pool_clone)
            .await;
    });

    let user_row = sqlx::query("SELECT id, username, email, avatar FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

    let Some(user_row) = user_row else {
        return Ok(None);
    };

    let provider_rows =
        sqlx::query("SELECT provider FROM user_oauth_accounts WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(pool)
            .await?;

    let providers = provider_rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("provider").ok())
        .collect();

    Ok(Some(AuthUser {
        id: user_row.try_get("id")?,
        username: user_row.try_get::<Option<String>, _>("username")?.unwrap_or_default(),
        email: user_row.try_get("email")?,
        avatar: user_row.try_get("avatar")?,
        providers,
        is_static_admin: false,
    }))
}

pub async fn revoke_session(state: &AppState, session_token: &str) -> AppResult<()> {
    if session_token.is_empty() {
        return Ok(());
    }
    let pool = state.db()?;
    let token_hash = sha256_hex(session_token);
    sqlx::query("UPDATE sessions SET revoked = TRUE WHERE token_hash = ?")
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke_all_user_sessions(state: &AppState, user_id: i64) -> AppResult<()> {
    let pool = state.db()?;
    sqlx::query("UPDATE sessions SET revoked = TRUE WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}
