use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub environment: String,
    pub base_url: String,
    pub frontend_url: String,
    pub go_base_url: String,
    pub timezone: String,
    pub base_dir: PathBuf,

    // Database
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: String,
    pub db_name: String,
    pub db_connection_limit: u32,

    // Security
    pub jwt_secret: String,
    pub genshin_cookie_secret: String,

    // OAuth (Login & Account Linking)
    pub discord_client_id: String,
    pub discord_client_secret: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub github_client_id: String,
    pub github_client_secret: String,

    // Payment (SePay & VietQR)
    pub sepay_webhook_token: String,
    pub bank_id: String,
    pub bank_account_no: String,
    pub bank_account_name: String,

    // Admin (Email-based via `ADMIN=saikomusic09@gmail.com` + optional legacy fallbacks)
    pub admin_emails: Vec<String>,
    pub admin_username: String,
    pub admin_password: String,
    pub admin_token: String,

    // Email (Resend)
    pub resend_api_key: String,
    pub email_from: String,

    // Product Providers (AppMMO auto price-sync)
    pub provider_appmmo_url: String,
    pub provider_appmmo_api_key: String,
    pub provider_sync_interval_hours: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();

        let base_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let admin_emails: Vec<String> = env::var("ADMIN")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let admin_username = env::var("ADMIN_USERNAME")
            .or_else(|_| env::var("ADMIN_USER"))
            .unwrap_or_default();

        let admin_password = env::var("ADMIN_PASSWORD")
            .or_else(|_| env::var("ADMIN_PASS"))
            .unwrap_or_default();

        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(25243),
            environment: env::var("ENVIRONMENT").unwrap_or_else(|_| "production".to_string()),
            base_url: env::var("BASE_URL").unwrap_or_else(|_| "https://apis.elyriax.com".to_string()),
            frontend_url: env::var("FRONTEND_URL").unwrap_or_else(|_| "https://elyriax.com".to_string()),
            go_base_url: env::var("GO_BASE_URL").unwrap_or_else(|_| "https://go.elyriax.com".to_string()),
            timezone: env::var("TZ").unwrap_or_else(|_| "Asia/Ho_Chi_Minh".to_string()),
            base_dir,

            db_host: env::var("DB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            db_port: env::var("DB_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3306),
            db_user: env::var("DB_USER").unwrap_or_else(|_| "root".to_string()),
            db_password: env::var("DB_PASSWORD").unwrap_or_default(),
            db_name: env::var("DB_NAME").unwrap_or_else(|_| "elyriax".to_string()),
            db_connection_limit: env::var("DB_CONNECTION_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),

            jwt_secret: env::var("JWT_SECRET")
                .unwrap_or_else(|_| "change_me_jwt_secret_in_env".to_string()),
            genshin_cookie_secret: env::var("GENSHIN_COOKIE_SECRET")
                .unwrap_or_else(|_| "change_me_genshin_cookie_secret_in_env".to_string()),

            discord_client_id: env::var("DISCORD_CLIENT_ID").unwrap_or_default(),
            discord_client_secret: env::var("DISCORD_CLIENT_SECRET").unwrap_or_default(),
            google_client_id: env::var("GOOGLE_CLIENT_ID").unwrap_or_default(),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default(),
            github_client_id: env::var("GITHUB_CLIENT_ID").unwrap_or_default(),
            github_client_secret: env::var("GITHUB_CLIENT_SECRET").unwrap_or_default(),

            sepay_webhook_token: env::var("SEPAY_WEBHOOK_TOKEN").unwrap_or_default(),
            bank_id: env::var("BANK_ID").unwrap_or_else(|_| "MB".to_string()),
            bank_account_no: env::var("BANK_ACCOUNT_NO").unwrap_or_default(),
            bank_account_name: env::var("BANK_ACCOUNT_NAME").unwrap_or_default(),

            admin_emails,
            admin_username,
            admin_password,
            admin_token: env::var("ADMIN_TOKEN").unwrap_or_default(),

            resend_api_key: env::var("RESEND_API_KEY").unwrap_or_default(),
            email_from: env::var("EMAIL_FROM")
                .unwrap_or_else(|_| "Elyriax <no-reply@elyriax.com>".to_string()),

            provider_appmmo_url: env::var("PROVIDER_APPMMO_URL")
                .unwrap_or_else(|_| "https://shop.appmmo.com/api/products.php".to_string()),
            provider_appmmo_api_key: env::var("PROVIDER_APPMMO_API_KEY").unwrap_or_default(),
            provider_sync_interval_hours: env::var("PROVIDER_SYNC_INTERVAL_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(12)
                .clamp(1, 24),
        }
    }

    pub fn is_admin_email(&self, email: Option<&str>) -> bool {
        let Some(e) = email else {
            return false;
        };
        let normalized = e.trim().to_lowercase();
        !normalized.is_empty() && self.admin_emails.iter().any(|a| a == &normalized)
    }

    pub fn data_dir(&self) -> PathBuf {
        self.base_dir.join("data")
    }

    pub fn uploads_dir(&self) -> PathBuf {
        self.base_dir.join("uploads")
    }

    pub fn templates_dir(&self) -> PathBuf {
        self.base_dir.join("templates")
    }
}
