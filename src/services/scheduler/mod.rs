use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{FixedOffset, NaiveDate, NaiveDateTime, Timelike, Utc};
use serde_json::{json, Value};

use crate::{
    core::security::decrypt_cookie,
    services::{
        email::{render_template, send_notification_email},
        genshin::{
            account::{
                get_auto_checkin_accounts, get_auto_daily_note_accounts, get_auto_redeem_accounts,
                update_checkin_status, update_daily_note_timestamp,
            },
            checkin::check_in_daily,
            daily_note::get_daily_note,
            redeem::auto_redeem_for_account,
        },
    },
    state::AppState,
};

fn now_vn() -> chrono::DateTime<FixedOffset> {
    let tz_vn = FixedOffset::east_opt(7 * 3600).expect("valid UTC+7 offset");
    Utc::now().with_timezone(&tz_vn)
}

fn already_processed_today(last_checkin_at: Option<NaiveDateTime>) -> bool {
    let Some(dt) = last_checkin_at else {
        return false;
    };
    let today_vn = now_vn().date_naive();
    dt.date() == today_vn
}

pub async fn run_genshin_auto_checkin(state: AppState) {
    if state
        .scheduler
        .checkin_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::info!("[GENSHIN CHECKIN] Already running, skipping overlapping trigger");
        return;
    }

    tracing::info!("[GENSHIN CHECKIN] Scheduler started");
    let accounts = match get_auto_checkin_accounts(&state).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!(error = %e, "[GENSHIN CHECKIN] Cannot fetch accounts");
            state.scheduler.checkin_running.store(false, Ordering::SeqCst);
            return;
        }
    };

    tracing::info!(count = accounts.len(), "[GENSHIN CHECKIN] Accounts queued");

    for acc in accounts {
        if already_processed_today(acc.last_checkin_at) {
            continue;
        }

        let cookie = match decrypt_cookie(&acc.cookie_encrypted, &state.config.genshin_cookie_secret) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(account_id = acc.id, error = %e, "[GENSHIN CHECKIN] Decrypt failed");
                let _ = update_checkin_status(&state, acc.id, "cookie_expired").await;
                continue;
            }
        };

        match check_in_daily(&state, &cookie, &acc.server).await {
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

                let _ = update_checkin_status(&state, acc.id, status_str).await;

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
                                    "[Genshin] Điểm danh hàng ngày thành công — {}",
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
                tracing::error!(account_id = acc.id, uid = %acc.uid, status = status_str, error = %err, "[GENSHIN CHECKIN] Failed");
                let _ = update_checkin_status(&state, acc.id, status_str).await;
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    *state.scheduler.last_checkin_run.write().await = Some(now_vn().to_rfc3339());
    state.scheduler.checkin_running.store(false, Ordering::SeqCst);
    tracing::info!("[GENSHIN CHECKIN] Scheduler finished");
}

pub async fn run_genshin_auto_redeem(state: AppState) {
    if state
        .scheduler
        .redeem_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::info!("[GENSHIN AUTO-REDEEM] Already running, skipping overlapping trigger");
        return;
    }

    tracing::info!("[GENSHIN AUTO-REDEEM] Scheduler started");
    let accounts = match get_auto_redeem_accounts(&state).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!(error = %e, "[GENSHIN AUTO-REDEEM] Cannot fetch accounts");
            state.scheduler.redeem_running.store(false, Ordering::SeqCst);
            return;
        }
    };

    let mut total_redeemed: usize = 0;
    let mut processed_accounts: usize = 0;

    for (idx, acc) in accounts.iter().enumerate() {
        let cookie = match decrypt_cookie(&acc.cookie_encrypted, &state.config.genshin_cookie_secret) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(account_id = acc.id, error = %e, "[GENSHIN AUTO-REDEEM] Decrypt failed");
                continue;
            }
        };

        // Only force-refresh code feeds on the first account in the batch; subsequent accounts use the 10m TTL cache
        match auto_redeem_for_account(
            &state,
            acc.id,
            &cookie,
            &acc.uid,
            &acc.server,
            &acc.nickname,
            acc.email.as_deref(),
            idx == 0,
        )
        .await
        {
            Ok(summary) => {
                processed_accounts += 1;
                total_redeemed += summary
                    .get("redeemed_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
            }
            Err(err) => {
                tracing::warn!(account_id = acc.id, uid = %acc.uid, error = %err, "[GENSHIN AUTO-REDEEM] Account error");
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
    }));
    state.scheduler.redeem_running.store(false, Ordering::SeqCst);
    tracing::info!(
        processed_accounts,
        total_redeemed,
        "[GENSHIN AUTO-REDEEM] Scheduler finished"
    );
}

pub async fn run_genshin_auto_daily_note(state: AppState) {
    if state
        .scheduler
        .daily_note_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let accounts = match get_auto_daily_note_accounts(&state).await {
        Ok(list) => list,
        Err(e) => {
            tracing::error!(error = %e, "[GENSHIN DAILY-NOTE] Cannot fetch accounts");
            state
                .scheduler
                .daily_note_running
                .store(false, Ordering::SeqCst);
            return;
        }
    };

    for acc in accounts {
        if let Ok(cookie) = decrypt_cookie(&acc.cookie_encrypted, &state.config.genshin_cookie_secret) {
            if let Ok(res) = get_daily_note(&state, &cookie, &acc.uid, &acc.server).await {
                if res.get("ok").and_then(Value::as_bool) == Some(true) {
                    let _ = update_daily_note_timestamp(&state, acc.id).await;
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

pub async fn get_scheduler_status(state: &AppState) -> Value {
    json!({
        "checkin_running": state.scheduler.checkin_running.load(Ordering::SeqCst),
        "redeem_running": state.scheduler.redeem_running.load(Ordering::SeqCst),
        "daily_note_running": state.scheduler.daily_note_running.load(Ordering::SeqCst),
        "last_checkin_run": *state.scheduler.last_checkin_run.read().await,
        "last_redeem_run": *state.scheduler.last_redeem_run.read().await,
        "last_redeem_summary": *state.scheduler.last_redeem_summary.read().await,
        "last_daily_note_run": *state.scheduler.last_daily_note_run.read().await,
        "server_time_vn": now_vn().to_rfc3339(),
    })
}

pub fn spawn_scheduler(state: AppState) {
    tokio::spawn(async move {
        tracing::info!("[SCHEDULER] Booting Genshin cron loop (Asia/Ho_Chi_Minh)...");
        let mut last_checkin_slot: Option<(NaiveDate, u32)> = None;
        let mut last_redeem_slot: Option<(NaiveDate, u32)> = None;
        let mut last_note_slot: Option<(NaiveDate, u32, u32)> = None;

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

            // 2. Auto Redeem Code every 3 hours (00:15, 03:15, 06:15, 09:15, 12:00, 15:15, 18:15, 20:00, 21:15)
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

        }
    });
}
