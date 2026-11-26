use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    core::{
        auth::AuthUser,
        response::{send_success, send_success_empty},
    },
    error::{AppError, AppResult},
    services::hoyo::{
        account::{
            add_account, delete_account, discover_all_roles_from_cookie, get_account_by_id,
            get_accounts, get_all_ready_payloads_for_user, get_all_user_accounts,
            get_ready_payload, link_all_games_with_single_cookie, update_account,
            update_shared_cookie_for_user,
        },
        checkin::{check_in_daily, get_check_in_list},
        feed::{get_all_games_codes, get_game_calendar, get_game_codes, get_game_news},
        game_record::{get_daily_note, get_role_and_stats},
        redeem::{auto_redeem_for_account, get_account_redeem_history, redeem_code},
        HoyoGame,
    },
    state::AppState,
};

/// Unified multi-game routes mounted at `/v1/hoyo`
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/codes", get(all_games_codes_ep))
        // One-Cookie Multi-Game Management (1 Cookie -> Auto-discover & sync Genshin + HSR + ZZZ)
        .route("/accounts", get(all_user_accounts_ep))
        .route("/accounts/preview", post(preview_single_cookie_ep))
        .route("/accounts/link", post(link_single_cookie_ep))
        .route("/accounts/cookie", patch(update_shared_cookie_ep))
        .route("/checkin-all", post(checkin_all_user_games_ep))
        .route("/auto-redeem-all", post(auto_redeem_all_user_games_ep))
        // Per-Game Dynamic Routes
        .route("/{game}/codes", get(game_codes_ep))
        .route("/{game}/calendar", get(game_calendar_ep))
        .route("/{game}/news/{kind}", get(game_news_ep))
        .route("/{game}/checkin", post(direct_checkin_ep))
        .route("/{game}/redeem", post(direct_redeem_ep))
        .route("/{game}/role-stats", post(direct_role_stats_ep))
        .route("/{game}/daily-note", post(direct_daily_note_ep))
        // Authenticated Account CRUD
        .route("/{game}/accounts", post(add_acc_ep).get(list_acc_ep))
        .route(
            "/{game}/accounts/{account_id}",
            get(get_acc_ep).patch(update_acc_ep).delete(delete_acc_ep),
        )
        // Authenticated Account Actions
        .route("/{game}/accounts/{account_id}/stats", get(acc_stats_ep))
        .route("/{game}/accounts/{account_id}/daily-note", get(acc_daily_note_ep))
        .route("/{game}/accounts/{account_id}/checkin-list", get(acc_checkin_list_ep))
        .route("/{game}/accounts/{account_id}/checkin", post(acc_checkin_ep))
        .route("/{game}/accounts/{account_id}/redeem", post(acc_redeem_ep))
        .route("/{game}/accounts/{account_id}/auto-redeem", post(acc_auto_redeem_ep))
        .route("/{game}/accounts/{account_id}/redeem-history", get(acc_redeem_history_ep))
}

/// Dedicated game router factory for `/v1/hsr` and `/v1/zzz`
pub fn game_router(game: HoyoGame) -> Router<AppState> {
    Router::new()
        .route(
            "/codes",
            get(move |st, q| async move { handle_game_codes(st, game, q).await }),
        )
        .route(
            "/calendar",
            get(move |st, q| async move { handle_game_calendar(st, game, q).await }),
        )
        .route(
            "/news/{kind}",
            get(move |st, Path(kind): Path<String>, q| async move {
                handle_game_news(st, game, kind, q).await
            }),
        )
        .route(
            "/checkin",
            post(move |st, body| async move { handle_direct_checkin(st, game, body).await }),
        )
        .route(
            "/redeem",
            post(move |st, body| async move { handle_direct_redeem(st, game, body).await }),
        )
        .route(
            "/role-stats",
            post(move |st, body| async move { handle_direct_role_stats(st, game, body).await }),
        )
        .route(
            "/daily-note",
            post(move |st, body| async move { handle_direct_daily_note(st, game, body).await }),
        )
        .route(
            "/accounts",
            post(move |st, u, body| async move { handle_add_acc(st, game, u, body).await })
                .get(move |st, u| async move { handle_list_acc(st, game, u).await }),
        )
        .route(
            "/accounts/{account_id}",
            get(move |st, u, Path(id): Path<i64>| async move {
                handle_get_acc(st, game, u, id).await
            })
            .patch(move |st, u, Path(id): Path<i64>, body| async move {
                handle_update_acc(st, game, u, id, body).await
            })
            .delete(move |st, u, Path(id): Path<i64>| async move {
                handle_delete_acc(st, game, u, id).await
            }),
        )
        .route(
            "/accounts/{account_id}/stats",
            get(move |st, u, Path(id): Path<i64>| async move {
                handle_acc_stats(st, game, u, id).await
            }),
        )
        .route(
            "/accounts/{account_id}/daily-note",
            get(move |st, u, Path(id): Path<i64>| async move {
                handle_acc_daily_note(st, game, u, id).await
            }),
        )
        .route(
            "/accounts/{account_id}/checkin-list",
            get(move |st, u, Path(id): Path<i64>| async move {
                handle_acc_checkin_list(st, game, u, id).await
            }),
        )
        .route(
            "/accounts/{account_id}/checkin",
            post(move |st, u, Path(id): Path<i64>| async move {
                handle_acc_checkin(st, game, u, id).await
            }),
        )
        .route(
            "/accounts/{account_id}/redeem",
            post(move |st, u, Path(id): Path<i64>, body| async move {
                handle_acc_redeem(st, game, u, id, body).await
            }),
        )
        .route(
            "/accounts/{account_id}/auto-redeem",
            post(move |st, u, Path(id): Path<i64>, q| async move {
                handle_acc_auto_redeem(st, game, u, id, q).await
            }),
        )
        .route(
            "/accounts/{account_id}/redeem-history",
            get(move |st, u, Path(id): Path<i64>| async move {
                handle_acc_redeem_history(st, game, u, id).await
            }),
        )
}

