use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use serde_json::{json, Value};

pub fn send_success<T: Serialize>(message: &str, data: T) -> Json<Value> {
    Json(json!({
        "success": true,
        "message": message,
        "data": data,
    }))
}

pub fn send_success_empty(message: &str) -> Json<Value> {
    Json(json!({
        "success": true,
        "message": message,
        "data": {},
    }))
}

pub fn send_error(status: StatusCode, message: &str) -> impl IntoResponse {
    (
        status,
        Json(json!({
            "success": false,
            "message": message,
        })),
    )
}
