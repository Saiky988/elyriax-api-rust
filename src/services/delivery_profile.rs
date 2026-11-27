use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    core::security::{decrypt_cookie, encrypt_cookie},
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub struct DeliveryProfileInput {
    pub label: String,
    pub contact_phone: Option<String>,
    pub profile_type: String,
    pub target_email: Option<String>,
    pub account_username: Option<String>,
    pub account_password: Option<String>,
    pub two_factor_note: Option<String>,
    pub is_default: Option<bool>,
}

fn clean_opt(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn validate_profile_type(pt: &str) -> AppResult<&'static str> {
    match pt.trim().to_lowercase().as_str() {
        "email" => Ok("email"),
        "account" => Ok("account"),
        _ => Err(AppError::bad_request(
            "profile_type không hợp lệ (chỉ hỗ trợ 'email' hoặc 'account')",
        )),
    }
}

pub async fn list_delivery_profiles(
    state: &AppState,
    user_id: i64,
    reveal: bool,
) -> AppResult<Vec<Value>> {
    let pool = state.db()?;
    let rows = sqlx::query(
        r#"
        SELECT id, user_id, label, contact_phone, profile_type, target_email,
               account_username, account_password_encrypted, two_factor_note,
               is_default, created_at, updated_at
        FROM user_delivery_profiles
        WHERE user_id = ?
        ORDER BY is_default DESC, id DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let enc: Option<String> = r.try_get("account_password_encrypted").ok().flatten();
        let has_password = enc.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
        let revealed_password = if reveal {
            enc.as_deref()
                .filter(|s| !s.is_empty())
                .and_then(|c| decrypt_cookie(c, &state.config.genshin_cookie_secret).ok())
        } else {
            None
        };
        let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
        let updated_at: Option<DateTime<Utc>> = r.try_get("updated_at").ok().flatten();
        let is_default: i8 = r.try_get("is_default").unwrap_or(0);

        out.push(json!({
            "id": r.try_get::<i64, _>("id").unwrap_or(0),
            "label": r.try_get::<String, _>("label").unwrap_or_default(),
            "contact_phone": r.try_get::<Option<String>, _>("contact_phone").ok().flatten(),
            "profile_type": r.try_get::<String, _>("profile_type").unwrap_or_else(|_| "email".to_string()),
            "target_email": r.try_get::<Option<String>, _>("target_email").ok().flatten(),
            "account_username": r.try_get::<Option<String>, _>("account_username").ok().flatten(),
            "has_password": has_password,
            "account_password": revealed_password,
            "account_password_masked": if has_password { Some("••••••••") } else { None },
            "two_factor_note": r.try_get::<Option<String>, _>("two_factor_note").ok().flatten(),
            "is_default": is_default != 0,
            "created_at": created_at,
            "updated_at": updated_at,
        }));
    }

    Ok(out)
}