#[derive(Deserialize)]
struct RefreshQuery {
    refresh: Option<bool>,
    force: Option<bool>,
}

#[derive(Deserialize)]
struct LangQuery {
    lang: Option<String>,
    refresh: Option<bool>,
}

#[derive(Deserialize)]
struct AddAccountBody {
    cookie: Option<String>,
    uid: Option<String>,
    server: Option<String>,
}

#[derive(Deserialize)]
struct SingleCookiePreviewBody {
    cookie: Option<String>,
}

#[derive(Deserialize)]
struct SingleCookieLinkBody {
    cookie: Option<String>,
    auto_checkin: Option<bool>,
    auto_redeem: Option<bool>,
    auto_daily_note: Option<bool>,
    games: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RedeemBody {
    code: Option<String>,
}

// =========================================================================
// ONE-COOKIE MULTI-GAME HANDLERS
// =========================================================================

async fn all_user_accounts_ep(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Value>> {
    let data = get_all_user_accounts(&state, user.id).await?;
    Ok(send_success(
        "Lấy toàn bộ tài khoản HoYoverse (Genshin, HSR, ZZZ) thành công",
        data,
    ))
}

async fn preview_single_cookie_ep(
    State(state): State<AppState>,
    _user: AuthUser,
    Json(body): Json<SingleCookiePreviewBody>,
) -> AppResult<Json<Value>> {
    let cookie = body
        .cookie
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu cookie HoYoLAB."))?;
    let roles = discover_all_roles_from_cookie(&state, &cookie, None).await?;
    Ok(send_success(
        "Quét danh sách nhân vật HoYoverse thành công",
        json!({ "count": roles.len(), "roles": roles }),
    ))
}

async fn link_single_cookie_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<SingleCookieLinkBody>,
) -> AppResult<Json<Value>> {
    let cookie = body
        .cookie
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu cookie HoYoLAB."))?;

    let parsed_games: Option<Vec<HoyoGame>> = if let Some(list) = body.games {
        let mut out = Vec::new();
        for g in list {
            out.push(HoyoGame::from_slug(&g)?);
        }
        Some(out)
    } else {
        None
    };

    let data = link_all_games_with_single_cookie(
        &state,
        user.id,
        &cookie,
        body.auto_checkin.unwrap_or(true),
        body.auto_redeem.unwrap_or(true),
        body.auto_daily_note.unwrap_or(false),
        parsed_games.as_deref(),
    )
    .await?;

    Ok(send_success(
        "Đã tự động liên kết toàn bộ nhân vật từ 1 Cookie HoYoLAB thành công",
        data,
    ))
}

async fn update_shared_cookie_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<SingleCookiePreviewBody>,
) -> AppResult<Json<Value>> {
    let cookie = body
        .cookie
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu cookie HoYoLAB mới."))?;
    let data = update_shared_cookie_for_user(&state, user.id, &cookie).await?;
    Ok(send_success(
        "Đã cập nhật Cookie mới đồng loạt cho tất cả tài khoản HoYoverse",
        data,
    ))
}

