use std::collections::HashMap;
use std::fs;
use regex::Regex;
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

pub fn render_template(state: &AppState, relative_path: &str, data: &HashMap<&str, String>) -> String {
    let candidates = [
        state.config.base_dir.join(relative_path),
        state.config.templates_dir().join(relative_path),
    ];

    let Some(path) = candidates.into_iter().find(|p| p.exists()) else {
        tracing::warn!(path = %relative_path, "Email template not found");
        return String::new();
    };

    let Ok(mut template) = fs::read_to_string(path) else {
        return String::new();
    };

    for (key, val) in data {
        let pattern = format!(r"\{{\{{\s*{}\s*\}}\}}", regex::escape(key));
        if let Ok(re) = Regex::new(&pattern) {
            template = re.replace_all(&template, val.as_str()).into_owned();
        }
    }

    template
}

pub fn render_inline_html(html: &str, template_data: &serde_json::Map<String, Value>) -> String {
    let mut final_html = html.to_string();
    for (k, v) in template_data {
        let val = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let pattern = format!(r"\{{\{{\s*{}\s*\}}\}}", regex::escape(k));
        if let Ok(re) = Regex::new(&pattern) {
            final_html = re.replace_all(&final_html, val.as_str()).into_owned();
        }
    }
    final_html
}

pub async fn send_notification_email(
    state: &AppState,
    to: Vec<String>,
    subject: &str,
    html: &str,
) -> AppResult<Value> {
    if to.is_empty() || subject.is_empty() || html.is_empty() {
        return Err(AppError::bad_request(
            "Thiếu thông tin bắt buộc: to, subject, hoặc html",
        ));
    }

    let api_key = &state.config.resend_api_key;
    if api_key.is_empty() {
        return Ok(json!({
            "id": "mock_id",
            "warning": "RESEND_API_KEY is missing",
        }));
    }

    let payload = json!({
        "from": state.config.email_from,
        "to": to,
        "subject": subject,
        "html": html,
    });

    let res = state
        .http
        .post("https://api.resend.com/emails")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&payload)
        .send()
        .await?;

    let status = res.status();
    let data: Value = res.json().await.unwrap_or_else(|_| json!({}));

    if !status.is_success() || data.get("error").is_some() {
        let err_msg = data
            .pointer("/error/message")
            .or_else(|| data.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Failed to send email via Resend");
        return Err(AppError::bad_request(err_msg));
    }

    Ok(data)
}
