use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    services::{
        email::{render_template, send_notification_email},
        genshin::{
            account::update_redeem_timestamp,
            codes::{get_active_code_entries, get_code_info},
            normalize_region, server_display_name,
        },
    },
    state::{AppState, RedeemHistoryStore, RedeemLogEntry},
};

const HOYO_ROLE_URL: &str =
    "https://api-os-takumi.hoyoverse.com/binding/api/getUserGameRolesByCookie";
const HOYO_REDEEM_URL: &str =
    "https://public-operation-hk4e.hoyoverse.com/common/apicdkey/api/webExchangeCdkey";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

const MAX_LOGS_PER_ACCOUNT: usize = 50;
const REDEEM_COOLDOWN_MS: u64 = 5500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedeemOutcomeKind {
    Redeemed,
    AlreadyClaimed,
    ExpiredOrInvalid,
    CookieExpired,
    Cooldown,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SingleRedeemResult {
    pub kind: RedeemOutcomeKind,
    pub retcode: i64,
    pub message: String,
    pub uid: String,
    pub region: String,
    pub code: String,
}

pub async fn load_redeem_history(state: &AppState) {
    let path = state.config.data_dir().join("genshin_redeem_history.json");
    if !path.exists() {
        return;
    }
    if let Ok(raw) = tokio::fs::read_to_string(&path).await {
        if let Ok(parsed) = serde_json::from_str::<RedeemHistoryStore>(&raw) {
            let mut guard = state.redeem_history.write().await;
            *guard = parsed;
        }
    }
}

async fn persist_redeem_history(state: &AppState, store: &RedeemHistoryStore) {
    let dir = state.config.data_dir();
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return;
    }
    let path = dir.join("genshin_redeem_history.json");
    let tmp = dir.join("genshin_redeem_history.json.tmp");
    if let Ok(serialized) = serde_json::to_string_pretty(store) {
        if tokio::fs::write(&tmp, serialized).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &path).await;
        }
    }
}

