use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    core::{auth::AuthUser, security::create_jwt_token},
    error::AppResult,
    services::{
        api_key::{delete_api_key, get_api_key_info, get_or_create_api_key},
        auth::{handle_oauth_login_or_link, OAuthProfile, OAuthResult},
        session::{create_session, revoke_all_user_sessions, revoke_session},
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(get_me_ep))
        .route("/login/{provider}", get(oauth_login))
        .route("/github/callback", get(github_callback))
        .route("/discord/callback", get(discord_callback))
        .route("/google/callback", get(google_callback))
        .route("/unlink/{provider}", post(unlink_provider))
        .route("/apikey", post(create_api_key_ep).get(get_api_key_ep).delete(delete_api_key_ep))
        .route("/logout", post(logout))
        .route("/logout-all", post(logout_all))
}

#[derive(Deserialize)]
struct LoginQuery {
    token: Option<String>,
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

fn build_session_cookie(session_token: String) -> Cookie<'static> {
    Cookie::build(("session", session_token))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::days(365))
        .build()
}

fn clear_session_cookie() -> Cookie<'static> {
    Cookie::build(("session", ""))
        .path("/")
        .max_age(time::Duration::seconds(0))
        .build()
}

async fn oauth_login(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<LoginQuery>,
) -> Response {
    let state_param = query
        .token
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "login".to_string());
    let auth_base = format!("{}/v1/auth", state.config.base_url);

    let auth_url = match provider.as_str() {
        "discord" => format!(
            "https://discord.com/api/oauth2/authorize?client_id={}&redirect_uri={}&response_type=code&scope=identify%20email&state={}",
            urlencoding::encode(&state.config.discord_client_id),
            urlencoding::encode(&format!("{}/discord/callback", auth_base)),
            urlencoding::encode(&state_param),
        ),
        "google" => format!(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=profile%20email&state={}",
            urlencoding::encode(&state.config.google_client_id),
            urlencoding::encode(&format!("{}/google/callback", auth_base)),
            urlencoding::encode(&state_param),
        ),
        "github" => format!(
            "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=user%3Aemail&state={}",
            urlencoding::encode(&state.config.github_client_id),
            urlencoding::encode(&format!("{}/github/callback", auth_base)),
            urlencoding::encode(&state_param),
        ),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Provider không hợp lệ!" })),
            )
                .into_response();
        }
    };

    Redirect::temporary(&auth_url).into_response()
}

async fn finish_oauth(
    state: &AppState,
    headers: &HeaderMap,
    jar: CookieJar,
    profile: OAuthProfile,
    state_param: Option<&str>,
) -> Response {
    let frontend = &state.config.frontend_url;
    let provider_name = profile.provider.clone();

    match handle_oauth_login_or_link(state, profile, state_param).await {
        Ok(OAuthResult::Linked { .. }) => {
            Redirect::temporary(&format!("{}/?link=success&provider={}", frontend, provider_name))
                .into_response()
        }
        Ok(OAuthResult::LoggedIn { user, .. }) => {
            let session_token = match create_session(state, user.id, headers, true).await {
                Ok(t) => t,
                Err(e) => {
                    return Redirect::temporary(&format!(
                        "{}/?error={}",
                        frontend,
                        urlencoding::encode(&e.to_string())
                    ))
                    .into_response();
                }
            };

            let jwt_token = match create_jwt_token(
                user.id,
                user.username,
                user.email,
                user.avatar,
                user.providers,
                &state.config.jwt_secret,
                365,
            ) {
                Ok(t) => t,
                Err(e) => {
                    return Redirect::temporary(&format!(
                        "{}/?error={}",
                        frontend,
                        urlencoding::encode(&e.to_string())
                    ))
                    .into_response();
                }
            };

            let updated_jar = jar.add(build_session_cookie(session_token));
            (
                updated_jar,
                Redirect::temporary(&format!("{}/?token={}", frontend, jwt_token)),
            )
                .into_response()
        }
        Err(e) => Redirect::temporary(&format!(
            "{}/?error={}",
            frontend,
            urlencoding::encode(&e.to_string())
        ))
        .into_response(),
    }
}

