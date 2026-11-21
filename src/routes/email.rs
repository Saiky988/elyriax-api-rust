use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    core::{auth::AuthUser, response::send_success},
    error::{AppError, AppResult},
    services::email::{render_inline_html, send_notification_email},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/notify", post(send_notification))
}

#[derive(Deserialize)]
struct NotifyRequest {
    to: Value,
    subject: Option<String>,
    html: Option<String>,
    template_data: Option<Map<String, Value>>,
}

async fn send_notification(
    State(state): State<AppState>,
    _user: AuthUser,
    Json(body): Json<NotifyRequest>,
) -> AppResult<Json<Value>> {
    let recipients: Vec<String> = match body.to {
        Value::String(s) if !s.trim().is_empty() => vec![s],
        Value::Array(arr) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .filter(|s| !s.trim().is_empty())
            .collect(),
        _ => vec![],
    };

    let subject = body
        .subject
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Vui lòng cung cấp đầy đủ params: to, subject, html"))?;
    let html = body
        .html
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("Vui lòng cung cấp đầy đủ params: to, subject, html"))?;

    if recipients.is_empty() {
        return Err(AppError::bad_request(
            "Vui lòng cung cấp đầy đủ params: to, subject, html",
        ));
    }

    let final_html = if let Some(ref td) = body.template_data {
        render_inline_html(&html, td)
    } else {
        html
    };

    let result = send_notification_email(&state, recipients, &subject, &final_html).await?;
    Ok(send_success("Gửi email thông báo thành công!", result))
}
