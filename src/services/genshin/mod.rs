pub mod account;
pub mod banner;
pub mod checkin;
pub mod checkin_list;
pub mod codes;
pub mod daily_note;
pub mod redeem;
pub mod stats;

pub fn normalize_region(server: &str) -> Option<&'static str> {
    match server.trim().to_lowercase().as_str() {
        "asia" | "os_asia" => Some("os_asia"),
        "america" | "usa" | "na" | "os_usa" => Some("os_usa"),
        "europe" | "eu" | "os_euro" => Some("os_euro"),
        "tw" | "hk" | "cht" | "sar" | "os_cht" => Some("os_cht"),
        _ => None,
    }
}

pub fn server_display_name(region: &str) -> &'static str {
    match region {
        "os_asia" => "Asia",
        "os_usa" => "NA",
        "os_euro" => "EU",
        "os_cht" => "TW/HK",
        _ => "Unknown",
    }
}
