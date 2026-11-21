use std::collections::HashMap;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::{
    core::security::generate_short_id,
    error::{AppError, AppResult},
    state::AppState,
};

const ALLOWED_DEVICES: &[&str] = &["pc", "mb", "tv"];
const ALLOWED_DOMAINS: &[&str] = &["netflix.com", "www.netflix.com"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoLinkRecord {
    pub id: String,
    pub device: String,
    pub destination: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
}

async fn read_go_db(state: &AppState) -> HashMap<String, GoLinkRecord> {
    let path = state.config.data_dir().join("go_database.json");
    if !path.exists() {
        return HashMap::new();
    }
    tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

async fn write_go_db(state: &AppState, db: &HashMap<String, GoLinkRecord>) -> AppResult<()> {
    let dir = state.config.data_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("go_database.json");
    let tmp = dir.join("go_database.json.tmp");
    let content = serde_json::to_string_pretty(db)?;
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

pub async fn create_go_link(
    state: &AppState,
    url_str: &str,
    device: &str,
    expires_in: i64,
) -> AppResult<Value> {
    if !ALLOWED_DEVICES.contains(&device) {
        return Err(AppError::bad_request("Invalid device. Allowed: pc, mb, tv"));
    }

    let parsed = Url::parse(url_str).map_err(|_| AppError::bad_request("Invalid URL format."))?;
    if parsed.scheme() != "https" {
        return Err(AppError::bad_request("Only HTTPS URLs are allowed."));
    }

    let host = parsed.host_str().unwrap_or("").to_lowercase();
    if !ALLOWED_DOMAINS.contains(&host.as_str()) {
        return Err(AppError::bad_request("Destination domain is not allowed."));
    }

    let valid_expires = expires_in.max(1);
    let link_id = generate_short_id(10, true);
    let now = Utc::now();
    let expires_at = (now + Duration::seconds(valid_expires)).to_rfc3339();

    let _guard = state.go_links_lock.write().await;
    let mut db = read_go_db(state).await;
    db.insert(
        link_id.clone(),
        GoLinkRecord {
            id: link_id.clone(),
            device: device.to_string(),
            destination: url_str.to_string(),
            created_at: now.to_rfc3339(),
            expires_at: expires_at.clone(),
        },
    );
    write_go_db(state, &db).await?;

    Ok(json!({
        "success": true,
        "id": link_id,
        "device": device,
        "expiresAt": expires_at,
        "url": format!("{}/n/{}/{}", state.config.go_base_url, device, link_id),
    }))
}

pub async fn get_go_metadata(state: &AppState, link_id: &str) -> Option<Value> {
    let _guard = state.go_links_lock.read().await;
    let db = read_go_db(state).await;
    let item = db.get(link_id)?;

    let is_expired = DateTime::parse_from_rfc3339(&item.expires_at)
        .map(|dt| Utc::now() >= dt.with_timezone(&Utc))
        .unwrap_or(false);

    Some(json!({
        "id": item.id,
        "device": item.device,
        "createdAt": item.created_at,
        "expiresAt": item.expires_at,
        "expired": is_expired,
    }))
}

pub async fn resolve_redirect(state: &AppState, device: &str, link_id: &str) -> Option<String> {
    let _guard = state.go_links_lock.write().await;
    let mut db = read_go_db(state).await;
    let item = db.get(link_id)?.clone();

    if item.device != device {
        return None;
    }

    if let Ok(exp_dt) = DateTime::parse_from_rfc3339(&item.expires_at) {
        if Utc::now() >= exp_dt.with_timezone(&Utc) {
            db.remove(link_id);
            let _ = write_go_db(state, &db).await;
            return None;
        }
    }

    Some(item.destination)
}
