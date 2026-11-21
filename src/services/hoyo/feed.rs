use std::collections::{HashMap, HashSet};
use std::time::Duration;

use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    services::hoyo::HoyoGame,
    state::{AppState, CachedValue},
};

const SERIA_BASE: &str = "https://hoyo-codes.seria.moe/codes";
const ENNEAD_BASE: &str = "https://api.ennead.cc/mihoyo";
const FANDOM_GENSHIN_WIKI: &str =
    "https://genshin-impact.fandom.com/api.php?action=parse&page=Promotional_Code&prop=text&format=json";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

const CODES_TTL: Duration = Duration::from_secs(300); // 5 mins (matches hoyoverse-api)
const CALENDAR_TTL: Duration = Duration::from_secs(300); // 5 mins
const NEWS_TTL: Duration = Duration::from_secs(900); // 15 mins

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoyoCodeCard {
    pub game: String,
    pub codes: Vec<String>,
    pub server: String,
    pub rewards: Vec<String>,
    pub validity: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveCodeEntry {
    pub game: String,
    pub code: String,
    pub rewards: Vec<String>,
    pub validity: Option<String>,
    pub source: String,
}

fn normalize_code(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    if cleaned.len() >= 6 && cleaned.len() <= 32 {
        Some(cleaned)
    } else {
        None
    }
}

pub fn normalize_lang(raw: Option<&str>) -> &'static str {
    match raw.unwrap_or("vi-vn").trim().to_lowercase().as_str() {
        "vi" | "vi-vn" => "vi-vn",
        "en" | "en-us" => "en-us",
        "zh" | "zh-cn" => "zh-cn",
        "zh-tw" => "zh-tw",
        "ja" | "ja-jp" => "ja-jp",
        "ko" | "ko-kr" => "ko-kr",
        "de" | "de-de" => "de-de",
        "es" | "es-es" => "es-es",
        "fr" | "fr-fr" => "fr-fr",
        "id" | "id-id" => "id-id",
        "it" | "it-it" => "it-it",
        "pt" | "pt-pt" => "pt-pt",
        "ru" | "ru-ru" => "ru-ru",
        "th" | "th-th" => "th-th",
        "tr" | "tr-tr" => "tr-tr",
        _ => "vi-vn",
    }
}

