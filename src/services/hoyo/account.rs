use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::{
    core::security::{decrypt_cookie, encrypt_cookie},
    error::{AppError, AppResult},
    services::{
        genshin::account as gi_account,
        hoyo::HoyoGame,
    },
    state::AppState,
};

const HOYO_ROLE_URL: &str =
    "https://api-os-takumi.hoyoverse.com/binding/api/getUserGameRolesByCookie";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct ReadyHoyoPayload {
    pub account_id: i64,
    pub game: HoyoGame,
    pub cookie: String,
    pub uid: String,
    pub server: String,
    pub nickname: String,
}

#[derive(Debug, Clone)]
pub struct AutoHoyoAccountRow {
    pub id: i64,
    pub user_id: i64,
    pub game: HoyoGame,
    pub nickname: String,
    pub uid: String,
    pub server: String,
    pub cookie_encrypted: String,
    pub last_checkin_at: Option<NaiveDateTime>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredRole {
    pub game: String,
    pub game_name: String,
    pub uid: String,
    pub nickname: String,
    pub server: String,
    pub server_name: String,
    pub level: i64,
}

/// Auto-create `hoyo_accounts` table for HSR & ZZZ at startup without modifying `genshin_accounts`.
pub async fn ensure_hoyo_accounts_table(state: &AppState) -> AppResult<()> {
    let pool = state.db()?;
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS hoyo_accounts (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            user_id BIGINT NOT NULL,
            game VARCHAR(16) NOT NULL,
            nickname VARCHAR(128) DEFAULT '',
            uid VARCHAR(32) NOT NULL,
            server VARCHAR(32) NOT NULL,
            cookie TEXT NOT NULL,
            is_default BOOLEAN NOT NULL DEFAULT FALSE,
            auto_checkin BOOLEAN NOT NULL DEFAULT TRUE,
            auto_redeem BOOLEAN NOT NULL DEFAULT TRUE,
            auto_daily_note BOOLEAN NOT NULL DEFAULT FALSE,
            last_checkin_at DATETIME NULL,
            last_checkin_status VARCHAR(32) NULL,
            last_redeem_at DATETIME NULL,
            last_daily_note_at DATETIME NULL,
            status VARCHAR(32) NOT NULL DEFAULT 'active',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
            INDEX idx_hoyo_user_game (user_id, game, status)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn fetch_roles_for_game(
    state: &AppState,
    game: HoyoGame,
    cookie: &str,
) -> AppResult<Vec<DiscoveredRole>> {
    let role_data: Value = state
        .http
        .get(format!("{}?game_biz={}", HOYO_ROLE_URL, game.game_biz()))
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send()
        .await?
        .json()
        .await?;

    let retcode = role_data.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    if retcode == -100 || retcode == -1071 || retcode == -10001 {
        return Err(AppError::bad_request(
            "Cookie HoYoLAB không hợp lệ hoặc đã hết hạn.",
        ));
    }
    if retcode != 0 {
        return Ok(vec![]);
    }

    let roles = role_data
        .pointer("/data/list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(roles
        .into_iter()
        .filter_map(|r| {
            let uid = r.get("game_uid")?.as_str()?.to_string();
            let region = r.get("region")?.as_str()?.to_string();
            let nickname = r
                .get("nickname")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let level = r.get("level").and_then(Value::as_i64).unwrap_or(0);
            Some(DiscoveredRole {
                game: game.slug().to_string(),
                game_name: game.display_name().to_string(),
                uid,
                nickname,
                server_name: HoyoGame::server_display_name(&region).to_string(),
                server: region,
                level,
            })
        })
        .collect())
}

/// Discover ALL game roles across Genshin Impact, Honkai: Star Rail, and Zenless Zone Zero
/// concurrently using a SINGLE HoYoLAB cookie.
pub async fn discover_all_roles_from_cookie(
    state: &AppState,
    cookie: &str,
    filter_games: Option<&[HoyoGame]>,
) -> AppResult<Vec<DiscoveredRole>> {
    if cookie.trim().is_empty() {
        return Err(AppError::bad_request("Vui lòng cung cấp cookie HoYoLAB."));
    }

    let (gi, hsr, zzz) = tokio::try_join!(
        fetch_roles_for_game(state, HoyoGame::Genshin, cookie),
        fetch_roles_for_game(state, HoyoGame::Hsr, cookie),
        fetch_roles_for_game(state, HoyoGame::Zzz, cookie),
    )?;

    let mut all = Vec::new();
    let allow = |g: HoyoGame| {
        filter_games
            .map(|list| list.is_empty() || list.contains(&g))
            .unwrap_or(true)
    };

    if allow(HoyoGame::Genshin) {
        all.extend(gi);
    }
    if allow(HoyoGame::Hsr) {
        all.extend(hsr);
    }
    if allow(HoyoGame::Zzz) {
        all.extend(zzz);
    }

    Ok(all)
}

/// Link or refresh ALL discovered Genshin + HSR + ZZZ accounts for a user using ONE cookie.
/// Existing accounts (matching user_id + uid + game) have their encrypted cookie & nickname refreshed automatically.
pub async fn link_all_games_with_single_cookie(
    state: &AppState,
    user_id: i64,
    cookie: &str,
    auto_checkin: bool,
    auto_redeem: bool,
    auto_daily_note: bool,
    filter_games: Option<&[HoyoGame]>,
) -> AppResult<Value> {
    let roles = discover_all_roles_from_cookie(state, cookie, filter_games).await?;
    if roles.is_empty() {
        return Err(AppError::bad_request(
            "Không tìm thấy nhân vật Genshin, HSR hoặc ZZZ nào gắn với Cookie này.",
        ));
    }

    let encrypted_cookie = encrypt_cookie(cookie, &state.config.genshin_cookie_secret)?;
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let mut linked_accounts = Vec::new();

    for role in &roles {
        let game = HoyoGame::from_slug(&role.game)?;

        if game == HoyoGame::Genshin {
            let existing = sqlx::query(
                "SELECT id FROM genshin_accounts WHERE user_id = ? AND uid = ? AND status != 'deleted' FOR UPDATE",
            )
            .bind(user_id)
            .bind(&role.uid)
            .fetch_optional(&mut *tx)
            .await?;

            let (acc_id, action) = if let Some(row) = existing {
                let id: i64 = row.try_get("id")?;
                sqlx::query(
                    r#"
                    UPDATE genshin_accounts
                    SET cookie = ?, nickname = ?, server = ?, status = 'active',
                        auto_checkin = ?, auto_redeem = ?, auto_daily_note = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&encrypted_cookie)
                .bind(&role.nickname)
                .bind(&role.server)
                .bind(auto_checkin)
                .bind(auto_redeem)
                .bind(auto_daily_note)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                (id, "updated")
            } else {
                let cnt_row = sqlx::query(
                    "SELECT COUNT(id) AS cnt FROM genshin_accounts WHERE user_id = ? AND status != 'deleted'",
                )
                .bind(user_id)
                .fetch_one(&mut *tx)
                .await?;
                let is_default = cnt_row.try_get::<i64, _>("cnt").unwrap_or(0) == 0;

                let res = sqlx::query(
                    r#"
                    INSERT INTO genshin_accounts
                    (user_id, nickname, uid, server, cookie, is_default, auto_checkin, auto_redeem, auto_daily_note)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(user_id)
                .bind(&role.nickname)
                .bind(&role.uid)
                .bind(&role.server)
                .bind(&encrypted_cookie)
                .bind(is_default)
                .bind(auto_checkin)
                .bind(auto_redeem)
                .bind(auto_daily_note)
                .execute(&mut *tx)
                .await?;
                (res.last_insert_id() as i64, "created")
            };

            linked_accounts.push(json!({
                "id": acc_id,
                "action": action,
                "game": "genshin",
                "game_name": "Genshin Impact",
                "uid": role.uid,
                "nickname": role.nickname,
                "server": role.server,
                "server_name": role.server_name,
                "level": role.level,
            }));
        } else {
            let existing = sqlx::query(
                "SELECT id FROM hoyo_accounts WHERE user_id = ? AND game = ? AND uid = ? AND status != 'deleted' FOR UPDATE",
            )
            .bind(user_id)
            .bind(game.slug())
            .bind(&role.uid)
            .fetch_optional(&mut *tx)
            .await?;

            let (acc_id, action) = if let Some(row) = existing {
                let id: i64 = row.try_get("id")?;
                sqlx::query(
                    r#"
                    UPDATE hoyo_accounts
                    SET cookie = ?, nickname = ?, server = ?, status = 'active',
                        auto_checkin = ?, auto_redeem = ?, auto_daily_note = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&encrypted_cookie)
                .bind(&role.nickname)
                .bind(&role.server)
                .bind(auto_checkin)
                .bind(auto_redeem)
                .bind(auto_daily_note)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                (id, "updated")
            } else {
                let cnt_row = sqlx::query(
                    "SELECT COUNT(id) AS cnt FROM hoyo_accounts WHERE user_id = ? AND game = ? AND status != 'deleted'",
                )
                .bind(user_id)
                .bind(game.slug())
                .fetch_one(&mut *tx)
                .await?;
                let is_default = cnt_row.try_get::<i64, _>("cnt").unwrap_or(0) == 0;

                let res = sqlx::query(
                    r#"
                    INSERT INTO hoyo_accounts
                    (user_id, game, nickname, uid, server, cookie, is_default, auto_checkin, auto_redeem, auto_daily_note)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(user_id)
                .bind(game.slug())
                .bind(&role.nickname)
                .bind(&role.uid)
                .bind(&role.server)
                .bind(&encrypted_cookie)
                .bind(is_default)
                .bind(auto_checkin)
                .bind(auto_redeem)
                .bind(auto_daily_note)
                .execute(&mut *tx)
                .await?;
                (res.last_insert_id() as i64, "created")
            };

            linked_accounts.push(json!({
                "id": acc_id,
                "action": action,
                "game": game.slug(),
                "game_name": game.display_name(),
                "uid": role.uid,
                "nickname": role.nickname,
                "server": role.server,
                "server_name": role.server_name,
                "level": role.level,
            }));
        }
    }

    tx.commit().await?;

    Ok(json!({
        "total_linked": linked_accounts.len(),
        "accounts": linked_accounts,
    }))
}

/// Update the stored HoYoLAB cookie across ALL active accounts (Genshin + HSR + ZZZ) of a user in one shot.
pub async fn update_shared_cookie_for_user(
    state: &AppState,
    user_id: i64,
    new_cookie: &str,
) -> AppResult<Value> {
    let roles = discover_all_roles_from_cookie(state, new_cookie, None).await?;
    if roles.is_empty() {
        return Err(AppError::bad_request(
            "Cookie mới không hợp lệ hoặc không có nhân vật HoYoverse nào.",
        ));
    }

    let encrypted = encrypt_cookie(new_cookie, &state.config.genshin_cookie_secret)?;
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let gi_res = sqlx::query(
        "UPDATE genshin_accounts SET cookie = ?, status = 'active' WHERE user_id = ? AND status != 'deleted'",
    )
    .bind(&encrypted)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    let hoyo_res = sqlx::query(
        "UPDATE hoyo_accounts SET cookie = ?, status = 'active' WHERE user_id = ? AND status != 'deleted'",
    )
    .bind(&encrypted)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(json!({
        "genshin_updated": gi_res.rows_affected(),
        "hsr_zzz_updated": hoyo_res.rows_affected(),
        "total_updated": gi_res.rows_affected() + hoyo_res.rows_affected(),
        "discovered_roles": roles,
    }))
}

/// Get all accounts for a user across Genshin, HSR, and ZZZ in one unified payload.
pub async fn get_all_user_accounts(state: &AppState, user_id: i64) -> AppResult<Value> {
    let (gi, hsr, zzz) = tokio::try_join!(
        get_accounts(state, HoyoGame::Genshin, user_id),
        get_accounts(state, HoyoGame::Hsr, user_id),
        get_accounts(state, HoyoGame::Zzz, user_id),
    )?;

    Ok(json!({
        "total": gi.len() + hsr.len() + zzz.len(),
        "genshin": gi,
        "hsr": hsr,
        "zzz": zzz,
    }))
}

/// Get decrypted ready payloads for ALL active accounts of a user across Genshin, HSR, and ZZZ.
pub async fn get_all_ready_payloads_for_user(
    state: &AppState,
    user_id: i64,
) -> AppResult<Vec<ReadyHoyoPayload>> {
    let pool = state.db()?;
    let mut list = Vec::new();

    let gi_rows = sqlx::query(
        "SELECT id, cookie, uid, server, nickname FROM genshin_accounts WHERE user_id = ? AND status = 'active'",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    for r in gi_rows {
        let enc: String = r.try_get("cookie")?;
        if let Ok(cookie) = decrypt_cookie(&enc, &state.config.genshin_cookie_secret) {
            list.push(ReadyHoyoPayload {
                account_id: r.try_get("id")?,
                game: HoyoGame::Genshin,
                cookie,
                uid: r.try_get("uid")?,
                server: r.try_get("server")?,
                nickname: r.try_get::<Option<String>, _>("nickname")?.unwrap_or_default(),
            });
        }
    }

    let hoyo_rows = sqlx::query(
        "SELECT id, game, cookie, uid, server, nickname FROM hoyo_accounts WHERE user_id = ? AND status = 'active'",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    for r in hoyo_rows {
        let g_str: String = r.try_get("game").unwrap_or_default();
        let Ok(game) = HoyoGame::from_slug(&g_str) else {
            continue;
        };
        let enc: String = r.try_get("cookie")?;
        if let Ok(cookie) = decrypt_cookie(&enc, &state.config.genshin_cookie_secret) {
            list.push(ReadyHoyoPayload {
                account_id: r.try_get("id")?,
                game,
                cookie,
                uid: r.try_get("uid")?,
                server: r.try_get("server")?,
                nickname: r.try_get::<Option<String>, _>("nickname")?.unwrap_or_default(),
            });
        }
    }

    Ok(list)
}

async fn verify_hoyo_role(
    state: &AppState,
    game: HoyoGame,
    cookie: &str,
    uid: &str,
    region: &str,
) -> AppResult<String> {
    let roles = fetch_roles_for_game(state, game, cookie).await?;
    let found = roles
        .into_iter()
        .find(|r| r.uid == uid || r.server == region);

    let Some(role) = found else {
        return Err(AppError::bad_request(format!(
            "Không tìm thấy nhân vật {} với UID {} trên server {}.",
            game.display_name(),
            uid,
            region
        )));
    };

    Ok(role.nickname)
}

pub async fn add_account(
    state: &AppState,
    game: HoyoGame,
    user_id: i64,
    cookie: &str,
    uid: &str,
    server: &str,
) -> AppResult<Value> {
    let region = game
        .normalize_region(server)
        .ok_or_else(|| AppError::bad_request(format!("Server không hợp lệ: {}", server)))?;

    if game == HoyoGame::Genshin {
        return gi_account::add_account(state, user_id, cookie, uid, region).await;
    }

    let nickname = verify_hoyo_role(state, game, cookie, uid, region).await?;
    let encrypted_cookie = encrypt_cookie(cookie, &state.config.genshin_cookie_secret)?;

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let count_row = sqlx::query(
        "SELECT COUNT(id) AS cnt FROM hoyo_accounts WHERE user_id = ? AND game = ? AND status != 'deleted'",
    )
    .bind(user_id)
    .bind(game.slug())
    .fetch_one(&mut *tx)
    .await?;
    let count: i64 = count_row.try_get("cnt").unwrap_or(0);
    let is_default = count == 0;

    let res = sqlx::query(
        r#"
        INSERT INTO hoyo_accounts (user_id, game, nickname, uid, server, cookie, is_default)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(game.slug())
    .bind(&nickname)
    .bind(uid)
    .bind(region)
    .bind(&encrypted_cookie)
    .bind(is_default)
    .execute(&mut *tx)
    .await?;

    let account_id = res.last_insert_id() as i64;
    tx.commit().await?;

    Ok(json!({
        "id": account_id,
        "game": game.slug(),
        "nickname": nickname,
        "uid": uid,
        "server": region,
        "is_default": is_default,
    }))
}

pub async fn get_accounts(
    state: &AppState,
    game: HoyoGame,
    user_id: i64,
) -> AppResult<Vec<Value>> {
    if game == HoyoGame::Genshin {
        return gi_account::get_accounts(state, user_id).await;
    }

    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT id, game, nickname, uid, server, is_default, auto_checkin, auto_redeem, auto_daily_note, status
        FROM hoyo_accounts
        WHERE user_id = ? AND game = ? AND status != 'deleted'
        ORDER BY is_default DESC, created_at DESC
        "#,
    )
    .bind(user_id)
    .bind(game.slug())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "game": r.try_get::<String, _>("game").unwrap_or_else(|_| game.slug().to_string()),
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
    game: HoyoGame,
    user_id: i64,
    account_id: i64,
) -> AppResult<Value> {
    if game == HoyoGame::Genshin {
        return gi_account::get_account_by_id(state, user_id, account_id).await;
    }

    let pool = state.db()?;
    let row = sqlx::query(
        r#"
        SELECT id, game, nickname, uid, server, is_default, auto_checkin, auto_redeem, auto_daily_note,
               last_checkin_at, last_redeem_at, last_daily_note_at, status
        FROM hoyo_accounts
        WHERE id = ? AND user_id = ? AND game = ? AND status != 'deleted'
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .bind(game.slug())
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Err(AppError::not_found("Tài khoản không tồn tại hoặc đã bị xóa."));
    };

    let last_checkin: Option<NaiveDateTime> = r.try_get("last_checkin_at").ok().flatten();
    let last_redeem: Option<NaiveDateTime> = r.try_get("last_redeem_at").ok().flatten();
    let last_note: Option<NaiveDateTime> = r.try_get("last_daily_note_at").ok().flatten();

    Ok(json!({
        "id": r.try_get::<i64, _>("id")?,
        "game": r.try_get::<String, _>("game")?,
        "nickname": r.try_get::<Option<String>, _>("nickname").ok().flatten(),
        "uid": r.try_get::<String, _>("uid")?,
        "server": r.try_get::<String, _>("server")?,
        "is_default": r.try_get::<bool, _>("is_default").unwrap_or(false),
        "auto_checkin": r.try_get::<bool, _>("auto_checkin").unwrap_or(false),
        "auto_redeem": r.try_get::<bool, _>("auto_redeem").unwrap_or(false),
        "auto_daily_note": r.try_get::<bool, _>("auto_daily_note").unwrap_or(false),
        "last_checkin_at": last_checkin.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
        "last_redeem_at": last_redeem.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
        "last_daily_note_at": last_note.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
        "status": r.try_get::<String, _>("status")?,
    }))
}

pub async fn update_account(
    state: &AppState,
    game: HoyoGame,
    user_id: i64,
    account_id: i64,
    data: &Map<String, Value>,
) -> AppResult<()> {
    if game == HoyoGame::Genshin {
        return gi_account::update_account(state, user_id, account_id, data).await;
    }

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let acc = sqlx::query(
        r#"
        SELECT id, auto_checkin, auto_redeem, auto_daily_note, is_default, status
        FROM hoyo_accounts WHERE id = ? AND user_id = ? AND game = ? FOR UPDATE
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .bind(game.slug())
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
        sqlx::query("UPDATE hoyo_accounts SET is_default = false WHERE user_id = ? AND game = ?")
            .bind(user_id)
            .bind(game.slug())
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
        UPDATE hoyo_accounts
        SET auto_checkin = ?, auto_redeem = ?, auto_daily_note = ?, is_default = ?, status = ?
        WHERE id = ? AND user_id = ? AND game = ?
        "#,
    )
    .bind(auto_checkin)
    .bind(auto_redeem)
    .bind(auto_daily_note)
    .bind(is_default)
    .bind(status)
    .bind(account_id)
    .bind(user_id)
    .bind(game.slug())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn delete_account(
    state: &AppState,
    game: HoyoGame,
    user_id: i64,
    account_id: i64,
) -> AppResult<()> {
    if game == HoyoGame::Genshin {
        return gi_account::delete_account(state, user_id, account_id).await;
    }

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let acc = sqlx::query(
        "SELECT is_default FROM hoyo_accounts WHERE id = ? AND user_id = ? AND game = ? AND status != 'deleted' FOR UPDATE",
    )
    .bind(account_id)
    .bind(user_id)
    .bind(game.slug())
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = acc else {
        return Err(AppError::bad_request("Tài khoản không tồn tại."));
    };
    let was_default: bool = row.try_get("is_default").unwrap_or(false);

    sqlx::query("UPDATE hoyo_accounts SET status = 'deleted', is_default = false WHERE id = ?")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;

    if was_default {
        sqlx::query(
            r#"
            UPDATE hoyo_accounts SET is_default = true
            WHERE user_id = ? AND game = ? AND status = 'active'
            ORDER BY created_at DESC LIMIT 1
            "#,
        )
        .bind(user_id)
        .bind(game.slug())
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn get_ready_payload(
    state: &AppState,
    game: HoyoGame,
    user_id: i64,
    account_id: Option<i64>,
) -> AppResult<ReadyHoyoPayload> {
    if game == HoyoGame::Genshin {
        let p = gi_account::get_ready_genshin_payload(state, user_id, account_id).await?;
        return Ok(ReadyHoyoPayload {
            account_id: p.account_id,
            game: HoyoGame::Genshin,
            cookie: p.cookie,
            uid: p.uid,
            server: p.server,
            nickname: p.nickname,
        });
    }

    let pool = state.db()?;
    let row = if let Some(acc_id) = account_id {
        sqlx::query(
            "SELECT id, cookie, uid, server, nickname FROM hoyo_accounts WHERE id = ? AND user_id = ? AND game = ? AND status = 'active'",
        )
        .bind(acc_id)
        .bind(user_id)
        .bind(game.slug())
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT id, cookie, uid, server, nickname FROM hoyo_accounts WHERE user_id = ? AND game = ? AND is_default = TRUE AND status = 'active'",
        )
        .bind(user_id)
        .bind(game.slug())
        .fetch_optional(pool)
        .await?
    };

    let Some(row) = row else {
        return Err(AppError::bad_request(format!(
            "Chưa chọn tài khoản {} khả dụng. Vui lòng thêm tài khoản hoặc chọn account_id!",
            game.display_name()
        )));
    };

    let encrypted: String = row.try_get("cookie")?;
    let cookie = decrypt_cookie(&encrypted, &state.config.genshin_cookie_secret)?;

    Ok(ReadyHoyoPayload {
        account_id: row.try_get("id")?,
        game,
        cookie,
        uid: row.try_get("uid")?,
        server: row.try_get("server")?,
        nickname: row.try_get::<Option<String>, _>("nickname")?.unwrap_or_default(),
    })
}

pub async fn get_all_auto_checkin_accounts(state: &AppState) -> AppResult<Vec<AutoHoyoAccountRow>> {
    let mut list = Vec::new();

    if let Ok(gi_rows) = gi_account::get_auto_checkin_accounts(state).await {
        for r in gi_rows {
            list.push(AutoHoyoAccountRow {
                id: r.id,
                user_id: r.user_id,
                game: HoyoGame::Genshin,
                nickname: r.nickname,
                uid: r.uid,
                server: r.server,
                cookie_encrypted: r.cookie_encrypted,
                last_checkin_at: r.last_checkin_at,
                email: r.email,
            });
        }
    }

    let pool = state.db()?;
    if let Ok(rows) = sqlx::query(
        r#"
        SELECT ha.id, ha.user_id, ha.game, ha.nickname, ha.uid, ha.server, ha.cookie, ha.last_checkin_at, u.email
        FROM hoyo_accounts ha
        INNER JOIN users u ON u.id = ha.user_id
        WHERE ha.status = 'active' AND ha.auto_checkin = 1 AND u.status = 'active'
        "#,
    )
    .fetch_all(pool)
    .await
    {
        for r in rows {
            let g_str: String = r.try_get("game").unwrap_or_default();
            if let Ok(game) = HoyoGame::from_slug(&g_str) {
                list.push(AutoHoyoAccountRow {
                    id: r.try_get("id").unwrap_or(0),
                    user_id: r.try_get("user_id").unwrap_or(0),
                    game,
                    nickname: r.try_get::<Option<String>, _>("nickname").ok().flatten().unwrap_or_default(),
                    uid: r.try_get("uid").unwrap_or_default(),
                    server: r.try_get("server").unwrap_or_default(),
                    cookie_encrypted: r.try_get("cookie").unwrap_or_default(),
                    last_checkin_at: r.try_get("last_checkin_at").ok().flatten(),
                    email: r.try_get("email").ok().flatten(),
                });
            }
        }
    }

    Ok(list)
}

pub async fn get_all_auto_redeem_accounts(state: &AppState) -> AppResult<Vec<AutoHoyoAccountRow>> {
    let mut list = Vec::new();

    if let Ok(gi_rows) = gi_account::get_auto_redeem_accounts(state).await {
        for r in gi_rows {
            list.push(AutoHoyoAccountRow {
                id: r.id,
                user_id: r.user_id,
                game: HoyoGame::Genshin,
                nickname: r.nickname,
                uid: r.uid,
                server: r.server,
                cookie_encrypted: r.cookie_encrypted,
                last_checkin_at: r.last_checkin_at,
                email: r.email,
            });
        }
    }

    let pool = state.db()?;
    if let Ok(rows) = sqlx::query(
        r#"
        SELECT ha.id, ha.user_id, ha.game, ha.nickname, ha.uid, ha.server, ha.cookie, ha.last_checkin_at, u.email
        FROM hoyo_accounts ha
        INNER JOIN users u ON u.id = ha.user_id
        WHERE ha.status = 'active' AND ha.auto_redeem = 1 AND u.status = 'active'
        "#,
    )
    .fetch_all(pool)
    .await
    {
        for r in rows {
            let g_str: String = r.try_get("game").unwrap_or_default();
            if let Ok(game) = HoyoGame::from_slug(&g_str) {
                list.push(AutoHoyoAccountRow {
                    id: r.try_get("id").unwrap_or(0),
                    user_id: r.try_get("user_id").unwrap_or(0),
                    game,
                    nickname: r.try_get::<Option<String>, _>("nickname").ok().flatten().unwrap_or_default(),
                    uid: r.try_get("uid").unwrap_or_default(),
                    server: r.try_get("server").unwrap_or_default(),
                    cookie_encrypted: r.try_get("cookie").unwrap_or_default(),
                    last_checkin_at: r.try_get("last_checkin_at").ok().flatten(),
                    email: r.try_get("email").ok().flatten(),
                });
            }
        }
    }

    Ok(list)
}

pub async fn get_all_auto_daily_note_accounts(
    state: &AppState,
) -> AppResult<Vec<AutoHoyoAccountRow>> {
    let mut list = Vec::new();

    if let Ok(gi_rows) = gi_account::get_auto_daily_note_accounts(state).await {
        for r in gi_rows {
            list.push(AutoHoyoAccountRow {
                id: r.id,
                user_id: r.user_id,
                game: HoyoGame::Genshin,
                nickname: r.nickname,
                uid: r.uid,
                server: r.server,
                cookie_encrypted: r.cookie_encrypted,
                last_checkin_at: r.last_checkin_at,
                email: r.email,
            });
        }
    }

    let pool = state.db()?;
    if let Ok(rows) = sqlx::query(
        r#"
        SELECT ha.id, ha.user_id, ha.game, ha.nickname, ha.uid, ha.server, ha.cookie, ha.last_checkin_at, u.email
        FROM hoyo_accounts ha
        INNER JOIN users u ON u.id = ha.user_id
        WHERE ha.status = 'active' AND ha.auto_daily_note = 1 AND u.status = 'active'
        "#,
    )
    .fetch_all(pool)
    .await
    {
        for r in rows {
            let g_str: String = r.try_get("game").unwrap_or_default();
            if let Ok(game) = HoyoGame::from_slug(&g_str) {
                list.push(AutoHoyoAccountRow {
                    id: r.try_get("id").unwrap_or(0),
                    user_id: r.try_get("user_id").unwrap_or(0),
                    game,
                    nickname: r.try_get::<Option<String>, _>("nickname").ok().flatten().unwrap_or_default(),
                    uid: r.try_get("uid").unwrap_or_default(),
                    server: r.try_get("server").unwrap_or_default(),
                    cookie_encrypted: r.try_get("cookie").unwrap_or_default(),
                    last_checkin_at: r.try_get("last_checkin_at").ok().flatten(),
                    email: r.try_get("email").ok().flatten(),
                });
            }
        }
    }

    Ok(list)
}

pub async fn update_checkin_status(
    state: &AppState,
    game: HoyoGame,
    account_id: i64,
    status: &str,
) -> AppResult<()> {
    if game == HoyoGame::Genshin {
        return gi_account::update_checkin_status(state, account_id, status).await;
    }
    let pool = state.db()?;
    sqlx::query(
        "UPDATE hoyo_accounts SET last_checkin_at = NOW(), last_checkin_status = ? WHERE id = ?",
    )
    .bind(status)
    .bind(account_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_redeem_timestamp(
    state: &AppState,
    game: HoyoGame,
    account_id: i64,
) -> AppResult<()> {
    if game == HoyoGame::Genshin {
        return gi_account::update_redeem_timestamp(state, account_id).await;
    }
    let pool = state.db()?;
    sqlx::query("UPDATE hoyo_accounts SET last_redeem_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_daily_note_timestamp(
    state: &AppState,
    game: HoyoGame,
    account_id: i64,
) -> AppResult<()> {
    if game == HoyoGame::Genshin {
        return gi_account::update_daily_note_timestamp(state, account_id).await;
    }
    let pool = state.db()?;
    sqlx::query("UPDATE hoyo_accounts SET last_daily_note_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}
