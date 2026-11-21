use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use axum_extra::extract::CookieJar;

use crate::{
    core::security::decode_jwt_token,
    error::AppError,
    services::{api_key, session},
    state::AppState,
};

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
    pub avatar: Option<String>,
    pub providers: Vec<String>,
    pub is_static_admin: bool,
}

#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthUser);

#[derive(Debug, Clone)]
pub struct ApiKeyUser {
    pub id: i64,
}

#[derive(Debug, Clone)]
pub struct SepayWebhookAuth;

#[derive(Debug, Clone)]
pub struct GoAdminAuth;

async fn extract_optional_user(parts: &Parts, state: &AppState) -> Result<Option<AuthUser>, AppError> {
    // 0. Static admin credentials via headers
    let admin_user = parts
        .headers
        .get("x-admin-user")
        .and_then(|v| v.to_str().ok());
    let admin_pass = parts
        .headers
        .get("x-admin-pass")
        .and_then(|v| v.to_str().ok());

    if let (Some(u), Some(p)) = (admin_user, admin_pass) {
        if u == state.config.admin_username && p == state.config.admin_password {
            return Ok(Some(AuthUser {
                id: 1,
                username: u.to_string(),
                email: None,
                avatar: None,
                providers: vec![],
                is_static_admin: true,
            }));
        }
    }

    // 1. Session cookie
    let jar = CookieJar::from_headers(&parts.headers);
    if let Some(cookie) = jar.get("session") {
        if let Ok(Some(user)) = session::verify_session(state, cookie.value()).await {
            return Ok(Some(user));
        }
    }

    // 2. JWT Bearer token
    if let Some(auth_val) = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = auth_val.strip_prefix("Bearer ") {
            let claims = decode_jwt_token(token.trim(), &state.config.jwt_secret)?;
            return Ok(Some(AuthUser {
                id: claims.id,
                username: claims.username,
                email: claims.email,
                avatar: claims.avatar,
                providers: claims.providers,
                is_static_admin: false,
            }));
        }
    }

    Ok(None)
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        extract_optional_user(parts, state)
            .await?
            .ok_or_else(|| AppError::unauthorized("Chưa đăng nhập"))
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;

        if user.is_static_admin || user.id == 1 {
            return Ok(AdminUser(user));
        }

        Err(AppError::forbidden(
            "Quyền truy cập bị từ chối. Chỉ dành cho Admin!",
        ))
    }
}

impl FromRequestParts<AppState> for ApiKeyUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_val = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::unauthorized("Missing or wrong API Key format"))?;

        let raw_key = auth_val
            .strip_prefix("Bearer ")
            .map(str::trim)
            .ok_or_else(|| AppError::unauthorized("Missing or wrong API Key format"))?;

        let user_id = api_key::validate_api_key(state, raw_key)
            .await?
            .ok_or_else(|| AppError::unauthorized("API Key is invalid or deleted"))?;

        Ok(ApiKeyUser { id: user_id })
    }
}

impl FromRequestParts<AppState> for SepayWebhookAuth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_val = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();

        let expected = format!("Apikey {}", state.config.sepay_webhook_token);
        if state.config.sepay_webhook_token.is_empty() || auth_val != expected {
            return Err(AppError::unauthorized("Unauthorized webhook"));
        }

        Ok(SepayWebhookAuth)
    }
}

impl FromRequestParts<AppState> for GoAdminAuth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let admin_user = parts
            .headers
            .get("x-admin-user")
            .and_then(|v| v.to_str().ok());
        let admin_pass = parts
            .headers
            .get("x-admin-pass")
            .and_then(|v| v.to_str().ok());

        if let (Some(u), Some(p)) = (admin_user, admin_pass) {
            if u == state.config.admin_username && p == state.config.admin_password {
                return Ok(GoAdminAuth);
            }
        }

        if !state.config.admin_token.is_empty() {
            if let Some(auth_val) = parts
                .headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
            {
                let token = auth_val
                    .strip_prefix("Bearer ")
                    .or_else(|| auth_val.strip_prefix("bearer "))
                    .unwrap_or(auth_val)
                    .trim();
                if token == state.config.admin_token {
                    return Ok(GoAdminAuth);
                }
            }
        }

        Err(AppError::unauthorized("Unauthorized"))
    }
}
