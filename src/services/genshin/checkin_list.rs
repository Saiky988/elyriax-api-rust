use serde_json::{json, Value};

use crate::{
    error::AppResult,
    services::genshin::checkin::ACT_ID,
    state::AppState,
};

pub async fn get_check_in_list(state: &AppState, cookie: &str) -> AppResult<Value> {
    let home_api = format!(
        "https://sg-act-public-api.hoyolab.com/event/sol/home?lang=vi-vn&act_id={}",
        ACT_ID
    );
    let extra_api = format!(
        "https://sg-act-public-api.hoyolab.com/event/sol/extra_award?lang=vi-vn&act_id={}",
        ACT_ID
    );

    let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

    let home_req = state
        .http
        .get(&home_api)
        .header("Cookie", cookie)
        .header("User-Agent", ua)
        .header("Accept", "application/json")
        .header("x-rpc-signgame", "hk4e")
        .header("x-rpc-client_type", "4")
        .send();

    let extra_req = state
        .http
        .get(&extra_api)
        .header("Cookie", cookie)
        .header("User-Agent", ua)
        .header("Accept", "application/json")
        .header("x-rpc-signgame", "hk4e")
        .header("x-rpc-client_type", "4")
        .send();

    let (home_res, extra_res) = tokio::try_join!(home_req, extra_req)?;
    let home_json: Value = home_res.json().await?;
    let extra_json: Value = extra_res.json().await?;

    let retcode = home_json.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    if retcode != 0 {
        return Ok(json!({
            "ok": false,
            "retcode": retcode,
            "message": home_json.get("message").and_then(Value::as_str).unwrap_or("Failed to fetch main check-in awards"),
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
        "month": home_json.pointer("/data/month"),
        "now_timestamp": home_json.pointer("/data/now"),
        "resign_available": home_json.pointer("/data/resign").and_then(Value::as_bool).unwrap_or(false),
        "monthly_awards": monthly_awards,
        "extra_awards": extra_awards,
        "has_month_card": extra_json.pointer("/data/mc/has_month_card").and_then(Value::as_bool).unwrap_or(false),
    }))
}
