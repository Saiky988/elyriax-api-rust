pub mod account;
pub mod checkin;
pub mod feed;
pub mod game_record;
pub mod redeem;

use chrono::Utc;
use md5::{Digest, Md5};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// HoYoLAB OS Dynamic Secret salt (v1 OS app_version 1.5.0, client_type 5)
const HOYOLAB_OS_SALT: &str = "6s25p5ox5y14umn1p61aqyyvbvvl3lrt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HoyoGame {
    Genshin,
    Hsr,
    Zzz,
}

impl HoyoGame {
    pub const ALL: [HoyoGame; 3] = [HoyoGame::Genshin, HoyoGame::Hsr, HoyoGame::Zzz];

    pub fn from_slug(slug: &str) -> AppResult<Self> {
        match slug.trim().to_lowercase().as_str() {
            "genshin" | "gi" | "hk4e" | "genshin-impact" => Ok(Self::Genshin),
            "hsr" | "starrail" | "hkrpg" | "honkai-star-rail" => Ok(Self::Hsr),
            "zzz" | "zenless" | "nap" | "zenless-zone-zero" => Ok(Self::Zzz),
            other => Err(AppError::bad_request(format!(
                "Game không hợp lệ: '{}'. Hỗ trợ: genshin, hsr (starrail), zzz (zenless)",
                other
            ))),
        }
    }

    pub fn slug(&self) -> &'static str {
        match self {
            Self::Genshin => "genshin",
            Self::Hsr => "hsr",
            Self::Zzz => "zzz",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Genshin => "Genshin Impact",
            Self::Hsr => "Honkai: Star Rail",
            Self::Zzz => "Zenless Zone Zero",
        }
    }

    /// Game query slug for https://hoyo-codes.seria.moe/codes?game=...
    pub fn seria_slug(&self) -> &'static str {
        match self {
            Self::Genshin => "genshin",
            Self::Hsr => "hkrpg",
            Self::Zzz => "nap",
        }
    }

    /// Game path slug for https://api.ennead.cc/mihoyo/{slug}/...
    pub fn ennead_slug(&self) -> &'static str {
        match self {
            Self::Genshin => "genshin",
            Self::Hsr => "starrail",
            Self::Zzz => "zenless",
        }
    }

    pub fn game_biz(&self) -> &'static str {
        match self {
            Self::Genshin => "hk4e_global",
            Self::Hsr => "hkrpg_global",
            Self::Zzz => "nap_global",
        }
    }

    pub fn act_id(&self) -> &'static str {
        match self {
            Self::Genshin => "e202102251931481",
            Self::Hsr => "e202303301540311",
            Self::Zzz => "e202406031448091",
        }
    }

    pub fn sign_game_header(&self) -> &'static str {
        match self {
            Self::Genshin => "hk4e",
            Self::Hsr => "hkrpg",
            Self::Zzz => "zzz",
        }
    }

    pub fn checkin_base_url(&self) -> &'static str {
        match self {
            Self::Genshin => "https://sg-hk4e-api.hoyolab.com/event/sol",
            Self::Hsr => "https://sg-public-api.hoyolab.com/event/luna/os",
            Self::Zzz => "https://sg-act-nap-api.hoyolab.com/event/luna/zzz/os",
        }
    }

    pub fn redeem_url(&self) -> &'static str {
        match self {
            Self::Genshin => {
                "https://public-operation-hk4e.hoyoverse.com/common/apicdkey/api/webExchangeCdkey"
            }
            Self::Hsr => {
                "https://public-operation-hkrpg.hoyoverse.com/common/apicdkey/api/webExchangeCdkeyHoyo"
            }
            Self::Zzz => {
                "https://public-operation-nap.hoyoverse.com/common/apicdkey/api/webExchangeCdkeyHoyo"
            }
        }
    }

    pub fn daily_note_url(&self) -> &'static str {
        match self {
            Self::Genshin => {
                "https://sg-act-public-api.hoyolab.com/event/game_record/app/genshin/api/dailyNote"
            }
            Self::Hsr => "https://bbs-api-os.hoyolab.com/game_record/hkrpg/api/note",
            Self::Zzz => "https://sg-act-nap-api.hoyolab.com/event/game_record_zzz/api/zzz/note",
        }
    }

    pub fn stats_url(&self) -> &'static str {
        match self {
            Self::Genshin => {
                "https://sg-act-public-api.hoyolab.com/event/game_record/genshin/api/index"
            }
            Self::Hsr => "https://bbs-api-os.hoyolab.com/game_record/hkrpg/api/index",
            Self::Zzz => "https://sg-act-nap-api.hoyolab.com/event/game_record_zzz/api/zzz/index",
        }
    }

    pub fn normalize_region(&self, server: &str) -> Option<&'static str> {
        let s = server.trim().to_lowercase();
        match self {
            Self::Genshin => match s.as_str() {
                "asia" | "os_asia" => Some("os_asia"),
                "america" | "usa" | "na" | "os_usa" => Some("os_usa"),
                "europe" | "eu" | "os_euro" => Some("os_euro"),
                "tw" | "hk" | "mo" | "cht" | "sar" | "os_cht" => Some("os_cht"),
                _ => None,
            },
            Self::Hsr => match s.as_str() {
                "asia" | "os_asia" | "prod_official_asia" => Some("prod_official_asia"),
                "america" | "usa" | "na" | "os_usa" | "prod_official_usa" => {
                    Some("prod_official_usa")
                }
                "europe" | "eu" | "os_euro" | "prod_official_euro" => Some("prod_official_euro"),
                "tw" | "hk" | "mo" | "cht" | "sar" | "os_cht" | "prod_official_cht" => {
                    Some("prod_official_cht")
                }
                _ => None,
            },
            Self::Zzz => match s.as_str() {
                "asia" | "jp" | "os_asia" | "prod_gf_jp" => Some("prod_gf_jp"),
                "america" | "usa" | "na" | "us" | "os_usa" | "prod_gf_us" => Some("prod_gf_us"),
                "europe" | "eu" | "os_euro" | "prod_gf_eu" => Some("prod_gf_eu"),
                "tw" | "hk" | "mo" | "sg" | "cht" | "sar" | "os_cht" | "prod_gf_sg" => {
                    Some("prod_gf_sg")
                }
                _ => None,
            },
        }
    }

    pub fn server_display_name(region: &str) -> &'static str {
        match region {
            "os_asia" | "prod_official_asia" | "prod_gf_jp" => "Asia",
            "os_usa" | "prod_official_usa" | "prod_gf_us" => "NA",
            "os_euro" | "prod_official_euro" | "prod_gf_eu" => "EU",
            "os_cht" | "prod_official_cht" | "prod_gf_sg" => "TW/HK/MO",
            _ => "Unknown",
        }
    }
}

/// Generate HoYoLAB OS Dynamic Secret (DS) header value: `{timestamp},{random6},{md5}`
pub fn generate_os_ds() -> String {
    let ts = Utc::now().timestamp();
    let rand_str: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(6)
        .map(char::from)
        .collect();
    let raw = format!("salt={}&t={}&r={}", HOYOLAB_OS_SALT, ts, rand_str);
    let mut hasher = Md5::new();
    hasher.update(raw.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("{},{},{}", ts, rand_str, digest)
}
