use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use tokio::sync::{broadcast, RwLock};

use crate::core::config::Config;

pub const SYSTEM_HISTORY_LIMIT: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHistoryPoint {
    pub time: String,
    pub app_cpu_percent: f64,
    pub app_ram_mb: f64,
    pub sys_ram_percent: f64,
}

#[derive(Debug, Clone)]
pub struct CachedValue<T> {
    pub value: T,
    pub expires_at: Instant,
}

impl<T: Clone> CachedValue<T> {
    pub fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    pub fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

#[derive(Debug, Default)]
pub struct SchedulerState {
    pub checkin_running: AtomicBool,
    pub redeem_running: AtomicBool,
    pub daily_note_running: AtomicBool,
    pub last_checkin_run: RwLock<Option<String>>,
    pub last_redeem_run: RwLock<Option<String>>,
    pub last_daily_note_run: RwLock<Option<String>>,
    pub last_redeem_summary: RwLock<Option<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RedeemHistoryStore {
    /// Map of "{game}:{account_id}" -> set of already attempted/claimed/regional-skipped codes
    #[serde(default)]
    pub accounts: HashMap<String, HashSet<String>>,
    /// Codes confirmed globally expired or invalid ("{game}:{code}") so all accounts skip them
    #[serde(default)]
    pub global_expired: HashSet<String>,
    /// Detailed recent redeem logs per "{game}:{account_id}" (bounded to 50 per account)
    #[serde(default)]
    pub logs: HashMap<String, Vec<RedeemLogEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeemLogEntry {
    pub game: String,
    pub code: String,
    pub status: String,
    pub message: String,
    pub rewards: Vec<String>,
    pub redeemed_at: String,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Option<MySqlPool>,
    pub http: reqwest::Client,
    pub start_time: Instant,

    // Realtime system metrics & WebSocket broadcast
    pub system_history: Arc<RwLock<VecDeque<SystemHistoryPoint>>>,
    pub metrics_tx: broadcast::Sender<Value>,

    // In-memory TTL caches
    pub qr_logo_cache: Arc<RwLock<Option<Vec<u8>>>>,
    pub hoyo_codes_cache: Arc<DashMap<String, CachedValue<Value>>>,
    pub hoyo_calendar_cache: Arc<DashMap<String, CachedValue<Value>>>,
    pub hoyo_news_cache: Arc<DashMap<String, CachedValue<Value>>>,
    pub evn_units_cache: Arc<DashMap<String, CachedValue<Value>>>,

    // Local JSON-backed stores protected by async RwLock
    pub go_links_lock: Arc<RwLock<()>>,
    pub file_db_lock: Arc<RwLock<()>>,
    pub redeem_history: Arc<RwLock<RedeemHistoryStore>>,

    // Scheduler concurrency & telemetry
    pub scheduler: Arc<SchedulerState>,
}

impl AppState {
    pub fn db(&self) -> crate::error::AppResult<&MySqlPool> {
        self.db
            .as_ref()
            .ok_or_else(|| crate::error::AppError::internal("Database pool is not initialized"))
    }
}
