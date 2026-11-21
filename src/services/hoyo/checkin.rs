use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    services::hoyo::HoyoGame,
    state::AppState,
};

const HOYO_ROLE_URL: &str =
    "https://api-os-takumi.hoyoverse.com/binding/api/getUserGameRolesByCookie";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub async fn check_in_daily(
    state: &AppState,
    game: HoyoGame,
    cookie: &str,
    server: &str,
) -> AppResult<Value> {
    if cookie.is_empty() {
        return Err(AppError::bad_request("Missing cookie"));
    }

    let target_region = game
        .normalize_region(server)
        .ok_or_else(|| AppError::bad_request(format!("Invalid server provided: {}", server)))?;

    let base = game.checkin_base_url();
    let act_id = game.act_id();
    let sign_game = game.sign_game_header();

    let home_req = state
        .http
        .get(format!("{}/home?lang=vi-vn&act_id={}", base, act_id))
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .header("x-rpc-signgame", sign_game)
        .send();

    let info_req = state
        .http
        .get(format!("{}/info?lang=vi-vn&act_id={}", base, act_id))
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .header("x-rpc-signgame", sign_game)
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
            .post(format!("{}/sign", base))
            .header("Cookie", cookie)
            .header("User-Agent", UA)
            .header("Accept", "application/json")
            .header("x-rpc-signgame", sign_game)
            .json(&json!({ "act_id": act_id, "lang": "vi-vn" }))
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

    // Fetch role information for this game
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
        "game": game.slug(),
        "game_name": game.display_name(),
        "claimed": claimed,
        "nickname": selected.get("nickname"),
        "uid": selected.get("game_uid"),
        "server": HoyoGame::server_display_name(region_str),
        "level": selected.get("level"),
        "adventure_rank": selected.get("level"),
        "signed_days": total_signed,
        "total_days_month": total_days_month,
        "missing_days": total_days_month.saturating_sub(total_signed),
        "today_reward": today_reward,
        "tomorrow_reward": tomorrow_reward,
        "risk_code": risk_code,
    }))
}

pub async fn get_check_in_list(
    state: &AppState,
    game: HoyoGame,
    cookie: &str,
) -> AppResult<Value> {
    let base = game.checkin_base_url();
    let act_id = game.act_id();
    let sign_game = game.sign_game_header();

    let home_api = format!("{}/home?lang=vi-vn&act_id={}", base, act_id);
    let extra_api = format!("{}/extra_award?lang=vi-vn&act_id={}", base, act_id);

    let home_req = state
        .http
        .get(&home_api)
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .header("x-rpc-signgame", sign_game)
        .header("x-rpc-client_type", "4")
        .send();

    let extra_req = state
        .http
        .get(&extra_api)
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .header("x-rpc-signgame", sign_game)
        .header("x-rpc-client_type", "4")
        .send();

    let (home_res, extra_res) = tokio::try_join!(home_req, extra_req)?;
    let home_json: Value = home_res.json().await?;
    let extra_json: Value = extra_res.json().await.unwrap_or_else(|_| json!({}));

    let retcode = home_json.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    if retcode != 0 {
        return Ok(json!({
            "ok": false,
            "game": game.slug(),
            "retcode": retcode,
            "message": home_json.get("message").and_then(Value::as_str).unwrap_or("Failed to fetch check-in awards"),
        }));
    }

    let monthly_awards: Vec<Value> = home_json
        .pointer("/data/awards")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(idx, award)| {
                    json!({
                        "day": idx + 1,
                        "name": award.get("name"),
                        "count": award.get("cnt"),
                        "icon": award.get("icon"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let extra_awards: Vec<Value> = extra_json
        .pointer("/data/awards")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|extra| {
                    json!({
                        "id": extra.get("id"),
                        "sign_day_required": extra.get("sign_day"),
                        "count": extra.get("cnt"),
                        "icon": extra.get("icon"),
                        "is_highlight": extra.get("highlight").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "ok": true,
        "game": game.slug(),
        "month": home_json.pointer("/data/month"),
        "now_timestamp": home_json.pointer("/data/now"),
        "resign_available": home_json.pointer("/data/resign").and_then(Value::as_bool).unwrap_or(false),
        "monthly_awards": monthly_awards,
        "extra_awards": extra_awards,
        "has_month_card": extra_json.pointer("/data/mc/has_month_card").and_then(Value::as_bool).unwrap_or(false),
    }))
}
