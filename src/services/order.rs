use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    core::security::{generate_order_code, generate_transaction_code},
    error::{AppError, AppResult},
    services::{
        delivery_profile::resolve_profile_snapshot,
        email::send_notification_email,
    },
    state::AppState,
};

#[derive(Debug, Default, Deserialize)]
pub struct CheckoutItemProfileInput {
    pub delivery_profile_id: Option<i64>,
    pub customer_data: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CheckoutRequest {
    /// Map keyed by product_id (string) or product_code -> selected profile ID or inline customer_data
    #[serde(default)]
    pub item_profiles: HashMap<String, CheckoutItemProfileInput>,
    /// Optional fallback delivery_profile_id applied to required items not explicitly mapped
    pub default_profile_id: Option<i64>,
}

struct LockedProductItem {
    product_id: i64,
    product_code: String,
    product_name: String,
    quantity: i64,
    unit_price: f64,
    subtotal: f64,
    delivery_type: String,
    customer_data: Option<Value>,
    assigned_ids: Vec<i64>,
}

fn mask_customer_data_for_buyer(raw: Option<String>) -> Option<Value> {
    let txt = raw?;
    let mut val: Value = serde_json::from_str(&txt).ok()?;
    if let Some(obj) = val.as_object_mut() {
        if obj
            .get("account_password")
            .and_then(Value::as_str)
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            obj.insert("account_password".to_string(), Value::String("••••••••".to_string()));
        }
    }
    Some(val)
}