async fn github_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let frontend = &state.config.frontend_url;
    let Some(code) = query.code.filter(|s| !s.is_empty()) else {
        return Redirect::temporary(&format!("{}/?error=GitHub_Login_Failed", frontend)).into_response();
    };

    let auth_base = format!("{}/v1/auth", state.config.base_url);
    let token_res = state
        .http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&json!({
            "client_id": state.config.github_client_id,
            "client_secret": state.config.github_client_secret,
            "code": code,
            "redirect_uri": format!("{}/github/callback", auth_base),
        }))
        .send()
        .await;

    let token_data: Value = match token_res {
        Ok(r) => r.json().await.unwrap_or_else(|_| json!({})),
        Err(e) => {
            return Redirect::temporary(&format!(
                "{}/?error={}",
                frontend,
                urlencoding::encode(&e.to_string())
            ))
            .into_response();
        }
    };

    let Some(access_token) = token_data.get("access_token").and_then(Value::as_str) else {
        let err = token_data
            .get("error_description")
            .or_else(|| token_data.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("GitHub_Token_Error");
        return Redirect::temporary(&format!(
            "{}/?error={}",
            frontend,
            urlencoding::encode(err)
        ))
        .into_response();
    };

    let bearer = format!("Bearer {}", access_token);
    let user_req = state
        .http
        .get("https://api.github.com/user")
        .header("Authorization", &bearer)
        .header("User-Agent", "Elyriax-API/3.0")
        .send();
    let email_req = state
        .http
        .get("https://api.github.com/user/emails")
        .header("Authorization", &bearer)
        .header("User-Agent", "Elyriax-API/3.0")
        .send();

    let Ok((user_res, email_res)) = tokio::try_join!(user_req, email_req) else {
        return Redirect::temporary(&format!("{}/?error=GitHub_Profile_Error", frontend)).into_response();
    };

    let user_data: Value = user_res.json().await.unwrap_or_else(|_| json!({}));
    let email_data: Value = email_res.json().await.unwrap_or_else(|_| json!([]));

    let primary_email = email_data
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("primary").and_then(Value::as_bool) == Some(true))
                .and_then(|e| e.get("email").and_then(Value::as_str))
        })
        .map(String::from);

    let profile = OAuthProfile {
        provider: "github".to_string(),
        id: user_data
            .get("id")
            .map(|v| v.to_string().trim_matches('"').to_string())
            .unwrap_or_default(),
        email: primary_email,
        name: user_data
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| user_data.get("login").and_then(Value::as_str))
            .map(String::from),
        avatar: user_data
            .get("avatar_url")
            .and_then(Value::as_str)
            .map(String::from),
    };

    finish_oauth(&state, &headers, jar, profile, query.state.as_deref()).await
}