async fn fetch_seria_codes(state: &AppState, game: HoyoGame) -> Vec<HoyoCodeCard> {
    let url = format!("{}?game={}", SERIA_BASE, game.seria_slug());
    let Ok(res) = state
        .http
        .get(&url)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    else {
        return vec![];
    };

    let Ok(data) = res.json::<Value>().await else {
        return vec![];
    };

    let Some(arr) = data.get("codes").and_then(Value::as_array) else {
        return vec![];
    };

    arr.iter()
        .filter_map(|item| {
            let status = item.get("status").and_then(Value::as_str).unwrap_or("OK");
            if !status.eq_ignore_ascii_case("OK") {
                return None;
            }
            let code = normalize_code(item.get("code")?.as_str()?)?;
            let rewards = item
                .get("rewards")
                .and_then(Value::as_str)
                .map(|r| {
                    r.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            Some(HoyoCodeCard {
                game: game.slug().to_string(),
                codes: vec![code],
                server: "All".to_string(),
                rewards,
                validity: Some("Active".to_string()),
                source: "hoyo-codes.seria.moe".to_string(),
            })
        })
        .collect()
}

async fn fetch_ennead_codes(state: &AppState, game: HoyoGame) -> Vec<HoyoCodeCard> {
    let url = format!("{}/{}/codes", ENNEAD_BASE, game.ennead_slug());
    let Ok(res) = state
        .http
        .get(&url)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    else {
        return vec![];
    };

    let Ok(data) = res.json::<Value>().await else {
        return vec![];
    };

    let Some(arr) = data.get("active").and_then(Value::as_array) else {
        return vec![];
    };

    arr.iter()
        .filter_map(|item| {
            let code = normalize_code(item.get("code")?.as_str()?)?;
            let rewards = item
                .get("rewards")
                .and_then(Value::as_array)
                .map(|r_arr| {
                    r_arr
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            Some(HoyoCodeCard {
                game: game.slug().to_string(),
                codes: vec![code],
                server: "All".to_string(),
                rewards,
                validity: Some("Active".to_string()),
                source: "api.ennead.cc".to_string(),
            })
        })
        .collect()
}

async fn fetch_fandom_genshin_wiki(state: &AppState) -> Vec<HoyoCodeCard> {
    let Ok(res) = state
        .http
        .get(FANDOM_GENSHIN_WIKI)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(12))
        .send()
        .await
    else {
        return vec![];
    };

    let Ok(data) = res.json::<Value>().await else {
        return vec![];
    };

    let Some(html) = data.pointer("/parse/text/*").and_then(Value::as_str) else {
        return vec![];
    };

    let doc = Html::parse_document(html);
    let tr_sel = Selector::parse("table.wikitable tbody tr").expect("valid selector");
    let td_sel = Selector::parse("td").expect("valid selector");
    let code_sel = Selector::parse("code").expect("valid selector");
    let item_sel = Selector::parse(".item-text").expect("valid selector");
    let ws_re = Regex::new(r"\s+").expect("valid regex");

    let mut cards = Vec::new();
    for row in doc.select(&tr_sel) {
        let tds: Vec<_> = row.select(&td_sel).collect();
        if tds.len() < 3 {
            continue;
        }

        let validity_raw = if tds.len() >= 4 {
            let text: String = tds[3].text().collect();
            Some(ws_re.replace_all(&text, " ").trim().replace("Ã—", "×"))
        } else {
            None
        };

        if let Some(ref val) = validity_raw {
            let lower = val.to_lowercase();
            if lower.contains("expired") && !lower.contains("valid") {
                continue;
            }
        }

        let codes: Vec<String> = tds[0]
            .select(&code_sel)
            .filter_map(|c| {
                let raw: String = c.text().collect();
                normalize_code(&raw)
            })
            .collect();

        if codes.is_empty() {
            continue;
        }

        let server_text: String = tds[1].text().collect();
        let server = ws_re.replace_all(&server_text, " ").trim().to_string();

        let rewards: Vec<String> = tds[2]
            .select(&item_sel)
            .filter_map(|el| {
                let t: String = el.text().collect();
                let cleaned = ws_re.replace_all(&t, " ").trim().replace("Ã—", "×");
                if cleaned.is_empty() {
                    None
                } else {
                    Some(cleaned)
                }
            })
            .collect();

        cards.push(HoyoCodeCard {
            game: "genshin".to_string(),
            codes,
            server,
            rewards,
            validity: validity_raw,
            source: "genshin-impact.fandom.com".to_string(),
        });
    }

    cards
}

/// Fetch active gift codes for any HoYo game (Genshin, HSR, ZZZ) by merging `seriaati/hoyo-codes`
/// and `torikushiii/hoyoverse-api` concurrently with 5-minute TTL caching.
pub async fn get_game_codes(
    state: &AppState,
    game: HoyoGame,
    force_refresh: bool,
) -> AppResult<Vec<HoyoCodeCard>> {
    let cache_key = game.slug().to_string();
    if !force_refresh {
        if let Some(entry) = state.hoyo_codes_cache.get(&cache_key) {
            if entry.is_valid() {
                if let Ok(cards) = serde_json::from_value::<Vec<HoyoCodeCard>>(entry.value.clone()) {
                    return Ok(cards);
                }
            }
        }
    }

    let (seria_cards, ennead_cards) = tokio::join!(
        fetch_seria_codes(state, game),
        fetch_ennead_codes(state, game),
    );

    let mut combined = Vec::new();
    combined.extend(seria_cards);
    combined.extend(ennead_cards);

    if combined.is_empty() && game == HoyoGame::Genshin {
        combined.extend(fetch_fandom_genshin_wiki(state).await);
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut deduped: Vec<HoyoCodeCard> = Vec::new();
    for mut card in combined {
        card.codes.retain(|c| seen.insert(c.clone()));
        if !card.codes.is_empty() {
            deduped.push(card);
        }
    }

    if let Ok(val) = serde_json::to_value(&deduped) {
        state
            .hoyo_codes_cache
            .insert(cache_key, CachedValue::new(val, CODES_TTL));
    }

    Ok(deduped)
}

pub async fn get_active_code_entries(
    state: &AppState,
    game: HoyoGame,
    force_refresh: bool,
) -> AppResult<Vec<ActiveCodeEntry>> {
    let cards = get_game_codes(state, game, force_refresh).await?;
    let mut entries = Vec::new();
    for card in cards {
        for code in card.codes {
            entries.push(ActiveCodeEntry {
                game: card.game.clone(),
                code,
                rewards: card.rewards.clone(),
                validity: card.validity.clone(),
                source: card.source.clone(),
            });
        }
    }
    Ok(entries)
}

pub async fn get_code_info(state: &AppState, game: HoyoGame, target_code: &str) -> Value {
    let upper = target_code.trim().to_uppercase();
    if let Ok(entries) = get_active_code_entries(state, game, false).await {
        let map: HashMap<String, ActiveCodeEntry> =
            entries.into_iter().map(|e| (e.code.clone(), e)).collect();
        if let Some(entry) = map.get(&upper) {
            return json!({
                "rewards": entry.rewards,
                "validity": entry.validity,
                "source": entry.source,
            });
        }
    }
    json!({ "rewards": [], "validity": null })
}

/// Fetch active codes for ALL 3 games (Genshin, HSR, ZZZ) concurrently.
pub async fn get_all_games_codes(state: &AppState, force_refresh: bool) -> AppResult<Value> {
    let (gi, hsr, zzz) = tokio::try_join!(
        get_game_codes(state, HoyoGame::Genshin, force_refresh),
        get_game_codes(state, HoyoGame::Hsr, force_refresh),
        get_game_codes(state, HoyoGame::Zzz, force_refresh),
    )?;

    let gi_count: usize = gi.iter().map(|c| c.codes.len()).sum();
    let hsr_count: usize = hsr.iter().map(|c| c.codes.len()).sum();
    let zzz_count: usize = zzz.iter().map(|c| c.codes.len()).sum();

    Ok(json!({
        "total_codes": gi_count + hsr_count + zzz_count,
        "genshin": { "count": gi_count, "cards": gi },
        "hsr": { "count": hsr_count, "cards": hsr },
        "zzz": { "count": zzz_count, "cards": zzz },
    }))
}

/// Fetch live event calendar (events, banners, endgame challenges) from `torikushiii/hoyoverse-api`.
pub async fn get_game_calendar(
    state: &AppState,
    game: HoyoGame,
    lang: Option<&str>,
    force_refresh: bool,
) -> AppResult<Value> {
    let locale = normalize_lang(lang);
    let cache_key = format!("{}:{}", game.slug(), locale);

    if !force_refresh {
        if let Some(entry) = state.hoyo_calendar_cache.get(&cache_key) {
            if entry.is_valid() {
                return Ok(entry.value.clone());
            }
        }
    }

    let url = format!(
        "{}/{}/calendar?lang={}",
        ENNEAD_BASE,
        game.ennead_slug(),
        locale
    );

    let res = state
        .http
        .get(&url)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(12))
        .send()
        .await?;

    if !res.status().is_success() {
        return Err(AppError::internal(format!(
            "Upstream calendar API returned HTTP {}",
            res.status().as_u16()
        )));
    }

    let data: Value = res.json().await?;
    state
        .hoyo_calendar_cache
        .insert(cache_key, CachedValue::new(data.clone(), CALENDAR_TTL));

    Ok(data)
}

/// Fetch HoYoLAB community news (`events`, `notices`, `info`) from `torikushiii/hoyoverse-api`.
pub async fn get_game_news(
    state: &AppState,
    game: HoyoGame,
    news_type: &str,
    lang: Option<&str>,
    force_refresh: bool,
) -> AppResult<Value> {
    let kind = match news_type.trim().to_lowercase().as_str() {
        "events" | "event" => "events",
        "notices" | "notice" => "notices",
        "info" => "info",
        other => {
            return Err(AppError::bad_request(format!(
                "Loại tin tức không hợp lệ: '{}'. Chỉ hỗ trợ: events, notices, info",
                other
            )));
        }
    };

    let locale = normalize_lang(lang);
    let cache_key = format!("{}:{}:{}", game.slug(), kind, locale);

    if !force_refresh {
        if let Some(entry) = state.hoyo_news_cache.get(&cache_key) {
            if entry.is_valid() {
                return Ok(entry.value.clone());
            }
        }
    }

    let url = format!(
        "{}/{}/news/{}?lang={}",
        ENNEAD_BASE,
        game.ennead_slug(),
        kind,
        locale
    );

    let res = state
        .http
        .get(&url)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(12))
        .send()
        .await?;

    if !res.status().is_success() {
        return Err(AppError::internal(format!(
            "Upstream news API returned HTTP {}",
            res.status().as_u16()
        )));
    }

    let data: Value = res.json().await?;
    state
        .hoyo_news_cache
        .insert(cache_key, CachedValue::new(data.clone(), NEWS_TTL));

    Ok(data)
}
