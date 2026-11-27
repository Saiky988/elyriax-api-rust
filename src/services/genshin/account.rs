use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::{
    core::security::{decrypt_cookie, encrypt_cookie},
    error::{AppError, AppResult},
    services::genshin::stats::get_role_and_stats,
    state::AppState,
};

#[derive(Debug, Clone)]
pub struct ReadyGenshinPayload {
    pub account_id: i64,
    pub cookie: String,
    pub uid: String,
    pub server: String,
    pub nickname: String,
}

#[derive(Debug, Clone)]
pub struct AutoAccountRow {
    pub id: i64,
    pub user_id: i64,
    pub nickname: String,
    pub uid: String,
    pub server: String,
    pub cookie_encrypted: String,
    pub last_checkin_at: Option<DateTime<Utc>>,
    pub email: Option<String>,
}

pub async fn add_account(
    state: &AppState,
    user_id: i64,
    cookie: &str,
    uid: &str,
    server: &str,
) -> AppResult<Value> {
    let stats_res = get_role_and_stats(state, cookie, uid, server).await?;
    if stats_res.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(AppError::bad_request(
            "Xác thực HoYoLAB thất bại. Cookie, UID hoặc Server không hợp lệ.",
        ));
    }

    let nickname = stats_res
        .pointer("/role/nickname")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let encrypted_cookie = encrypt_cookie(cookie, &state.config.genshin_cookie_secret)?;

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let count_row = sqlx::query(
        "SELECT COUNT(id) AS cnt FROM genshin_accounts WHERE user_id = ? AND status != 'deleted'",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    let count: i64 = count_row.try_get("cnt").unwrap_or(0);
    let is_default = count == 0;

    let res = sqlx::query(
        r#"
        INSERT INTO genshin_accounts (user_id, nickname, uid, server, cookie, is_default)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(&nickname)
    .bind(uid)
    .bind(server)
    .bind(&encrypted_cookie)
    .bind(is_default)
    .execute(&mut *tx)
    .await?;

    let account_id = res.last_insert_id() as i64;
    tx.commit().await?;

    Ok(json!({
        "id": account_id,
        "nickname": nickname,
        "uid": uid,
        "server": server,
        "is_default": is_default,
    }))
}

pub async fn get_accounts(state: &AppState, user_id: i64) -> AppResult<Vec<Value>> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT id, nickname, uid, server, is_default, auto_checkin, auto_redeem, auto_daily_note, status
        FROM genshin_accounts
        WHERE user_id = ? AND status != 'deleted'
        ORDER BY is_default DESC, created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "nickname": r.try_get::<Option<String>, _>("nickname").ok().flatten(),
                "uid": r.try_get::<String, _>("uid").unwrap_or_default(),
                "server": r.try_get::<String, _>("server").unwrap_or_default(),
                "is_default": r.try_get::<bool, _>("is_default").unwrap_or(false),
                "auto_checkin": r.try_get::<bool, _>("auto_checkin").unwrap_or(false),
                "auto_redeem": r.try_get::<bool, _>("auto_redeem").unwrap_or(false),
                "auto_daily_note": r.try_get::<bool, _>("auto_daily_note").unwrap_or(false),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
            })
        })
        .collect())
}

pub async fn get_account_by_id(
    state: &AppState,
    user_id: i64,
    account_id: i64,
) -> AppResult<Value> {
    let pool = state.db()?;
    let row = sqlx::query(
        r#"
        SELECT id, nickname, uid, server, is_default, auto_checkin, auto_redeem, auto_daily_note,
               last_checkin_at, last_redeem_at, last_daily_note_at, status
        FROM genshin_accounts
        WHERE id = ? AND user_id = ? AND status != 'deleted'
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Err(AppError::not_found("Tài khoản không tồn tại hoặc đã bị xóa."));
    };

    let last_checkin: Option<DateTime<Utc>> = r.try_get("last_checkin_at").ok().flatten();
    let last_redeem: Option<DateTime<Utc>> = r.try_get("last_redeem_at").ok().flatten();
    let last_note: Option<DateTime<Utc>> = r.try_get("last_daily_note_at").ok().flatten();

    Ok(json!({
        "id": r.try_get::<i64, _>("id")?,
        "nickname": r.try_get::<Option<String>, _>("nickname").ok().flatten(),
        "uid": r.try_get::<String, _>("uid")?,
        "server": r.try_get::<String, _>("server")?,
        "is_default": r.try_get::<bool, _>("is_default").unwrap_or(false),
        "auto_checkin": r.try_get::<bool, _>("auto_checkin").unwrap_or(false),
        "auto_redeem": r.try_get::<bool, _>("auto_redeem").unwrap_or(false),
        "auto_daily_note": r.try_get::<bool, _>("auto_daily_note").unwrap_or(false),
        "last_checkin_at": last_checkin,
        "last_redeem_at": last_redeem,
        "last_daily_note_at": last_note,
        "status": r.try_get::<String, _>("status")?,
    }))
}

