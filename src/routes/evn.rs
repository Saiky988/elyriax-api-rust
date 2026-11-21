use std::time::Duration;

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{Duration as ChronoDuration, FixedOffset, Utc};
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    state::{AppState, CachedValue},
};

const EVN_BASE_URL: &str = "https://www.cskh.evnspc.vn/TraCuu";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/provinces", get(get_evn_provinces))
        .route("/units", get(get_evn_units))
        .route("/outage", get(get_evn_outage))
}

async fn get_evn_provinces() -> Json<Value> {
    Json(json!({
        "success": true,
        "data": [
            {"code": "PB01", "name": "Đồng Nai"},
            {"code": "PB03", "name": "Lâm Đồng"},
            {"code": "PB05", "name": "Tây Ninh"},
            {"code": "PB07", "name": "Đồng Tháp"},
            {"code": "PB10", "name": "Vĩnh Long"},
            {"code": "PB11", "name": "TP Cần Thơ"},
            {"code": "PB12", "name": "An Giang"},
            {"code": "PB14", "name": "Cà Mau"}
        ]
    }))
}

#[derive(Deserialize)]
struct UnitsQuery {
    #[serde(rename = "provinceCode")]
    province_code: Option<String>,
}

async fn get_evn_units(
    State(state): State<AppState>,
    Query(query): Query<UnitsQuery>,
) -> AppResult<Json<Value>> {
    let province_code = query
        .province_code
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu tham số provinceCode"))?;

    if let Some(entry) = state.evn_units_cache.get(&province_code) {
        if entry.is_valid() {
            return Ok(Json(json!({ "success": true, "data": entry.value })));
        }
    }

    let url = format!(
        "{}/GetDanhMucDienLuc?pMA_DVICTREN={}",
        EVN_BASE_URL,
        urlencoding::encode(&province_code)
    );

    let html = state
        .http
        .get(&url)
        .header("User-Agent", UA)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Accept", "text/html, */*; q=0.01")
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .text()
        .await?;

    let doc = Html::parse_fragment(&html);
    let opt_sel = Selector::parse("option").expect("valid selector");
    let mut units = Vec::new();
    for el in doc.select(&opt_sel) {
        if let Some(val) = el.value().attr("value").filter(|v| !v.is_empty()) {
            let name: String = el.text().collect();
            units.push(json!({ "code": val, "name": name.trim() }));
        }
    }

    let data = Value::Array(units);
    state.evn_units_cache.insert(
        province_code,
        CachedValue::new(data.clone(), Duration::from_secs(3600)),
    );

    Ok(Json(json!({ "success": true, "data": data })))
}

#[derive(Deserialize)]
struct OutageQuery {
    #[serde(rename = "type")]
    query_type: Option<String>,
    code: Option<String>,
    #[serde(rename = "tuNgay")]
    tu_ngay: Option<String>,
    #[serde(rename = "denNgay")]
    den_ngay: Option<String>,
}

fn parse_outage_html(html: &str) -> Value {
    let doc = Html::parse_document(html);
    let red_sel = Selector::parse(".notification .ttl .red").expect("valid selector");
    let span_sel = Selector::parse(".notification .ttl span").expect("valid selector");
    let entry_sel = Selector::parse(".entry").expect("valid selector");
    let where_sel = Selector::parse(".where").expect("valid selector");
    let time_sel = Selector::parse(".time span").expect("valid selector");
    let cause_sel = Selector::parse(".cause span").expect("valid selector");
    let ws_re = Regex::new(r"\s+").expect("valid regex");

    let no_msg = doc
        .select(&red_sel)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    if no_msg.to_lowercase().contains("không có lịch") {
        return json!({
            "title": "",
            "unitName": "",
            "hasOutage": false,
            "message": "Hiện tại khách hàng/đơn vị không có lịch ngừng giảm cung cấp điện",
            "schedules": [],
        });
    }

    let unit_name = doc
        .select(&span_sel)
        .next()
        .map(|el| {
            el.text()
                .collect::<String>()
                .replace("Đơn vị:", "")
                .trim()
                .to_string()
        })
        .unwrap_or_default();

    let mut schedules = Vec::new();
    for el in doc.select(&entry_sel) {
        let location = el
            .select(&where_sel)
            .next()
            .map(|w| {
                w.text()
                    .collect::<String>()
                    .replace("KHU VỰC:", "")
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();

        let time_text = el
            .select(&time_sel)
            .next()
            .map(|t| {
                let raw: String = t.text().collect();
                ws_re.replace_all(&raw, " ").trim().to_string()
            })
            .unwrap_or_default();

        let cause = el
            .select(&cause_sel)
            .next()
            .map(|c| c.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if !location.is_empty() || !time_text.is_empty() {
            schedules.push(json!({
                "location": location,
                "time": time_text,
                "cause": cause,
            }));
        }
    }

    json!({
        "title": no_msg,
        "unitName": unit_name,
        "hasOutage": true,
        "schedules": schedules,
    })
}

async fn get_evn_outage(
    State(state): State<AppState>,
    Query(query): Query<OutageQuery>,
) -> AppResult<Json<Value>> {
    let q_type = query
        .query_type
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::bad_request(
                "Vui lòng cung cấp tham số type (\"customer\" hoặc \"unit\") và code",
            )
        })?;
    let code = query.code.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        AppError::bad_request("Vui lòng cung cấp tham số type (\"customer\" hoặc \"unit\") và code")
    })?;

    let tz_vn = FixedOffset::east_opt(7 * 3600).expect("valid UTC+7");
    let today = Utc::now().with_timezone(&tz_vn).date_naive();
    let start_date = query
        .tu_ngay
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| today.format("%d-%m-%Y").to_string());
    let end_date = query
        .den_ngay
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| (today + ChronoDuration::days(4)).format("%d-%m-%Y").to_string());

    let url = match q_type.as_str() {
        "customer" => format!(
            "{}/GetThongTinLichNgungGiamCungCapDien?tuNgay={}&denNgay={}&maKH={}&ChucNang=MaKhachHang",
            EVN_BASE_URL,
            urlencoding::encode(&start_date),
            urlencoding::encode(&end_date),
            urlencoding::encode(&code),
        ),
        "unit" => format!(
            "{}/GetThongTinLichNgungGiamCungCapDien?madvi={}&tuNgay={}&denNgay={}&ChucNang=MaDonVi",
            EVN_BASE_URL,
            urlencoding::encode(&code),
            urlencoding::encode(&start_date),
            urlencoding::encode(&end_date),
        ),
        _ => {
            return Err(AppError::bad_request(
                "Type không hợp lệ. Chỉ chấp nhận \"customer\" hoặc \"unit\"",
            ));
        }
    };

    let html = state
        .http
        .get(&url)
        .header("User-Agent", UA)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Accept", "text/html, */*; q=0.01")
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .text()
        .await?;

    let parsed = parse_outage_html(&html);

    Ok(Json(json!({
        "success": true,
        "query": {
            "type": q_type,
            "code": code,
            "tuNgay": start_date,
            "denNgay": end_date,
        },
        "data": parsed,
    })))
}