pub async fn create_delivery_profile(
    state: &AppState,
    user_id: i64,
    input: DeliveryProfileInput,
) -> AppResult<Value> {
    let label = input.label.trim().to_string();
    if label.is_empty() {
        return Err(AppError::bad_request("Vui lòng nhập tên gợi nhớ cho hồ sơ"));
    }

    let ptype = validate_profile_type(&input.profile_type)?;
    let contact_phone = clean_opt(input.contact_phone);
    let target_email = clean_opt(input.target_email);
    let account_username = clean_opt(input.account_username);
    let raw_password = clean_opt(input.account_password);
    let two_factor_note = clean_opt(input.two_factor_note);

    let enc_password = if ptype == "email" {
        if target_email.is_none() {
            return Err(AppError::bad_request(
                "Vui lòng nhập Email nhận hàng / nâng cấp",
            ));
        }
        None
    } else {
        if account_username.is_none() || raw_password.is_none() {
            return Err(AppError::bad_request(
                "Vui lòng nhập đầy đủ Tài khoản và Mật khẩu",
            ));
        }
        let pw = raw_password.as_deref().unwrap_or_default();
        Some(encrypt_cookie(pw, &state.config.genshin_cookie_secret)?)
    };

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    // If this is the user's first profile, make it default automatically
    let count_row = sqlx::query("SELECT COUNT(id) AS cnt FROM user_delivery_profiles WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    let existing_cnt: i64 = count_row.try_get("cnt").unwrap_or(0);
    let is_default = input.is_default.unwrap_or(existing_cnt == 0);

    if is_default {
        sqlx::query("UPDATE user_delivery_profiles SET is_default = 0 WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }

    let res = sqlx::query(
        r#"
        INSERT INTO user_delivery_profiles
        (user_id, label, contact_phone, profile_type, target_email, account_username, account_password_encrypted, two_factor_note, is_default)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(&label)
    .bind(&contact_phone)
    .bind(ptype)
    .bind(&target_email)
    .bind(&account_username)
    .bind(&enc_password)
    .bind(&two_factor_note)
    .bind(if is_default { 1i8 } else { 0i8 })
    .execute(&mut *tx)
    .await?;

    let new_id = res.last_insert_id() as i64;
    tx.commit().await?;

    Ok(json!({
        "id": new_id,
        "label": label,
        "contact_phone": contact_phone,
        "profile_type": ptype,
        "target_email": target_email,
        "account_username": account_username,
        "has_password": enc_password.is_some(),
        "two_factor_note": two_factor_note,
        "is_default": is_default,
    }))
}

pub async fn update_delivery_profile(
    state: &AppState,
    user_id: i64,
    profile_id: i64,
    input: DeliveryProfileInput,
) -> AppResult<Value> {
    let pool = state.db()?;
    let existing = sqlx::query(
        "SELECT id, account_password_encrypted FROM user_delivery_profiles WHERE id = ? AND user_id = ?",
    )
    .bind(profile_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let Some(ex_row) = existing else {
        return Err(AppError::not_found("Không tìm thấy hồ sơ thông tin nhận hàng"));
    };

    let label = input.label.trim().to_string();
    if label.is_empty() {
        return Err(AppError::bad_request("Vui lòng nhập tên gợi nhớ cho hồ sơ"));
    }

    let ptype = validate_profile_type(&input.profile_type)?;
    let contact_phone = clean_opt(input.contact_phone);
    let target_email = clean_opt(input.target_email);
    let account_username = clean_opt(input.account_username);
    let raw_password = clean_opt(input.account_password);
    let two_factor_note = clean_opt(input.two_factor_note);

    let prev_enc: Option<String> = ex_row.try_get("account_password_encrypted").ok().flatten();
    let enc_password = if ptype == "email" {
        if target_email.is_none() {
            return Err(AppError::bad_request(
                "Vui lòng nhập Email nhận hàng / nâng cấp",
            ));
        }
        None
    } else {
        if account_username.is_none() {
            return Err(AppError::bad_request("Vui lòng nhập Tài khoản / Email"));
        }
        if let Some(pw) = raw_password {
            Some(encrypt_cookie(&pw, &state.config.genshin_cookie_secret)?)
        } else if prev_enc.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            prev_enc
        } else {
            return Err(AppError::bad_request("Vui lòng nhập Mật khẩu tài khoản"));
        }
    };

    let mut tx = pool.begin().await?;
    if input.is_default == Some(true) {
        sqlx::query("UPDATE user_delivery_profiles SET is_default = 0 WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(
        r#"
        UPDATE user_delivery_profiles
        SET label = ?, contact_phone = ?, profile_type = ?, target_email = ?,
            account_username = ?, account_password_encrypted = ?, two_factor_note = ?,
            is_default = COALESCE(?, is_default)
        WHERE id = ? AND user_id = ?
        "#,
    )
    .bind(&label)
    .bind(&contact_phone)
    .bind(ptype)
    .bind(&target_email)
    .bind(&account_username)
    .bind(&enc_password)
    .bind(&two_factor_note)
    .bind(input.is_default.map(|b| if b { 1i8 } else { 0i8 }))
    .bind(profile_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(json!({ "id": profile_id }))
}

pub async fn delete_delivery_profile(
    state: &AppState,
    user_id: i64,
    profile_id: i64,
) -> AppResult<()> {
    let pool = state.db()?;
    let res = sqlx::query("DELETE FROM user_delivery_profiles WHERE id = ? AND user_id = ?")
        .bind(profile_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::not_found("Không tìm thấy hồ sơ để xóa"));
    }
    Ok(())
}

/// Resolves an owned delivery profile into a decrypted JSON snapshot for `order_items.customer_data`.
pub async fn resolve_profile_snapshot(
    state: &AppState,
    user_id: i64,
    profile_id: i64,
) -> AppResult<Value> {
    let pool = state.db()?;
    let row = sqlx::query(
        r#"
        SELECT id, label, contact_phone, profile_type, target_email,
               account_username, account_password_encrypted, two_factor_note
        FROM user_delivery_profiles
        WHERE id = ? AND user_id = ?
        "#,
    )
    .bind(profile_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Err(AppError::bad_request(
            "Hồ sơ thông tin nhận hàng không tồn tại hoặc không thuộc tài khoản của bạn",
        ));
    };

    let enc: Option<String> = r.try_get("account_password_encrypted").ok().flatten();
    let decrypted_pw = enc
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|c| decrypt_cookie(c, &state.config.genshin_cookie_secret))
        .transpose()?;

    Ok(json!({
        "profile_id": r.try_get::<i64, _>("id").unwrap_or(0),
        "label": r.try_get::<String, _>("label").unwrap_or_default(),
        "contact_phone": r.try_get::<Option<String>, _>("contact_phone").ok().flatten(),
        "profile_type": r.try_get::<String, _>("profile_type").unwrap_or_else(|_| "email".to_string()),
        "target_email": r.try_get::<Option<String>, _>("target_email").ok().flatten(),
        "account_username": r.try_get::<Option<String>, _>("account_username").ok().flatten(),
        "account_password": decrypted_pw,
        "two_factor_note": r.try_get::<Option<String>, _>("two_factor_note").ok().flatten(),
    }))
}