async fn checkin_all_user_games_ep(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Value>> {
    let payloads = get_all_ready_payloads_for_user(&state, user.id).await?;
    let mut results = Vec::with_capacity(payloads.len());

    for p in payloads {
        match check_in_daily(&state, p.game, &p.cookie, &p.server).await {
            Ok(res) => results.push(res),
            Err(err) => results.push(json!({
                "ok": false,
                "game": p.game.slug(),
                "account_id": p.account_id,
                "uid": p.uid,
                "error": err.to_string(),
            })),
        }
    }

    Ok(send_success(
        "Hoàn tất điểm danh tất cả game HoYoverse",
        json!({ "total": results.len(), "results": results }),
    ))
}

async fn auto_redeem_all_user_games_ep(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<RefreshQuery>,
) -> AppResult<Json<Value>> {
    let payloads = get_all_ready_payloads_for_user(&state, user.id).await?;
    let mut results = Vec::with_capacity(payloads.len());
    let force_refresh = q.refresh.unwrap_or(false);
    let force_recheck = q.force.unwrap_or(false);

    for (idx, p) in payloads.into_iter().enumerate() {
        match auto_redeem_for_account(
            &state,
            p.game,
            p.account_id,
            &p.cookie,
            &p.uid,
            &p.server,
            &p.nickname,
            user.email.as_deref(),
            force_refresh && idx == 0,
            force_recheck,
        )
        .await
        {
            Ok(summary) => results.push(summary),
            Err(err) => results.push(json!({
                "ok": false,
                "game": p.game.slug(),
                "account_id": p.account_id,
                "uid": p.uid,
                "error": err.to_string(),
            })),
        }
    }

    Ok(send_success(
        "Hoàn tất tự động nhập giftcode cho tất cả game HoYoverse",
        json!({ "total": results.len(), "results": results }),
    ))
}

// =========================================================================
// PER-GAME HANDLERS
// =========================================================================

async fn all_games_codes_ep(
    State(state): State<AppState>,
    Query(q): Query<RefreshQuery>,
) -> AppResult<Json<Value>> {
    let data = get_all_games_codes(&state, q.refresh.unwrap_or(false)).await?;
    Ok(Json(json!({
        "status": "success",
        "timestamp": Utc::now().to_rfc3339(),
        "data": data,
    })))
}

async fn handle_game_codes(
    State(state): State<AppState>,
    game: HoyoGame,
    Query(q): Query<RefreshQuery>,
) -> AppResult<Json<Value>> {
    let cards = get_game_codes(&state, game, q.refresh.unwrap_or(false)).await?;
    let count: usize = cards.iter().map(|c| c.codes.len()).sum();
    Ok(Json(json!({
        "status": "success",
        "game": game.slug(),
        "game_name": game.display_name(),
        "timestamp": Utc::now().to_rfc3339(),
        "count": count,
        "cards": cards,
    })))
}

async fn handle_game_calendar(
    State(state): State<AppState>,
    game: HoyoGame,
    Query(q): Query<LangQuery>,
) -> AppResult<Json<Value>> {
    let data =
        get_game_calendar(&state, game, q.lang.as_deref(), q.refresh.unwrap_or(false)).await?;
    Ok(Json(json!({
        "status": "success",
        "game": game.slug(),
        "game_name": game.display_name(),
        "timestamp": Utc::now().to_rfc3339(),
        "data": data,
    })))
}

async fn handle_game_news(
    State(state): State<AppState>,
    game: HoyoGame,
    kind: String,
    Query(q): Query<LangQuery>,
) -> AppResult<Json<Value>> {
    let data = get_game_news(
        &state,
        game,
        &kind,
        q.lang.as_deref(),
        q.refresh.unwrap_or(false),
        q.force.unwrap_or(false),
    )
    .await?;
    Ok(Json(json!({
        "status": "success",
        "game": game.slug(),
        "kind": kind,
        "timestamp": Utc::now().to_rfc3339(),
        "data": data,
    })))
}

async fn handle_direct_checkin(
    State(state): State<AppState>,
    game: HoyoGame,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let cookie = body.get("cookie").and_then(Value::as_str).unwrap_or("");
    let server = body.get("server").and_then(Value::as_str).unwrap_or("");
    Ok(Json(check_in_daily(&state, game, cookie, server).await?))
}

async fn handle_direct_redeem(
    State(state): State<AppState>,
    game: HoyoGame,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let cookie = body.get("cookie").and_then(Value::as_str).unwrap_or("");
    let server = body.get("server").and_then(Value::as_str).unwrap_or("");
    let code = body.get("code").and_then(Value::as_str).unwrap_or("");
    let uid = body.get("uid").and_then(Value::as_str);
    Ok(Json(redeem_code(&state, game, cookie, server, code, uid).await?))
}

async fn handle_direct_role_stats(
    State(state): State<AppState>,
    game: HoyoGame,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let cookie = body.get("cookie").and_then(Value::as_str).unwrap_or("");
    let uid = body.get("uid").and_then(Value::as_str).unwrap_or("");
    let server = body.get("server").and_then(Value::as_str).unwrap_or("");
    Ok(Json(get_role_and_stats(&state, game, cookie, uid, server).await?))
}

async fn handle_direct_daily_note(
    State(state): State<AppState>,
    game: HoyoGame,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let cookie = body.get("cookie").and_then(Value::as_str).unwrap_or("");
    let uid = body.get("uid").and_then(Value::as_str).unwrap_or("");
    let server = body.get("server").and_then(Value::as_str).unwrap_or("");
    Ok(Json(get_daily_note(&state, game, cookie, uid, server).await?))
}

async fn handle_add_acc(
    State(state): State<AppState>,
    game: HoyoGame,
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

    let data = add_account(&state, game, user.id, &cookie, &uid, &server).await?;
    Ok(send_success(
        &format!("Thêm tài khoản {} thành công", game.display_name()),
        data,
    ))
}

async fn handle_list_acc(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
) -> AppResult<Json<Value>> {
    let data = get_accounts(&state, game, user.id).await?;
    Ok(send_success("Lấy danh sách tài khoản thành công", data))
}

async fn handle_get_acc(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
) -> AppResult<Json<Value>> {
    let data = get_account_by_id(&state, game, user.id, account_id).await?;
    Ok(send_success("Lấy chi tiết tài khoản thành công", data))
}

async fn handle_update_acc(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
    Json(body): Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    update_account(&state, game, user.id, account_id, &body).await?;
    Ok(send_success_empty("Cập nhật cài đặt tài khoản thành công"))
}

async fn handle_delete_acc(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
) -> AppResult<Json<Value>> {
    delete_account(&state, game, user.id, account_id).await?;
    Ok(send_success_empty("Đã xóa tài khoản khỏi hệ thống"))
}

async fn handle_acc_stats(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
) -> AppResult<Response> {
    let p = get_ready_payload(&state, game, user.id, Some(account_id)).await?;
    let res = get_role_and_stats(&state, game, &p.cookie, &p.uid, &p.server).await?;
    let status = if res.get("ok").and_then(Value::as_bool) == Some(true) {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    Ok((status, Json(res)).into_response())
}

async fn handle_acc_daily_note(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
) -> AppResult<Response> {
    let p = get_ready_payload(&state, game, user.id, Some(account_id)).await?;
    let res = get_daily_note(&state, game, &p.cookie, &p.uid, &p.server).await?;
    let status = if res.get("ok").and_then(Value::as_bool) == Some(true) {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    Ok((status, Json(res)).into_response())
}

async fn handle_acc_checkin_list(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
) -> AppResult<Response> {
    let p = get_ready_payload(&state, game, user.id, Some(account_id)).await?;
    let res = get_check_in_list(&state, game, &p.cookie).await?;
    let status = if res.get("ok").and_then(Value::as_bool) == Some(true) {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    Ok((status, Json(res)).into_response())
}

async fn handle_acc_checkin(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
) -> AppResult<Json<Value>> {
    let p = get_ready_payload(&state, game, user.id, Some(account_id)).await?;
    Ok(Json(check_in_daily(&state, game, &p.cookie, &p.server).await?))
}

async fn handle_acc_redeem(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
    Json(body): Json<RedeemBody>,
) -> AppResult<Json<Value>> {
    let code = body
        .code
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Thiếu giftcode cần nhập (code)"))?;
    let p = get_ready_payload(&state, game, user.id, Some(account_id)).await?;
    Ok(Json(
        redeem_code(&state, game, &p.cookie, &p.server, &code, Some(&p.uid)).await?,
    ))
}

async fn handle_acc_auto_redeem(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
    Query(q): Query<RefreshQuery>,
) -> AppResult<Json<Value>> {
    let p = get_ready_payload(&state, game, user.id, Some(account_id)).await?;
    let summary = auto_redeem_for_account(
        &state,
        game,
        p.account_id,
        &p.cookie,
        &p.uid,
        &p.server,
        &p.nickname,
        user.email.as_deref(),
        q.refresh.unwrap_or(false),
        q.force.unwrap_or(false),
    )
    .await?;
    Ok(send_success(
        &format!("Hoàn tất tự động nhập giftcode {}", game.display_name()),
        summary,
    ))
}

async fn handle_acc_redeem_history(
    State(state): State<AppState>,
    game: HoyoGame,
    user: AuthUser,
    account_id: i64,
) -> AppResult<Json<Value>> {
    let _ = get_account_by_id(&state, game, user.id, account_id).await?;
    let history = get_account_redeem_history(&state, game, account_id).await;
    Ok(send_success("Lấy lịch sử nhập giftcode thành công", history))
}

// Dynamic /{game}/... wrapper handlers
async fn game_codes_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    q: Query<RefreshQuery>,
) -> AppResult<Json<Value>> {
    handle_game_codes(st, HoyoGame::from_slug(&game)?, q).await
}

async fn game_calendar_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    q: Query<LangQuery>,
) -> AppResult<Json<Value>> {
    handle_game_calendar(st, HoyoGame::from_slug(&game)?, q).await
}

async fn game_news_ep(
    st: State<AppState>,
    Path((game, kind)): Path<(String, String)>,
    q: Query<LangQuery>,
) -> AppResult<Json<Value>> {
    handle_game_news(st, HoyoGame::from_slug(&game)?, kind, q).await
}

async fn direct_checkin_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    body: Json<Value>,
) -> AppResult<Json<Value>> {
    handle_direct_checkin(st, HoyoGame::from_slug(&game)?, body).await
}

async fn direct_redeem_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    body: Json<Value>,
) -> AppResult<Json<Value>> {
    handle_direct_redeem(st, HoyoGame::from_slug(&game)?, body).await
}

