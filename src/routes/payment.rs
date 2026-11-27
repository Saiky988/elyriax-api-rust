use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    core::{auth::SepayWebhookAuth, security::decode_qr_token},
    services::wallet::process_sepay_webhook,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sepay/webhook", post(sepay_webhook_ep))
        .route("/qr", get(generate_qr_ep))
}

async fn sepay_webhook_ep(
    State(state): State<AppState>,
    _auth: SepayWebhookAuth,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let st = state.clone();
    tokio::spawn(async move {
        if let Err(err) = process_sepay_webhook(&st, payload).await {
            tracing::error!(error = %err, "SePay webhook processing failed");
        }
    });

    Json(json!({ "success": true }))
}

#[derive(Deserialize)]
struct QrQuery {
    token: Option<String>,
}

async fn generate_qr_ep(
    State(state): State<AppState>,
    Query(query): Query<QrQuery>,
) -> Response {
    let Some(token) = query.token.filter(|s| !s.is_empty()) else {
        return (StatusCode::BAD_REQUEST, "Missing token").into_response();
    };

    let Ok(claims) = decode_qr_token(&token, &state.config.jwt_secret) else {
        return (StatusCode::FORBIDDEN, "Token invalid or expired").into_response();
    };

    let amount_int = claims.amount.round() as i64;
    let viet_qr_url = format!(
        "https://img.vietqr.io/image/{}-{}-qr_only.svg?amount={}&addInfo={}&accountName={}",
        state.config.bank_id,
        state.config.bank_account_no,
        amount_int,
        urlencoding::encode(&claims.content),
        urlencoding::encode(&state.config.bank_account_name),
    );

    let Ok(qr_res) = state.http.get(&viet_qr_url).send().await else {
        return (StatusCode::BAD_GATEWAY, "Failed to fetch VietQR SVG").into_response();
    };

    if !qr_res.status().is_success() {
        return (StatusCode::BAD_GATEWAY, "VietQR upstream returned error").into_response();
    }

    let Ok(svg_bytes) = qr_res.bytes().await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read VietQR SVG").into_response();
    };

    (
        [
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        svg_bytes.to_vec(),
    )
        .into_response()
}
