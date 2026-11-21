use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

fn default_preferences() -> Value {
    json!({
        "animations": true,
        "glass_effect": true,
        "compact_mode": false,
        "sidebar_collapsed": false,
    })
}

fn default_notifications() -> Value {
    json!({
        "email": true,
        "push": true,
        "discord": false,
        "security_alert": false,
        "system_update": false,
    })
}

fn default_privacy() -> Value {
    json!({
        "profile_public": false,
        "show_email": false,
        "analytics": true,
        "crash_report": true,
    })
}

fn parse_json_col(raw: Option<String>, default: Value) -> Value {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(default)
}

fn merge_json_objects(base: Value, patch: &Map<String, Value>) -> Value {
    let mut map = match base {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    for (k, v) in patch {
        map.insert(k.clone(), v.clone());
    }
    Value::Object(map)
}

pub async fn get_settings(state: &AppState, user_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let row = sqlx::query(
        r#"
        SELECT theme, accent, language, timezone, developer_mode, show_api_logs, show_debug_info,
               CAST(preferences AS CHAR) AS preferences,
               CAST(notifications AS CHAR) AS notifications,
               CAST(privacy AS CHAR) AS privacy
        FROM user_settings WHERE user_id = ?
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let row = if let Some(r) = row {
        r
    } else {
        sqlx::query(
            r#"
            INSERT IGNORE INTO user_settings
            (user_id, theme, accent, language, timezone, developer_mode, show_api_logs, show_debug_info, preferences, notifications, privacy)
            VALUES (?, 'dark', 'cyan', 'vi-VN', 'Asia/Ho_Chi_Minh', false, false, false, ?, ?, ?)
            "#,
        )
        .bind(user_id)
        .bind(default_preferences().to_string())
        .bind(default_notifications().to_string())
        .bind(default_privacy().to_string())
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            SELECT theme, accent, language, timezone, developer_mode, show_api_logs, show_debug_info,
                   CAST(preferences AS CHAR) AS preferences,
                   CAST(notifications AS CHAR) AS notifications,
                   CAST(privacy AS CHAR) AS privacy
            FROM user_settings WHERE user_id = ?
            "#,
        )
        .bind(user_id)
        .fetch_one(pool)
        .await?
    };

    Ok(json!({
        "theme": row.try_get::<Option<String>, _>("theme")?.unwrap_or_else(|| "dark".to_string()),
        "accent": row.try_get::<Option<String>, _>("accent")?.unwrap_or_else(|| "cyan".to_string()),
        "language": row.try_get::<Option<String>, _>("language")?.unwrap_or_else(|| "vi-VN".to_string()),
        "timezone": row.try_get::<Option<String>, _>("timezone")?.unwrap_or_else(|| "Asia/Ho_Chi_Minh".to_string()),
        "developer_mode": row.try_get::<bool, _>("developer_mode").unwrap_or(false),
        "show_api_logs": row.try_get::<bool, _>("show_api_logs").unwrap_or(false),
        "show_debug_info": row.try_get::<bool, _>("show_debug_info").unwrap_or(false),
        "preferences": parse_json_col(row.try_get("preferences").ok().flatten(), default_preferences()),
        "notifications": parse_json_col(row.try_get("notifications").ok().flatten(), default_notifications()),
        "privacy": parse_json_col(row.try_get("privacy").ok().flatten(), default_privacy()),
    }))
}

pub async fn update_settings(
    state: &AppState,
    user_id: i64,
    data: &Map<String, Value>,
) -> AppResult<Value> {
    let current = get_settings(state, user_id).await?;
    let pool = state.db()?;

    let theme = if let Some(v) = data.get("theme").and_then(Value::as_str) {
        if !matches!(v, "dark" | "light" | "system" | "classic_dark") {
            return Err(AppError::bad_request("Theme không hợp lệ"));
        }
        v.to_string()
    } else {
        current["theme"].as_str().unwrap_or("dark").to_string()
    };

    let accent = if let Some(v) = data.get("accent").and_then(Value::as_str) {
        if !matches!(v, "cyan" | "blue" | "purple" | "pink" | "red" | "amber") {
            return Err(AppError::bad_request("Accent không hợp lệ"));
        }
        v.to_string()
    } else {
        current["accent"].as_str().unwrap_or("cyan").to_string()
    };

    let language = if let Some(v) = data.get("language").and_then(Value::as_str) {
        if !matches!(v, "vi-VN" | "en-US") {
            return Err(AppError::bad_request("Ngôn ngữ không hợp lệ"));
        }
        v.to_string()
    } else {
        current["language"].as_str().unwrap_or("vi-VN").to_string()
    };

    let timezone = if let Some(v) = data.get("timezone").and_then(Value::as_str) {
        if v.len() > 50 {
            return Err(AppError::bad_request("Timezone không hợp lệ (tối đa 50 ký tự)"));
        }
        v.to_string()
    } else {
        current["timezone"].as_str().unwrap_or("Asia/Ho_Chi_Minh").to_string()
    };

    let developer_mode = data
        .get("developer_mode")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| current["developer_mode"].as_bool().unwrap_or(false));
    let show_api_logs = data
        .get("show_api_logs")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| current["show_api_logs"].as_bool().unwrap_or(false));
    let show_debug_info = data
        .get("show_debug_info")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| current["show_debug_info"].as_bool().unwrap_or(false));

    let preferences = if let Some(obj) = data.get("preferences").and_then(Value::as_object) {
        merge_json_objects(current["preferences"].clone(), obj)
    } else {
        current["preferences"].clone()
    };

    let notifications = if let Some(obj) = data.get("notifications").and_then(Value::as_object) {
        merge_json_objects(current["notifications"].clone(), obj)
    } else {
        current["notifications"].clone()
    };

    let privacy = if let Some(obj) = data.get("privacy").and_then(Value::as_object) {
        merge_json_objects(current["privacy"].clone(), obj)
    } else {
        current["privacy"].clone()
    };

    sqlx::query(
        r#"
        UPDATE user_settings
        SET theme = ?, accent = ?, language = ?, timezone = ?,
            developer_mode = ?, show_api_logs = ?, show_debug_info = ?,
            preferences = ?, notifications = ?, privacy = ?
        WHERE user_id = ?
        "#,
    )
    .bind(&theme)
    .bind(&accent)
    .bind(&language)
    .bind(&timezone)
    .bind(developer_mode)
    .bind(show_api_logs)
    .bind(show_debug_info)
    .bind(preferences.to_string())
    .bind(notifications.to_string())
    .bind(privacy.to_string())
    .bind(user_id)
    .execute(pool)
    .await?;

    get_settings(state, user_id).await
}