async fn resolve_uid_and_region(
    state: &AppState,
    cookie: &str,
    server: &str,
    uid_hint: Option<&str>,
) -> AppResult<(String, String)> {
    let target_region = normalize_region(server)
        .ok_or_else(|| AppError::bad_request(format!("Invalid server provided: {}", server)))?;

    if let Some(uid) = uid_hint {
        if !uid.trim().is_empty() {
            return Ok((uid.trim().to_string(), target_region.to_string()));
        }
    }

    let role_data: Value = state
        .http
        .get(format!("{}?game_biz=hk4e_global", HOYO_ROLE_URL))
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send()
        .await?
        .json()
        .await?;

    if role_data.get("retcode").and_then(Value::as_i64) != Some(0) {
        return Err(AppError::bad_request(
            "Invalid cookie or failed to fetch account list",
        ));
    }

    let roles = role_data
        .pointer("/data/list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let selected = roles
        .into_iter()
        .find(|r| r.get("region").and_then(Value::as_str) == Some(target_region))
        .ok_or_else(|| AppError::bad_request(format!("No account found on server: {}", server)))?;

    let uid = selected
        .get("game_uid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Ok((uid, target_region.to_string()))
}

fn classify_retcode(retcode: i64, msg: &str) -> RedeemOutcomeKind {
    let lower = msg.to_lowercase();
    match retcode {
        0 => RedeemOutcomeKind::Redeemed,
        -2017 | -2018 => RedeemOutcomeKind::AlreadyClaimed,
        -2001 | -2004 | -2006 => RedeemOutcomeKind::ExpiredOrInvalid,
        -1071 | -100 | -10001 => RedeemOutcomeKind::CookieExpired,
        -2016 => RedeemOutcomeKind::Cooldown,
        _ if lower.contains("already in use") || lower.contains("already claimed") => {
            RedeemOutcomeKind::AlreadyClaimed
        }
        _ if lower.contains("expired") || lower.contains("invalid") => {
            RedeemOutcomeKind::ExpiredOrInvalid
        }
        _ if lower.contains("login") || lower.contains("cookie") => RedeemOutcomeKind::CookieExpired,
        _ if lower.contains("cooldown") => RedeemOutcomeKind::Cooldown,
        _ => RedeemOutcomeKind::Failed,
    }
}

async fn execute_redeem_http(
    state: &AppState,
    cookie: &str,
    uid: &str,
    region: &str,
    code: &str,
) -> AppResult<SingleRedeemResult> {
    let clean_code = code.trim().to_uppercase();
    let url = format!(
        "{}?uid={}&region={}&lang=en&cdkey={}&game_biz=hk4e_global&sLangKey=en-us",
        HOYO_REDEEM_URL,
        urlencoding::encode(uid),
        urlencoding::encode(region),
        urlencoding::encode(&clean_code),
    );

    let data: Value = state
        .http
        .get(&url)
        .header("Cookie", cookie)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send()
        .await?
        .json()
        .await?;

    let retcode = data.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
    let msg = data
        .pointer("/data/msg")
        .or_else(|| data.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(if retcode == 0 {
            "Success"
        } else {
            "Redemption failed"
        })
        .to_string();

    let kind = classify_retcode(retcode, &msg);
    Ok(SingleRedeemResult {
        kind,
        retcode,
        message: msg,
        uid: uid.to_string(),
        region: region.to_string(),
        code: clean_code,
    })
}

/// Public single-code redeem (compatible with `POST /v1/genshin?type=redeemcode` and account redeem).
pub async fn redeem_code(
    state: &AppState,
    cookie: &str,
    server: &str,
    code: &str,
    uid_hint: Option<&str>,
) -> AppResult<Value> {
    if cookie.is_empty() || server.is_empty() || code.trim().is_empty() {
        return Err(AppError::bad_request("Missing cookie, server, or code"));
    }

    let (uid, region) = resolve_uid_and_region(state, cookie, server, uid_hint).await?;
    let mut res = execute_redeem_http(state, cookie, &uid, &region, code).await?;

    // Retry once automatically if HoYoLAB reports 5s cooldown (-2016)
    if res.kind == RedeemOutcomeKind::Cooldown {
        tokio::time::sleep(Duration::from_millis(REDEEM_COOLDOWN_MS)).await;
        res = execute_redeem_http(state, cookie, &uid, &region, code).await?;
    }

    if res.retcode != 0 {
        return Ok(json!({
            "ok": false,
            "error": res.message,
            "retcode": res.retcode,
            "code": res.code,
            "uid": res.uid,
            "server": server_display_name(&res.region),
        }));
    }

    let info = get_code_info(state, &res.code).await;

    Ok(json!({
        "ok": true,
        "message": res.message,
        "uid": res.uid,
        "server": server_display_name(&res.region),
        "code": res.code,
        "rewards": info.get("rewards").cloned().unwrap_or_else(|| json!([])),
        "validity": info.get("validity").cloned().unwrap_or(Value::Null),
    }))
}

/// Auto-fetch all active Genshin codes and redeem every unclaimed code for a specific account.
pub async fn auto_redeem_for_account(
    state: &AppState,
    account_id: i64,
    cookie: &str,
    uid: &str,
    server: &str,
    nickname: &str,
    notify_email: Option<&str>,
    force_refresh_codes: bool,
) -> AppResult<Value> {
    let active_entries = get_active_code_entries(state, force_refresh_codes).await?;
    if active_entries.is_empty() {
        return Ok(json!({
            "ok": true,
            "account_id": account_id,
            "uid": uid,
            "total_active_codes": 0,
            "redeemed": [],
            "already_claimed": [],
            "skipped_from_history": 0,
            "expired": [],
            "failed": [],
        }));
    }

    let acc_key = account_id.to_string();
    let (candidate_entries, skipped_count) = {
        let guard = state.redeem_history.read().await;
        let claimed_set = guard.accounts.get(&acc_key);
        let mut candidates = Vec::new();
        let mut skipped = 0usize;

        for entry in &active_entries {
            let already_done = claimed_set.map(|s| s.contains(&entry.code)).unwrap_or(false);
            let globally_dead = guard.global_expired.contains(&entry.code);
            if already_done || globally_dead {
                skipped += 1;
            } else {
                candidates.push(entry.clone());
            }
        }
        (candidates, skipped)
    };

    if candidate_entries.is_empty() {
        return Ok(json!({
            "ok": true,
            "account_id": account_id,
            "uid": uid,
            "total_active_codes": active_entries.len(),
            "redeemed": [],
            "already_claimed": [],
            "skipped_from_history": skipped_count,
            "expired": [],
            "failed": [],
        }));
    }

    let (resolved_uid, region) =
        resolve_uid_and_region(state, cookie, server, Some(uid)).await?;

    let mut redeemed: Vec<Value> = Vec::new();
    let mut redeemed_code_names: Vec<String> = Vec::new();
    let mut already_claimed: Vec<String> = Vec::new();
    let mut expired: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut cookie_expired = false;

    for (idx, entry) in candidate_entries.iter().enumerate() {
        if idx > 0 {
            tokio::time::sleep(Duration::from_millis(REDEEM_COOLDOWN_MS)).await;
        }

        let mut attempt =
            execute_redeem_http(state, cookie, &resolved_uid, &region, &entry.code).await?;

        if attempt.kind == RedeemOutcomeKind::Cooldown {
            tokio::time::sleep(Duration::from_millis(REDEEM_COOLDOWN_MS)).await;
            attempt =
                execute_redeem_http(state, cookie, &resolved_uid, &region, &entry.code).await?;
        }

        let now_iso = Utc::now().to_rfc3339();
        let mut store = state.redeem_history.write().await;

        match attempt.kind {
            RedeemOutcomeKind::Redeemed => {
                store
                    .accounts
                    .entry(acc_key.clone())
                    .or_default()
                    .insert(entry.code.clone());
                let logs = store.logs.entry(acc_key.clone()).or_default();
                logs.insert(
                    0,
                    RedeemLogEntry {
                        code: entry.code.clone(),
                        status: "redeemed".to_string(),
                        message: attempt.message.clone(),
                        rewards: entry.rewards.clone(),
                        redeemed_at: now_iso,
                    },
                );
                logs.truncate(MAX_LOGS_PER_ACCOUNT);

                redeemed_code_names.push(entry.code.clone());
                redeemed.push(json!({
                    "code": entry.code,
                    "message": attempt.message,
                    "rewards": entry.rewards,
                }));
            }
            RedeemOutcomeKind::AlreadyClaimed => {
                store
                    .accounts
                    .entry(acc_key.clone())
                    .or_default()
                    .insert(entry.code.clone());
                already_claimed.push(entry.code.clone());
            }
            RedeemOutcomeKind::ExpiredOrInvalid => {
                store.global_expired.insert(entry.code.clone());
                expired.push(entry.code.clone());
            }
            RedeemOutcomeKind::CookieExpired => {
                cookie_expired = true;
                failed.push(json!({
                    "code": entry.code,
                    "retcode": attempt.retcode,
                    "error": attempt.message,
                }));
                drop(store);
                break;
            }
            _ => {
                failed.push(json!({
                    "code": entry.code,
                    "retcode": attempt.retcode,
                    "error": attempt.message,
                }));
            }
        }
    }

    // Persist updated history atomically
    {
        let store = state.redeem_history.read().await;
        persist_redeem_history(state, &store).await;
    }

    let _ = update_redeem_timestamp(state, account_id).await;

    // Send email notification if at least 1 new code was successfully redeemed
    if !redeemed_code_names.is_empty() {
        if let Some(email) = notify_email.filter(|e| !e.trim().is_empty()) {
            let mut tpl_data = HashMap::new();
            let display_name = if nickname.is_empty() { uid } else { nickname };
            tpl_data.insert("nickname", display_name.to_string());
            tpl_data.insert("uid", resolved_uid.clone());
            tpl_data.insert("server", server_display_name(&region).to_string());
            tpl_data.insert("redeemed_count", redeemed_code_names.len().to_string());
            tpl_data.insert("codes_list", redeemed_code_names.join(", "));

            let html = render_template(state, "templates/email/genshin-redeem-success.html", &tpl_data);
            if !html.is_empty() {
                let subject = format!(
                    "[Genshin] Tự động nhập thành công {} giftcode — {}",
                    redeemed_code_names.len(),
                    display_name
                );
                if let Err(err) =
                    send_notification_email(state, vec![email.to_string()], &subject, &html).await
                {
                    tracing::warn!(uid = %resolved_uid, error = %err, "Failed to send auto-redeem email");
                }
            }
        }
    }

    Ok(json!({
        "ok": !cookie_expired,
        "account_id": account_id,
        "uid": resolved_uid,
        "server": server_display_name(&region),
        "cookie_expired": cookie_expired,
        "total_active_codes": active_entries.len(),
        "skipped_from_history": skipped_count,
        "redeemed_count": redeemed.len(),
        "redeemed": redeemed,
        "already_claimed": already_claimed,
        "expired": expired,
        "failed": failed,
    }))
}

pub async fn get_account_redeem_history(state: &AppState, account_id: i64) -> Value {
    let acc_key = account_id.to_string();
    let guard = state.redeem_history.read().await;
    let claimed_codes: Vec<String> = guard
        .accounts
        .get(&acc_key)
        .map(|s| {
            let mut v: Vec<String> = s.iter().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    let logs = guard.logs.get(&acc_key).cloned().unwrap_or_default();

    json!({
        "account_id": account_id,
        "total_claimed_codes": claimed_codes.len(),
        "claimed_codes": claimed_codes,
        "recent_logs": logs,
    })
}
