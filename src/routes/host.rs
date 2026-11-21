use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{FixedOffset, Utc};
use serde_json::{json, Value};
use sysinfo::{Disks, Networks, Pid, ProcessesToUpdate, System};

use crate::state::{AppState, SystemHistoryPoint, SYSTEM_HISTORY_LIMIT};

const RAM_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;
const CPU_LIMIT_PERCENT: f64 = 100.0;
const DISK_LIMIT_BYTES: u64 = 20 * 1024 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/system", get(get_system_stats))
        .route("/ws", get(ws_system_metrics))
}

fn format_bytes(bytes_val: u64) -> String {
    if bytes_val == 0 {
        return "0 B".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut val = bytes_val as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx < UNITS.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    format!("{:.2} {}", val, UNITS[idx])
}

fn format_uptime(seconds: u64) -> String {
    let d = seconds / 86400;
    let h = (seconds % 86400) / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(format!("{}d", d));
    }
    if h > 0 {
        parts.push(format!("{}h", h));
    }
    if m > 0 {
        parts.push(format!("{}m", m));
    }
    parts.push(format!("{}s", s));
    parts.join(" ")
}

fn collect_snapshot(
    sys: &mut System,
    disks: &mut Disks,
    networks: &mut Networks,
    state: &AppState,
    history: Vec<SystemHistoryPoint>,
    latency_ms: u128,
) -> (Value, SystemHistoryPoint) {
    let pid = Pid::from_u32(std::process::id());
    sys.refresh_memory();
    sys.refresh_cpu_all();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    disks.refresh(false);
    networks.refresh(false);

    let (app_cpu, app_rss) = sys
        .process(pid)
        .map(|p| (p.cpu_usage() as f64, p.memory()))
        .unwrap_or((0.0, 0));

    let app_cpu_round = (app_cpu * 100.0).round() / 100.0;
    let app_ram_mb = ((app_rss as f64 / 1024.0 / 1024.0) * 100.0).round() / 100.0;

    let total_mem = sys.total_memory();
    let avail_mem = sys.available_memory();
    let used_mem = total_mem.saturating_sub(avail_mem);
    let total_os_mb = ((total_mem as f64 / 1024.0 / 1024.0) * 100.0).round() / 100.0;
    let free_os_mb = ((avail_mem as f64 / 1024.0 / 1024.0) * 100.0).round() / 100.0;
    let used_os_mb = ((used_mem as f64 / 1024.0 / 1024.0) * 100.0).round() / 100.0;
    let sys_ram_pct = if total_mem > 0 {
        ((used_mem as f64 / total_mem as f64) * 10000.0).round() / 100.0
    } else {
        0.0
    };

    let disk_used_bytes: u64 = disks
        .list()
        .first()
        .map(|d| d.total_space().saturating_sub(d.available_space()))
        .unwrap_or(0);

    let mut inbound: u64 = 0;
    let mut outbound: u64 = 0;
    for (_name, data) in networks.list() {
        inbound = inbound.saturating_add(data.total_received());
        outbound = outbound.saturating_add(data.total_transmitted());
    }

    let load = System::load_average();
    let load_avg = vec![
        format!("{:.2}", load.one),
        format!("{:.2}", load.five),
        format!("{:.2}", load.fifteen),
    ];

    let cpu_model = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "CPU".to_string());

    let tz_vn = FixedOffset::east_opt(7 * 3600).expect("valid UTC+7");
    let now_str = Utc::now().with_timezone(&tz_vn).format("%H:%M:%S").to_string();

    let point = SystemHistoryPoint {
        time: now_str,
        app_cpu_percent: app_cpu_round,
        app_ram_mb,
        sys_ram_percent: sys_ram_pct,
    };

    let ram_pct = (((app_rss as f64 / RAM_LIMIT_BYTES as f64) * 10000.0).round()) / 100.0;
    let disk_pct = (((disk_used_bytes as f64 / DISK_LIMIT_BYTES as f64) * 10000.0).round()) / 100.0;

    let snapshot = json!({
        "success": true,
        "status": "online",
        "latency": format!("{}ms", latency_ms),
        "api_info": {
            "version": env!("CARGO_PKG_VERSION"),
            "environment": state.config.environment,
            "app_uptime": format_uptime(state.start_time.elapsed().as_secs()),
            "runtime": "Rust (Axum + Tokio)",
        },
        "panel_stats": {
            "cpu": {
                "used_percent": app_cpu_round,
                "limit_percent": CPU_LIMIT_PERCENT,
                "formatted": format!("{}% / {}%", app_cpu_round, CPU_LIMIT_PERCENT),
            },
            "ram": {
                "used_bytes": app_rss,
                "limit_bytes": RAM_LIMIT_BYTES,
                "usage_percent": ram_pct,
                "formatted": format!("{} / {}", format_bytes(app_rss), format_bytes(RAM_LIMIT_BYTES)),
            },
            "disk": {
                "used_bytes": disk_used_bytes,
                "limit_bytes": DISK_LIMIT_BYTES,
                "usage_percent": disk_pct,
                "formatted": format!("{} / {}", format_bytes(disk_used_bytes), format_bytes(DISK_LIMIT_BYTES)),
            },
            "network": {
                "inbound": format_bytes(inbound),
                "outbound": format_bytes(outbound),
            },
        },
        "system_info": {
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "os_uptime": format_uptime(System::uptime()),
            "cpu": {
                "model": cpu_model,
                "cores": sys.cpus().len().max(1),
                "load_average_1m_5m_15m": load_avg,
            },
            "memory": {
                "total": format!("{} MB", total_os_mb),
                "used": format!("{} MB", used_os_mb),
                "free": format!("{} MB", free_os_mb),
            },
        },
        "history_chart_data": history,
    });

    (snapshot, point)
}

pub fn spawn_metrics_emitter(state: AppState) {
    tokio::spawn(async move {
        let mut sys = System::new_all();
        let mut disks = Disks::new_with_refreshed_list();
        let mut networks = Networks::new_with_refreshed_list();
        let mut interval = tokio::time::interval(Duration::from_secs(2));

        loop {
            interval.tick().await;
            let history_vec = {
                let guard = state.system_history.read().await;
                guard.iter().cloned().collect::<Vec<_>>()
            };

            let (snapshot, point) =
                collect_snapshot(&mut sys, &mut disks, &mut networks, &state, history_vec, 0);

            {
                let mut guard = state.system_history.write().await;
                if guard.len() >= SYSTEM_HISTORY_LIMIT {
                    guard.pop_front();
                }
                guard.push_back(point);
            }

            let _ = state.metrics_tx.send(snapshot);
        }
    });
}

async fn get_system_stats(State(state): State<AppState>) -> Json<Value> {
    let started = Instant::now();
    let history_vec = {
        let guard = state.system_history.read().await;
        guard.iter().cloned().collect::<Vec<_>>()
    };

    let mut sys = System::new_all();
    let mut disks = Disks::new_with_refreshed_list();
    let mut networks = Networks::new_with_refreshed_list();
    let latency = started.elapsed().as_millis();

    let (snapshot, _) =
        collect_snapshot(&mut sys, &mut disks, &mut networks, &state, history_vec, latency);
    Json(snapshot)
}

async fn ws_system_metrics(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_client(socket, state))
}

async fn handle_ws_client(mut socket: WebSocket, state: AppState) {
    let mut rx = state.metrics_tx.subscribe();
    while let Ok(snapshot) = rx.recv().await {
        let payload = json!({
            "event": "system_realtime_update",
            "data": snapshot,
        });
        if socket
            .send(Message::Text(payload.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}
