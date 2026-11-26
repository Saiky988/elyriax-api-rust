use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    core::{
        auth::{AdminUser, AuthUser},
        response::{send_success, send_success_empty},
    },
    error::{AppError, AppResult},
    services::{
        genshin::{
            account::{
                add_account, delete_account, get_account_by_id, get_accounts,
                get_ready_genshin_payload, update_account,
            },
            banner::get_banners,
            checkin::check_in_daily,
            checkin_list::get_check_in_list,
            codes::get_all_codes,
            daily_note::get_daily_note,
            redeem::{auto_redeem_for_account, get_account_redeem_history, redeem_code},
            stats::get_role_and_stats,
        },
        hoyo::{
            feed::{get_game_calendar, get_game_news},
            HoyoGame,
        },
        scheduler::run_genshin_auto_redeem,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(genshin_get_router).post(genshin_post_router))
        .route("/codes", get(get_genshin_codes_ep))
        .route("/calendar", get(get_genshin_calendar))
        .route("/news/{kind}", get(get_genshin_news))
        .route("/role-stats", post(post_role_stats))
        .route("/daily-note", post(post_daily_note))
        .route("/stats", get(get_default_stats))
        // Account CRUD
        .route("/accounts", post(add_account_ep).get(get_accounts_ep))
        .route(
            "/accounts/{account_id}",
            get(get_account_detail_ep)
                .patch(update_account_ep)
                .delete(delete_account_ep),
        )
        // Account Actions
        .route("/accounts/{account_id}/stats", get(get_account_stats))
        .route("/accounts/{account_id}/daily-note", get(get_account_daily_note))
        .route("/accounts/{account_id}/checkin-list", get(get_account_checkin_list))
        .route("/accounts/{account_id}/checkin", post(post_account_checkin))
        // Auto Redeem Code Genshin Endpoints
        .route("/accounts/{account_id}/redeem", post(post_account_redeem))
        .route("/accounts/{account_id}/auto-redeem", post(post_account_auto_redeem))
        .route("/accounts/{account_id}/redeem-history", get(get_account_redeem_history_ep))
        .route("/auto-redeem/trigger", post(trigger_global_auto_redeem))
}

#[derive(Deserialize)]
struct GenshinQuery {
    #[serde(rename = "type")]
    query_type: Option<String>,
    refresh: Option<bool>,
    lang: Option<String>,
}

#[derive(Deserialize)]
struct LangQuery {
    lang: Option<String>,
    refresh: Option<bool>,
}

async fn genshin_get_router(
    State(state): State<AppState>,
    Query(q): Query<GenshinQuery>,
) -> AppResult<Response> {
    match q.query_type.as_deref() {
        Some("banner") => {
            // Try live banners from hoyoverse-api first, fall back to local data/banners.json
            let live = get_game_calendar(&state, HoyoGame::Genshin, q.lang.as_deref(), q.refresh.unwrap_or(false))
                .await
                .ok()
                .and_then(|cal| cal.get("banners").cloned())
                .filter(|b| b.as_array().map(|a| !a.is_empty()).unwrap_or(false));

            if let Some(live_banners) = live {
                return Ok(Json(json!({
                    "status": "success",
                    "source": "api.ennead.cc",
                    "timestamp": Utc::now().to_rfc3339(),
                    "data": live_banners,
                }))
                .into_response());
            }

            let banners = get_banners(&state).await?;
            Ok(Json(json!({
                "status": "success",
                "source": "local",
                "timestamp": Utc::now().to_rfc3339(),
                "data": banners,
            }))
            .into_response())
        }
        Some("codes") => {
            let cards = get_all_codes(&state, q.refresh.unwrap_or(false)).await?;
            let total_codes: usize = cards.iter().map(|c| c.codes.len()).sum();
            Ok(Json(json!({
                "status": "success",
                "timestamp": Utc::now().to_rfc3339(),
                "count": total_codes,
                "cards": cards,
            }))
            .into_response())
        }
        _ => Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "message": "Missing or invalid query. Use ?type=banner or ?type=codes",
            })),
        )
            .into_response()),
    }
}

