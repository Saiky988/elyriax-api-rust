use std::time::Duration;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions};
use crate::core::config::Config;

pub async fn init_db_pool(config: &Config) -> Result<MySqlPool, sqlx::Error> {
    let connect_opts = MySqlConnectOptions::new()
        .host(&config.db_host)
        .port(config.db_port)
        .username(&config.db_user)
        .password(&config.db_password)
        .database(&config.db_name)
        .charset("utf8mb4");

    MySqlPoolOptions::new()
        .min_connections(2)
        .max_connections(config.db_connection_limit.max(5))
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .max_lifetime(Duration::from_secs(1800))
        .test_before_acquire(true)
        .connect_with(connect_opts)
        .await
}
