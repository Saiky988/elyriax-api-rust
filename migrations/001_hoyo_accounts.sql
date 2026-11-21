-- Migration 001: Multi-game HoYo accounts table for Honkai: Star Rail (hsr) and Zenless Zone Zero (zzz)
-- Note: Existing `genshin_accounts` table is kept 100% intact for backward compatibility.
-- This table is also auto-created at server boot via `CREATE TABLE IF NOT EXISTS`.

CREATE TABLE IF NOT EXISTS hoyo_accounts (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    game VARCHAR(16) NOT NULL COMMENT 'hsr | zzz',
    nickname VARCHAR(128) DEFAULT '',
    uid VARCHAR(32) NOT NULL,
    server VARCHAR(32) NOT NULL COMMENT 'prod_official_asia / prod_gf_jp / ...',
    cookie TEXT NOT NULL COMMENT 'AES-256-CBC encrypted using GENSHIN_COOKIE_SECRET',
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    auto_checkin BOOLEAN NOT NULL DEFAULT TRUE,
    auto_redeem BOOLEAN NOT NULL DEFAULT TRUE,
    auto_daily_note BOOLEAN NOT NULL DEFAULT FALSE,
    last_checkin_at DATETIME NULL,
    last_checkin_status VARCHAR(32) NULL,
    last_redeem_at DATETIME NULL,
    last_daily_note_at DATETIME NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'active',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_hoyo_user_game (user_id, game, status),
    INDEX idx_hoyo_auto_checkin (game, auto_checkin, status),
    INDEX idx_hoyo_auto_redeem (game, auto_redeem, status)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