async fn discord_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let frontend = &state.config.frontend_url;
    let Some(code) = query.code.filter(|s| !s.is_empty()) else {
        return Redirect::temporary(&format!("{}/?error=Discord_Login_Failed", frontend)).into_response();
    };

    let auth_base = format!("{}/v1/auth", state.config.base_url);
    let redirect_uri = format!("{}/discord/callback", auth_base);
    let form = [
        ("client_id", state.config.discord_client_id.as_str()),
        ("client_secret", state.config.discord_client_secret.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
    ];

    let token_res = state
        .http
        .post("https://discord.com/api/oauth2/token")
        .form(&form)
        .send()
        .await;

    let token_data: Value = match token_res {
        Ok(r) => r.json().await.unwrap_or_else(|_| json!({})),
        Err(e) => {
            return Redirect::temporary(&format!(
                "{}/?error={}",
                frontend,
                urlencoding::encode(&e.to_string())
            ))
            .into_response();
        }
    };

    let Some(access_token) = token_data.get("access_token").and_then(Value::as_str) else {
        let err = token_data
            .get("error_description")
            .or_else(|| token_data.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("Discord_Token_Error");
        return Redirect::temporary(&format!(
            "{}/?error={}",
            frontend,
            urlencoding::encode(err)
        ))
        .into_response();
    };

    let Ok(user_res) = state
        .http
        .get("https://discord.com/api/users/@me")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
    else {
        return Redirect::temporary(&format!("{}/?error=Discord_Profile_Error", frontend)).into_response();
    };

    let user_data: Value = user_res.json().await.unwrap_or_else(|_| json!({}));
    let uid = user_data
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let avatar_url = user_data
        .get("avatar")
        .and_then(Value::as_str)
        .map(|a| format!("https://cdn.discordapp.com/avatars/{}/{}.png", uid, a));

    let profile = OAuthProfile {
        provider: "discord".to_string(),
        id: uid,
        email: user_data.get("email").and_then(Value::as_str).map(String::from),
        name: user_data
            .get("global_name")
            .and_then(Value::as_str)
            .or_else(|| user_data.get("username").and_then(Value::as_str))
            .map(String::from),
        avatar: avatar_url,
    };

    finish_oauth(&state, &headers, jar, profile, query.state.as_deref()).await
}

async fn google_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let frontend = &state.config.frontend_url;
    let Some(code) = query.code.filter(|s| !s.is_empty()) else {
        return Redirect::temporary(&format!("{}/?error=Google_Login_Failed", frontend)).into_response();
    };

    let auth_base = format!("{}/v1/auth", state.config.base_url);
    let token_res = state
        .http
        .post("https://oauth2.googleapis.com/token")
        .json(&json!({
            "client_id": state.config.google_client_id,
            "client_secret": state.config.google_client_secret,
            "code": code,
            "grant_type": "authorization_code",
            "redirect_uri": format!("{}/google/callback", auth_base),
        }))
        .send()
        .await;

    let token_data: Value = match token_res {
        Ok(r) => r.json().await.unwrap_or_else(|_| json!({})),
        Err(e) => {
            return Redirect::temporary(&format!(
                "{}/?error={}",
                frontend,
                urlencoding::encode(&e.to_string())
            ))
            .into_response();
        }
    };

    let Some(access_token) = token_data.get("access_token").and_then(Value::as_str) else {
        let err = token_data
            .get("error_description")
            .or_else(|| token_data.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("Google_Token_Error");
        return Redirect::temporary(&format!(
            "{}/?error={}",
            frontend,
            urlencoding::encode(err)
        ))
        .into_response();
    };

    let Ok(user_res) = state
        .http
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
    else {
        return Redirect::temporary(&format!("{}/?error=Google_Profile_Error", frontend)).into_response();
    };

    let user_data: Value = user_res.json().await.unwrap_or_else(|_| json!({}));
    let profile = OAuthProfile {
        provider: "google".to_string(),
        id: user_data
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        email: user_data.get("email").and_then(Value::as_str).map(String::from),
        name: user_data.get("name").and_then(Value::as_str).map(String::from),
        avatar: user_data.get("picture").and_then(Value::as_str).map(String::from),
    };

    finish_oauth(&state, &headers, jar, profile, query.state.as_deref()).await
}

async fn unlink_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(provider): Path<String>,
) -> AppResult<Response> {
    let pool = state.db()?;
    let links = sqlx::query("SELECT provider FROM user_oauth_accounts WHERE user_id = ?")
        .bind(user.id)
        .fetch_all(pool)
        .await?;

    let u = sqlx::query("SELECT password_hash FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_optional(pool)
        .await?;
    let has_password = u
        .and_then(|r| r.try_get::<Option<String>, _>("password_hash").ok().flatten())
        .is_some();

    let total_methods = links.len() + if has_password { 1 } else { 0 };
    if total_methods <= 1 {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "Không thể hủy liên kết. Đây là phương thức đăng nhập duy nhất của bạn!",
            })),
        )
            .into_response());
    }

    let exists = links
        .iter()
        .any(|r| r.try_get::<String, _>("provider").ok().as_deref() == Some(provider.as_str()));
    if !exists {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": "Tài khoản chưa được liên kết phương thức này.",
            })),
        )
            .into_response());
    }

    sqlx::query("DELETE FROM user_oauth_accounts WHERE user_id = ? AND provider = ?")
        .bind(user.id)
        .bind(&provider)
        .execute(pool)
        .await?;

    Ok(Json(json!({
        "success": true,
        "message": format!("Đã hủy liên kết {} thành công.", provider),
    }))
    .into_response())
}

async fn create_api_key_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(get_or_create_api_key(&state, user.id).await?))
}

async fn get_api_key_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Response> {
    match get_api_key_info(&state, user.id).await? {
        Some(info) => Ok(Json(json!({ "success": true, "data": info })).into_response()),
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "error": "Chưa có API Key" })),
        )
            .into_response()),
    }
}

async fn delete_api_key_ep(State(state): State<AppState>, user: AuthUser) -> AppResult<Response> {
    if delete_api_key(&state, user.id).await? {
        Ok(Json(json!({ "success": true, "message": "Đã xóa API Key thành công" })).into_response())
    } else {
        Ok((
            StatusCode::NOT_FOUND,
            Json(json!({ "success": false, "error": "Chưa có API Key để xóa" })),
        )
            .into_response())
    }
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> AppResult<impl IntoResponse> {
    if let Some(cookie) = jar.get("session") {
        let _ = revoke_session(&state, cookie.value()).await;
    }
    let updated_jar = jar.add(clear_session_cookie());
    Ok((
        updated_jar,
        Json(json!({ "success": true, "message": "Đăng xuất thành công" })),
    ))
}

async fn logout_all(
    State(state): State<AppState>,
    jar: CookieJar,
    user: AuthUser,
) -> AppResult<impl IntoResponse> {
    revoke_all_user_sessions(&state, user.id).await?;
    let updated_jar = jar.add(clear_session_cookie());
    Ok((
        updated_jar,
        Json(json!({ "success": true, "message": "Đã đăng xuất khỏi toàn bộ thiết bị" })),
    ))
}

async fn get_me_ep(State(state): State<AppState>, user: AuthUser) -> Json<Value> {
    let is_admin = user.is_admin(&state);
    Json(json!({
        "success": true,
        "data": {
            "id": user.id,
            "username": user.username,
            "email": user.email,
            "avatar": user.avatar,
            "providers": user.providers,
            "is_admin": is_admin,
        }
    }))
}
