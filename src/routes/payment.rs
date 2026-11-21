use std::io::Cursor;

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use image::{imageops, ImageFormat};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    core::{auth::SepayWebhookAuth, security::decode_qr_token},
    services::wallet::process_sepay_webhook,
    state::AppState,
};

const LOGO_URL: &str = "https://www.elyriax.com/assets/logoqr.png";

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

    let viet_qr_url = format!(
        "https://img.vietqr.io/image/{}-{}-qr_only.png?amount={}&addInfo={}&accountName={}",
        state.config.bank_id,
        state.config.bank_account_no,
        claims.amount,
        urlencoding::encode(&claims.content),
        urlencoding::encode(&state.config.bank_account_name),
    );

    let Ok(qr_res) = state.http.get(&viet_qr_url).send().await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch VietQR").into_response();
    };
    let Ok(qr_bytes) = qr_res.bytes().await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read VietQR bytes").into_response();
    };

    // Fetch & cache logo bytes once
    let logo_bytes = {
        let read_guard = state.qr_logo_cache.read().await;
        read_guard.clone()
    };

    let logo_bytes = if logo_bytes.is_some() {
        logo_bytes
    } else if let Ok(res) = state.http.get(LOGO_URL).send().await {
        if res.status().is_success() {
            if let Ok(b) = res.bytes().await {
                let vec = b.to_vec();
                let mut write_guard = state.qr_logo_cache.write().await;
                *write_guard = Some(vec.clone());
                Some(vec)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let qr_vec = qr_bytes.to_vec();
    let png_result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut qr_img = image::load_from_memory(&qr_vec)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        qr_img = imageops::resize(&qr_img, 400, 400, imageops::FilterType::Lanczos3);

        if let Some(lb) = logo_bytes {
            if let Ok(logo_dyn) = image::load_from_memory(&lb) {
                let logo_resized = imageops::resize(
                    &logo_dyn.to_rgba8(),
                    60,
                    60,
                    imageops::FilterType::Lanczos3,
                );
                imageops::overlay(&mut qr_img, &logo_resized, 170, 170);
            }
        }

        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(qr_img)
            .write_to(&mut out, ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(out.into_inner())
    })
    .await;

    match png_result {
        Ok(Ok(png_bytes)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=300"),
            ],
            png_bytes,
        )
            .into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response(),
    }
}
