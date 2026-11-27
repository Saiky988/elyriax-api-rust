-- Safe, idempotent migration for Product Variants (Parent -> Child grouped products)
-- Compatible with MariaDB 10.11+ (`s171735_sayraaxyz`)
-- Run via:
--   mysql -u <DB_USER> -p s171735_sayraaxyz < migrations/add_product_variants.sql

-- 1. Add `parent_id` and `variant_name` columns, index, and self-referencing Foreign Key
ALTER TABLE `products`
    ADD COLUMN IF NOT EXISTS `parent_id` INT(11) NULL DEFAULT NULL AFTER `id`,
    ADD COLUMN IF NOT EXISTS `variant_name` VARCHAR(100) NULL DEFAULT NULL AFTER `name`,
    ADD KEY IF NOT EXISTS `idx_parent_id` (`parent_id`),
    ADD CONSTRAINT `fk_products_parent`
        FOREIGN KEY IF NOT EXISTS (`parent_id`) REFERENCES `products` (`id`) ON DELETE CASCADE;

-- 2. Upsert Parent Group products (idempotent via INSERT IGNORE on unique product_code)
INSERT IGNORE INTO `products`
    (`product_code`, `category`, `name`, `slug`, `description`, `price`, `stock`, `delivery_type`, `status`, `thumbnail`, `sort_order`)
VALUES
    (
        'WDOW-GROUP', 'license', 'Bản quyền Windows', 'ban-quyen-windows',
        'Key bản quyền Windows chính hãng (11 Pro, 10 Pro, 11 Enterprise). Bảo hành kích hoạt thành công lần đầu.',
        21000.00, 0, 'manual', 'active',
        'https://play-lh.googleusercontent.com/Umfq8FhCCYwt-9GmHu8TjIRwEfZaxBLd5izew_Z4aXFAmytgzPo0WqkUju8Tok_9wwenAjBeoSL9XAVEaSifxMo=w480-h960',
        1
    ),
    (
        'CAVA-GROUP', 'account', 'Canva Edu', 'canva-edu',
        'Tài khoản Canva Education nâng cấp chính chủ (1 Tháng / 1 Năm).',
        11000.00, 0, 'manual', 'active',
        'https://play-lh.googleusercontent.com/52fzAlhKzDyZV8ttyQ_DHB-jOeLZAZEdKlgXnWL__YNMZuvsoRCshJX5mr_ArFZjK7rte4hqEJ4ucjtuKJK5=w480-h960',
        2
    ),
    (
        'CCPR-GROUP', 'account', 'Tài khoản CapCut Pro', 'tai-khoan-capcut-pro',
        'Tài khoản CapCut Pro cá nhân (7 Ngày / 30 Ngày). Bảo hành trọn thời hạn gói.',
        10000.00, 0, 'manual', 'active',
        'https://play-lh.googleusercontent.com/M78HyakHaxKrjoeqYx41E9DXfVYYtx67nvc7Ks4G4zFQeaAJdGCi8gzzGSrHIwlrmnJS6zD9S4fAXqdEwfuHQAQ=w480-h960',
        3
    ),
    (
        'MAIL-GROUP', 'mail', 'Tài khoản Google', 'tai-khoan-google',
        'Tài khoản Google đa dạng gói (Domain sống 10 phút, Domain sống 12h, Đuôi @gmail.com). Giao tự động.',
        200.00, 0, 'automatic', 'active',
        'https://play-lh.googleusercontent.com/c6KD_8-GvnGRJA4lRQ4y5YnUa3-LOeYAS7ubw1jWyB9bmr6Kj6zzRvh2A5WBB-Vd3e6zhElmhDv07fCJx2Fc=w240-h480',
        4
    ),
    (
        'DISC-GROUP', 'account', 'Tài khoản Discord', 'tai-khoan-discord',
        'Tài khoản Discord chất lượng cao (Ver Mail + SĐT, Nitro Boost).',
        5000.00, 0, 'manual', 'active',
        'https://play-lh.googleusercontent.com/w2DdjDHGSj3DQElQQM0_J3PsNsALS-WD3CBeH0aXOYceR-F3aT02gFonhoFzz-Tc9ec8Dx9cLqh48-tzIFcP=w240-h480',
        5
    );

-- 3. Attach existing child variants to their Parent Groups
SET @wdow_id = (SELECT `id` FROM `products` WHERE `product_code` = 'WDOW-GROUP' LIMIT 1);
UPDATE `products` SET `parent_id` = @wdow_id, `variant_name` = '11 Pro',        `sort_order` = 1 WHERE `product_code` = 'WDOW-001' AND @wdow_id IS NOT NULL;
UPDATE `products` SET `parent_id` = @wdow_id, `variant_name` = '10 Pro',        `sort_order` = 2 WHERE `product_code` = 'WDOW-002' AND @wdow_id IS NOT NULL;
UPDATE `products` SET `parent_id` = @wdow_id, `variant_name` = '11 Enterprise', `sort_order` = 3 WHERE `product_code` = 'WDOW-003' AND @wdow_id IS NOT NULL;

SET @cava_id = (SELECT `id` FROM `products` WHERE `product_code` = 'CAVA-GROUP' LIMIT 1);
UPDATE `products` SET `parent_id` = @cava_id, `variant_name` = '1 Tháng', `sort_order` = 1 WHERE `product_code` = 'CAVA-002' AND @cava_id IS NOT NULL;
UPDATE `products` SET `parent_id` = @cava_id, `variant_name` = '1 Năm',   `sort_order` = 2 WHERE `product_code` = 'CAVA-001' AND @cava_id IS NOT NULL;

SET @ccpr_id = (SELECT `id` FROM `products` WHERE `product_code` = 'CCPR-GROUP' LIMIT 1);
UPDATE `products` SET `parent_id` = @ccpr_id, `variant_name` = '7 Ngày',  `sort_order` = 1 WHERE `product_code` = 'CCPR-002' AND @ccpr_id IS NOT NULL;
UPDATE `products` SET `parent_id` = @ccpr_id, `variant_name` = '30 Ngày', `sort_order` = 2 WHERE `product_code` = 'CCPR-001' AND @ccpr_id IS NOT NULL;

SET @mail_id = (SELECT `id` FROM `products` WHERE `product_code` = 'MAIL-GROUP' LIMIT 1);
UPDATE `products` SET `parent_id` = @mail_id, `variant_name` = 'Domain sống 10 phút', `sort_order` = 1 WHERE `product_code` = 'MAIL-001' AND @mail_id IS NOT NULL;
UPDATE `products` SET `parent_id` = @mail_id, `variant_name` = 'Domain sống 12h',     `sort_order` = 2 WHERE `product_code` = 'MAIL-002' AND @mail_id IS NOT NULL;
UPDATE `products` SET `parent_id` = @mail_id, `variant_name` = 'Đuôi @gmail.com',     `sort_order` = 3 WHERE `product_code` = 'MAIL-003' AND @mail_id IS NOT NULL;

SET @disc_id = (SELECT `id` FROM `products` WHERE `product_code` = 'DISC-GROUP' LIMIT 1);
UPDATE `products` SET `parent_id` = @disc_id, `variant_name` = 'Ver Mail + SĐT', `sort_order` = 1 WHERE `product_code` = 'DISC-002' AND @disc_id IS NOT NULL;
UPDATE `products` SET `parent_id` = @disc_id, `variant_name` = 'Nitro Boost',    `sort_order` = 2 WHERE `product_code` = 'DISC-001' AND @disc_id IS NOT NULL;
