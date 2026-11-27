-- Elyriax v3.5.0 — Product Provider Price Sync Columns
-- Safe & idempotent for MariaDB 10.11+
-- Keeps all existing products, variants, manual delivery_type, name, icon, description, and product_code untouched.

ALTER TABLE `products`
  ADD COLUMN IF NOT EXISTS `provider_name` VARCHAR(50) NULL DEFAULT NULL AFTER `sort_order`,
  ADD COLUMN IF NOT EXISTS `provider_product_id` VARCHAR(100) NULL DEFAULT NULL AFTER `provider_name`,
  ADD COLUMN IF NOT EXISTS `provider_markup_percent` DECIMAL(5,2) NOT NULL DEFAULT 10.00 AFTER `provider_product_id`,
  ADD COLUMN IF NOT EXISTS `provider_base_price` DECIMAL(15,2) NULL DEFAULT NULL AFTER `provider_markup_percent`,
  ADD COLUMN IF NOT EXISTS `provider_last_synced_at` TIMESTAMP NULL DEFAULT NULL AFTER `provider_base_price`,
  ADD INDEX IF NOT EXISTS `idx_products_provider` (`provider_name`, `provider_product_id`);
