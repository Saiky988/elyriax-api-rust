pub mod admin_product;
pub mod admin_scheduler;
pub mod auth;
pub mod cart;
pub mod email;
pub mod evn;
pub mod file_up;
pub mod genshin;
pub mod go;
pub mod host;
pub mod hoyo;
pub mod order;
pub mod payment;
pub mod product;
pub mod settings;
pub mod wallet;
pub mod wishlist;

use axum::{routing::get, Json, Router};
use chrono::Utc;
use serde_json::{json, Value};

use crate::{services::hoyo::HoyoGame, state::AppState};

async fn health_check(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Json<Value> {
    let db_ok = if let Some(ref pool) = state.db {
        sqlx::query("SELECT 1").execute(pool).await.is_ok()
    } else {
        false
    };

    Json(json!({
        "success": true,
        "status": "ok",
        "service": "elyriax-api",
        "version": env!("CARGO_PKG_VERSION"),
        "runtime": "rust",
        "supported_games": ["genshin", "hsr", "zzz"],
        "database": if db_ok { "connected" } else { "disconnected" },
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .nest("/v1/host", host::router())
        .nest("/v1/auth", auth::router())
        .nest("/v1/settings", settings::router())
        .nest("/v1/cart", cart::router())
        .nest("/v1/orders", order::router())
        .nest("/v1/wishlist", wishlist::router())
        .nest("/v1/wallet", wallet::router())
        .nest("/v1/payment", payment::router())
        .nest("/v1/products", product::router())
        .nest("/v1/admin", admin_product::router())
        .nest("/v1/admin/scheduler", admin_scheduler::router())
        .nest("/v1/email", email::router())
        .nest("/v1/genshin", genshin::router())
        .nest("/v1/hsr", hoyo::game_router(HoyoGame::Hsr))
        .nest("/v1/zzz", hoyo::game_router(HoyoGame::Zzz))
        .nest("/v1/hoyo", hoyo::router())
        .nest("/v1/evn", evn::router())
        .merge(go::router())
        .merge(file_up::router())
        .with_state(state)
}
