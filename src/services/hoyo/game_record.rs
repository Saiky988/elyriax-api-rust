use chrono::{Duration, FixedOffset, Utc};
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    services::{
        genshin::{daily_note as gi_daily_note, stats as gi_stats},
        hoyo::{generate_os_ds, HoyoGame},
    },
    state::AppState,
};

const UA_MOBILE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) miHoYoBBS/2.50.1";

fn calculate_estimated_time(seconds_remaining: i64) -> String {
    if seconds_remaining <= 0 {
        return "Đã đầy/Xong".to_string();
    }
    let tz_vn = FixedOffset::east_opt(7 * 3600).expect("valid UTC+7 offset");
    let now = Utc::now().with_timezone(&tz_vn);
    let future = now + Duration::seconds(seconds_remaining);
    let hm = future.format("%H:%M").to_string();

    if now.date_naive() == future.date_naive() {
        format!("Hôm nay {}", hm)
    } else {
        format!("Ngày mai {}", hm)
    }
}

fn i64_val(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

pub async fn get_daily_note(
    state: &AppState,
    game: HoyoGame,
    cookie: &str,
    uid: &str,
    server: &str,
) -> AppResult<Value> {
    if cookie.is_empty() || uid.is_empty() || server.is_empty() {
        return Ok(json!({ "ok": false, "error": "Thiếu cookie, uid hoặc server" }));
    }

    let region = game
        .normalize_region(server)
        .unwrap_or(server);

    if game == HoyoGame::Genshin {
        return gi_daily_note::get_daily_note(state, cookie, uid, region).await;
    }

    let url = format!("{}?server={}&role_id={}", game.daily_note_url(), region, uid);
    let ds = generate_os_ds();

    let res_data: Value = state
        .http
        .get(&url)
        .header("Cookie", cookie)
        .header("Accept", "application/json, text/plain, */*")
        .header("User-Agent", UA_MOBILE)
        .header("DS", ds)
        .header("x-rpc-client_type", "5")
        .header("x-rpc-app_version", "1.5.0")
        .header("x-rpc-language", "vi-vn")
        .send()
        .await?
        .json()
        .await?;

    let retcode = res_data.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    if retcode != 0 {
        return Ok(json!({
            "ok": false,
            "game": game.slug(),
            "error": res_data.get("message").and_then(Value::as_str).unwrap_or("Không thể lấy dữ liệu từ HoYoLAB"),
            "retcode": retcode,
        }));
    }

    let raw = res_data.get("data").cloned().unwrap_or_else(|| json!({}));

    match game {
        HoyoGame::Hsr => {
            let stamina = i64_val(raw.get("current_stamina"));
            let max_stamina = i64_val(raw.get("max_stamina"));
            let recover_sec = i64_val(raw.get("stamina_recover_time"));

            let expeditions: Vec<Value> = raw
                .get("expeditions")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .map(|exp| {
                            let rem = i64_val(exp.get("remaining_time"));
                            json!({
                                "name": exp.get("name"),
                                "status": exp.get("status"),
                                "avatars": exp.get("avatars"),
                                "remaining_time_seconds": rem,
                                "estimated_finished_time": calculate_estimated_time(rem),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(json!({
                "ok": true,
                "game": "hsr",
                "data": {
                    "trailblaze_power": {
                        "current": stamina,
                        "max": max_stamina,
                        "is_full": stamina >= max_stamina,
                        "recovery_time_seconds": recover_sec,
                        "estimated_full_time": calculate_estimated_time(recover_sec),
                    },
                    "reserve_trailblaze_power": {
                        "current": i64_val(raw.get("current_reserve_stamina")),
                        "is_full": raw.get("is_reserve_stamina_full").and_then(Value::as_bool).unwrap_or(false),
                    },
                    "daily_training": {
                        "current_score": i64_val(raw.get("current_train_score")),
                        "max_score": i64_val(raw.get("max_train_score")),
                    },
                    "simulated_universe": {
                        "current_score": i64_val(raw.get("current_rogue_score")),
                        "max_score": i64_val(raw.get("max_rogue_score")),
                    },
                    "echo_of_war": {
                        "remain_num": i64_val(raw.get("weekly_cocoon_cnt")),
                        "limit_num": i64_val(raw.get("weekly_cocoon_limit")),
                    },
                    "expeditions": {
                        "accepted": i64_val(raw.get("accepted_epedition_num")),
                        "total": i64_val(raw.get("total_expedition_num")),
                        "list": expeditions,
                    },
                }
            }))
        }
        HoyoGame::Zzz => {
            let energy_cur = i64_val(raw.pointer("/energy/progress/current"));
            let energy_max = i64_val(raw.pointer("/energy/progress/max"));
            let restore_sec = i64_val(raw.pointer("/energy/restore"));

            Ok(json!({
                "ok": true,
                "game": "zzz",
                "data": {
                    "battery_charge": {
                        "current": energy_cur,
                        "max": energy_max,
                        "is_full": energy_cur >= energy_max,
                        "recovery_time_seconds": restore_sec,
                        "estimated_full_time": calculate_estimated_time(restore_sec),
                    },
                    "engagement": {
                        "current": i64_val(raw.pointer("/vitality/current")),
                        "max": i64_val(raw.pointer("/vitality/max")),
                    },
                    "scratch_card": raw.get("card_sign"),
                    "video_store": raw.pointer("/vhs_sale/sale_state"),
                    "bounty_commission": raw.get("bounty_commission"),
                    "ridu_weekly": raw.get("weekly_task"),
                    "survey_points": raw.get("survey_points"),
                }
            }))
        }
        HoyoGame::Genshin => unreachable!(),
    }
}

pub async fn get_role_and_stats(
    state: &AppState,
    game: HoyoGame,
    cookie: &str,
    uid: &str,
    server: &str,
) -> AppResult<Value> {
    if cookie.is_empty() || uid.is_empty() || server.is_empty() {
        return Err(AppError::bad_request(
            "Missing required parameters: cookie, uid, or server",
        ));
    }

    let region = game
        .normalize_region(server)
        .unwrap_or(server);

    if game == HoyoGame::Genshin {
        return gi_stats::get_role_and_stats(state, cookie, uid, region).await;
    }

    let url = format!("{}?server={}&role_id={}", game.stats_url(), region, uid);
    let ds = generate_os_ds();

    let res_data: Value = state
        .http
        .get(&url)
        .header("Cookie", cookie)
        .header("Accept", "application/json, text/plain, */*")
        .header("User-Agent", UA_MOBILE)
        .header("DS", ds)
        .header("x-rpc-client_type", "5")
        .header("x-rpc-app_version", "1.5.0")
        .header("x-rpc-language", "vi-vn")
        .send()
        .await?
        .json()
        .await?;

    let retcode = res_data.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    if retcode != 0 {
        return Ok(json!({
            "ok": false,
            "game": game.slug(),
            "retcode": retcode,
            "message": res_data.get("message").and_then(Value::as_str).unwrap_or("Failed to fetch data from HoYoLAB API"),
        }));
    }

    let data = res_data.get("data").cloned().unwrap_or_else(|| json!({}));

    match game {
        HoyoGame::Hsr => Ok(json!({
            "ok": true,
            "game": "hsr",
            "stats": data.get("stats"),
            "avatars": data.get("avatar_list").cloned().unwrap_or_else(|| json!([])),
            "cur_head_icon_url": data.get("cur_head_icon_url"),
        })),
        HoyoGame::Zzz => Ok(json!({
            "ok": true,
            "game": "zzz",
            "stats": data.get("stats"),
            "agents": data.get("avatar_list").cloned().unwrap_or_else(|| json!([])),
            "bangboos": data.get("buddy_list").cloned().unwrap_or_else(|| json!([])),
            "cur_head_icon_url": data.get("cur_head_icon_url"),
        })),
        HoyoGame::Genshin => unreachable!(),
    }
}
