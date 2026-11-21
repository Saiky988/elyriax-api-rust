use serde_json::Value;

use crate::{
    error::AppResult,
    services::hoyo::{redeem as hoyo_redeem, HoyoGame},
    state::AppState,
};

pub async fn redeem_code(
    state: &AppState,
    cookie: &str,
    server: &str,
    code: &str,
    uid_hint: Option<&str>,
) -> AppResult<Value> {
    hoyo_redeem::redeem_code(state, HoyoGame::Genshin, cookie, server, code, uid_hint).await
}

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
    hoyo_redeem::auto_redeem_for_account(
        state,
        HoyoGame::Genshin,
        account_id,
        cookie,
        uid,
        server,
        nickname,
        notify_email,
        force_refresh_codes,
    )
    .await
}

pub async fn get_account_redeem_history(state: &AppState, account_id: i64) -> Value {
    hoyo_redeem::get_account_redeem_history(state, HoyoGame::Genshin, account_id).await
}
