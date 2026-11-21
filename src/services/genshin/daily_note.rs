use chrono::{Duration, FixedOffset, Utc};
use serde_json::{json, Value};

use crate::{error::AppResult, state::AppState};

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

fn parse_i64_field(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn parse_f64_field(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub async fn get_daily_note(
    state: &AppState,
    cookie: &str,
    uid: &str,
    server: &str,
) -> AppResult<Value> {
    if cookie.is_empty() || uid.is_empty() || server.is_empty() {
        return Ok(json!({ "ok": false, "error": "Thiếu cookie, uid hoặc server" }));
    }

    let url = format!(
        "https://sg-act-public-api.hoyolab.com/event/game_record/app/genshin/api/dailyNote?server={}&role_id={}",
        server, uid
    );

    let res_data: Value = state
        .http
        .get(&url)
        .header("Cookie", cookie)
        .header("Accept", "application/json, text/plain, */*")
        .header(
            "User-Agent",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) miHoYoBBS/2.50.1",
        )
        .header("X-Rpc-Client_type", "2")
        .header("X-Rpc-App_version", "2.50.1")
        .send()
        .await?
        .json()
        .await?;

    let retcode = res_data.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    if retcode != 0 {
        return Ok(json!({
            "ok": false,
            "error": res_data.get("message").and_then(Value::as_str).unwrap_or("Không thể lấy dữ liệu từ HoYoLAB"),
            "retcode": retcode,
        }));
    }

    let raw = res_data.get("data").cloned().unwrap_or_else(|| json!({}));
    let resin_rec_sec = parse_i64_field(raw.get("resin_recovery_time"));
    let home_coin_rec_sec = parse_i64_field(raw.get("home_coin_recovery_time"));

    let current_resin = parse_i64_field(raw.get("current_resin"));
    let max_resin = parse_i64_field(raw.get("max_resin"));
    let remain_discount = parse_i64_field(raw.get("remain_resin_discount_num"));
    let limit_discount = raw
        .get("resin_discount_num_limit")
        .and_then(Value::as_i64)
        .unwrap_or(3);

    let finished_task = parse_i64_field(raw.get("finished_task_num"));
    let total_task = parse_i64_field(raw.get("total_task_num"));
    let extra_received = raw
        .get("is_extra_task_reward_received")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let stored_countdown = parse_i64_field(
        raw.pointer("/daily_task/stored_attendance_refresh_countdown"),
    );
    let stored_attendance = parse_f64_field(
        raw.pointer("/daily_task/stored_attendance")
            .or_else(|| raw.get("stored_attendance")),
    );

    let current_home_coin = parse_i64_field(raw.get("current_home_coin"));
    let max_home_coin = parse_i64_field(raw.get("max_home_coin"));

    let expeditions: Vec<Value> = raw
        .get("expeditions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|exp| {
                    let rem = parse_i64_field(exp.get("remained_time"));
                    json!({
                        "avatar_icon": exp.get("avatar_side_icon"),
                        "status": exp.get("status"),
                        "remained_time_seconds": rem,
                        "estimated_finished_time": calculate_estimated_time(rem),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let default_recovery = json!({
        "Day": 0, "Hour": 0, "Minute": 0, "Second": 0, "reached": true
    });

    Ok(json!({
        "ok": true,
        "data": {
            "resin": {
                "current": current_resin,
                "max": max_resin,
                "is_full": current_resin == max_resin,
                "recovery_time_seconds": resin_rec_sec,
                "estimated_full_time": calculate_estimated_time(resin_rec_sec),
                "resin_discount": {
                    "remain_num": remain_discount,
                    "limit_num": limit_discount,
                    "is_used_up": remain_discount == 0,
                },
            },
            "daily_tasks": {
                "finished_num": finished_task,
                "total_num": total_task,
                "is_extra_reward_received": extra_received,
                "is_all_finished": finished_task == total_task && extra_received,
                "task_rewards": raw.pointer("/daily_task/task_rewards").cloned().unwrap_or_else(|| json!([])),
                "attendance_rewards": raw.pointer("/daily_task/attendance_rewards").cloned().unwrap_or_else(|| json!([])),
                "attendance_visible": raw.pointer("/daily_task/attendance_visible").and_then(Value::as_bool).unwrap_or(true),
                "stored_attendance": stored_attendance,
                "stored_attendance_refresh_countdown_seconds": stored_countdown,
                "estimated_refresh_time": calculate_estimated_time(stored_countdown),
            },
            "expeditions": {
                "current_num": raw.get("current_expedition_num"),
                "max_num": raw.get("max_expedition_num"),
                "list": expeditions,
            },
            "home_coin": {
                "current": current_home_coin,
                "max": max_home_coin,
                "is_full": current_home_coin == max_home_coin,
                "recovery_time_seconds": home_coin_rec_sec,
                "estimated_full_time": calculate_estimated_time(home_coin_rec_sec),
            },
            "transformer": {
                "obtained": raw.pointer("/transformer/obtained").and_then(Value::as_bool).unwrap_or(false),
                "ready": raw.pointer("/transformer/recovery_time/reached").and_then(Value::as_bool).unwrap_or(false),
                "recovery_time": raw.pointer("/transformer/recovery_time").cloned().unwrap_or(default_recovery),
                "latest_job_id": raw.pointer("/transformer/latest_job_id").and_then(Value::as_str).unwrap_or("0"),
            },
            "archon_quest_progress": {
                "is_open": raw.pointer("/archon_quest_progress/is_open_archon_quest").and_then(Value::as_bool).unwrap_or(true),
                "is_finish_all_mainline": raw.pointer("/archon_quest_progress/is_finish_all_mainline").and_then(Value::as_bool).unwrap_or(false),
                "is_finish_all_interchapter": raw.pointer("/archon_quest_progress/is_finish_all_interchapter").and_then(Value::as_bool).unwrap_or(false),
                "list": raw.pointer("/archon_quest_progress/list").cloned().unwrap_or_else(|| json!([])),
            },
            "week_active_progress": {
                "unlock": raw.pointer("/week_active_progress/unlock").and_then(Value::as_bool).unwrap_or(false),
                "progress_current": raw.pointer("/week_active_progress/progress_current").and_then(Value::as_i64).unwrap_or(0),
                "progress_total": raw.pointer("/week_active_progress/progress_total").and_then(Value::as_i64).unwrap_or(5),
                "period_progress_current": raw.pointer("/week_active_progress/period_progress_current").and_then(Value::as_i64).unwrap_or(0),
                "period_progress_total": raw.pointer("/week_active_progress/period_progress_total").and_then(Value::as_i64).unwrap_or(8),
                "current_weekday": raw.pointer("/week_active_progress/current_weekday").and_then(Value::as_i64).unwrap_or(0),
                "is_active_period": raw.pointer("/week_active_progress/is_active_period").and_then(Value::as_bool).unwrap_or(false),
            },
            "calendar_url": raw.get("calendar_url").and_then(Value::as_str).unwrap_or(""),
        }
    }))
}
