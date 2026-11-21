use serde_json::Value;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

pub async fn get_banners(state: &AppState) -> AppResult<Vec<Value>> {
    let path = state.config.data_dir().join("banners.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| AppError::bad_request(format!("Không thể tải data banner: {}", e)))?;
    let parsed: Vec<Value> = serde_json::from_str(&raw)
        .map_err(|e| AppError::bad_request(format!("Không thể tải data banner: {}", e)))?;
    Ok(parsed)
}