async fn direct_role_stats_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    body: Json<Value>,
) -> AppResult<Json<Value>> {
    handle_direct_role_stats(st, HoyoGame::from_slug(&game)?, body).await
}

async fn direct_daily_note_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    body: Json<Value>,
) -> AppResult<Json<Value>> {
    handle_direct_daily_note(st, HoyoGame::from_slug(&game)?, body).await
}

async fn add_acc_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    u: AuthUser,
    body: Json<AddAccountBody>,
) -> AppResult<Json<Value>> {
    handle_add_acc(st, HoyoGame::from_slug(&game)?, u, body).await
}

async fn list_acc_ep(
    st: State<AppState>,
    Path(game): Path<String>,
    u: AuthUser,
) -> AppResult<Json<Value>> {
    handle_list_acc(st, HoyoGame::from_slug(&game)?, u).await
}

async fn get_acc_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
) -> AppResult<Json<Value>> {
    handle_get_acc(st, HoyoGame::from_slug(&game)?, u, id).await
}

async fn update_acc_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
    body: Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    handle_update_acc(st, HoyoGame::from_slug(&game)?, u, id, body).await
}

async fn delete_acc_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
) -> AppResult<Json<Value>> {
    handle_delete_acc(st, HoyoGame::from_slug(&game)?, u, id).await
}

