-- Elyriax v4.0.0 — Customer Delivery Profiles, Checkout Customer Data & Shop Operations
-- Safe & idempotent for MariaDB 10.11+

CREATE TABLE IF NOT EXISTS `user_delivery_profiles` (
  `id` INT(11) NOT NULL AUTO_INCREMENT,
  `user_id` INT(11) NOT NULL,
  `label` VARCHAR(100) NOT NULL,
  `contact_phone` VARCHAR(20) NULL DEFAULT NULL,
  `profile_type` ENUM('email', 'account') NOT NULL DEFAULT 'email',
  `target_email` VARCHAR(255) NULL DEFAULT NULL,
  `account_username` VARCHAR(255) NULL DEFAULT NULL,
  `account_password_encrypted` TEXT NULL DEFAULT NULL,
  `two_factor_note` TEXT NULL DEFAULT NULL,
  `is_default` TINYINT(1) NOT NULL DEFAULT 0,
  `created_at` TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  `updated_at` TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`),
  KEY `idx_udp_user_default` (`user_id`, `is_default`),
  CONSTRAINT `fk_udp_user` FOREIGN KEY IF NOT EXISTS (`user_id`) REFERENCES `users` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

ALTER TABLE `products`
  ADD COLUMN IF NOT EXISTS `require_profile_type` ENUM('none', 'email', 'account', 'any') NOT NULL DEFAULT 'none' AFTER `delivery_type`;

ALTER TABLE `order_items`
  ADD COLUMN IF NOT EXISTS `customer_data` LONGTEXT NULL DEFAULT NULL AFTER `delivery_type`;

ALTER TABLE `orders`
  ADD COLUMN IF NOT EXISTS `admin_note` TEXT NULL DEFAULT NULL AFTER `status`;

ALTER TABLE `orders`
  MODIFY COLUMN `status` VARCHAR(32) NOT NULL DEFAULT 'pending';
