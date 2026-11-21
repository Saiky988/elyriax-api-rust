use std::collections::{HashMap, HashSet};
use std::time::Duration;

use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    error::AppResult,
    state::{AppState, CachedValue},
};

const SERIA_CODES_URL: &str = "https://hoyo-codes.seria.moe/codes?game=genshin";
const ENNEAD_CODES_URL: &str = "https://api.ennead.cc/mihoyo/genshin/codes";
const FANDOM_WIKI_URL: &str =
    "https://genshin-impact.fandom.com/api.php?action=parse&page=Promotional_Code&prop=text&format=json";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

const CODES_CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenshinCodeCard {
    pub codes: Vec<String>,
    pub server: String,
    pub rewards: Vec<String>,
    pub validity: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveCodeEntry {
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

async fn fetch_seria_codes(state: &AppState) -> Vec<GenshinCodeCard> {
    let Ok(res) = state
        .http
        .get(SERIA_CODES_URL)
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

            Some(GenshinCodeCard {
                codes: vec![code],
                server: "All".to_string(),
                rewards,
                validity: Some("Active".to_string()),
                source: "hoyo-codes.seria.moe".to_string(),
            })
        })
        .collect()
}

async fn fetch_ennead_codes(state: &AppState) -> Vec<GenshinCodeCard> {
    let Ok(res) = state
        .http
        .get(ENNEAD_CODES_URL)
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

            Some(GenshinCodeCard {
                codes: vec![code],
                server: "All".to_string(),
                rewards,
                validity: Some("Active".to_string()),
                source: "api.ennead.cc".to_string(),
            })
        })
        .collect()
}

async fn fetch_fandom_wiki_codes(state: &AppState) -> Vec<GenshinCodeCard> {
    let Ok(res) = state
        .http
        .get(FANDOM_WIKI_URL)
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

        // Skip rows explicitly marked expired on the wiki
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

        cards.push(GenshinCodeCard {
            codes,
            server,
            rewards,
            validity: validity_raw,
            source: "genshin-impact.fandom.com".to_string(),
        });
    }

    cards
}

/// Fetch active Genshin gift codes from multiple sources concurrently, deduplicated and cached for 10 minutes.
pub async fn get_all_codes(state: &AppState, force_refresh: bool) -> AppResult<Vec<GenshinCodeCard>> {
    if !force_refresh {
        let guard = state.genshin_codes_cache.read().await;
        if let Some(cached) = guard.as_ref() {
            if cached.is_valid() {
                if let Ok(cards) = serde_json::from_value::<Vec<GenshinCodeCard>>(cached.value.clone()) {
                    return Ok(cards);
                }
            }
        }
    }

    let (seria_cards, ennead_cards) = tokio::join!(
        fetch_seria_codes(state),
        fetch_ennead_codes(state),
    );

    let mut combined = Vec::new();
    combined.extend(seria_cards);
    combined.extend(ennead_cards);

    // Fallback to Fandom Wiki if fast JSON feeds returned nothing
    if combined.is_empty() {
        let wiki_cards = fetch_fandom_wiki_codes(state).await;
        combined.extend(wiki_cards);
    }

    // Deduplicate codes while preserving first-seen metadata (richer rewards preferred)
    let mut seen: HashSet<String> = HashSet::new();
    let mut deduped: Vec<GenshinCodeCard> = Vec::new();

    for mut card in combined {
        card.codes.retain(|c| seen.insert(c.clone()));
        if !card.codes.is_empty() {
            deduped.push(card);
        }
    }

    if let Ok(val) = serde_json::to_value(&deduped) {
        let mut guard = state.genshin_codes_cache.write().await;
        *guard = Some(CachedValue::new(val, CODES_CACHE_TTL));
    }

    Ok(deduped)
}

/// Flatten active codes into a map for fast lookup during auto-redeem.
pub async fn get_active_code_entries(
    state: &AppState,
    force_refresh: bool,
) -> AppResult<Vec<ActiveCodeEntry>> {
    let cards = get_all_codes(state, force_refresh).await?;
    let mut entries = Vec::new();
    for card in cards {
        for code in card.codes {
            entries.push(ActiveCodeEntry {
                code,
                rewards: card.rewards.clone(),
                validity: card.validity.clone(),
                source: card.source.clone(),
            });
        }
    }
    Ok(entries)
}

pub async fn get_code_info(state: &AppState, target_code: &str) -> Value {
    let target_upper = target_code.trim().to_uppercase();
    if let Ok(entries) = get_active_code_entries(state, false).await {
        let map: HashMap<String, ActiveCodeEntry> = entries
            .into_iter()
            .map(|e| (e.code.clone(), e))
            .collect();
        if let Some(entry) = map.get(&target_upper) {
            return json!({
                "rewards": entry.rewards,
                "validity": entry.validity,
                "source": entry.source,
            });
        }
    }
    json!({ "rewards": [], "validity": null })
}