async fn acc_stats_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
) -> AppResult<Response> {
    handle_acc_stats(st, HoyoGame::from_slug(&game)?, u, id).await
}

async fn acc_daily_note_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
) -> AppResult<Response> {
    handle_acc_daily_note(st, HoyoGame::from_slug(&game)?, u, id).await
}

async fn acc_checkin_list_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
) -> AppResult<Response> {
    handle_acc_checkin_list(st, HoyoGame::from_slug(&game)?, u, id).await
}

async fn acc_checkin_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
) -> AppResult<Json<Value>> {
    handle_acc_checkin(st, HoyoGame::from_slug(&game)?, u, id).await
}

async fn acc_redeem_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
    body: Json<RedeemBody>,
) -> AppResult<Json<Value>> {
    handle_acc_redeem(st, HoyoGame::from_slug(&game)?, u, id, body).await
}

async fn acc_auto_redeem_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
    q: Query<RefreshQuery>,
) -> AppResult<Json<Value>> {
    handle_acc_auto_redeem(st, HoyoGame::from_slug(&game)?, u, id, q).await
}

async fn acc_redeem_history_ep(
    st: State<AppState>,
    Path((game, id)): Path<(String, i64)>,
    u: AuthUser,
) -> AppResult<Json<Value>> {
    handle_acc_redeem_history(st, HoyoGame::from_slug(&game)?, u, id).await
}