fn validate_customer_snapshot(
    req_type: &str,
    snap: &Value,
    product_name: &str,
) -> AppResult<()> {
    let ptype = snap
        .get("profile_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let target_email = snap
        .get("target_email")
        .or_else(|| snap.get("email"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let username = snap
        .get("account_username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let password = snap
        .get("account_password")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match req_type {
        "email" => {
            if target_email.is_none() && username.is_none() {
                return Err(AppError::bad_request(format!(
                    "Sản phẩm \"{product_name}\" yêu cầu thông tin Email nhận hàng / nâng cấp"
                )));
            }
        }
        "account" => {
            if username.is_none() || password.is_none() {
                return Err(AppError::bad_request(format!(
                    "Sản phẩm \"{product_name}\" yêu cầu thông tin Tài khoản & Mật khẩu"
                )));
            }
        }
        "any" => {
            let has_email = target_email.is_some();
            let has_acc = username.is_some() && password.is_some();
            if !has_email && !has_acc {
                return Err(AppError::bad_request(format!(
                    "Sản phẩm \"{product_name}\" yêu cầu thông tin nhận hàng hợp lệ (Email hoặc Tài khoản)"
                )));
            }
        }
        _ => {
            let _ = ptype;
        }
    }
    Ok(())
}

fn escape_html_str(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn format_vnd_simple(v: f64) -> String {
    let n = v.round() as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    let formatted: String = out.chars().rev().collect();
    if n < 0 {
        format!("-{formatted} ₫")
    } else {
        format!("{formatted} ₫")
    }
}

pub async fn get_order_by_id(state: &AppState, user_id: i64, order_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let order = sqlx::query(
        r#"
        SELECT id, order_code, CAST(subtotal AS DOUBLE) AS subtotal,
               CAST(total AS DOUBLE) AS total, currency, status, admin_note, created_at, completed_at
        FROM orders
        WHERE id = ? AND user_id = ?
        "#,
    )
    .bind(order_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let Some(order) = order else {
        return Err(AppError::not_found("Đơn hàng không tồn tại"));
    };

    let order_items = sqlx::query(
        r#"
        SELECT id, product_id, product_code, product_name, quantity,
               CAST(unit_price AS DOUBLE) AS unit_price,
               CAST(subtotal AS DOUBLE) AS subtotal, delivery_type, customer_data
        FROM order_items WHERE order_id = ?
        "#,
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    let delivered_items = sqlx::query(
        "SELECT id, product_id, identifier, content, sold_at FROM product_items WHERE order_id = ?",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    let formatted_items: Vec<Value> = order_items
        .into_iter()
        .map(|item| {
            let pid: i64 = item.try_get("product_id").unwrap_or(0);
            let delivery_type: String = item.try_get("delivery_type").unwrap_or_default();
            let raw_cdata: Option<String> = item.try_get("customer_data").ok().flatten();
            let belong: Vec<Value> = if delivery_type == "automatic" {
                delivered_items
                    .iter()
                    .filter(|di| di.try_get::<i64, _>("product_id").unwrap_or(-1) == pid)
                    .map(|di| {
                        json!({
                            "id": di.try_get::<i64, _>("id").unwrap_or(0),
                            "identifier": di.try_get::<Option<String>, _>("identifier").ok().flatten(),
                            "content": di.try_get::<Option<String>, _>("content").ok().flatten(),
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };

            json!({
                "product_code": item.try_get::<String, _>("product_code").unwrap_or_default(),
                "product_name": item.try_get::<String, _>("product_name").unwrap_or_default(),
                "quantity": item.try_get::<i64, _>("quantity").unwrap_or(0),
                "unit_price": item.try_get::<f64, _>("unit_price").unwrap_or(0.0),
                "subtotal": item.try_get::<f64, _>("subtotal").unwrap_or(0.0),
                "delivery_type": delivery_type,
                "customer_data": mask_customer_data_for_buyer(raw_cdata),
                "items": belong,
            })
        })
        .collect();

    let created_at: Option<DateTime<Utc>> = order.try_get("created_at").ok().flatten();
    let completed_at: Option<DateTime<Utc>> = order.try_get("completed_at").ok().flatten();

    Ok(json!({
        "id": order.try_get::<i64, _>("id").unwrap_or(0),
        "order_code": order.try_get::<String, _>("order_code").unwrap_or_default(),
        "subtotal": order.try_get::<f64, _>("subtotal").unwrap_or(0.0),
        "total": order.try_get::<f64, _>("total").unwrap_or(0.0),
        "currency": order.try_get::<String, _>("currency").unwrap_or_else(|_| "VND".to_string()),
        "status": order.try_get::<String, _>("status").unwrap_or_default(),
        "admin_note": order.try_get::<Option<String>, _>("admin_note").ok().flatten(),
        "created_at": created_at,
        "completed_at": completed_at,
        "items": formatted_items,
    }))
}

pub async fn checkout(
    state: &AppState,
    user_id: i64,
    req: CheckoutRequest,
) -> AppResult<Value> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    // 1. Lock cart items
    let cart_items = sqlx::query(
        r#"
        SELECT ci.id AS cart_item_id, ci.product_id, ci.quantity,
               p.product_code, p.name AS product_name, p.variant_name,
               CAST(p.price AS DOUBLE) AS price,
               p.stock, p.delivery_type,
               COALESCE(p.require_profile_type, 'none') AS require_profile_type,
               p.status
        FROM cart_items ci
        JOIN products p ON ci.product_id = p.id
        WHERE ci.user_id = ? FOR UPDATE
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;

    if cart_items.is_empty() {
        return Err(AppError::bad_request("Giỏ hàng của bạn đang trống"));
    }

    let mut total_amount = 0.0_f64;
    let mut to_process: Vec<LockedProductItem> = Vec::with_capacity(cart_items.len());

    // 2. Validate availability, delivery profiles & lock inventory
    for item in cart_items {
        let prod_status: String = item.try_get("status")?;
        let base_name: String = item.try_get("product_name")?;
        let variant_name: Option<String> = item.try_get("variant_name").ok().flatten();
        let product_name = match variant_name.as_deref().filter(|s| !s.is_empty()) {
            Some(v) => format!("{base_name} — {v}"),
            None => base_name,
        };

        if prod_status != "active" {
            return Err(AppError::bad_request(format!(
                "Sản phẩm \"{product_name}\" hiện đã ngừng kinh doanh"
            )));
        }

        let product_id: i64 = item.try_get("product_id")?;
        let product_code: String = item.try_get("product_code")?;
        let unit_price: f64 = item.try_get("price")?;
        let product_stock: i64 = item.try_get("stock").unwrap_or(0);
        let qty: i64 = item.try_get("quantity")?;
        let delivery_type: String = item
            .try_get::<Option<String>, _>("delivery_type")
            .ok()
            .flatten()
            .unwrap_or_else(|| "manual".to_string());
        let require_profile_type: String = item
            .try_get::<String, _>("require_profile_type")
            .unwrap_or_else(|_| "none".to_string())
            .to_lowercase();
        let subtotal = unit_price * (qty as f64);
        total_amount += subtotal;

        // Resolve customer delivery profile if provided or required
        let mapped = req
            .item_profiles
            .get(&product_id.to_string())
            .or_else(|| req.item_profiles.get(&product_code));

        let customer_data: Option<Value> = if let Some(m) = mapped {
            if let Some(pid) = m.delivery_profile_id {
                Some(resolve_profile_snapshot(state, user_id, pid).await?)
            } else {
                m.customer_data.clone()
            }
        } else if let Some(def_id) = req.default_profile_id {
            if require_profile_type != "none" {
                Some(resolve_profile_snapshot(state, user_id, def_id).await?)
            } else {
                None
            }
        } else {
            None
        };

        if require_profile_type != "none" {
            let Some(ref snap) = customer_data else {
                return Err(AppError::bad_request(format!(
                    "Vui lòng chọn hoặc nhập thông tin nhận hàng / nâng cấp cho sản phẩm \"{product_name}\""
                )));
            };
            validate_customer_snapshot(&require_profile_type, snap, &product_name)?;
        }

        let assigned_ids: Vec<i64> = if delivery_type.eq_ignore_ascii_case("manual") {
            if product_stock < qty {
                return Err(AppError::bad_request(format!(
                    "Sản phẩm \"{product_name}\" không đủ số lượng trong kho (còn {product_stock})"
                )));
            }
            Vec::new()
        } else {
            let available = sqlx::query(
                r#"
                SELECT id FROM product_items
                WHERE product_id = ? AND status = 'available'
                LIMIT ? FOR UPDATE
                "#,
            )
            .bind(product_id)
            .bind(qty)
            .fetch_all(&mut *tx)
            .await?;

            if (available.len() as i64) < qty {
                return Err(AppError::bad_request(format!(
                    "Sản phẩm \"{product_name}\" không đủ số lượng trong kho (còn {})",
                    available.len()
                )));
            }

            available
                .into_iter()
                .filter_map(|r| r.try_get("id").ok())
                .collect()
        };

        to_process.push(LockedProductItem {
            product_id,
            product_code,
            product_name,
            quantity: qty,
            unit_price,
            subtotal,
            delivery_type,
            customer_data,
            assigned_ids,
        });
    }

    // 3. Lock wallet & verify balance
    let wallet = sqlx::query(
        "SELECT id, CAST(balance AS DOUBLE) AS balance FROM wallets WHERE user_id = ? FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(wallet) = wallet else {
        return Err(AppError::bad_request("Ví người dùng không tồn tại"));
    };
    let wallet_id: i64 = wallet.try_get("id")?;
    let balance_before: f64 = wallet.try_get("balance")?;

    if balance_before < total_amount {
        return Err(AppError::bad_request(format!(
            "Số dư ví không đủ (Hiện có: {:.0} VND, Cần: {:.0} VND)",
            balance_before, total_amount
        )));
    }

    let balance_after = balance_before - total_amount;

    // 4. Deduct wallet balance
    sqlx::query("UPDATE wallets SET balance = balance - ?, total_spent = total_spent + ? WHERE id = ?")
        .bind(total_amount)
        .bind(total_amount)
        .bind(wallet_id)
        .execute(&mut *tx)
        .await?;

    // 5. Create order (`processing` if any item is manual, `completed` if all automatic)
    let has_manual = to_process
        .iter()
        .any(|p| p.delivery_type.eq_ignore_ascii_case("manual"));
    let order_code = generate_order_code();
    let order_res = if has_manual {
        sqlx::query(
            r#"
            INSERT INTO orders (order_code, user_id, wallet_id, subtotal, total, currency, status, completed_at)
            VALUES (?, ?, ?, ?, ?, 'VND', 'processing', NULL)
            "#,
        )
        .bind(&order_code)
        .bind(user_id)
        .bind(wallet_id)
        .bind(total_amount)
        .bind(total_amount)
        .execute(&mut *tx)
        .await?
    } else {
        sqlx::query(
            r#"
            INSERT INTO orders (order_code, user_id, wallet_id, subtotal, total, currency, status, completed_at)
            VALUES (?, ?, ?, ?, ?, 'VND', 'completed', CURRENT_TIMESTAMP)
            "#,
        )
        .bind(&order_code)
        .bind(user_id)
        .bind(wallet_id)
        .bind(total_amount)
        .bind(total_amount)
        .execute(&mut *tx)
        .await?
    };
    let order_id = order_res.last_insert_id() as i64;

    // 6. Insert order items & mark inventory sold
    for p in &to_process {
        let cdata_str = p
            .customer_data
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());

        sqlx::query(
            r#"
            INSERT INTO order_items
            (order_id, product_id, product_code, product_name, quantity, unit_price, subtotal, delivery_type, customer_data)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(order_id)
        .bind(p.product_id)
        .bind(&p.product_code)
        .bind(&p.product_name)
        .bind(p.quantity)
        .bind(p.unit_price)
        .bind(p.subtotal)
        .bind(&p.delivery_type)
        .bind(&cdata_str)
        .execute(&mut *tx)
        .await?;

        for item_id in &p.assigned_ids {
            sqlx::query(
                "UPDATE product_items SET status = 'sold', order_id = ?, sold_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(order_id)
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        }

        if p.delivery_type.eq_ignore_ascii_case("manual") {
            sqlx::query("UPDATE products SET stock = GREATEST(0, stock - ?) WHERE id = ?")
                .bind(p.quantity)
                .bind(p.product_id)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE products
                SET stock = (SELECT COUNT(id) FROM product_items WHERE product_id = ? AND status = 'available')
                WHERE id = ?
                "#,
            )
            .bind(p.product_id)
            .bind(p.product_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    // 7. Record transaction
    let tx_code = generate_transaction_code();
    sqlx::query(
        r#"
        INSERT INTO transactions
        (transaction_code, user_id, wallet_id, type, amount, balance_before, balance_after, status, payment_method, reference_id, description, completed_at)
        VALUES (?, ?, ?, 'purchase', ?, ?, ?, 'completed', 'wallet', ?, ?, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(&tx_code)
    .bind(user_id)
    .bind(wallet_id)
    .bind(total_amount)
    .bind(balance_before)
    .bind(balance_after)
    .bind(&order_code)
    .bind(format!("Thanh toán đơn hàng {}", order_code))
    .execute(&mut *tx)
    .await?;

    // 8. Clear cart
    sqlx::query("DELETE FROM cart_items WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // 9. Async Admin Email Notification (non-blocking)
    let admin_recipients = state.config.admin_emails.clone();
    if !admin_recipients.is_empty() {
        let st_clone = state.clone();
        let oc_clone = order_code.clone();
        let order_status = if has_manual { "processing" } else { "completed" };
        let items_summary: Vec<(String, String, i64, String, Option<Value>)> = to_process
            .into_iter()
            .map(|p| {
                (
                    p.product_code,
                    p.product_name,
                    p.quantity,
                    p.delivery_type,
                    p.customer_data,
                )
            })
            .collect();

        tokio::spawn(async move {
            let buyer_row = if let Ok(pool) = st_clone.db() {
                sqlx::query("SELECT username, email FROM users WHERE id = ?")
                    .bind(user_id)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            let buyer_name = buyer_row
                .as_ref()
                .and_then(|r| r.try_get::<String, _>("username").ok())
                .unwrap_or_else(|| format!("User #{user_id}"));
            let buyer_email = buyer_row
                .as_ref()
                .and_then(|r| r.try_get::<Option<String>, _>("email").ok().flatten())
                .unwrap_or_else(|| "—".to_string());

            let mut rows_html = String::new();
            for (code, name, qty, dtype, cdata) in &items_summary {
                let mut cd_html = String::new();
                if let Some(cd) = cdata {
                    let lbl = cd.get("label").and_then(Value::as_str).unwrap_or("");
                    let phone = cd.get("contact_phone").and_then(Value::as_str).unwrap_or("");
                    let email = cd.get("target_email").and_then(Value::as_str).unwrap_or("");
                    let uname = cd.get("account_username").and_then(Value::as_str).unwrap_or("");
                    let pw = cd.get("account_password").and_then(Value::as_str).unwrap_or("");
                    let note = cd.get("two_factor_note").and_then(Value::as_str).unwrap_or("");
                    cd_html.push_str("<div style='margin-top:6px;padding:8px 10px;background:#f4f6fa;border-radius:6px;font-size:12px;font-family:monospace'>");
                    if !lbl.is_empty() {
                        cd_html.push_str(&format!("<div><b>Hồ sơ:</b> {}</div>", escape_html_str(lbl)));
                    }
                    if !email.is_empty() {
                        cd_html.push_str(&format!("<div><b>Email nhận:</b> {}</div>", escape_html_str(email)));
                    }
                    if !uname.is_empty() {
                        cd_html.push_str(&format!("<div><b>Tài khoản:</b> {}</div>", escape_html_str(uname)));
                    }
                    if !pw.is_empty() {
                        cd_html.push_str(&format!("<div><b>Mật khẩu:</b> {}</div>", escape_html_str(pw)));
                    }
                    if !phone.is_empty() {
                        cd_html.push_str(&format!("<div><b>SĐT / Zalo:</b> {}</div>", escape_html_str(phone)));
                    }
                    if !note.is_empty() {
                        cd_html.push_str(&format!("<div><b>2FA / Ghi chú:</b> {}</div>", escape_html_str(note)));
                    }
                    cd_html.push_str("</div>");
                }
                rows_html.push_str(&format!(
                    "<tr><td style='padding:10px;border-bottom:1px solid #e5e7eb'><b>{}</b> <span style='color:#6b7280'>({})</span><br><span style='font-size:12px;color:#4b5563'>SL: {} · Giao: {}</span>{}</td></tr>",
                    escape_html_str(name),
                    escape_html_str(code),
                    qty,
                    escape_html_str(dtype),
                    cd_html
                ));
            }

            let subject = format!(
                "[Elyriax Shop] Đơn mới {} — {} ({})",
                oc_clone,
                format_vnd_simple(total_amount),
                if order_status == "processing" { "Cần xử lý" } else { "Hoàn tất" }
            );
            let html = format!(
                r#"<div style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;max-width:600px;margin:0 auto;color:#111827">
                  <h2 style="margin:0 0 12px">Đơn hàng mới trên Elyriax</h2>
                  <p style="margin:0 0 14px;color:#374151">
                    <b>Mã đơn:</b> {code}<br>
                    <b>Khách hàng:</b> {buyer} ({email})<br>
                    <b>Tổng thanh toán:</b> {total}<br>
                    <b>Trạng thái:</b> {status}
                  </p>
                  <table style="width:100%;border-collapse:collapse;border:1px solid #e5e7eb;border-radius:8px">
                    {rows}
                  </table>
                  <p style="margin-top:16px"><a href="{fe}/shop-ops" style="display:inline-block;padding:10px 16px;background:#2563eb;color:#fff;text-decoration:none;border-radius:8px;font-weight:600">Mở trang Xử lý đơn hàng</a></p>
                </div>"#,
                code = escape_html_str(&oc_clone),
                buyer = escape_html_str(&buyer_name),
                email = escape_html_str(&buyer_email),
                total = format_vnd_simple(total_amount),
                status = order_status,
                rows = rows_html,
                fe = st_clone.config.frontend_url,
            );

            if let Err(err) = send_notification_email(&st_clone, admin_recipients, &subject, &html).await {
                tracing::warn!(order_code = %oc_clone, error = %err, "[ORDER EMAIL] Failed to notify admin");
            }
        });
    }

    get_order_by_id(state, user_id, order_id).await
}

pub async fn get_orders(
    state: &AppState,
    user_id: i64,
    page: i64,
    limit: i64,
    status: Option<&str>,
) -> AppResult<Value> {
    let pool = state.db()?;
    let page = page.max(1);
    let limit = limit.clamp(1, 100);
    let offset = (page - 1) * limit;

    let mut sql = String::from(
        r#"
        SELECT
            o.id, o.order_code,
            CAST(o.subtotal AS DOUBLE) AS subtotal,
            CAST(o.total AS DOUBLE) AS total,
            o.currency, o.status, o.admin_note, o.created_at, o.completed_at,
            (SELECT COUNT(oi.id) FROM order_items oi WHERE oi.order_id = o.id) AS items_count,
            (SELECT oi.product_name FROM order_items oi WHERE oi.order_id = o.id ORDER BY oi.id ASC LIMIT 1) AS first_item_name
        FROM orders o
        WHERE o.user_id = ?
        "#,
    );
    let mut count_sql = String::from("SELECT COUNT(id) AS total FROM orders WHERE user_id = ?");

    let status_filter = status
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("all"));
    if status_filter.is_some() {
        sql.push_str(" AND o.status = ?");
        count_sql.push_str(" AND status = ?");
    }
    sql.push_str(" ORDER BY o.created_at DESC LIMIT ? OFFSET ?");

    let mut q = sqlx::query(&sql).bind(user_id);
    let mut cq = sqlx::query(&count_sql).bind(user_id);

    if let Some(st) = status_filter {
        q = q.bind(st);
        cq = cq.bind(st);
    }

    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    let count_row = cq.fetch_one(pool).await?;
    let total: i64 = count_row.try_get("total").unwrap_or(0);

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let created_at: Option<DateTime<Utc>> = r.try_get("created_at").ok().flatten();
            let completed_at: Option<DateTime<Utc>> = r.try_get("completed_at").ok().flatten();
            json!({
                "id": r.try_get::<i64, _>("id").unwrap_or(0),
                "order_code": r.try_get::<String, _>("order_code").unwrap_or_default(),
                "subtotal": r.try_get::<f64, _>("subtotal").unwrap_or(0.0),
                "total": r.try_get::<f64, _>("total").unwrap_or(0.0),
                "currency": r.try_get::<String, _>("currency").unwrap_or_else(|_| "VND".to_string()),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "admin_note": r.try_get::<Option<String>, _>("admin_note").ok().flatten(),
                "items_count": r.try_get::<i64, _>("items_count").unwrap_or(0),
                "first_item_name": r.try_get::<Option<String>, _>("first_item_name").ok().flatten(),
                "created_at": created_at,
                "completed_at": completed_at,
            })
        })
        .collect();

    let total_pages = ((total as f64) / (limit as f64)).ceil().max(1.0) as i64;

    Ok(json!({
        "items": items,
        "pagination": {
            "total": total,
            "page": page,
            "limit": limit,
            "total_pages": total_pages,
        }
    }))
}

pub async fn cancel_order(state: &AppState, user_id: i64, order_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    let order = sqlx::query(
        r#"
        SELECT id, order_code, wallet_id, CAST(total AS DOUBLE) AS total, status
        FROM orders WHERE id = ? AND user_id = ? FOR UPDATE
        "#,
    )
    .bind(order_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(order) = order else {
        return Err(AppError::not_found("Đơn hàng không tồn tại"));
    };

    let status: String = order.try_get("status")?;
    if status != "pending" && status != "processing" {
        return Err(AppError::bad_request(
            "Chỉ có thể hủy đơn hàng đang ở trạng thái chờ xử lý (pending/processing)",
        ));
    }

    let order_code: String = order.try_get("order_code")?;
    let wallet_id: i64 = order.try_get("wallet_id")?;
    let total: f64 = order.try_get("total")?;

    // Refund wallet atomically
    let wallet = sqlx::query(
        "SELECT CAST(balance AS DOUBLE) AS balance FROM wallets WHERE id = ? FOR UPDATE",
    )
    .bind(wallet_id)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(w) = wallet {
        let bal_before: f64 = w.try_get("balance").unwrap_or(0.0);
        let bal_after = bal_before + total;
        sqlx::query("UPDATE wallets SET balance = balance + ?, total_spent = GREATEST(0, total_spent - ?) WHERE id = ?")
            .bind(total)
            .bind(total)
            .bind(wallet_id)
            .execute(&mut *tx)
            .await?;

        let tx_code = generate_transaction_code();
        sqlx::query(
            r#"
            INSERT INTO transactions
            (transaction_code, user_id, wallet_id, type, amount, balance_before, balance_after, status, payment_method, reference_id, description, completed_at)
            VALUES (?, ?, ?, 'refund', ?, ?, ?, 'completed', 'wallet', ?, ?, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(&tx_code)
        .bind(user_id)
        .bind(wallet_id)
        .bind(total)
        .bind(bal_before)
        .bind(bal_after)
        .bind(format!("REFUND-{}", order_code))
        .bind(format!("Hoàn tiền hủy đơn hàng {}", order_code))
        .execute(&mut *tx)
        .await?;
    }

    // Restore manual product stock
    let oitems = sqlx::query(
        "SELECT product_id, quantity, delivery_type FROM order_items WHERE order_id = ?",
    )
    .bind(order_id)
    .fetch_all(&mut *tx)
    .await?;
    for oi in oitems {
        let dt: String = oi.try_get::<Option<String>, _>("delivery_type").ok().flatten().unwrap_or_default();
        if dt.eq_ignore_ascii_case("manual") {
            let pid: i64 = oi.try_get("product_id").unwrap_or(0);
            let qty: i64 = oi.try_get("quantity").unwrap_or(0);
            sqlx::query("UPDATE products SET stock = stock + ? WHERE id = ?")
                .bind(qty)
                .bind(pid)
                .execute(&mut *tx)
                .await?;
        }
    }

    sqlx::query("UPDATE orders SET status = 'cancelled' WHERE id = ?")
        .bind(order_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(json!({ "id": order_id, "status": "cancelled", "refunded": total }))
}