pub async fn update_account(
    state: &AppState,
    user_id: i64,
    account_id: i64,
    data: &Map<String, Value>,
) -> AppResult<()> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let acc = sqlx::query(
        r#"
        SELECT id, auto_checkin, auto_redeem, auto_daily_note, is_default, status
        FROM genshin_accounts WHERE id = ? AND user_id = ? FOR UPDATE
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = acc else {
        return Err(AppError::bad_request("Tài khoản không tồn tại."));
    };

    let is_default = data
        .get("is_default")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| row.try_get("is_default").unwrap_or(false));

    if data.get("is_default").and_then(Value::as_bool) == Some(true) {
        sqlx::query("UPDATE genshin_accounts SET is_default = false WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }

    let auto_checkin = data
        .get("auto_checkin")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| row.try_get("auto_checkin").unwrap_or(false));
    let auto_redeem = data
        .get("auto_redeem")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| row.try_get("auto_redeem").unwrap_or(false));
    let auto_daily_note = data
        .get("auto_daily_note")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| row.try_get("auto_daily_note").unwrap_or(false));
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| row.try_get("status").unwrap_or_else(|_| "active".to_string()));

    sqlx::query(
        r#"
        UPDATE genshin_accounts
        SET auto_checkin = ?, auto_redeem = ?, auto_daily_note = ?, is_default = ?, status = ?
        WHERE id = ? AND user_id = ?
        "#,
    )
    .bind(auto_checkin)
    .bind(auto_redeem)
    .bind(auto_daily_note)
    .bind(is_default)
    .bind(status)
    .bind(account_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn delete_account(state: &AppState, user_id: i64, account_id: i64) -> AppResult<()> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let acc = sqlx::query(
        "SELECT is_default FROM genshin_accounts WHERE id = ? AND user_id = ? AND status != 'deleted' FOR UPDATE",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = acc else {
        return Err(AppError::bad_request("Tài khoản không tồn tại."));
    };
    let was_default: bool = row.try_get("is_default").unwrap_or(false);

    sqlx::query("UPDATE genshin_accounts SET status = 'deleted', is_default = false WHERE id = ?")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;

    if was_default {
        sqlx::query(
            r#"
            UPDATE genshin_accounts SET is_default = true
            WHERE user_id = ? AND status = 'active'
            ORDER BY created_at DESC LIMIT 1
            "#,
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn get_ready_genshin_payload(
    state: &AppState,
    user_id: i64,
    account_id: Option<i64>,
) -> AppResult<ReadyGenshinPayload> {
    let pool = state.db()?;
    let row = if let Some(acc_id) = account_id {
        sqlx::query(
            "SELECT id, cookie, uid, server, nickname FROM genshin_accounts WHERE id = ? AND user_id = ? AND status = 'active'",
        )
        .bind(acc_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT id, cookie, uid, server, nickname FROM genshin_accounts WHERE user_id = ? AND is_default = TRUE AND status = 'active'",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await?
    };

    let Some(row) = row else {
        return Err(AppError::bad_request(
            "Chưa chọn tài khoản Genshin khả dụng. Vui lòng thêm tài khoản hoặc chọn account_id!",
        ));
    };

    let encrypted: String = row.try_get("cookie")?;
    let cookie = decrypt_cookie(&encrypted, &state.config.genshin_cookie_secret)?;

    Ok(ReadyGenshinPayload {
        account_id: row.try_get("id")?,
        cookie,
        uid: row.try_get("uid")?,
        server: row.try_get("server")?,
        nickname: row.try_get::<Option<String>, _>("nickname")?.unwrap_or_default(),
    })
}

pub async fn get_auto_checkin_accounts(state: &AppState) -> AppResult<Vec<AutoAccountRow>> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT ga.id, ga.user_id, ga.nickname, ga.uid, ga.server, ga.cookie, ga.last_checkin_at, u.email
        FROM genshin_accounts ga
        INNER JOIN users u ON u.id = ga.user_id
        WHERE ga.status = 'active' AND ga.auto_checkin = 1 AND u.status = 'active'
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AutoAccountRow {
            id: r.try_get("id").unwrap_or(0),
            user_id: r.try_get("user_id").unwrap_or(0),
            nickname: r.try_get::<Option<String>, _>("nickname").ok().flatten().unwrap_or_default(),
            uid: r.try_get("uid").unwrap_or_default(),
            server: r.try_get("server").unwrap_or_default(),
            cookie_encrypted: r.try_get("cookie").unwrap_or_default(),
            last_checkin_at: r.try_get("last_checkin_at").ok().flatten(),
            email: r.try_get("email").ok().flatten(),
        })
        .collect())
}

pub async fn get_auto_redeem_accounts(state: &AppState) -> AppResult<Vec<AutoAccountRow>> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT ga.id, ga.user_id, ga.nickname, ga.uid, ga.server, ga.cookie, ga.last_checkin_at, u.email
        FROM genshin_accounts ga
        INNER JOIN users u ON u.id = ga.user_id
        WHERE ga.status = 'active' AND ga.auto_redeem = 1 AND u.status = 'active'
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AutoAccountRow {
            id: r.try_get("id").unwrap_or(0),
            user_id: r.try_get("user_id").unwrap_or(0),
            nickname: r.try_get::<Option<String>, _>("nickname").ok().flatten().unwrap_or_default(),
            uid: r.try_get("uid").unwrap_or_default(),
            server: r.try_get("server").unwrap_or_default(),
            cookie_encrypted: r.try_get("cookie").unwrap_or_default(),
            last_checkin_at: r.try_get("last_checkin_at").ok().flatten(),
            email: r.try_get("email").ok().flatten(),
        })
        .collect())
}

