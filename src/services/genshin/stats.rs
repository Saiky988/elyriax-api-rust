use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    services::hoyo::generate_os_ds,
    state::AppState,
};

pub async fn get_role_and_stats(
    state: &AppState,
    cookie: &str,
    uid: &str,
    server: &str,
) -> AppResult<Value> {
    if cookie.is_empty() || uid.is_empty() || server.is_empty() {
        return Err(AppError::bad_request(
            "Missing required parameters: cookie, uid, or server",
        ));
    }

    let url = format!(
        "https://sg-act-public-api.hoyolab.com/event/game_record/genshin/api/index?avatar_list_type=1&server={}&role_id={}",
        server, uid
    );

    let res_data: Value = state
        .http
        .get(&url)
        .header("Cookie", cookie)
        .header("DS", generate_os_ds())
        .header("Accept", "application/json, text/plain, */*")
        .header("x-rpc-client_type", "5")
        .header("x-rpc-app_version", "1.5.0")
        .header("x-rpc-language", "vi-vn")
        .header("x-rpc-env", "default")
        .header("x-rpc-lang", "vi-vn")
        .header("x-rpc-device_id", "3d6d7575-0aa3-4f16-b913-ed082eaf7074")
        .header("x-rpc-device_fp", "38d7f7634fe00")
        .header("x-rpc-platform", "5")
        .header("x-rpc-page", "v6.7.1-gr-sea_#/ys")
        .header("Cache-Control", "no-cache")
        .send()
        .await?
        .json()
        .await?;

    let retcode = res_data.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    if retcode != 0 {
        return Ok(json!({
            "ok": false,
            "retcode": retcode,
            "message": res_data.get("message").and_then(Value::as_str).unwrap_or("Failed to fetch data from HoYoLab API"),
        }));
    }

    let data = res_data.get("data").cloned().unwrap_or_else(|| json!({}));
    let role_data = data.get("role").cloned().unwrap_or_else(|| json!({}));
    let stats_data = data.get("stats").cloned().unwrap_or_else(|| json!({}));

    let avatars: Vec<Value> = data
        .get("avatars")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|av| {
                    let weapon = av.get("weapon").filter(|w| w.is_object()).map(|w| {
                        json!({
                            "name": w.get("name"),
                            "level": w.get("level"),
                            "rarity": w.get("rarity"),
                            "affix_level": w.get("affix_level"),
                        })
                    });

                    let relics: Vec<Value> = av
                        .get("relics")
                        .and_then(Value::as_array)
                        .map(|r_arr| {
                            r_arr
                                .iter()
                                .map(|re| {
                                    json!({
                                        "name": re.get("name"),
                                        "pos_name": re.get("pos_name"),
                                        "rarity": re.get("rarity"),
                                        "level": re.get("level"),
                                        "set_name": re.pointer("/set/name"),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    json!({
                        "id": av.get("id"),
                        "name": av.get("name"),
                        "element": av.get("element"),
                        "level": av.get("level"),
                        "rarity": av.get("rarity"),
                        "fetter": av.get("fetter"),
                        "constellation": av.get("actived_constellation_num"),
                        "image": av.get("image"),
                        "weapon": weapon,
                        "relics": relics,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let avatar_url = role_data
        .get("AvatarUrl")
        .filter(|v| !v.is_null() && v.as_str().map(|s| !s.is_empty()).unwrap_or(false))
        .or_else(|| role_data.get("game_head_icon"));

    Ok(json!({
        "ok": true,
        "role": {
            "nickname": role_data.get("nickname"),
            "level": role_data.get("level"),
            "avatar_url": avatar_url,
        },
        "stats": {
            "active_days": stats_data.get("active_day_number"),
            "achievements": stats_data.get("achievement_number"),
            "avatars_count": stats_data.get("avatar_number"),
            "way_points": stats_data.get("way_point_number"),
            "domains": stats_data.get("domain_number"),
            "spiral_abyss": stats_data.get("spiral_abyss"),
            "chests": {
                "common": stats_data.get("common_chest_number"),
                "exquisite": stats_data.get("exquisite_chest_number"),
                "precious": stats_data.get("precious_chest_number"),
                "luxurious": stats_data.get("luxurious_chest_number"),
                "magic": stats_data.get("magic_chest_number"),
            },
            "oculus": {
                "anemo": stats_data.get("anemoculus_number"),
                "geo": stats_data.get("geoculus_number"),
                "electro": stats_data.get("electroculus_number"),
                "dendro": stats_data.get("dendroculus_number"),
                "hydro": stats_data.get("hydroculus_number"),
                "pyro": stats_data.get("pyroculus_number"),
            },
        },
        "avatars": avatars,
    }))
}