async fn get_genshin_calendar(
    State(state): State<AppState>,
    Query(q): Query<LangQuery>,
) -> AppResult<Json<Value>> {
    let data = get_game_calendar(
        &state,
        HoyoGame::Genshin,
        q.lang.as_deref(),
        q.refresh.unwrap_or(false),
        q.force.unwrap_or(false),
    )
    .await?;
    Ok(Json(json!({
        "status": "success",
        "game": "genshin",
        "timestamp": Utc::now().to_rfc3339(),
        "data": data,
    })))
}

async fn get_genshin_news(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Query(q): Query<LangQuery>,
) -> AppResult<Json<Value>> {
    let data = get_game_news(
        &state,
        HoyoGame::Genshin,
        &kind,
        q.lang.as_deref(),
        q.refresh.unwrap_or(false),
        q.force.unwrap_or(false),
    )
    .await?;
    Ok(Json(json!({
        "status": "success",
        "game": "genshin",
        "kind": kind,
        "timestamp": Utc::now().to_rfc3339(),
        "data": data,
    })))
}

async fn genshin_post_router(
    State(state): State<AppState>,
    Query(q): Query<GenshinQuery>,
    Json(body): Json<Value>,
) -> AppResult<Response> {
    let cookie = body.get("cookie").and_then(Value::as_str).unwrap_or("");
    let server = body.get("server").and_then(Value::as_str).unwrap_or("");

    match q.query_type.as_deref() {
        Some("checkindaily") => {
            let res = check_in_daily(&state, cookie, server).await?;
            Ok(Json(res).into_response())
        }
        Some("redeemcode") => {
            let code = body.get("code").and_then(Value::as_str).unwrap_or("");
            let uid = body.get("uid").and_then(Value::as_str);
            let res = redeem_code(&state, cookie, server, code, uid).await?;
            Ok(Json(res).into_response())
        }
        _ => Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "Missing or invalid query" })),
        )
            .into_response()),
    }
}

async fn post_role_stats(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let cookie = body.get("cookie").and_then(Value::as_str).unwrap_or("");
    let uid = body.get("uid").and_then(Value::as_str).unwrap_or("");
    let server = body.get("server").and_then(Value::as_str).unwrap_or("");
    Ok(Json(get_role_and_stats(&state, cookie, uid, server).await?))
}

async fn post_daily_note(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let cookie = body.get("cookie").and_then(Value::as_str).unwrap_or("");
    let uid = body.get("uid").and_then(Value::as_str).unwrap_or("");
    let server = body.get("server").and_then(Value::as_str).unwrap_or("");
    Ok(Json(get_daily_note(&state, cookie, uid, server).await?))
}

