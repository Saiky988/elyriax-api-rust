use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::{
    core::{auth::AdminUser, response::send_success},
    services::scheduler::{
        get_scheduler_status, run_genshin_auto_checkin, run_genshin_auto_daily_note,
        run_genshin_auto_redeem,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", get(scheduler_status_ep))
        .route("/test-checkin", post(test_checkin_ep))
        .route("/test-redeem", post(test_redeem_ep))
        .route("/test-daily-note", post(test_daily_note_ep))
}

async fn scheduler_status_ep(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    let status = get_scheduler_status(&state).await;
    send_success("Lấy trạng thái Scheduler thành công", status)
}

async fn test_checkin_ep(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    let st = state.clone();
    tokio::spawn(async move {
        run_genshin_auto_checkin(st).await;
    });
    Json(json!({ "success": true, "message": "testing checkin" }))
}

async fn test_redeem_ep(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    let st = state.clone();
    tokio::spawn(async move {
        run_genshin_auto_redeem(st).await;
    });
    Json(json!({ "success": true, "message": "testing auto-redeem" }))
}

async fn test_daily_note_ep(State(state): State<AppState>, _admin: AdminUser) -> Json<Value> {
    let st = state.clone();
    tokio::spawn(async move {
        run_genshin_auto_daily_note(st).await;
    });
    Json(json!({ "success": true, "message": "testing daily-note" }))
}
