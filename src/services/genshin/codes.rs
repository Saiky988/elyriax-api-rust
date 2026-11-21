use serde_json::Value;

use crate::{
    error::AppResult,
    services::hoyo::{
        feed::{self, HoyoCodeCard},
        HoyoGame,
    },
    state::AppState,
};

pub async fn get_all_codes(state: &AppState, force_refresh: bool) -> AppResult<Vec<HoyoCodeCard>> {
    feed::get_game_codes(state, HoyoGame::Genshin, force_refresh).await
}

pub async fn get_code_info(state: &AppState, target_code: &str) -> Value {
    feed::get_code_info(state, HoyoGame::Genshin, target_code).await
}
