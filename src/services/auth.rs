use serde_json::json;
use sqlx::Row;

use crate::{
    core::security::decode_jwt_token,
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Debug, Clone)]
pub struct OAuthProfile {
    pub provider: String,
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OAuthUser {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
    pub avatar: Option<String>,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum OAuthResult {
    Linked { message: String },
    LoggedIn { user: OAuthUser, is_new: bool },
}

pub async fn create_default_user_settings(state: &AppState, user_id: i64) -> AppResult<()> {
    let pool = state.db()?;
    let preferences = json!({
        "animations": true,
        "glass_effect": true,
        "compact_mode": false,
        "sidebar_collapsed": false,
    })
    .to_string();
    let notifications = json!({
        "email": true,
        "push": true,
        "discord": false,
        "security_alert": true,
        "system_update": true,
    })
    .to_string();
    let privacy = json!({
        "profile_public": false,
        "show_email": false,
        "analytics": true,
        "crash_report": true,
    })
    .to_string();

    sqlx::query(
        r#"
        INSERT IGNORE INTO user_settings
        (user_id, theme, accent, language, timezone, developer_mode, show_api_logs, show_debug_info, preferences, notifications, privacy)
        VALUES (?, 'dark', 'cyan', 'vi-VN', 'Asia/Ho_Chi_Minh', false, false, false, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(preferences)
    .bind(notifications)
    .bind(privacy)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn handle_oauth_login_or_link(
    state: &AppState,
    profile: OAuthProfile,
    state_token: Option<&str>,
) -> AppResult<OAuthResult> {
    let pool = state.db()?;
    let mut current_user_id: Option<i64> = None;

    if let Some(st) = state_token {
        if !st.is_empty() && st != "login" {
            let claims = decode_jwt_token(st, &state.config.jwt_secret).map_err(|_| {
                AppError::bad_request("Token không hợp lệ hoặc đã hết hạn. Vui lòng đăng nhập lại.")
            })?;
            current_user_id = Some(claims.id);
        }
    }

    let existing_oauth = sqlx::query(
        "SELECT id, user_id FROM user_oauth_accounts WHERE provider = ? AND provider_user_id = ?",
    )
    .bind(&profile.provider)
    .bind(&profile.id)
    .fetch_optional(pool)
    .await?;

    // CASE 1: Account Linking
    if let Some(cur_uid) = current_user_id {
        if let Some(existing) = existing_oauth {
            let existing_oauth_id: i64 = existing.try_get("id")?;
            let existing_user_id: i64 = existing.try_get("user_id")?;

            if existing_user_id == cur_uid {
                return Ok(OAuthResult::Linked {
                    message: format!(
                        "{} đã được liên kết với tài khoản này từ trước.",
                        profile.provider
                    ),
                });
            }

            let old_user =
                sqlx::query("SELECT id, username, email, password_hash FROM users WHERE id = ?")
                    .bind(existing_user_id)
                    .fetch_optional(pool)
                    .await?;

            if let Some(old) = old_user {
                let old_username: String = old
                    .try_get::<Option<String>, _>("username")?
                    .unwrap_or_default();
                let old_password: Option<String> = old.try_get("password_hash")?;
                let old_email: Option<String> = old.try_get("email")?;

                if old_username.starts_with("discord_")
                    || old_password.is_none()
                    || (old_email.is_some() && old_email == profile.email)
                {
                    let mut tx = pool.begin().await?;
                    sqlx::query("UPDATE genshin_accounts SET user_id = ? WHERE user_id = ?")
                        .bind(cur_uid)
                        .bind(existing_user_id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("UPDATE user_oauth_accounts SET user_id = ? WHERE id = ?")
                        .bind(cur_uid)
                        .bind(existing_oauth_id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("DELETE FROM user_settings WHERE user_id = ?")
                        .bind(existing_user_id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("DELETE FROM users WHERE id = ?")
                        .bind(existing_user_id)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;

                    return Ok(OAuthResult::Linked {
                        message: format!(
                            "Đã chuyển và liên kết {} thành công!",
                            profile.provider
                        ),
                    });
                }
            }

            return Err(AppError::bad_request(format!(
                "Tài khoản {} này đã được liên kết với một người dùng khác.",
                profile.provider
            )));
        }

        sqlx::query(
            r#"
            INSERT INTO user_oauth_accounts
            (user_id, provider, provider_user_id, provider_email, provider_name, provider_avatar)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(cur_uid)
        .bind(&profile.provider)
        .bind(&profile.id)
        .bind(&profile.email)
        .bind(&profile.name)
        .bind(&profile.avatar)
        .execute(pool)
        .await?;

        return Ok(OAuthResult::Linked {
            message: format!("Đã liên kết {} thành công!", profile.provider),
        });
    }

    // CASE 2: Login or Register
    let (user_id, is_new) = if let Some(existing) = existing_oauth {
        let uid: i64 = existing.try_get("user_id")?;
        create_default_user_settings(state, uid).await?;
        (uid, false)
    } else {
        let res = sqlx::query("INSERT INTO users (email, username, avatar) VALUES (?, ?, ?)")
            .bind(&profile.email)
            .bind(&profile.name)
            .bind(&profile.avatar)
            .execute(pool)
            .await?;
        let new_uid = res.last_insert_id() as i64;
        create_default_user_settings(state, new_uid).await?;
        sqlx::query(
            r#"
            INSERT INTO user_oauth_accounts
            (user_id, provider, provider_user_id, provider_email, provider_name, provider_avatar)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(new_uid)
        .bind(&profile.provider)
        .bind(&profile.id)
        .bind(&profile.email)
        .bind(&profile.name)
        .bind(&profile.avatar)
        .execute(pool)
        .await?;
        (new_uid, true)
    };

    let user_row = sqlx::query("SELECT id, username, email, avatar FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;

    let provider_rows = sqlx::query("SELECT provider FROM user_oauth_accounts WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await?;

    let providers = provider_rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("provider").ok())
        .collect();

    Ok(OAuthResult::LoggedIn {
        user: OAuthUser {
            id: user_row.try_get("id")?,
            username: user_row.try_get::<Option<String>, _>("username")?.unwrap_or_default(),
            email: user_row.try_get("email")?,
            avatar: user_row.try_get("avatar")?,
            providers,
        },
        is_new,
    })
}