pub async fn get_auto_daily_note_accounts(state: &AppState) -> AppResult<Vec<AutoAccountRow>> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT ga.id, ga.user_id, ga.nickname, ga.uid, ga.server, ga.cookie, ga.last_checkin_at, u.email
        FROM genshin_accounts ga
        INNER JOIN users u ON u.id = ga.user_id
        WHERE ga.status = 'active' AND ga.auto_daily_note = 1 AND u.status = 'active'
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AutoAccountRow {
            id: r.try_get("id").unwrap_or(0),
            user_id: r.try_get("user_id").unwrap_or(0),
            nickname: r.try_get::<Option<String>, _>("nickname").ok().flatten().unwrap_or_default(),
            uid: r.try_get("uid").unwrap_or_default(),
            server: r.try_get("server").unwrap_or_default(),
            cookie_encrypted: r.try_get("cookie").unwrap_or_default(),
            last_checkin_at: r.try_get("last_checkin_at").ok().flatten(),
            email: r.try_get("email").ok().flatten(),
        })
        .collect())
}

pub async fn update_checkin_status(state: &AppState, account_id: i64, status: &str) -> AppResult<()> {
    let pool = state.db()?;
    sqlx::query("UPDATE genshin_accounts SET last_checkin_at = NOW(), last_checkin_status = ? WHERE id = ?")
        .bind(status)
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_redeem_timestamp(state: &AppState, account_id: i64) -> AppResult<()> {
    let pool = state.db()?;
    sqlx::query("UPDATE genshin_accounts SET last_redeem_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_daily_note_timestamp(state: &AppState, account_id: i64) -> AppResult<()> {
    let pool = state.db()?;
    sqlx::query("UPDATE genshin_accounts SET last_daily_note_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}
