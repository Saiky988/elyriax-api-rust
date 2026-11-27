use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, Timelike, Utc};
use serde_json::{json, Value};

use crate::{
    core::security::decrypt_cookie,
    services::{
        email::{render_template, send_notification_email},
        hoyo::{
            account::{
                get_all_auto_checkin_accounts, get_all_auto_daily_note_accounts,
                get_all_auto_redeem_accounts, update_checkin_status, update_daily_note_timestamp,
            },
            checkin::check_in_daily,
            game_record::get_daily_note,
            redeem::auto_redeem_for_account,
            HoyoGame,
        },
        provider::sync_provider_prices,
    },
    state::AppState,
};

fn now_vn() -> chrono::DateTime<FixedOffset> {
    let tz_vn = FixedOffset::east_opt(7 * 3600).expect("valid UTC+7 offset");
    Utc::now().with_timezone(&tz_vn)
}

fn already_processed_today(last_checkin_at: Option<DateTime<Utc>>) -> bool {
    let Some(dt) = last_checkin_at else {
        return false;
    };
    let tz_vn = FixedOffset::east_opt(7 * 3600).expect("valid UTC+7 offset");
    let today_vn = now_vn().date_naive();
    dt.with_timezone(&tz_vn).date_naive() == today_vn
}

/// Runs daily check-in across ALL 3 games: Genshin Impact, Honkai: Star Rail, and Zenless Zone Zero.
pub async fn run_genshin_auto_checkin(state: AppState) {
    if state
        .scheduler
        .checkin_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::info!("[HOYO CHECKIN] Already running, skipping overlapping trigger");
        return;
    }

    tracing::info!("[HOYO CHECKIN] Multi-game scheduler started (Genshin, HSR, ZZZ)");
    let accounts = match get_all_auto_checkin_accounts(&state).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!(error = %e, "[HOYO CHECKIN] Cannot fetch accounts");
            state.scheduler.checkin_running.store(false, Ordering::SeqCst);
            return;
        }
    };

    tracing::info!(count = accounts.len(), "[HOYO CHECKIN] Accounts queued");

    for acc in accounts {
        if already_processed_today(acc.last_checkin_at) {
            continue;
        }

        let cookie = match decrypt_cookie(&acc.cookie_encrypted, &state.config.genshin_cookie_secret) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(game = acc.game.slug(), account_id = acc.id, error = %e, "[HOYO CHECKIN] Decrypt failed");
                let _ = update_checkin_status(&state, acc.game, acc.id, "cookie_expired").await;
                continue;
            }
        };

        match check_in_daily(&state, acc.game, &cookie, &acc.server).await {
            Ok(result) => {
                let risk_code = result.get("risk_code").and_then(Value::as_i64).unwrap_or(0);
                let claimed = result.get("claimed").and_then(Value::as_bool).unwrap_or(false);
                let status_str = if risk_code > 0 {
                    "captcha"
                } else if claimed {
                    "success"
                } else {
                    "already_claimed"
                };

                let _ = update_checkin_status(&state, acc.game, acc.id, status_str).await;

                if status_str == "success" {
                    if let Some(ref email) = acc.email {
                        if !email.is_empty() {
                            let nickname = result
                                .get("nickname")
                                .and_then(Value::as_str)
                                .unwrap_or(&acc.nickname)
                                .to_string();
                            let uid = result
                                .get("uid")
                                .and_then(Value::as_str)
                                .unwrap_or(&acc.uid)
                                .to_string();

                            let mut tpl = HashMap::new();
                            tpl.insert("nickname", nickname.clone());
                            tpl.insert("uid", uid.clone());
                            tpl.insert(
                                "today_reward_name",
                                result
                                    .pointer("/today_reward/name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Không xác định")
                                    .to_string(),
                            );
                            tpl.insert(
                                "today_reward_count",
                                result
                                    .pointer("/today_reward/count")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "1".to_string()),
                            );
                            tpl.insert(
                                "signed_days",
                                result
                                    .get("signed_days")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "0".to_string()),
                            );
                            tpl.insert(
                                "total_days_month",
                                result
                                    .get("total_days_month")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "30".to_string()),
                            );
                            tpl.insert(
                                "tomorrow_reward_name",
                                result
                                    .pointer("/tomorrow_reward/name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Không xác định")
                                    .to_string(),
                            );
                            tpl.insert(
                                "tomorrow_reward_count",
                                result
                                    .pointer("/tomorrow_reward/count")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "1".to_string()),
                            );

                            let html = render_template(
                                &state,
                                "templates/email/genshin-checkin-success.html",
                                &tpl,
                            );
                            if !html.is_empty() {
                                let subject = format!(
                                    "[{}] Điểm danh hàng ngày thành công — {}",
                                    acc.game.display_name(),
                                    nickname
                                );
                                let _ = send_notification_email(
                                    &state,
                                    vec![email.clone()],
                                    &subject,
                                    &html,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
            Err(err) => {
                let lower = err.to_string().to_lowercase();
                let status_str = if lower.contains("cookie")
                    || lower.contains("login")
                    || lower.contains("account")
                {
                    "cookie_expired"
                } else if lower.contains("risk") || lower.contains("captcha") {
                    "captcha"
                } else {
                    "error"
                };
                tracing::error!(game = acc.game.slug(), account_id = acc.id, uid = %acc.uid, status = status_str, error = %err, "[HOYO CHECKIN] Failed");
                let _ = update_checkin_status(&state, acc.game, acc.id, status_str).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    *state.scheduler.last_checkin_run.write().await = Some(now_vn().to_rfc3339());
    state.scheduler.checkin_running.store(false, Ordering::SeqCst);
    tracing::info!("[HOYO CHECKIN] Multi-game scheduler finished");
}

/// Runs auto-redeem across ALL 3 games: Genshin Impact, Honkai: Star Rail, and Zenless Zone Zero.
pub async fn run_genshin_auto_redeem(state: AppState) {
    if state
        .scheduler
        .redeem_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::info!("[HOYO AUTO-REDEEM] Already running, skipping overlapping trigger");
        return;
    }

    tracing::info!("[HOYO AUTO-REDEEM] Multi-game scheduler started (Genshin, HSR, ZZZ)");
    let accounts = match get_all_auto_redeem_accounts(&state).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!(error = %e, "[HOYO AUTO-REDEEM] Cannot fetch accounts");
            state.scheduler.redeem_running.store(false, Ordering::SeqCst);
            return;
        }
    };

    let mut total_redeemed: usize = 0;
    let mut processed_accounts: usize = 0;
    let mut refreshed_games: HashSet<HoyoGame> = HashSet::new();
    let mut per_game_redeemed: HashMap<&'static str, usize> = HashMap::new();

    for acc in &accounts {
        let cookie = match decrypt_cookie(&acc.cookie_encrypted, &state.config.genshin_cookie_secret) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(game = acc.game.slug(), account_id = acc.id, error = %e, "[HOYO AUTO-REDEEM] Decrypt failed");
                continue;
            }
        };

        let should_refresh = refreshed_games.insert(acc.game);

        match auto_redeem_for_account(
            &state,
            acc.game,
            acc.id,
            &cookie,
            &acc.uid,
            &acc.server,
            &acc.nickname,
            acc.email.as_deref(),
            should_refresh,
            false,
        )
        .await
        {
            Ok(summary) => {
                processed_accounts += 1;
                let count = summary
                    .get("redeemed_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                total_redeemed += count;
                *per_game_redeemed.entry(acc.game.slug()).or_insert(0) += count;
            }
            Err(err) => {
                tracing::warn!(game = acc.game.slug(), account_id = acc.id, uid = %acc.uid, error = %err, "[HOYO AUTO-REDEEM] Account error");
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let now_str = now_vn().to_rfc3339();
    *state.scheduler.last_redeem_run.write().await = Some(now_str.clone());
    *state.scheduler.last_redeem_summary.write().await = Some(json!({
        "ran_at": now_str,
        "accounts_processed": processed_accounts,
        "total_codes_redeemed": total_redeemed,
        "by_game": per_game_redeemed,
    }));
    state.scheduler.redeem_running.store(false, Ordering::SeqCst);
    tracing::info!(
        processed_accounts,
        total_redeemed,
        "[HOYO AUTO-REDEEM] Multi-game scheduler finished"
    );
}

/// Runs daily-note sync across ALL 3 games: Genshin Impact, Honkai: Star Rail, and Zenless Zone Zero.
pub async fn run_genshin_auto_daily_note(state: AppState) {
    if state
        .scheduler
        .daily_note_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let accounts = match get_all_auto_daily_note_accounts(&state).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!(error = %e, "[HOYO DAILY-NOTE] Cannot fetch accounts");
            state
                .scheduler
                .daily_note_running
                .store(false, Ordering::SeqCst);
            return;
        }
    };

    for acc in accounts {
        if let Ok(cookie) = decrypt_cookie(&acc.cookie_encrypted, &state.config.genshin_cookie_secret) {
            if let Ok(res) = get_daily_note(&state, acc.game, &cookie, &acc.uid, &acc.server).await {
                if res.get("ok").and_then(Value::as_bool) == Some(true) {
                    let _ = update_daily_note_timestamp(&state, acc.game, acc.id).await;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    *state.scheduler.last_daily_note_run.write().await = Some(now_vn().to_rfc3339());
    state
        .scheduler
        .daily_note_running
        .store(false, Ordering::SeqCst);
}

/// Runs automatic provider price sync (e.g., Shop AppMMO -> price + 5-10% markup).
pub async fn run_provider_price_sync(state: AppState) -> Value {
    if state
        .scheduler
        .provider_sync_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return json!({ "skipped": true, "message": "Provider price sync is already running" });
    }

    tracing::info!("[PROVIDER SYNC] Starting scheduled product price sync...");
    let summary = match sync_provider_prices(&state).await {
        Ok(val) => {
            tracing::info!(
                updated = val.get("updated").and_then(Value::as_u64).unwrap_or(0),
                unchanged = val.get("unchanged").and_then(Value::as_u64).unwrap_or(0),
                "[PROVIDER SYNC] Finished price sync"
            );
            val
        }
        Err(err) => {
            tracing::error!(error = %err, "[PROVIDER SYNC] Price sync failed");
            json!({ "error": err.to_string(), "updated": 0 })
        }
    };

    let now_str = now_vn().to_rfc3339();
    *state.scheduler.last_provider_sync_run.write().await = Some(now_str);
    *state.scheduler.last_provider_sync_summary.write().await = Some(summary.clone());
    state
        .scheduler
        .provider_sync_running
        .store(false, Ordering::SeqCst);
    summary
}

pub async fn get_scheduler_status(state: &AppState) -> Value {
    json!({
        "games": ["genshin", "hsr", "zzz"],
        "checkin_running": state.scheduler.checkin_running.load(Ordering::SeqCst),
        "redeem_running": state.scheduler.redeem_running.load(Ordering::SeqCst),
        "daily_note_running": state.scheduler.daily_note_running.load(Ordering::SeqCst),
        "provider_sync_running": state.scheduler.provider_sync_running.load(Ordering::SeqCst),
        "provider_sync_interval_hours": state.config.provider_sync_interval_hours,
        "last_checkin_run": *state.scheduler.last_checkin_run.read().await,
        "last_redeem_run": *state.scheduler.last_redeem_run.read().await,
        "last_redeem_summary": *state.scheduler.last_redeem_summary.read().await,
        "last_daily_note_run": *state.scheduler.last_daily_note_run.read().await,
        "last_provider_sync_run": *state.scheduler.last_provider_sync_run.read().await,
        "last_provider_sync_summary": *state.scheduler.last_provider_sync_summary.read().await,
        "server_time_vn": now_vn().to_rfc3339(),
    })
}

pub fn spawn_scheduler(state: AppState) {
    tokio::spawn(async move {
        tracing::info!("[SCHEDULER] Booting multi-game HoYo cron loop (Genshin, HSR, ZZZ | Asia/Ho_Chi_Minh)...");
        let mut last_checkin_slot: Option<(NaiveDate, u32)> = None;
        let mut last_redeem_slot: Option<(NaiveDate, u32)> = None;
        let mut last_note_slot: Option<(NaiveDate, u32, u32)> = None;
        let mut last_provider_slot: Option<(NaiveDate, u32)> = None;

        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let now = now_vn();
            let day = now.date_naive();
            let hour = now.hour();
            let minute = now.minute();

            // 1. Auto Check-in at 00:05 and backup at 06:00 VN
            if (hour == 0 && minute >= 5) || (hour == 6 && minute == 0) {
                let slot = (day, hour);
                if last_checkin_slot != Some(slot) {
                    last_checkin_slot = Some(slot);
                    let st = state.clone();
                    tokio::spawn(async move { run_genshin_auto_checkin(st).await });
                }
            }

            // 2. Auto Redeem Code every 3 hours + 12:00 & 20:00 VN
            let is_redeem_window = (matches!(hour, 12 | 20) && minute == 0)
                || (hour % 3 == 0 && minute == 15);
            if is_redeem_window {
                let slot = (day, hour);
                if last_redeem_slot != Some(slot) {
                    last_redeem_slot = Some(slot);
                    let st = state.clone();
                    tokio::spawn(async move { run_genshin_auto_redeem(st).await });
                }
            }

            // 3. Auto Daily Note every 30 mins (:00, :30)
            if minute == 0 || minute == 30 {
                let slot = (day, hour, minute);
                if last_note_slot != Some(slot) {
                    last_note_slot = Some(slot);
                    let st = state.clone();
                    tokio::spawn(async move { run_genshin_auto_daily_note(st).await });
                }
            }

            // 4. Provider Price Sync every 12h (default 00:05 and 12:05 VN)
            let sync_every = (state.config.provider_sync_interval_hours as u32).clamp(1, 24);
            if !state.config.provider_appmmo_api_key.trim().is_empty()
                && hour % sync_every == 0
                && minute == 5
            {
                let slot = (day, hour);
                if last_provider_slot != Some(slot) {
                    last_provider_slot = Some(slot);
                    let st = state.clone();
                    tokio::spawn(async move {
                        let _ = run_provider_price_sync(st).await;
                    });
                }
            }
        }
    });
}
