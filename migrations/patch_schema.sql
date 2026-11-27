-- Safe, idempotent schema migration for MariaDB 10.11+ (`s171735_sayraaxyz`)
-- Run via:
--   mysql -u <DB_USER> -p s171735_sayraaxyz < migrations/patch_schema.sql

START TRANSACTION;

-- 1. Clean orphaned rows before adding foreign keys
DELETE FROM `wallets`
WHERE `user_id` NOT IN (SELECT `id` FROM `users`);

DELETE FROM `transactions`
WHERE `user_id` NOT IN (SELECT `id` FROM `users`)
   OR `wallet_id` NOT IN (SELECT `id` FROM `wallets`);

UPDATE `product_items`
SET `order_id` = NULL
WHERE `order_id` IS NOT NULL
  AND `order_id` NOT IN (SELECT `id` FROM `orders`);

-- 2. Deduplicate any historical duplicate `reference_id` values (e.g. dev test rows)
--    so adding UNIQUE KEY `uq_reference_id` never fails with Error 1062.
UPDATE `transactions` t
JOIN (
    SELECT `reference_id`, MIN(`id`) AS keep_id
    FROM `transactions`
    WHERE `reference_id` IS NOT NULL AND `reference_id` <> ''
    GROUP BY `reference_id`
    HAVING COUNT(*) > 1
) dup ON t.`reference_id` = dup.`reference_id` AND t.`id` > dup.`keep_id`
SET t.`reference_id` = CONCAT(t.`reference_id`, '-dup-', t.`id`);

COMMIT;

-- 3. Alter `transactions`: add `expired_at`, backfill deposits, add UNIQUE & Foreign Keys
ALTER TABLE `transactions`
    ADD COLUMN IF NOT EXISTS `expired_at` TIMESTAMP NULL DEFAULT NULL AFTER `created_at`;

UPDATE `transactions`
SET `expired_at` = DATE_ADD(`created_at`, INTERVAL 15 MINUTE)
WHERE `type` = 'deposit'
  AND `expired_at` IS NULL
  AND `created_at` IS NOT NULL;

UPDATE `transactions`
SET `payment_method` = 'qr'
WHERE `type` = 'deposit'
  AND `payment_method` IS NULL;

ALTER TABLE `transactions`
    ADD UNIQUE KEY IF NOT EXISTS `uq_reference_id` (`reference_id`),
    ADD CONSTRAINT IF NOT EXISTS `fk_transactions_user`
        FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON DELETE CASCADE,
    ADD CONSTRAINT IF NOT EXISTS `fk_transactions_wallet`
        FOREIGN KEY (`wallet_id`) REFERENCES `wallets` (`id`) ON DELETE CASCADE;

-- 4. Add missing Foreign Keys on `wallets` and `product_items`
ALTER TABLE `wallets`
    ADD CONSTRAINT IF NOT EXISTS `fk_wallets_user`
        FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON DELETE CASCADE;

ALTER TABLE `product_items`
    ADD CONSTRAINT IF NOT EXISTS `fk_product_items_order`
        FOREIGN KEY (`order_id`) REFERENCES `orders` (`id`) ON DELETE SET NULL;

-- 5. Standardize `hoyo_accounts` timestamp columns from DATETIME -> TIMESTAMP NULL DEFAULT NULL
ALTER TABLE `hoyo_accounts`
    MODIFY COLUMN `last_checkin_at` TIMESTAMP NULL DEFAULT NULL,
    MODIFY COLUMN `last_redeem_at` TIMESTAMP NULL DEFAULT NULL,
    MODIFY COLUMN `last_daily_note_at` TIMESTAMP NULL DEFAULT NULL;
