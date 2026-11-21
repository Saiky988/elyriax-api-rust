use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    services::genshin::{normalize_region, server_display_name},
    state::AppState,
};

pub const ACT_ID: &str = "e202102251931481";
const HOYO_HOME: &str = "https://sg-hk4e-api.hoyolab.com/event/sol/home";
const HOYO_INFO: &str = "https://sg-hk4e-api.hoyolab.com/event/sol/info";
const HOYO_SIGN: &str = "https://sg-hk4e-api.hoyolab.com/event/sol/sign";
const HOYO_ROLE: &str = "https://api-os-takumi.hoyoverse.com/binding/api/getUserGameRolesByCookie";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub async fn check_in_daily(
    state: &AppState,
    cookie: &str,
    server: &str,
) -> AppResult<Value> {
    if cookie.is_empty() {
        return Err(AppError::bad_request("Missing cookie"));
    }

    let target_region = normalize_region(server)
        .ok_or_else(|| AppError::bad_request("Invalid server provided"))?;

    // Fetch home awards & current sign info concurrently
    let home_req = state
        .http
        .get(format!("{}?act_id={}", HOYO_HOME, ACT_ID))
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send();

    let info_req = state
        .http
        .get(format!("{}?act_id={}", HOYO_INFO, ACT_ID))
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send();

    let (home_res, info_res) = tokio::try_join!(home_req, info_req)?;
    let home_data: Value = home_res.json().await?;
    let info_data: Value = info_res.json().await?;

    if home_data.get("retcode").and_then(Value::as_i64) != Some(0) {
        return Err(AppError::bad_request(
            home_data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Home API Error"),
        ));
    }

    if info_data.get("retcode").and_then(Value::as_i64) != Some(0) {
        return Err(AppError::bad_request(
            info_data
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Info API Error"),
        ));
    }

    let awards = home_data
        .pointer("/data/awards")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total_days_month = awards.len();

    let mut total_signed = info_data
        .pointer("/data/total_sign_day")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let is_signed = info_data
        .pointer("/data/is_sign")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut claimed = false;
    let mut risk_code: i64 = 0;

    if !is_signed {
        let sign_data: Value = state
            .http
            .post(HOYO_SIGN)
            .header("Cookie", cookie)
            .header("User-Agent", UA)
            .header("Accept", "application/json")
            .json(&json!({ "act_id": ACT_ID }))
            .send()
            .await?
            .json()
            .await?;

        risk_code = sign_data
            .pointer("/data/risk_code")
            .or_else(|| sign_data.get("risk_code"))
            .and_then(Value::as_i64)
            .unwrap_or(0);

        let retcode = sign_data.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
        if retcode == 0 {
            claimed = true;
            total_signed += 1;
        } else if retcode != -5003 {
            return Err(AppError::bad_request(
                sign_data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Sign API Error"),
            ));
        }
    }

    let today_idx = total_signed.saturating_sub(1);
    let today_reward = awards.get(today_idx).map(|r| {
        json!({
            "name": r.get("name"),
            "count": r.get("cnt"),
            "icon": r.get("icon"),
        })
    });
    let tomorrow_reward = awards.get(total_signed).map(|r| {
        json!({
            "name": r.get("name"),
            "count": r.get("cnt"),
            "icon": r.get("icon"),
        })
    });

    // Fetch role information
    let role_data: Value = state
        .http
        .get(format!("{}?game_biz=hk4e_global", HOYO_ROLE))
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send()
        .await?
        .json()
        .await?;

    if role_data.get("retcode").and_then(Value::as_i64) != Some(0) {
        return Err(AppError::bad_request("Failed to fetch account list"));
    }

    let roles = role_data
        .pointer("/data/list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let selected = roles
        .into_iter()
        .find(|r| r.get("region").and_then(Value::as_str) == Some(target_region))
        .ok_or_else(|| AppError::bad_request(format!("No account found on server: {}", server)))?;

    let region_str = selected
        .get("region")
        .and_then(Value::as_str)
        .unwrap_or(target_region);

    Ok(json!({
        "ok": true,
        "claimed": claimed,
        "nickname": selected.get("nickname"),
        "uid": selected.get("game_uid"),
        "server": server_display_name(region_str),
        "adventure_rank": selected.get("level"),
        "signed_days": total_signed,
        "total_days_month": total_days_month,
        "missing_days": total_days_month.saturating_sub(total_signed),
        "today_reward": today_reward,
        "tomorrow_reward": tomorrow_reward,
        "risk_code": risk_code,
    }))
}
