use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    core::{
        config::Config,
        security::{create_qr_token, generate_transaction_code},
    },
    error::{AppError, AppResult},
    state::AppState,
};

const DEPOSIT_EXPIRY_MINUTES: i64 = 15;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaymentInfo {
    pub qr_image: String,
    pub bank: String,
    pub account_number: String,
    pub account_name: String,
    pub transfer_content: String,
    pub expired_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Transaction {
    pub id: i64,
    pub transaction_code: String,
    pub user_id: i64,
    pub wallet_id: i64,
    #[serde(rename = "type")]
    pub tx_type: String,
    pub status: String,
    pub amount: f64,
    pub payment_method: Option<String>,
    pub reference_id: Option<String>,
    pub description: Option<String>,
    pub balance_before: Option<f64>,
    pub balance_after: Option<f64>,
    pub created_at: Option<DateTime<Utc>>,
    pub expired_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub payment_info: Option<PaymentInfo>,
}

pub fn build_payment_info(
    config: &Config,
    amount: f64,
    transaction_code: &str,
    expired_at: DateTime<Utc>,
) -> AppResult<PaymentInfo> {
    let token = create_qr_token(
        amount,
        transaction_code.to_string(),
        &config.jwt_secret,
        expired_at,
    )?;

    Ok(PaymentInfo {
        qr_image: format!("{}/v1/payment/qr?token={}", config.base_url, token),
        bank: config.bank_id.clone(),
        account_number: config.bank_account_no.clone(),
        account_name: config.bank_account_name.clone(),
        transfer_content: transaction_code.to_string(),
        expired_at,
    })
}

fn parse_tx_row(r: &sqlx::mysql::MySqlRow) -> Transaction {
    Transaction {
        id: r.try_get::<i64, _>("id").unwrap_or(0),
        transaction_code: r.try_get::<String, _>("transaction_code").unwrap_or_default(),
        user_id: r.try_get::<i64, _>("user_id").unwrap_or(0),
        wallet_id: r.try_get::<i64, _>("wallet_id").unwrap_or(0),
        tx_type: r.try_get::<String, _>("type").unwrap_or_default(),
        status: r.try_get::<String, _>("status").unwrap_or_default(),
        amount: r.try_get::<f64, _>("amount").unwrap_or(0.0),
        payment_method: r.try_get::<Option<String>, _>("payment_method").ok().flatten(),
        reference_id: r.try_get::<Option<String>, _>("reference_id").ok().flatten(),
        description: r.try_get::<Option<String>, _>("description").ok().flatten(),
        balance_before: r.try_get::<Option<f64>, _>("balance_before").ok().flatten(),
        balance_after: r.try_get::<Option<f64>, _>("balance_after").ok().flatten(),
        created_at: r.try_get::<Option<DateTime<Utc>>, _>("created_at").ok().flatten(),
        expired_at: r.try_get::<Option<DateTime<Utc>>, _>("expired_at").ok().flatten(),
        completed_at: r.try_get::<Option<DateTime<Utc>>, _>("completed_at").ok().flatten(),
        payment_info: None,
    }
}

pub async fn get_or_create_wallet(state: &AppState, user_id: i64) -> AppResult<Value> {
    let pool = state.db()?;
    let select_sql = r#"
        SELECT id, user_id,
               CAST(balance AS DOUBLE) AS balance,
               CAST(total_deposit AS DOUBLE) AS total_deposit,
               CAST(total_spent AS DOUBLE) AS total_spent,
               created_at, updated_at
        FROM wallets WHERE user_id = ?
    "#;

    let row = if let Some(w) = sqlx::query(select_sql).bind(user_id).fetch_optional(pool).await? {
        w
    } else {
        sqlx::query("INSERT IGNORE INTO wallets (user_id) VALUES (?)")
            .bind(user_id)
            .execute(pool)
            .await?;
        sqlx::query(select_sql).bind(user_id).fetch_one(pool).await?
    };

    let created_at: Option<DateTime<Utc>> = row.try_get("created_at").ok().flatten();
    let updated_at: Option<DateTime<Utc>> = row.try_get("updated_at").ok().flatten();

    Ok(json!({
        "id": row.try_get::<i64, _>("id")?,
        "user_id": row.try_get::<i64, _>("user_id")?,
        "balance": row.try_get::<f64, _>("balance").unwrap_or(0.0),
        "total_deposit": row.try_get::<f64, _>("total_deposit").unwrap_or(0.0),
        "total_spent": row.try_get::<f64, _>("total_spent").unwrap_or(0.0),
        "created_at": created_at,
        "updated_at": updated_at,
    }))
}

pub async fn create_deposit_info(state: &AppState, user_id: i64, amount: f64) -> AppResult<Value> {
    if !amount.is_finite() || amount < 10_000.0 {
        return Err(AppError::bad_request("Số tiền nạp tối thiểu là 10,000 VND"));
    }

    let wallet = get_or_create_wallet(state, user_id).await?;
    let wallet_id = wallet["id"].as_i64().unwrap_or(0);
    let tx_code = generate_transaction_code();
    let now = Utc::now();
    let expired_at = now + Duration::minutes(DEPOSIT_EXPIRY_MINUTES);
    let pool = state.db()?;

    sqlx::query(
        r#"
        INSERT INTO transactions
        (transaction_code, user_id, wallet_id, type, amount, status, payment_method, description, created_at, expired_at)
        VALUES (?, ?, ?, 'deposit', ?, 'pending', 'qr', 'Nạp tiền vào ví ELYRIAX', ?, ?)
        "#,
    )
    .bind(&tx_code)
    .bind(user_id)
    .bind(wallet_id)
    .bind(amount)
    .bind(now)
    .bind(expired_at)
    .execute(pool)
    .await?;

    let tx_row = sqlx::query(
        r#"
        SELECT id, transaction_code, user_id, wallet_id, type,
               CAST(amount AS DOUBLE) AS amount,
               CAST(balance_before AS DOUBLE) AS balance_before,
               CAST(balance_after AS DOUBLE) AS balance_after,
               status, payment_method, reference_id, description,
               created_at, expired_at, completed_at
        FROM transactions WHERE transaction_code = ?
        "#,
    )
    .bind(&tx_code)
    .fetch_one(pool)
    .await?;

    let mut transaction = parse_tx_row(&tx_row);
    let effective_exp = transaction.expired_at.unwrap_or(expired_at);
    transaction.expired_at = Some(effective_exp);

    let payment_info = build_payment_info(&state.config, amount, &tx_code, effective_exp)?;
    transaction.payment_info = Some(payment_info.clone());

    Ok(json!({
        "transaction": transaction,
        "payment_info": payment_info,
    }))
}

pub async fn process_sepay_webhook(state: &AppState, payload: Value) -> AppResult<()> {
    if payload.get("transferType").and_then(Value::as_str) != Some("in") {
        return Ok(());
    }

    let content = payload.get("content").and_then(Value::as_str).unwrap_or("");
    let re = Regex::new(r"ELYRIAX [A-Z0-9]{6}").expect("valid static regex");
    let Some(mat) = re.find(content) else {
        return Ok(());
    };
    let tx_code = mat.as_str();
    let transfer_amount = payload
        .get("transferAmount")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let reference_code = payload
        .get("referenceCode")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let pool = state.db()?;
    let mut tx = pool.begin().await?;

    // Fast idempotency check for duplicate reference_id before mutating balances
    if let Some(ref_code) = reference_code {
        let dup = sqlx::query("SELECT id FROM transactions WHERE reference_id = ? LIMIT 1")
            .bind(ref_code)
            .fetch_optional(&mut *tx)
            .await?;
        if dup.is_some() {
            tracing::warn!(reference_id = ref_code, tx_code, "Duplicate SePay webhook reference_id ignored");
            tx.rollback().await?;
            return Ok(());
        }
    }

    let tx_row = sqlx::query(
        r#"
        SELECT id, wallet_id, status, CAST(amount AS DOUBLE) AS amount
        FROM transactions WHERE transaction_code = ? FOR UPDATE
        "#,
    )
    .bind(tx_code)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(tx_row) = tx_row else {
        tx.rollback().await?;
        return Ok(());
    };

    let status: String = tx_row.try_get("status")?;
    if status != "pending" {
        tx.rollback().await?;
        return Ok(());
    }

    let tx_id: i64 = tx_row.try_get("id")?;
    let wallet_id: i64 = tx_row.try_get("wallet_id")?;
    let expected_amount: f64 = tx_row.try_get("amount")?;

    let wallet_row = sqlx::query(
        "SELECT id, CAST(balance AS DOUBLE) AS balance FROM wallets WHERE id = ? FOR UPDATE",
    )
    .bind(wallet_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(wallet_row) = wallet_row else {
        tx.rollback().await?;
        return Ok(());
    };

    let balance_before: f64 = wallet_row.try_get("balance")?;
    let payload_str = payload.to_string();

    if transfer_amount >= expected_amount {
        let balance_after = balance_before + expected_amount;

        // Update transaction first so UNIQUE(reference_id) catches any concurrent race before crediting wallet
        let update_res = sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'completed', balance_before = ?, balance_after = ?,
                completed_at = NOW(), reference_id = ?, metadata = ?
            WHERE id = ? AND status = 'pending'
            "#,
        )
        .bind(balance_before)
        .bind(balance_after)
        .bind(reference_code)
        .bind(&payload_str)
        .bind(tx_id)
        .execute(&mut *tx)
        .await;

        match update_res {
            Ok(res) if res.rows_affected() == 0 => {
                tx.rollback().await?;
                return Ok(());
            }
            Ok(_) => {}
            Err(err) => {
                let msg = err.to_string();
                if msg.contains("Duplicate entry") || msg.contains("1062") {
                    tracing::warn!(reference_id = ?reference_code, tx_code, "Concurrent duplicate reference_id caught by uq_reference_id");
                    tx.rollback().await?;
                    return Ok(());
                }
                return Err(AppError::Database(err));
            }
        }

        sqlx::query(
            "UPDATE wallets SET balance = ?, total_deposit = total_deposit + ? WHERE id = ?",
        )
        .bind(balance_after)
        .bind(expected_amount)
        .bind(wallet_id)
        .execute(&mut *tx)
        .await?;
    } else {
        let update_res = sqlx::query(
            r#"
            UPDATE transactions
            SET status = 'failed', completed_at = NOW(), reference_id = ?, metadata = ?,
                description = 'Chuyển khoản không đủ số tiền yêu cầu'
            WHERE id = ? AND status = 'pending'
            "#,
        )
        .bind(reference_code)
        .bind(&payload_str)
        .bind(tx_id)
        .execute(&mut *tx)
        .await;

        if let Err(err) = update_res {
            let msg = err.to_string();
            if msg.contains("Duplicate entry") || msg.contains("1062") {
                tracing::warn!(reference_id = ?reference_code, tx_code, "Duplicate reference_id on failed deposit ignored");
                tx.rollback().await?;
                return Ok(());
            }
            return Err(AppError::Database(err));
        }
    }

    tx.commit().await?;
    Ok(())
}

pub async fn get_transactions(
    state: &AppState,
    user_id: i64,
    page: i64,
    limit: i64,
    tx_type: Option<&str>,
    status: Option<&str>,
    sort: Option<&str>,
) -> AppResult<Value> {
    let pool = state.db()?;
    let page = page.max(1);
    let limit = limit.clamp(1, 100);
    let offset = (page - 1) * limit;
    let sort_dir = if sort.unwrap_or("DESC").eq_ignore_ascii_case("ASC") {
        "ASC"
    } else {
        "DESC"
    };

    let mut sql = String::from(
        r#"
        SELECT id, transaction_code, user_id, wallet_id, type,
               CAST(amount AS DOUBLE) AS amount,
               CAST(balance_before AS DOUBLE) AS balance_before,
               CAST(balance_after AS DOUBLE) AS balance_after,
               status, payment_method, reference_id, description,
               created_at, expired_at, completed_at
        FROM transactions WHERE user_id = ?
        "#,
    );
    let mut count_sql = String::from("SELECT COUNT(id) AS total FROM transactions WHERE user_id = ?");

    if tx_type.is_some() {
        sql.push_str(" AND type = ?");
        count_sql.push_str(" AND type = ?");
    }
    if status.is_some() {
        sql.push_str(" AND status = ?");
        count_sql.push_str(" AND status = ?");
    }
    sql.push_str(&format!(" ORDER BY created_at {} LIMIT ? OFFSET ?", sort_dir));

    let mut q = sqlx::query(&sql).bind(user_id);
    let mut cq = sqlx::query(&count_sql).bind(user_id);

    if let Some(t) = tx_type {
        q = q.bind(t);
        cq = cq.bind(t);
    }
    if let Some(s) = status {
        q = q.bind(s);
        cq = cq.bind(s);
    }

    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    let count_row = cq.fetch_one(pool).await?;
    let total: i64 = count_row.try_get("total").unwrap_or(0);

    let items: Vec<Transaction> = rows.iter().map(parse_tx_row).collect();
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

pub async fn get_transaction_by_id(
    state: &AppState,
    user_id: i64,
    tx_id: i64,
) -> AppResult<Transaction> {
    let pool = state.db()?;
    let row = sqlx::query(
        r#"
        SELECT id, transaction_code, user_id, wallet_id, type,
               CAST(amount AS DOUBLE) AS amount,
               CAST(balance_before AS DOUBLE) AS balance_before,
               CAST(balance_after AS DOUBLE) AS balance_after,
               status, payment_method, reference_id, description,
               created_at, expired_at, completed_at
        FROM transactions WHERE user_id = ? AND id = ?
        "#,
    )
    .bind(user_id)
    .bind(tx_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Err(AppError::not_found("Giao dịch không tồn tại"));
    };

    let mut transaction = parse_tx_row(&row);

    if transaction.tx_type == "deposit" && transaction.status == "pending" {
        let exp = transaction.expired_at.unwrap_or_else(|| {
            transaction.created_at.unwrap_or_else(Utc::now)
                + Duration::minutes(DEPOSIT_EXPIRY_MINUTES)
        });
        transaction.expired_at = Some(exp);

        if Utc::now() >= exp {
            let _ = sqlx::query(
                "UPDATE transactions SET status = 'expired' WHERE id = ? AND status = 'pending'",
            )
            .bind(transaction.id)
            .execute(pool)
            .await;
            transaction.status = "expired".to_string();
            transaction.payment_info = None;
        } else {
            transaction.payment_info = Some(build_payment_info(
                &state.config,
                transaction.amount,
                &transaction.transaction_code,
                exp,
            )?);
        }
    } else {
        transaction.payment_info = None;
    }

    Ok(transaction)
}
