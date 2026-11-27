mod core;
mod error;
mod routes;
mod services;
mod state;

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::{header, Method};
use dashmap::DashMap;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    core::{config::Config, db::init_db_pool},
    routes::{build_router, host::spawn_metrics_emitter},
    services::{
        hoyo::{account::ensure_hoyo_accounts_table, redeem::load_redeem_history},
        scheduler::spawn_scheduler,
    },
    state::{AppState, RedeemHistoryStore, SchedulerState, SYSTEM_HISTORY_LIMIT},
};

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, draining connections...");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,elyriax_api=debug,sqlx=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Arc::new(Config::from_env());

    // Ensure local runtime directories exist
    let _ = tokio::fs::create_dir_all(config.data_dir()).await;
    let _ = tokio::fs::create_dir_all(config.uploads_dir()).await;

    // Initialize shared HTTP/2 pooled client
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(32)
        .pool_idle_timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .tcp_keepalive(Duration::from_secs(60))
        .build()?;

    // Initialize MySQL connection pool (graceful warning if unavailable at boot)
    let db = match init_db_pool(&config).await {
        Ok(pool) => {
            tracing::info!("[Database] MySQL Connection Pool initialized.");
            Some(pool)
        }
        Err(err) => {
            tracing::warn!(error = %err, "[Database] Could not connect to MySQL at startup");
            None
        }
    };

    let (metrics_tx, _) = broadcast::channel(64);

    let state = AppState {
        config: config.clone(),
        db,
        http,
        start_time: Instant::now(),
        system_history: Arc::new(RwLock::new(VecDeque::with_capacity(SYSTEM_HISTORY_LIMIT))),
        metrics_tx,
        hoyo_codes_cache: Arc::new(DashMap::new()),
        hoyo_calendar_cache: Arc::new(DashMap::new()),
        hoyo_news_cache: Arc::new(DashMap::new()),
        evn_units_cache: Arc::new(DashMap::new()),
        provider_catalog_cache: Arc::new(DashMap::new()),
        go_links_lock: Arc::new(RwLock::new(())),
        file_db_lock: Arc::new(RwLock::new(())),
        redeem_history: Arc::new(RwLock::new(RedeemHistoryStore::default())),
        scheduler: Arc::new(SchedulerState::default()),
    };

    // Auto-ensure multi-game `hoyo_accounts` table for HSR & ZZZ (keeps `genshin_accounts` intact)
    if state.db.is_some() {
        if let Err(err) = ensure_hoyo_accounts_table(&state).await {
            tracing::warn!(error = %err, "[Database] Could not auto-create hoyo_accounts table");
        }
    }

    // Hydrate multi-game HoYo redeem history cache from disk
    load_redeem_history(&state).await;

    // Start background workers (metrics emitter + multi-game HoYo scheduler)
    spawn_metrics_emitter(state.clone());
    spawn_scheduler(state.clone());

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
        .expose_headers([
            header::CONTENT_RANGE,
            header::CONTENT_LENGTH,
            header::ACCEPT_RANGES,
        ]);

    let app = build_router(state.clone())
        .layer(CompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(100 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(
        "Elyriax API v{} (Rust | Genshin + HSR + ZZZ) listening on http://{}",
        env!("CARGO_PKG_VERSION"),
        addr
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    if let Some(pool) = state.db {
        pool.close().await;
        tracing::info!("[Database] MySQL Connection Pool closed.");
    }

    Ok(())
}