async fn get_default_stats(State(state): State<AppState>, user: AuthUser) -> AppResult<Response> {
    let payload = get_ready_genshin_payload(&state, user.id, None).await?;
    let res = get_role_and_stats(&state, &payload.cookie, &payload.uid, &payload.server).await?;
    let status = if res.get("ok").and_then(Value::as_bool) == Some(true) {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    Ok((status, Json(res)).into_response())
}

async fn get_account_stats(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
) -> AppResult<Response> {
    let payload = get_ready_genshin_payload(&state, user.id, Some(account_id)).await?;
    let res = get_role_and_stats(&state, &payload.cookie, &payload.uid, &payload.server).await?;
    let status = if res.get("ok").and_then(Value::as_bool) == Some(true) {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    Ok((status, Json(res)).into_response())
}

async fn get_account_daily_note(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
) -> AppResult<Response> {
    let payload = get_ready_genshin_payload(&state, user.id, Some(account_id)).await?;
    let res = get_daily_note(&state, &payload.cookie, &payload.uid, &payload.server).await?;
    let status = if res.get("ok").and_then(Value::as_bool) == Some(true) {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    Ok((status, Json(res)).into_response())
}

async fn get_account_checkin_list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
) -> AppResult<Response> {
    let payload = get_ready_genshin_payload(&state, user.id, Some(account_id)).await?;
    let res = get_check_in_list(&state, &payload.cookie).await?;
    let status = if res.get("ok").and_then(Value::as_bool) == Some(true) {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    Ok((status, Json(res)).into_response())
}

async fn post_account_checkin(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let payload = get_ready_genshin_payload(&state, user.id, Some(account_id)).await?;
    let res = check_in_daily(&state, &payload.cookie, &payload.server).await?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct RedeemSingleBody {
    code: Option<String>,
}

async fn post_account_redeem(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
    Json(body): Json<RedeemSingleBody>,
) -> AppResult<Json<Value>> {
    let code = body
        .code
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu giftcode cần nhập (code)"))?;
    let payload = get_ready_genshin_payload(&state, user.id, Some(account_id)).await?;
    let res = redeem_code(
        &state,
        &payload.cookie,
        &payload.server,
        &code,
        Some(&payload.uid),
    )
    .await?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct AutoRedeemQuery {
    refresh: Option<bool>,
    force: Option<bool>,
}

async fn post_account_auto_redeem(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
    Query(q): Query<AutoRedeemQuery>,
) -> AppResult<Json<Value>> {
    let payload = get_ready_genshin_payload(&state, user.id, Some(account_id)).await?;
    let summary = auto_redeem_for_account(
        &state,
        payload.account_id,
        &payload.cookie,
        &payload.uid,
        &payload.server,
        &payload.nickname,
        user.email.as_deref(),
        q.refresh.unwrap_or(false),
        q.force.unwrap_or(false),
    )
    .await?;
    Ok(send_success("Hoàn tất tự động nhập giftcode Genshin", summary))
}

async fn get_account_redeem_history_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let _ = get_account_by_id(&state, user.id, account_id).await?;
    let history = get_account_redeem_history(&state, account_id).await;
    Ok(send_success("Lấy lịch sử nhập giftcode thành công", history))
}

async fn trigger_global_auto_redeem(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Json<Value> {
    let st = state.clone();
    tokio::spawn(async move {
        run_genshin_auto_redeem(st).await;
    });
    send_success_empty("Đã kích hoạt tiến trình Auto Redeem Code toàn hệ thống (Genshin, HSR, ZZZ)")
}

#[derive(Deserialize)]
struct AddAccountBody {
    cookie: Option<String>,
    uid: Option<String>,
    server: Option<String>,
}

async fn add_account_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<AddAccountBody>,
) -> AppResult<Json<Value>> {
    let cookie = body
        .cookie
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu thông tin cookie, uid hoặc server."))?;
    let uid = body
        .uid
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu thông tin cookie, uid hoặc server."))?;
    let server = body
        .server
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu thông tin cookie, uid hoặc server."))?;

    let data = add_account(&state, user.id, &cookie, &uid, &server).await?;
    Ok(send_success("Thêm tài khoản Genshin thành công", data))
}

async fn get_accounts_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let data = get_accounts(&state, user.id).await?;
    Ok(send_success("Lấy danh sách tài khoản thành công", data))
}

async fn get_account_detail_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let data = get_account_by_id(&state, user.id, account_id).await?;
    Ok(send_success("Lấy chi tiết tài khoản thành công", data))
}

async fn update_account_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
    Json(body): Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    update_account(&state, user.id, account_id, &body).await?;
    Ok(send_success_empty("Cập nhật cài đặt tài khoản thành công"))
}

async fn delete_account_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<i64>,
) -> AppResult<Json<Value>> {
    delete_account(&state, user.id, account_id).await?;
    Ok(send_success_empty("Đã xóa tài khoản khỏi hệ thống"))
}

async fn get_genshin_codes_ep(
    State(state): State<AppState>,
    Query(q): Query<LangQuery>,
) -> AppResult<Json<Value>> {
    let cards = get_all_codes(&state, q.refresh.unwrap_or(false)).await?;
    let total_codes: usize = cards.iter().map(|c| c.codes.len()).sum();
    Ok(Json(json!({
        "status": "success",
        "game": "genshin",
        "timestamp": Utc::now().to_rfc3339(),
        "count": total_codes,
        "cards": cards,
    })))
}
